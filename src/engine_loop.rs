//! T3.2 — engine 线程：消费 `RING`、驱动纯函数状态机、执行 FFI 动作。
//!
//! 架构（GOAL §3/§4，三线程之"engine 线程"）：
//! - `run_in_thread(cfg)` 开 engine 线程（自己的 `GetMessageW` 泵），线程内先取
//!   `GetCurrentThreadId()` 再 `hook::spawn(tid, &cfg.modifiers)` 绑定 hook 线程，
//!   返回该 tid（调用方 main/托盘用它发 `WM_QUIT`）。
//! - 收到 hook 的 `WM_APP` → `RING.drain` 批量取事件 → `DragState::feed` →
//!   执行 `Vec<Action>`：
//!   - `BeginNativeDrag` → `post_sys_command(hwnd, SC_DRAGMOVE)`（T2.3 spike 背书）；
//!   - `SetPos{x,y}` 同一事件内**多条只执行最后一条**（fallback 合并，GOAL T3.2）；
//!   - `Maximize`/`Restore` → syscommand；`End` → 状态回 Idle。
//!   - `enabled=false`：仍泵消息，但跳过 feed（状态恒 Idle）。
//! - 消费者约定：本线程是 `RING` 唯一消费者（程序结构保证，非类型系统）；
//!   静态只能拿到 `&self`，故走 `drain_impl`（`&self` 原子内核，T1.2 守恒测试
//!   100k×10 并发验证过同模式）。

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, WM_APP, WM_TIMER,
};

use crate::config::{self, Config, ENABLED};
use crate::engine::{Action, DragState, EngineCfg, EngineEvent};
use crate::hook;
use crate::winmod;

/// 开 engine 线程（内部 spawn hook 线程并绑定本线程 tid），返回 engine 线程 tid。
/// tid 经 channel 取回——保证返回时钩子已装好、hook 线程已在跑。
pub fn run_in_thread(cfg: Arc<Config>) -> u32 {
    let (tx, rx) = mpsc::channel::<u32>();
    let handle = std::thread::Builder::new()
        .name("anydrag-engine".into())
        .spawn(move || {
            let tid = unsafe { GetCurrentThreadId() };
            // 钩子线程必须绑定到 engine 线程（PostThreadMessage 目标）。
            // 读键 = modifiers ∪ resize_modifiers（去重并集）——见 Config::all_vks。
            hook::spawn(tid, &cfg.all_vks());
            tx.send(tid).ok();
            run(cfg);
        })
        .expect("spawn engine thread");
    let tid = rx.recv().expect("engine thread boot signal");
    // 丢弃 JoinHandle = detach：engine 线程继续活到进程退出（main 跑托盘泵）。
    drop(handle);
    tid
}

/// 待落地的几何更新（节流用）。
///
/// **为什么节流（2026-10-16 缩放延迟的根因，实测数据）**：对 Edge（4000 个卡片的页面）
/// 以约 160 次/秒灌缩放请求时，窗口在最后一次鼠标移动之后**还在继续变化 314ms**，
/// 且最终宽度比期望少 18px —— 说明我们发得比目标应用重排得快，它的消息队列越堆越多，
/// 窗口永远追不上光标。引擎侧只有 5 个批次超过 8ms，所以瓶颈不在我们这边，而是**灌太多**。
/// 移动之所以顺滑，是因为移动只让窗口管理器挪一下表面、应用几乎不干活。
/// 对策：几何更新节流到显示器刷新周期，中间态直接丢掉——每次下发的都是**绝对**几何
/// （`start + 位移`），丢掉中间帧不会累积误差。AltSnap 的 `conf.RefreshRate` 是同一招。
#[derive(Clone, Copy, Debug)]
enum Geom {
    Pos { x: i32, y: i32 },
    Rect { x: i32, y: i32, w: i32, h: i32 },
}

/// 重试轮数上限：目标长期追不上（最小尺寸钳制 / 窗口已最小化 / 应用不重排）时
/// 放弃本帧——几何是绝对的、不累积误差，下一次事件或收尾 Up 会重发。
const MAX_RETRY_ROUNDS: u32 = 400;

/// 下发一次几何更新，返回**本次请求的窗口矩形**（供背压判断"目标是否已追上"）。
///
/// 缩放路径这里还做了一次**坐标语义换算**：engine 算出的目标矩形是 **DWM 帧**
/// (`EXTENDED_FRAME_BOUNDS`) 语义，而 `SetWindowPos` 接受的是**窗口矩形**——两者相差
/// 左右下各约 7px 的不可见边框。旧版直接当窗口矩形下发，结果比预期大 7px（既有偏差）；
/// 现按实测内缩量换算，既修正了尺寸，也让背压的等值比较成立。
fn apply_geom(g: Geom, hwnd: Option<isize>) -> Option<[i32; 4]> {
    let h = hwnd?;
    if std::env::var_os("ANYDRAG_DEBUG").is_some() {
        // 注意与 `[engine]` 行的区别：那行是**状态机输出的动作**（每个事件一条），
        // 这行才是**真正下发到目标窗口的次数**——节流/背压是否生效看这个。
        eprintln!("[apply] {g:?}");
    }
    match g {
        Geom::Pos { x, y } => {
            let cur = winmod::window_rect(h)?;
            let (w, hh) = (cur[2] - cur[0], cur[3] - cur[1]);
            winmod::fallback_move(h, x, y);
            Some([x, y, x + w, y + hh])
        }
        Geom::Rect { x, y, w, h: hh } => {
            let win = winmod::window_rect(h)?;
            let dwn = winmod::frame_bounds(h)?;
            let (dl, dt) = (dwn[0] - win[0], dwn[1] - win[1]);
            let (dr, db) = (win[2] - dwn[2], win[3] - dwn[3]);
            let (wx, wy) = (x - dl, y - dt);
            let (ww, wh) = (w + dl + dr, hh + dt + db);
            winmod::fallback_resize(h, wx, wy, ww, wh);
            Some([wx, wy, wx + ww, wy + wh])
        }
    }
}

/// engine 线程消息泵（`pub`：`run_in_thread` 的线程体；也供单测直接驱动）。
pub fn run(cfg: Arc<Config>) {
    let mut state = DragState::Idle;
    // 生效配置快照 + 版本号：设置界面保存后 `config::replace()` 会自增版本号，
    // 这里检测到变化就刷新快照并同步钩子的监视键列表 —— 即"改完即时生效"。
    let mut live: Config = (*cfg).clone();
    let mut live_ver = config::version();
    // 几何节流/背压状态（见 `Geom` 注释）。
    //
    // **线程定时器（SetTimer(NULL,…)）的 nIDEvent 参数会被系统忽略，返回值才是
    // KillTimer 要用的 ID**——2026-10-20 冻结事故根因：旧代码用固定常量 ID 反复
    // Set/Kill，KillTimer 从未真正生效，每次"目标没追上"的推迟都新增一个定时器，
    // 一个 278 帧的缩放手势能攒上百个定时器同时以 ~4-8ms 放炮 → 引擎消息泵被
    // WM_TIMER 灌满（心跳仪实测 24 万条/秒且持续增长）→ 100% CPU 空转、真实事件
    // 饿死 → "程序整个卡死"且手势结束后**永久持续**（定时器杀不掉）。
    // 修复三件套：① 存返回值按真 ID 拆弹；② timer_id.is_none() 才武装（同一时刻
    // 最多一个，它自己会在到点时重挂）；③ MAX_RETRY_ROUNDS 兜底放弃本帧。
    let frame_ms = winmod::refresh_interval_ms();
    let mut pending: Option<(Geom, Option<isize>)> = None;
    let mut last_apply: Option<std::time::Instant> = None;
    // 上一次**请求**的窗口矩形：用于判断目标应用是否已经把它落地了。
    let mut req: Option<[i32; 4]> = None;
    let mut timer_id: Option<usize> = None;
    let mut retry_rounds: u32 = 0;
    let mut msg: _ = unsafe { core::mem::zeroed() };
    // 心跳仪（仅 ANYDRAG_DEBUG）：按消息类型计数，每秒一条——诊断"CPU 空转但
    // 无日志"用。哪一类在暴涨，洪水就是谁灌的（2026-10-20 冻结事故现场工具）。
    let dbg_probe = std::env::var_os("ANYDRAG_DEBUG").is_some();
    let mut dbg_n: [u64; 3] = [0; 3]; // [WM_APP, WM_TIMER, other]
    let mut dbg_last = std::time::Instant::now();
    loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break; // 0 = WM_QUIT；-1 = 错误——两种都退泵（进程随 main 退出，LL 钩子自动卸载）。
        }
        if dbg_probe {
            let slot = if msg.message == WM_APP {
                0
            } else if msg.message == WM_TIMER {
                1
            } else {
                2
            };
            dbg_n[slot] += 1;
            if dbg_last.elapsed() >= std::time::Duration::from_secs(1) {
                eprintln!(
                    "[heartbeat] 1s: app={} timer={} other={} pending={} armed={} mods={}",
                    dbg_n[0],
                    dbg_n[1],
                    dbg_n[2],
                    pending.is_some(),
                    timer_id.is_some(),
                    hook::mods_len()
                );
                dbg_n = [0; 3];
                dbg_last = std::time::Instant::now();
            }
        }
        // 节流定时器到点 → 重试把被推迟的那次几何落地
        // （目标还没追上的话，下个定时器再试；这是背压的重试节拍）。
        if msg.message == WM_TIMER {
            // 到点即拆弹：按 SetTimer 返回的真 ID 杀（线程定时器忽略 nIDEvent，
            // 用常量 ID  KillTimer 是 2026-10-20 定时器炸弹的根因，见上方注释）。
            if let Some(id) = timer_id.take() {
                unsafe { KillTimer(std::ptr::null_mut(), id) };
            }
            if let Some((g, h)) = pending.take() {
                if caught_up(req, h) {
                    req = apply_geom(g, h);
                    last_apply = Some(std::time::Instant::now());
                    retry_rounds = 0;
                } else if retry_rounds < MAX_RETRY_ROUNDS {
                    // 还没追上：继续等，但计一轮（超上限就放弃本帧，见常量注释）
                    pending = Some((g, h));
                    retry_rounds += 1;
                    if timer_id.is_none() {
                        let id =
                            unsafe { SetTimer(std::ptr::null_mut(), 0, frame_ms as u32, None) };
                        if id != 0 {
                            timer_id = Some(id as usize);
                        }
                    }
                }
                // 超上限：pending 已 take 走 = 放弃本帧（不重挂定时器）。
                // 绝对几何不累积误差；下一个事件 / 收尾 Up 会重发。
            }
        }
        if msg.message == WM_APP {
            // 自愈校验：钩子监视列表与生效配置必须一致（长度不等 = 被谁清空/污染了，
            // 立即重发）。2026-10-20 事故：运行中监视列表归零 → 修饰键恒不命中 →
            // "所有手势全灭但进程毫无异常"。每事件一次的代价是两次无竞争读锁。
            let want = live.all_vks();
            if hook::mods_len() != want.len() {
                hook::set_mods(&want);
            }
            // 热重载检查（无锁读一个原子；无变化时零开销）。
            let v = config::version();
            if v != live_ver {
                live = config::snapshot();
                live_ver = v;
                hook::set_mods(&live.all_vks());
            }
            // hook 每事件 Post 一条 WM_APP；一次 drain 吃干净（可能攒了多条）。
            let mut buf: [core::mem::MaybeUninit<EngineEvent>; hook::RING_SLOTS] =
                unsafe { core::mem::zeroed() };
            let n = hook::RING.drain_impl(&mut buf);
            if n > 0 {
                let events: Vec<EngineEvent> =
                    unsafe { (0..n).map(|i| buf[i].assume_init()).collect() };
                // enabled 开关读原子镜像（T3.3：托盘线程翻转，Arc<Config> 不可原地改）。
                if ENABLED.load(Ordering::Acquire) {
                    for ev in &events {
                        let (geom, hwnd) = step(&mut state, ev, &live);
                        let Some(g) = geom else { continue };
                        // 抬起事件必须落地（手势收尾不能留中间态）；
                        // 其余要同时满足：距上次下发 ≥ 刷新周期 **且** 目标已追上上次请求。
                        let is_up = matches!(ev, EngineEvent::Up { .. });
                        let due = match last_apply {
                            None => true,
                            Some(t) => t.elapsed().as_millis() as u64 >= frame_ms,
                        };
                        if is_up || (due && caught_up(req, hwnd)) {
                            req = apply_geom(g, hwnd);
                            last_apply = Some(std::time::Instant::now());
                            pending = None;
                            retry_rounds = 0;
                            if let Some(id) = timer_id.take() {
                                unsafe { KillTimer(std::ptr::null_mut(), id) };
                            }
                        } else {
                            // 推迟：中间态丢弃（几何是绝对的，不累积误差），
                            // 由定时器在目标追上后补发最新值。
                            pending = Some((g, hwnd));
                            // 只在没有定时器时才武装（线程定时器忽略传入 ID，
                            // 返回值才是拆弹凭据；固定 ID 重复武装 = 炸弹，见上方注释）。
                            if timer_id.is_none() {
                                let id =
                                    unsafe { SetTimer(std::ptr::null_mut(), 0, frame_ms as u32, None) };
                                if id != 0 {
                                    timer_id = Some(id as usize);
                                }
                            }
                        }
                    }
                } else if !matches!(state, DragState::Idle) {
                    // 运行中被 Disable：状态回 Idle（GOAL"feed 恒 None"的清理语义）。
                    state = DragState::Idle;
                }
            }
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 背压判断：目标窗口的当前矩形是否已经等于上一次请求的矩形。
/// 不等 = 目标应用还在消化上一帧（重排/重绘没做完），此时再灌只会让它的队列越堆越长。
fn caught_up(req: Option<[i32; 4]>, hwnd: Option<isize>) -> bool {
    match (req, hwnd) {
        (Some(r), Some(h)) => winmod::window_rect(h).map(|c| c == r).unwrap_or(true),
        // 没有历史请求 / 拿不到窗口 → 不拦。
        _ => true,
    }
}

/// 单事件一步：feed + 执行动作（`&cfg` 现取现算快照）。
/// 返回**待落地的几何更新**（由 `run()` 的节流器决定何时下发，见 `Geom`）与目标 hwnd。
/// `ANYDRAG_DEBUG=1` 时向 stderr 打印每事件动作（冒烟/验收诊断用，默认静默）。
fn step(state: &mut DragState, ev: &EngineEvent, cfg: &Config) -> (Option<Geom>, Option<isize>) {
    // 动作执行需要的目标 hwnd：feed 会改状态（如双击 Armed→Idle），故**先**快照。
    let pre_hwnd: Option<isize> = match state {
        DragState::Armed { target, .. }
        | DragState::Native { target }
        | DragState::Fallback { target, .. }
        | DragState::ArmedResize { target, .. }
        | DragState::Resize { target, .. }
        | DragState::MiddleArmed { target, .. } => Some(target.hwnd),
        DragState::Idle => None,
    };
    let ecfg = EngineCfg {
        modifiers_mask: cfg.modifiers_mask(),
        // resize_mask 由 config 现算（[] = 0 = 关闭）；方向由 engine 按九宫格/就近角判定，
        // 不再需要"边缘带"配置（2026-10-16 废弃贴边触发，见 engine::resize_edges_for）。
        resize_mask: cfg.resize_mask(),
        threshold_px: cfg.drag_threshold_px,
        double_click_ms: cfg.double_click_ms.unwrap_or_else(winmod::system_double_click_ms),
        middle_snap: cfg.middle_snap,
        middle_threshold_px: cfg.middle_snap_threshold_px,
    };
    let resolve = |x: i32, y: i32| winmod::resolve_target(x, y, cfg);
    let actions = state.feed(ev, &resolve, &ecfg);

    // 是否需要"伪装修饰键"（见 winmod::disguise_modifier）。
    // **只在 Alt 是孤立触发键时才需要**：若触发键里本来就有 Ctrl/Shift，那么 Alt 按住
    // 期间已经有别的键被按下，系统根本不会激活窗口菜单；此时若还硬发一次合成的 Ctrl
    // 抬起，反而会把用户正按着的 Ctrl **逻辑释放**（2026-10-16 冒烟实测：Alt+Ctrl 缩放
    // 提交后事件 mods 从 393216 掉成 262144，就是合成 Ctrl-up 干的）。
    let alt_bit = 1u32 << (0x12 & 0x1F);
    let other_mod_bits = (1u32 << (0x11 & 0x1F)) | (1u32 << (0x10 & 0x1F)); // Ctrl | Shift
    let mmask = cfg.modifiers_mask();
    let needs_disguise = mmask & alt_bit != 0 && mmask & other_mod_bits == 0;

    if std::env::var_os("ANYDRAG_DEBUG").is_some() {
        eprintln!(
            "[engine] {:?} → {:?} (pre_hwnd={:?}, exe-target={})",
            ev,
            actions,
            pre_hwnd,
            pre_hwnd.map(|h| winmod::exe_name(h).unwrap_or_default()).unwrap_or_default()
        );
    }

    // 合并：同一事件内多条 SetPos / SetRect 各只执行最后一条（GOAL T3.2 执行规则）。
    let mut last_setpos: Option<(i32, i32)> = None;
    let mut last_setrect: Option<(i32, i32, i32, i32, u8)> = None;
    // 本手势是否真的做了事（移动/缩放/最大化/还原）。用于在**手势真正收尾（鼠标抬起）**
    // 时"伪装"修饰键，见 winmod::disguise_modifier。
    let mut gesture_done = false;
    // 只有鼠标抬起事件才算手势收尾：提交帧（Armed → Fallback/Resize）也会带一条
    // `Action::End`，但那只是"armed 相位结束"，此时发伪装会提前释放用户按着的键。
    let is_up = matches!(ev, EngineEvent::Up { .. });
    let t_exec = std::time::Instant::now();
    for a in actions {
        match a {
            Action::None => {}
            Action::BeginNativeDrag => {
                if let Some(h) = pre_hwnd {
                    winmod::post_sys_command(h, winmod::SC_DRAGMOVE);
                }
                gesture_done = true;
            }
            // 尺寸调整相位（成对）：目标应用据此把重排推迟到相位结束——
            // 缩放顺滑的关键配套，见 winmod::WM_ENTERSIZEMOVE 常量注释。
            Action::BeginSizeMove => {
                if let Some(h) = pre_hwnd {
                    winmod::notify_size_move(h, true);
                }
                gesture_done = true;
            }
            Action::EndSizeMove => {
                if let Some(h) = pre_hwnd {
                    winmod::notify_size_move(h, false);
                }
            }
            Action::SetPos { x, y } => last_setpos = Some((x, y)),
            Action::SetRect { x, y, w, h, edges } => last_setrect = Some((x, y, w, h, edges)),
            Action::Maximize => {
                if let Some(h) = pre_hwnd {
                    // 不走目标应用的消息泵（菜单模式下 PostMessage 会被吞），见函数注释。
                    winmod::set_placement_max_restore(h, true);
                }
                gesture_done = true;
            }
            Action::Restore => {
                if let Some(h) = pre_hwnd {
                    winmod::set_placement_max_restore(h, false);
                }
                gesture_done = true;
            }
            Action::SnapTo(zone) => {
                if let Some(h) = pre_hwnd {
                    winmod::snap_to_zone(h, zone);
                }
                gesture_done = true;
            }
            Action::End => {
                // End 是"相位结束"通知：feed 已把正确的下一状态提交进 `state`
                // （Up 后保留 Armed = 双击记忆 / 双击后 Idle / 提交后 Native），
                // 执行器**不得**在此重置——否则 Up[End] 会抹掉双击记忆，
                // Down/Up/Down 生产手势永远判不出双击（2026-10-15 冒烟实测：
                // 四个 Down 全 [None]，窗口纹丝不动，根因即此）。
                //
                // 唯一副作用：手势真正收尾（鼠标抬起）时补一次修饰键伪装
                // （让 Alt 不激活目标应用的菜单栏）。
                if is_up && gesture_done {
                    if needs_disguise {
                        winmod::disguise_modifier();
                    }
                    gesture_done = false;
                }
            }
        }
    }
    // 耗时仪表（诊断缩放延迟用）：本批次"执行动作"花了多久。移动只是挪表面、目标应用
    // 几乎不干活（约 1ms）；缩放要目标应用按新尺寸重排+重绘，成本全在它那边——这里能量到。
    if std::env::var_os("ANYDRAG_DEBUG").is_some() {
        let ms = t_exec.elapsed().as_millis();
        if ms >= 8 {
            eprintln!("[engine] 执行耗时 {ms}ms（被目标应用的重排/重绘拖住）");
        }
    }
    // 几何更新**不在这里落地**——交给 run() 的节流器（见 `Geom` 注释）：
    // 同一事件内多条只保留最后一条（合并，GOAL T3.2）。
    match (last_setpos, last_setrect) {
        (_, Some((x, y, w, h, _edges))) => (Some(Geom::Rect { x, y, w, h }), pre_hwnd),
        (Some((x, y)), None) => (Some(Geom::Pos { x, y }), pre_hwnd),
        _ => (None, pre_hwnd),
    }
}
