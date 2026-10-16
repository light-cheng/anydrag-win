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
    DispatchMessageW, GetMessageW, TranslateMessage, WM_APP,
};

use crate::config::{Config, ENABLED};
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
            // E：读键 = modifiers ∪ resize_modifiers 去重并集——resize_mask 的
            // 触发位（如 Ctrl bit17）若缺位，事件 mods 永远没有该 bit，resize 永不触发。
            let mut vks: Vec<u16> = cfg.modifiers.clone();
            for &vk in cfg.resize_modifiers.iter() {
                if !vks.contains(&vk) {
                    vks.push(vk);
                }
            }
            hook::spawn(tid, &vks);
            tx.send(tid).ok();
            run(cfg);
        })
        .expect("spawn engine thread");
    let tid = rx.recv().expect("engine thread boot signal");
    // 丢弃 JoinHandle = detach：engine 线程继续活到进程退出（main 跑托盘泵）。
    drop(handle);
    tid
}

/// engine 线程消息泵（`pub`：`run_in_thread` 的线程体；也供单测直接驱动）。
pub fn run(cfg: Arc<Config>) {
    let mut state = DragState::Idle;
    let mut msg: _ = unsafe { core::mem::zeroed() };
    loop {
        let r = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if r <= 0 {
            break; // 0 = WM_QUIT；-1 = 错误——两种都退泵（进程随 main 退出，LL 钩子自动卸载）。
        }
        if msg.message == WM_APP {
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
                        step(&mut state, ev, &cfg);
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

/// 单事件一步：feed + 执行动作（`&cfg` 现取现算快照）。
/// `ANYDRAG_DEBUG=1` 时向 stderr 打印每事件动作（冒烟/验收诊断用，默认静默）。
fn step(state: &mut DragState, ev: &EngineEvent, cfg: &Config) {
    // 动作执行需要的目标 hwnd：feed 会改状态（如双击 Armed→Idle），故**先**快照。
    let pre_hwnd: Option<isize> = match state {
        DragState::Armed { target, .. }
        | DragState::Native { target }
        | DragState::Fallback { target, .. }
        | DragState::ArmedResize { target, .. }
        | DragState::Resize { target, .. } => Some(target.hwnd),
        DragState::Idle => None,
    };
    let ecfg = EngineCfg {
        modifiers_mask: cfg.modifiers_mask(),
        // resize_mask 由 config 现算（[] = 0 = 关闭）；方向由 engine 按九宫格/就近角判定，
        // 不再需要"边缘带"配置（2026-10-16 废弃贴边触发，见 engine::resize_edges_for）。
        resize_mask: cfg.resize_mask(),
        threshold_px: cfg.drag_threshold_px,
        double_click_ms: cfg.double_click_ms.unwrap_or_else(winmod::system_double_click_ms),
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
    if let Some((x, y)) = last_setpos {
        if let Some(h) = pre_hwnd {
            winmod::fallback_move(h, x, y);
        }
    }
    if let Some((rx, ry, rw, rh, _edges)) = last_setrect {
        if let Some(h) = pre_hwnd {
            // edges 不再下传：旧版把它翻成 WMSZ_* 塞进一条跨进程 WM_SIZING（且消息号
            // 还是错的），2026-10-16 已改为用 BeginSizeMove/EndSizeMove 表达相位。
            winmod::fallback_resize(h, rx, ry, rw, rh);
        }
    }
}
