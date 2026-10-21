//! T3.2 — engine 线程：消费 `RING`、驱动纯函数状态机、执行 FFI 动作。
//!
//! 架构（三线程之"engine 线程"）：
//! - `run_in_thread(cfg)` 开 engine 线程（自己的 `GetMessageW` 泵），线程内先取
//!   `GetCurrentThreadId()` 再 `hook::spawn(tid, &cfg.modifiers)` 绑定 hook 线程，
//!   返回该 tid（调用方 main/托盘用它发 `WM_QUIT`）。
//! - 收到 hook 的 `WM_APP` → `RING.drain` 批量取事件 → `DragState::feed` →
//!   执行 `Vec<Action>`：
//!   - `SetPos{x,y}` 同一事件内**多条只执行最后一条**（移动合并）；
//!   - `Maximize`/`Restore` → `SetWindowPlacement`；`End` → 状态回 Idle。
//!   - 拖动只有 `SetWindowPos` 一条路（native/SC_DRAGMOVE 通道已于 2026-10-21 移除，
//!     见 AGENTS.md「已移除的设计」）。
//!   - `enabled=false`：仍泵消息，但跳过 feed（状态恒 Idle）。
//! - 消费者约定：本线程是 `RING` 唯一消费者（程序结构保证，非类型系统）；
//!   静态只能拿到 `&self`，故走 `drain_impl`（`&self` 原子内核，T1.2 守恒测试
//!   100k×10 并发验证过同模式）。

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, KillTimer, SetTimer, TranslateMessage, WM_APP, WM_TIMER,
};

use crate::config::{self, Config, ENABLED};
use crate::engine::{Action, DragState, EngineCfg, EngineEvent};
use crate::hook;
use crate::throttle::{Decision, Geom, Throttle};
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

/// 节流 + 背压的决策逻辑在 `throttle` 模块（纯逻辑、可单测）。
///
/// **为什么节流（2026-10-16 缩放延迟的根因，实测数据）**：对 Edge（4000 个卡片的页面）
/// 以约 160 次/秒灌缩放请求时，窗口在最后一次鼠标移动之后**还在继续变化 314ms**，
/// 且最终宽度比期望少 18px —— 说明我们发得比目标应用重排得快，它的消息队列越堆越多，
/// 窗口永远追不上光标。引擎侧只有 5 个批次超过 8ms，所以瓶颈不在我们这边，而是**灌太多**。
/// 移动之所以顺滑，是因为移动只让窗口管理器挪一下表面、应用几乎不干活。
/// 对策：几何更新节流到显示器刷新周期，中间态直接丢掉——每次下发的都是**绝对**几何
/// （`start + 位移`），丢掉中间帧不会累积误差。AltSnap 的 `conf.RefreshRate` 是同一招。
///
/// 背压见 `throttle` 模块注释（2026-10-21 硬闩锁死锁事故）。
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
    // 几何节流/背压状态（决策逻辑在 `throttle`，这里只做 I/O）。
    //
    // **线程定时器（SetTimer(NULL,…)）的 nIDEvent 参数会被系统忽略，返回值才是
    // KillTimer 要用的 ID**——2026-10-20 冻结事故根因：旧代码用固定常量 ID 反复
    // Set/Kill，KillTimer 从未真正生效，每次"目标没追上"的推迟都新增一个定时器，
    // 一个 278 帧的缩放手势能攒上百个定时器同时以 ~4-8ms 放炮 → 引擎消息泵被
    // WM_TIMER 灌满（心跳仪实测 24 万条/秒且持续增长）→ 100% CPU 空转、真实事件
    // 饿死 → "程序整个卡死"且手势结束后**永久持续**（定时器杀不掉）。
    // 修复三件套：① 存返回值按真 ID 拆弹；② timer_id.is_none() 才武装（同一时刻
    // 最多一个，它自己会在到点时重挂）；③ 推迟有上限（`throttle::FORCE_AFTER_MS`）。
    let frame_ms = winmod::refresh_interval_ms();
    let mut throttle = Throttle::new(frame_ms);
    let mut pending: Option<(Geom, Option<isize>)> = None;
    let mut timer_id: Option<usize> = None;
    // 最后一次收到事件（任意类型）的时刻——状态看门狗用它判断手势是否已停滞。
    let mut idle_since = std::time::Instant::now();
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
                    "[heartbeat] 1s: app={} timer={} other={} pending={} armed={} mods={} dropped={} state={}",
                    dbg_n[0],
                    dbg_n[1],
                    dbg_n[2],
                    pending.is_some(),
                    timer_id.is_some(),
                    hook::mods_len(),
                    hook::RING.dropped(),
                    state_name(&state),
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
                let now = std::time::Instant::now();
                // 只有"同一窗口、尚未落地"时才值得花一次 GetWindowRect。
                let current = if throttle.outstanding(h) {
                    h.and_then(winmod::window_rect)
                } else {
                    None
                };
                // 定时器到点是"重试节拍"，**不是**强制放行：目标还没追上就再等一轮。
                // 真正的兜底在 `throttle::FORCE_AFTER_MS`——等太久它自己会返回 Apply，
                // 因此这里绝不会无限循环。
                match throttle.decide(now, h, current, false) {
                    Decision::Apply => {
                        let req = apply_geom(g, h);
                        throttle.commit(now, h, req);
                    }
                    Decision::Defer => {
                        // 目标还没追上：留着下轮再试（`FORCE_AFTER_MS` 保证有尽头）。
                        throttle.note_deferred(now);
                        pending = Some((g, h));
                        arm_timer(&mut timer_id, frame_ms);
                    }
                }
            }
        }
        // **状态看门狗**（2026-10-21）：状态机若停在 Fallback/Resize 态，说明提交时
        // 发过 `BeginSizeMove` 却始终没等到配对的 `EndSizeMove`（`Up` 被丢弃 /
        // 目标窗口在相位中途销毁）。目标应用会**永久卡在尺寸调整相位**，此后所有
        // 重排被无限推迟（MEMORY 坑 #1 的死法）。这里在"状态非 Idle 但已长时间没有
        // 任何事件"时强制收尾：补发 `EndSizeMove`、把状态打回 Idle。
        if !matches!(state, DragState::Idle)
            && idle_since.elapsed() >= Duration::from_millis(STALE_GESTURE_MS)
        {
            let in_phase =
                matches!(state, DragState::Fallback { .. } | DragState::Resize { .. });
            if let (true, Some(h)) = (in_phase, gesture_hwnd(&state)) {
                winmod::notify_size_move(h, false);
            }
            if dbg_probe {
                eprintln!(
                    "[watchdog] 手势停滞 {}ms，强制收尾 state={} in_phase={in_phase}",
                    idle_since.elapsed().as_millis(),
                    state_name(&state)
                );
            }
            state = DragState::Idle;
            pending = None;
            throttle.reset();
            if let Some(id) = timer_id.take() {
                unsafe { KillTimer(std::ptr::null_mut(), id) };
            }
        }
        // ── 配置热重载 + 钩子监视表自愈（**每条消息**都检查，不限 WM_APP）──
        //
        // **为什么必须放在 `WM_APP` 分支外（2026-10-21）**：这两件事原先只在
        // `WM_APP` 里做，而 `WM_APP` 只由钩子在**鼠标事件发生时**投递。于是：
        //   设置界面点“保存” → `config::replace()` 换掉内存值 → 引擎正阻塞在
        //   `GetMessageW`（用户没动鼠标）→ 它**根本不醒来**，新配置要等下一次
        //   鼠标事件才生效。
        // 更要紧的是钩子的 `MODS_VKS` 也在这里刷新：改触发键（Alt→Ctrl）后，
        // 钩子线程的 `GetAsyncKeyState` 是**按旧键表**算 mods 的，注入的第一批
        // 事件就已经错了 —— 用户会觉得“改了没反应”。现在 hook::wake_engine()
        // 会在保存时补发一条空 WM_APP 把它叫醒，这条空消息正好走这里。
        //
        // 自愈校验用**内容级**比较（`mods_match`，不是只比长度）：Alt→Ctrl
        // 这类“个数不变”的修改，旧的长度比较会判定“没变”从而永不刷新。
        // 2026-10-20 事故：运行中监视列表归零 → 修饰键恒不命中 → “所有手势全灭
        // 但进程毫无异常”。每消息一次的代价是两次无竞争读锁。
        {
            let want = live.all_vks();
            if !hook::mods_match(&want) {
                hook::set_mods(&want);
            }
            // 热重载检查，两条来源都要看：
            // 1) `config::version()` —— 设置界面点"保存"走 `replace()` 自增；
            // 2) **磁盘文件 mtime** —— 用户直接手改 config.json（2026-10-21 用户报障：
            //    他改完文件仍拖不动，因为旧实现只认版本号，手改永远不会触发重载）。
            //    这一步是每批一次 `metadata()`，无变化时立即返回。
            let file_changed = config::reload_if_changed();
            let v = config::version();
            if v != live_ver || file_changed {
                live = config::snapshot();
                live_ver = config::version();
                hook::set_mods(&live.all_vks());
                if std::env::var_os("ANYDRAG_DEBUG").is_some() {
                    eprintln!(
                        "[config] 已重载（file_changed={file_changed}）mods={:?}",
                        live.modifiers
                    );
                }
            }
        }
        if msg.message == WM_APP {
            idle_since = std::time::Instant::now();
            // hook 每事件 Post 一条 WM_APP；一次 drain 吃干净（可能攒了多条）。
            let mut buf: [core::mem::MaybeUninit<EngineEvent>; hook::RING_SLOTS] =
                unsafe { core::mem::zeroed() };
            let n = hook::RING.drain_impl(&mut buf);
            if n > 0 {
                let mut events: Vec<EngineEvent> =
                    unsafe { (0..n).map(|i| buf[i].assume_init()).collect() };
                // **批内合并**（2026-10-21 高频卡死修复）：连续 Move 只保留最后一个。
                // 状态机对 Move 是纯函数（`frame + (cur - start)`），跳过中间态不改变
                // 结果；Down/Up 是相位边界、绝不丢。这让每批工作量从 O(事件数) 降到
                // O(1)，是"一直高频缩放"下消费端不再落后的关键。
                let kept = crate::coalesce::coalesce_moves(&mut events);
                events.truncate(kept);
                // enabled 开关读原子镜像（T3.3：托盘线程翻转，Arc<Config> 不可原地改）。
                if ENABLED.load(Ordering::Acquire) {
                    for ev in &events {
                        // **手势边界清闩锁**（2026-10-21 修复）：Down/Up 是手势的开始与
                        // 结束，陈旧的下发时刻与"上次请求矩形"绝不允许跨手势存活——
                        // 否则一个属于上一个窗口/上一次缩放的陈旧请求会把新手势永久挡住
                        // （这正是"快速缩放几次后拖拽/缩放全灭、只有最大化还好使"的成因）。
                        //
                        // Up 的顺序很关键：**先补发待发几何，再清闩锁**。收尾那一帧的
                        // 目标矩形只存在于 `pending`（Up 本身在 Fallback/Resize 态不产出
                        // SetPos/SetRect），先清就等于把用户松手时的最终位置丢掉——窗口
                        // 会停在中途尺寸/位置。
                        if matches!(ev, EngineEvent::Up { .. }) {
                            if let Some((g, h)) = pending.take() {
                                let now = std::time::Instant::now();
                                let req = apply_geom(g, h);
                                throttle.commit(now, h, req);
                            }
                            throttle.reset();
                            if let Some(id) = timer_id.take() {
                                unsafe { KillTimer(std::ptr::null_mut(), id) };
                            }
                        } else if matches!(ev, EngineEvent::Down { .. }) {
                            // Down = 新手势开始：旧手势遗留的待发几何必须直接丢弃
                            // （几何是绝对的，但它属于旧目标——放炮会摆错窗口）。
                            throttle.reset();
                            pending = None;
                            if let Some(id) = timer_id.take() {
                                unsafe { KillTimer(std::ptr::null_mut(), id) };
                            }
                        }
                        let (geom, hwnd) = step(&mut state, ev, &live);
                        let Some(g) = geom else { continue };
                        let now = std::time::Instant::now();
                        // 抬起事件必须落地（手势收尾不能留中间态）；其余走节流+软背压。
                        let is_up = matches!(ev, EngineEvent::Up { .. });
                        let current = if throttle.outstanding(hwnd) {
                            hwnd.and_then(winmod::window_rect)
                        } else {
                            None
                        };
                        match throttle.decide(now, hwnd, current, is_up) {
                            Decision::Apply => {
                                let req = apply_geom(g, hwnd);
                                throttle.commit(now, hwnd, req);
                                pending = None;
                                if let Some(id) = timer_id.take() {
                                    unsafe { KillTimer(std::ptr::null_mut(), id) };
                                }
                            }
                            Decision::Defer => {
                                // 推迟：中间态丢弃（几何是绝对的，不累积误差），
                                // 由定时器在目标追上后补发最新值。
                                throttle.note_deferred(now);
                                pending = Some((g, hwnd));
                                // 只在没有定时器时才武装（线程定时器忽略传入 ID，
                                // 返回值才是拆弹凭据；固定 ID 重复武装 = 炸弹，见上方注释）。
                                arm_timer(&mut timer_id, frame_ms);
                            }
                        }
                    }
                } else if !matches!(state, DragState::Idle) {
                    // 运行中被 Disable：状态回 Idle（"feed 恒 None"的清理语义）。
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

/// 手势停滞多久后强制收尾（ms）。
///
/// **为什么需要（2026-10-21）**：状态机若停在 `Fallback`/`Resize` 态，说明提交时
/// 发过 `BeginSizeMove` 却没等到配对的 `EndSizeMove`（`Up` 被丢弃 / 目标窗口在相位
/// 中途销毁）。目标应用会**永久卡在尺寸调整相位**，此后所有重排被无限推迟
/// （MEMORY 坑 #1 的死法）。2 秒远大于任何正常手势的事件间隔，绝不会误杀。
const STALE_GESTURE_MS: u64 = 2000;

/// 状态机状态名（心跳仪/看门狗日志用；不打印 target 细节，避免被窗口句柄淹没）。
fn state_name(s: &DragState) -> &'static str {
    match s {
        DragState::Idle => "Idle",
        DragState::Armed { .. } => "Armed",
        DragState::ArmedResize { .. } => "ArmedResize",
        DragState::Fallback { .. } => "Fallback",
        DragState::Resize { .. } => "Resize",
        DragState::MiddleArmed { .. } => "MiddleArmed",
    }
}

/// 状态机当前手势的目标窗口（看门狗补发 `EndSizeMove` 用）。
fn gesture_hwnd(s: &DragState) -> Option<isize> {
    match s {
        DragState::Armed { target, .. }
        | DragState::Fallback { target, .. }
        | DragState::ArmedResize { target, .. }
        | DragState::Resize { target, .. }
        | DragState::MiddleArmed { target, .. } => Some(target.hwnd),
        DragState::Idle => None,
    }
}

/// 武装节流定时器（同一时刻最多一个）。
///
/// **必须存 `SetTimer` 的返回值**：线程定时器（`SetTimer(NULL, …)`）的 `nIDEvent`
/// 参数会被系统忽略，返回值才是 `KillTimer` 要用的 ID。用固定常量反复 Set/Kill
/// 是 2026-10-20 定时器炸弹事故的根因（一个手势攒上百个定时器同时放炮）。
fn arm_timer(timer_id: &mut Option<usize>, frame_ms: u64) {
    if timer_id.is_some() {
        return;
    }
    let id = unsafe { SetTimer(std::ptr::null_mut(), 0, frame_ms as u32, None) };
    if id != 0 {
        *timer_id = Some(id as usize);
    }
}

/// 单事件一步：feed + 执行动作（`&cfg` 现取现算快照）。
/// 返回**待落地的几何更新**（由 `run()` 的节流器决定何时下发，见 `Geom`）与目标 hwnd。
/// `ANYDRAG_DEBUG=1` 时向 stderr 打印每事件动作（冒烟/验收诊断用，默认静默）。
fn step(state: &mut DragState, ev: &EngineEvent, cfg: &Config) -> (Option<Geom>, Option<isize>) {
    // 动作执行需要的目标 hwnd：feed 会改状态（如双击 Armed→Idle），故**先**快照。
    let pre_hwnd: Option<isize> = match state {
        DragState::Armed { target, .. }
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

    // 合并：同一事件内多条 SetPos / SetRect 各只执行最后一条。
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
                // （Up 后保留 Armed = 双击记忆 / 双击后 Idle / 提交后 Fallback），
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
    // 同一事件内多条只保留最后一条（合并）。
    match (last_setpos, last_setrect) {
        (_, Some((x, y, w, h, _edges))) => (Some(Geom::Rect { x, y, w, h }), pre_hwnd),
        (Some((x, y)), None) => (Some(Geom::Pos { x, y }), pre_hwnd),
        _ => (None, pre_hwnd),
    }
}
