//! T3.1 — `WH_MOUSE_LL` 低级鼠标钩子 + 钩子线程消息泵。
//!
//! 架构（GOAL §3/§4，三线程之"hook 线程"）：
//! - `spawn(engine_tid, mods)` 开钩子线程：`SetWindowsHookExW(WH_MOUSE_LL, …)` +
//!   自己的 `GetMessageW` 泵（线程活着 LL 钩子才持续生效）；
//! - `ll_proc` 回调 **O(1)、无堆分配、无阻塞**（GOAL 不变量）：
//!   仅当 `code >= 0` 且消息 ∈ {WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE} 时
//!   读 `MSLLHOOKSTRUCT`，用 `GetAsyncKeyState` 读配置 VK 组装 `mods` mask
//!   （bit = `VK & 0x1F`，与 mods.rs / engine.rs 的 u32 扩展约定一致），
//!   `RING.push(...)` + `PostThreadMessageW(engine_tid, WM_APP, 0, 0)`；
//!   最后**无条件** `CallNextHookEx`（不截断系统钩子链）。
//! - 环形缓冲 64 槽固定（`RING_SLOTS`），一次性堆分配发生在 `LazyLock`/`OnceLock`
//!   首次触碰（钩子线程启动阶段），热路径（push/drain）零分配。

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{LazyLock, RwLock};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, MSLLHOOKSTRUCT, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WM_APP, WM_LBUTTONUP,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WH_MOUSE_LL,
};

use crate::engine::EngineEvent;
use crate::ring::Ring;

/// 环容量（GOAL T1.2/§5：生产者 = hook 线程，消费者 = engine 线程）。
///
/// **为什么从 64 扩到 512（2026-10-21）**：`WH_MOUSE_LL` 按鼠标硬件率推事件，
/// 实测注入 4183 次/秒时 64 槽环在消费端一批 `SetWindowPos + sync_paint` 的
/// 时间窗内就溢出——心跳仪实测 `dropped=7172/12000`（60% 被丢）。丢 Move 无害
/// （几何是绝对值），但**丢 `Up` 会让状态机永远停在 `Resize` 态、`EndSizeMove`
/// 永不发出、目标应用永久卡在尺寸调整相位**。512 槽 = 128 个事件/帧的余量，
/// 足以吸收任何突发；消费者侧再用 `coalesce::coalesce_moves` 把每批工作量压到 O(1)。
pub const RING_SLOTS: usize = 512;

/// 全局事件环。`LazyLock` 保证一次性堆分配在钩子线程启动时完成，
/// LL 回调内 `RING.push` 无分配（满时自旋 ≤ SPIN_LIMIT 后丢新，O(1)）。
pub static RING: LazyLock<Ring<EngineEvent, RING_SLOTS>> = LazyLock::new(Ring::new);

/// engine 线程 tid：由 `spawn` 注入，回调 `PostThreadMessageW(它, WM_APP)`。
static ENGINE_TID: AtomicU32 = AtomicU32::new(0);
/// LL 钩子句柄（`HHOOK` = 指针；回调 `CallNextHookEx` 用；未安装时为 null）。
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
/// 要监视的修饰键 VK 列表。用 `RwLock` 而非 `OnceLock`：设置界面热重载改了触发键
/// 也必须能生效（写方 = 保存时调 `set_mods`，读方 = 钩子回调）。无竞争时读锁是
/// 纳秒级、不分配、不阻塞，仍满足"回调 O(1)"约束。
static MODS_VKS: RwLock<Vec<u16>> = RwLock::new(Vec::new());

/// 更新要监视的修饰键（设置界面保存后调用；启动时 `spawn` 也会调一次）。
///
/// **锁污染恢复**：`write()` 中毒时 `unwrap_or_else(|e| e.into_inner())` 就地拆锁，
/// 绝不像旧版 `if let Ok(..)` 那样静默跳过——一次静默跳过就会让钩子永久收不到
/// 修饰键（2026-10-20 事故：手势全灭但进程毫无异常，根因就在这条路径上）。
pub fn set_mods(vks: &[u16]) {
    let mut w = MODS_VKS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *w = vks.to_vec();
}

/// 当前监视列表长度（引擎自愈校验用：与 `live.all_vks().len()` 不等即重发）。
pub fn mods_len() -> usize {
    MODS_VKS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len()
}

/// LL 钩子是否仍安装在位（callback 卡顿超 300ms 系统会自动移除，
/// 故这是"无卡顿"的量化哨兵；T3.1 冒烟与 T4.2 验收用）。
pub fn is_installed() -> bool {
    HOOK_HANDLE.load(Ordering::Relaxed) != 0
}

/// 启动 hook 线程。回调把命中事件推入 `RING` 并以 `PostThreadMessageW(engine_tid, WM_APP)`
/// 唤醒 engine 线程。
///
/// **签名偏差注记（对 GOAL T3.1 原文 `spawn() -> engine_tid`）**：钩子线程必须**先知道**
/// engine 线程 tid 才能向它发 `PostThreadMessage`，tid 因此是入参而非返回值
/// （engine 线程由 T3.2 `engine_loop::run_in_thread` 创建，spawn 钩子线程时传入）。
/// GOAL 原句按"spawn 使回调绑定到一个 engine_tid"解读，语义不变。
pub fn spawn(engine_tid: u32, mods: &[u16]) -> std::thread::JoinHandle<()> {
    set_mods(mods);
    ENGINE_TID.store(engine_tid, Ordering::Release);
    std::thread::Builder::new()
        .name("anydrag-hook".into())
        .spawn(|| hook_thread())
        .expect("spawn hook thread")
}

/// 钩子线程主体：装钩子 → 消息泵（保持线程存活，LL 钩子依赖它）→ 退出时卸钩。
fn hook_thread() {
    unsafe {
        let hh = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_proc), std::ptr::null_mut(), 0);
        if hh.is_null() {
            eprintln!(
                "anydrag-win: SetWindowsHookExW(WH_MOUSE_LL) failed, GetLastError={}",
                GetLastError()
            );
            // 钩子没装上：线程直接退出（engine 侧收不到事件，功能静默降级）。
            return;
        }
        HOOK_HANDLE.store(hh as isize, Ordering::Release);

        let mut msg = core::mem::zeroed();
        // GetMessage 返回 0（WM_QUIT）/ −1（错误）即退出。
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(hh);
        HOOK_HANDLE.store(0, Ordering::Release);
    }
}

/// LL 回调（`unsafe extern "system"` = stdcall）。**不得堆分配/阻塞**（GOAL 不变量）：
/// 读指针 + 若干原子读 + 有界自旋 push + 一条 PostThreadMessage，全部 O(1)。
///
/// windows-sys 0.61 约定：`WPARAM = usize`、`LPARAM = isize`、`LRESULT = isize`。
unsafe extern "system" fn ll_proc(nCode: i32, wParam: usize, lParam: isize) -> isize {
    if nCode >= 0 {
        let msg = wParam as u32;
        if msg == WM_LBUTTONDOWN
            || msg == WM_LBUTTONUP
            || msg == WM_MBUTTONDOWN
            || msg == WM_MBUTTONUP
            || msg == WM_MOUSEMOVE
        {
            let info = lParam as *const MSLLHOOKSTRUCT;
            let x = (*info).pt.x;
            let y = (*info).pt.y;
            // 组装按下的修饰键 mask：bit = VK & 0x1F（mods.rs 注记：左侧/右侧修饰键
            // 与基础码落不同 bit；MVP"配置哪个 VK 就检查哪个 bit"）。
            // 锁污染同样就地拆锁读——绝不让一次中毒把 mods 永久清零。
            let mut mods = 0u32;
            {
                let vks = MODS_VKS
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for &vk in vks.iter() {
                    if GetAsyncKeyState(vk as i32) as i32 & 0x8000 != 0 {
                        mods |= 1u32 << (vk & 0x1F);
                    }
                }
            }
            let ev = match msg {
                WM_LBUTTONDOWN => EngineEvent::Down { x, y, mods },
                WM_LBUTTONUP => EngineEvent::Up { x, y, mods },
                // 中键不参与修饰键判定（分屏手势是"裸中键拖拽"，见 engine 的
                // EngineEvent::MiddleDown/MiddleUp）。
                WM_MBUTTONDOWN => EngineEvent::MiddleDown { x, y },
                WM_MBUTTONUP => EngineEvent::MiddleUp { x, y },
                _ => EngineEvent::Move { x, y, mods },
            };
            RING.push(ev);
            // 唤醒 engine 线程（固定信号，非阻塞）。
            PostThreadMessageW(ENGINE_TID.load(Ordering::Relaxed), WM_APP, 0, 0);
        }
    }
    // 无条件放行（无论是否命中）：低级钩子链绝不能被截断。
    CallNextHookEx(
        HOOK_HANDLE.load(Ordering::Relaxed) as _,
        nCode,
        wParam,
        lParam,
    )
}
