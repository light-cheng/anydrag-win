//! P/Invoke 安全封装（GOAL §7：所有 FFI 集中于此，其他模块只调它）。
//! 纯逻辑模块（config/ring/mods/engine）零 windows-sys 导入，本文件是唯一例外。
//!
//! windows-sys 0.61 符号位置注记（§5 逃生门实测）：
//! QueryFullProcessImageNameW 在 Win32_System_Threading（非 ProcessStatus）；
//! MonitorFromWindow/GetMonitorInfoW/MONITORINFO 在 Win32_Graphics_Gdi；
//! GetDoubleClickTime 在 Win32_UI_Input_KeyboardAndMouse；HWND/HANDLE 是
//! `*mut c_void` 的 type alias（不是 tuple struct，不能 `HWND(...)` 构造）。

use crate::config::Config;
use crate::engine::{SnapZone, Strategy, Target};
use std::ffi::c_void;
use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowPlacement, GetWindowRect,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, SendMessageTimeoutW,
    SetWindowPlacement, SetWindowPos, GWL_EXSTYLE, SMTO_NORMAL, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOZORDER, SWP_NOSIZE, SW_MAXIMIZE, SW_RESTORE, WM_SYSCOMMAND,
    WINDOWPLACEMENT, WPF_ASYNCWINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

// ---- 未文档化但数十年稳定的 syscommand 常量（GOAL §3：0xF012 由 T2.3 spike 背书）----
pub const SC_DRAGMOVE: u32 = 0xF012; // SC_MOVE | HTCAPTION：让目标应用自己跑 caption 拖动循环
// SC_MAXIMIZE / SC_RESTORE 已删除：最大化/还原改用 SetWindowPlacement
// （set_placement_max_restore），不再经目标应用的消息泵，见该函数注释。

/// 尺寸调整相位通知（AltSnap `hooks.c:580-591` 的 `NotifySizeMoveStaEnd` 手法）：
/// 拖拽开始/结束各 Post 一条，让目标应用进入"尺寸调整模式"、把重排推迟到结束时
/// 一次性做——这是缩放/移动顺滑的关键配套，浏览器、终端、编辑器都依赖它。
///
/// **常量勘误（2026-10-16）**：此前这里把 `0x0231` 标成了 `WM_SIZING`，而
/// `WM_SIZING` 真值是 `0x0214`，`0x0231` 是 **`WM_ENTERSIZEMOVE`**。后果是缩放路径
/// **每一帧都发一条假的 ENTERSIZEMOVE（wParam=0x0001、lParam=本进程栈指针）却从不发
/// EXITSIZEMOVE**，应用被永久卡在尺寸调整态 → 全部重排被无限推迟 → 这就是
/// "缩放有很严重的延迟"的直接元凶。常量值已按 windows-sys 0.61.2 源码核对。
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
/// 强制重绘同步（异步 SetWindowPos 后补发，见 `sync_paint`）。
pub const WM_SYNCPAINT: u32 = 0x0088;

/// 必须是进程**第一个** FFI 调用（GOAL 不变量 1）：
/// 之后所有坐标（钩子/窗口框/绘制）统一物理像素。
pub fn dpi_aware_v2() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// DWM 可见几何（不变量 2：禁止 GetWindowRect——标准窗口有 ~7px 隐形 resize 边框）。
pub fn frame_bounds(hwnd: isize) -> Option<[i32; 4]> {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(
            hwnd as *mut c_void,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        ) == 0
        {
            Some([r.left, r.top, r.right, r.bottom])
        } else {
            None
        }
    }
}

/// 进程文件名（小写）。取不到（elevated 进程 UIPI 拒绝 OpenProcess 等）→ None，
/// 调用方（resolve_target）据此跳过手势（GOAL 风险 #4：非提权进程对 elevated 窗口不可见）。
pub fn exe_name(hwnd: isize) -> Option<String> {
    let hwnd_ptr: HWND = hwnd as *mut c_void;
    unsafe {
        let mut pid: u32 = 0;
        if GetWindowThreadProcessId(hwnd_ptr, &mut pid) == 0 || pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc.is_null() {
            return None;
        }
        let result = query_image_name(proc);
        CloseHandle(proc);
        result
    }
}

unsafe fn query_image_name(proc: HWND) -> Option<String> {
    let mut len: u32 = 1024;
    let mut buf = vec![0u16; 1024];
    if QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr() as PWSTR, &mut len) == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..len as usize]);
    // 取文件名片段并转小写（排除表按文件名匹配，GOAL 不变量 4/5）。
    let name = s.rsplit('\\').next().unwrap_or(&s);
    Some(name.to_lowercase())
}

/// 目标解析（GOAL 不变量 4/5）：命中测试 + 可见/非工具窗过滤 + 排除表 +
/// DWM 帧 + 身份 + 最大化判定（MVP 简化）+ 策略选择。
///
/// **命中测试法注记（对 GOAL T2.1 原文 `WindowFromPoint` 的偏差，2026-10-15 实测）**：
/// Win11 打包应用（notepad 等）会在真实窗口上浮动 `Microsoft.UI.Content.
/// DesktopChildSiteBridge` 辅助窗，`WindowFromPoint` 被它劫持，而其 DWM 帧读取
/// 恒失败 ⇒ 整条 resolve 失败、手势静默失效。改为 **EnumWindows Z 序扫描
/// （顶→底）+ GetWindowRect 包含**，并**跳过 DWM 帧读取失败/不可名/桌面**的
/// 候选——取点所在的最顶层"可拖"窗口。排除表命中则直接 None（无视手势，
/// GOAL 语义，不穿透到下层窗口）。
pub fn resolve_target(x: i32, y: i32, cfg: &Config) -> Option<Target> {
    let dbg = std::env::var_os("ANYDRAG_DEBUG").is_some();
    // 候选收集：EnumWindows 按 Z 序（顶先）枚举，回调同步执行；栈上 RefCell
    // 存活覆盖整个调用，零泄漏。
    let st: std::cell::RefCell<(i32, i32, Vec<isize>)> =
        std::cell::RefCell::new((x, y, Vec::new()));
    unsafe extern "system" fn collect(hwnd: HWND, lparam: isize) -> i32 {
        let st = lparam as *const std::cell::RefCell<(i32, i32, Vec<isize>)>;
        let (mx, my) = {
            let b = (*st).borrow();
            (b.0, b.1)
        };
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) == 0 {
            return 1;
        }
        if mx >= r.left && mx < r.right && my >= r.top && my < r.bottom {
            (*st).borrow_mut().2.push(hwnd as isize);
        }
        1 // TRUE：继续枚举
    }
    unsafe {
        let _ = EnumWindows(Some(collect), &st as *const _ as isize);
    }
    let cands = st.borrow().2.clone();
    unsafe {
        for h in cands.iter() {
            let hwnd: HWND = *h as HWND;
            if IsWindowVisible(hwnd) == 0 {
                if dbg {
                    eprintln!("[resolve] ({x},{y}) 0x{:x} skip: not visible", hwnd as usize);
                }
                continue;
            }
            // 工具窗（WS_EX_TOOLWINDOW）不参与。
            if GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW as i32 != 0 {
                if dbg {
                    eprintln!("[resolve] ({x},{y}) 0x{:x} skip: tool window", hwnd as usize);
                }
                continue;
            }
            // 桌面/WorkerW 不是可拖目标。
            let mut class = [0u16; 64];
            if GetClassNameW(hwnd, class.as_mut_ptr() as PWSTR, 64) > 0 {
                // 截到第一个 NUL（否则 0 填充区被 lossy 转成 U+FFFD，== "Progman" 永不成立）。
                let end = class.iter().position(|&c| c == 0).unwrap_or(class.len());
                let cls = String::from_utf16_lossy(&class[..end]);
                if cls == "Progman" || cls == "WorkerW" {
                    continue;
                }
            }
            let Some(exe) = exe_name(*h) else {
                if dbg {
                    eprintln!("[resolve] ({x},{y}) 0x{:x} skip: exe_name failed", hwnd as usize);
                }
                continue;
            };
            // 本工具自己的窗口（设置界面等）不参与手势：对它做拖动/缩放没有意义。
            if exe == "anydrag-win.exe" {
                if dbg {
                    eprintln!("[resolve] ({x},{y}) 0x{:x} skip: own window", hwnd as usize);
                }
                continue;
            }
            // 排除表：完全无视手势（不穿透到下层窗口，GOAL 语义）。
            if cfg.native_excluded.iter().any(|n| n == &exe) {
                if dbg {
                    eprintln!("[resolve] ({x},{y}) 0x{:x} stop: excluded {exe}", hwnd as usize);
                }
                return None;
            }
            let Some(frame) = frame_bounds(*h) else {
                // DWM 帧失败 = 辅助窗（DesktopChildSiteBridge 等）→ 继续下探 Z 序。
                if dbg {
                    eprintln!(
                        "[resolve] ({x},{y}) 0x{:x} skip: dwm failed ({exe})",
                        hwnd as usize
                    );
                }
                continue;
            };
            if dbg {
                eprintln!(
                    "[resolve] ({x},{y}) 0x{:x} hit exe={exe} frame=[{}, {}, {}, {}]",
                    hwnd as usize, frame[0], frame[1], frame[2], frame[3]
                );
            }
            // 最大化判定 MVP 简化：frame == 所属 monitor 工作区 即视为最大化态
            // （非最大化的"手动拉满"窗口会被误判——双击发 Restore 而非 Maximize，
            //  行为上等价，注释记录简化；GOAL T2.1 指定此法）。
            let is_maximized = is_maximized_simplified(hwnd, &frame);
            // 策略选择：fallback_apps 命中 → 强制 Fallback，否则默认策略。
            let strategy = if cfg.fallback_apps.iter().any(|n| n == &exe) {
                Strategy::Fallback
            } else if cfg.default_strategy == "native" {
                Strategy::Native
            } else {
                Strategy::Fallback
            };
            return Some(Target {
                hwnd: *h,
                exe,
                frame,
                is_maximized,
                strategy,
            });
        }
    }
    if dbg {
        eprintln!("[resolve] ({x},{y}) no candidate");
    }
    None
}

unsafe fn is_maximized_simplified(hwnd: HWND, frame: &[i32; 4]) -> bool {
    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    if mon.is_null() {
        return false;
    }
    let mut mi = std::mem::zeroed::<MONITORINFO>();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) == 0 {
        return false;
    }
    // ±2px 容差：Win11 DWM 对最大化窗口的 EXTENDED_FRAME_BOUNDS 有 1px 取整
    // 偏差（实测 2026-10-15：最大化 notepad 帧 [0,0,2880,1754] vs rcWork
    // [0,0,2880,1755]，严格 == 恒假 ⇒ 双击永远发 Maximize 而非 Restore）。
    let d = |a: i32, b: i32| (a - b).abs() <= 2;
    d(frame[0], mi.rcWork.left)
        && d(frame[1], mi.rcWork.top)
        && d(frame[2], mi.rcWork.right)
        && d(frame[3], mi.rcWork.bottom)
}

/// 发 syscommand（现在只有 `BeginNativeDrag` 用它发 SC_DRAGMOVE）。
/// PostMessage 跨进程投递 SC_DRAGMOVE 的生效性由 T2.3 spike 实测背书。
///
/// 最大化/还原**已不再走这里**——见 `set_placement_max_restore`。
pub fn post_sys_command(hwnd: isize, cmd: u32) {
    unsafe {
        PostMessageW(hwnd as *mut c_void, WM_SYSCOMMAND, cmd as usize, 0);
    }
}

/// 最大化 / 还原：用 `SetWindowPlacement`（AltSnap `MaximizeRestore_atpt` 的做法，
/// `hooks.c:1287-1319`），**完全不依赖目标应用的消息泵**。
///
/// 为什么不再用 `PostMessage(WM_SYSCOMMAND, SC_MAXIMIZE)`：当 Alt 泄漏给目标应用、
/// 应用进入菜单模式时，它的消息泵处在模态状态，投递的 WM_SYSCOMMAND 会被吞掉或无限
/// 期延后——这正是 2026-10-16 用户实测的"Alt 双击根本无法最大化、只弹出菜单栏快捷键
/// 提示"。`SetWindowPlacement` 由窗口管理器直接处理，与应用的泵状态无关。
/// `WPF_ASYNCWINDOWPLACEMENT` 让调用立即返回，不阻塞 engine 线程。
pub fn set_placement_max_restore(hwnd: isize, maximize: bool) {
    let hwnd_ptr: HWND = hwnd as *mut c_void;
    unsafe {
        if IsWindow(hwnd_ptr) == 0 {
            return;
        }
        let mut wndpl: WINDOWPLACEMENT = std::mem::zeroed();
        wndpl.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd_ptr, &mut wndpl) == 0 {
            return;
        }
        // WINDOWPLACEMENT.showCmd 是 u32，而 SW_* 常量在 windows-sys 里是 i32。
        let cmd: u32 = if maximize {
            SW_MAXIMIZE as u32
        } else {
            SW_RESTORE as u32
        };
        wndpl.showCmd = cmd;
        wndpl.flags |= WPF_ASYNCWINDOWPLACEMENT;
        SetWindowPlacement(hwnd_ptr, &wndpl);
    }
}

/// "伪装"修饰键：发一对合成的 Ctrl 按下/抬起（AltSnap `HotkeyUp` 的手法，
/// `hooks.c:2514-2526`，原注释即 "the alt key is disguised by sending ctrl
/// keydown/keyup events"）。
///
/// **原理**：Windows 只在 Alt 作为**孤立按键**释放时才激活窗口菜单；只要在 Alt 按住
/// 期间有另一个键被按下，系统就把它当成快捷键组合、不再激活菜单。我们在手势结束时
/// 塞一次瞬时 Ctrl 敲击，目标应用就永远看不到"孤立 Alt"——菜单栏快捷键提示不再乱弹。
///
/// **比"吞掉 Alt 抬起"安全**：不改动任何真实按键事件、不做键位拦截，因此
/// Alt+Tab / Alt+F4 / Alt+菜单 等系统组合键完全不受影响。
pub fn disguise_modifier() {
    // 只发一对 Ctrl down/up：VK-only 注入（wScan 留 0，系统会补扫描码）。
    let mk = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_CONTROL,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let seq = [mk(false), mk(true)];
    unsafe {
        SendInput(
            seq.len() as u32,
            seq.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// 尺寸调整相位开/关（`Action::BeginSizeMove` / `EndSizeMove` 的落地）。
/// 成对调用是硬要求：只发 ENTERSIZEMOVE 会让目标应用永久停在尺寸调整态
/// （2026-10-16 的卡顿根因，见 `WM_ENTERSIZEMOVE` 常量注释）。
pub fn notify_size_move(hwnd: isize, start: bool) {
    let hwnd_ptr: HWND = hwnd as *mut c_void;
    unsafe {
        if IsWindow(hwnd_ptr) == 0 {
            return;
        }
        let msg = if start { WM_ENTERSIZEMOVE } else { WM_EXITSIZEMOVE };
        PostMessageW(hwnd_ptr, msg, 0, 0);
    }
}

/// 异步 `SetWindowPos` 之后的同步点（AltSnap `hooks.c:1491-1494`）：
/// `SWP_ASYNCWINDOWPOS` 让调用立即返回，但目标窗口可能积压一串 WM_MOVE/WM_SIZE，
/// 表现为窗口"滑过"中途位置。用带超时的 `WM_SYNCPAINT` 逼它把队列消化到当前状态；
/// 120ms 上限保证绝不长时间阻塞 engine 线程。
unsafe fn sync_paint(hwnd: HWND) {
    let mut dummy: usize = 0;
    SendMessageTimeoutW(hwnd, WM_SYNCPAINT, 0, 0, SMTO_NORMAL, 120, &mut dummy);
}

/// T2.2 回退执行器：hwnd 有效性守卫 + `SetWindowPos` 保持大小。
///
/// **`SWP_ASYNCWINDOWPOS` 是必需的**（AltSnap `MOVEASYNC` 宏 `hooks.c:1510` 必带）：
/// 不带它时跨进程 `SetWindowPos` 会**同步等待目标窗口线程**处理完；而 engine 线程是
/// 事件环的唯一消费者，一旦阻塞在这里整条管线就堵住、延迟逐帧累积（"越拖越卡"）。
pub fn fallback_move(hwnd: isize, x: i32, y: i32) {
    let hwnd_ptr: HWND = hwnd as *mut c_void;
    unsafe {
        if IsWindow(hwnd_ptr) == 0 {
            return;
        }
        SetWindowPos(
            hwnd_ptr,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
        );
        sync_paint(hwnd_ptr);
    }
}

/// 系统双击间隔（ms）：配置 double_click_ms=None 时由调用方用此值解析。
pub fn system_double_click_ms() -> u32 {
    unsafe { GetDoubleClickTime() }
}

/// 中键方向分屏的落地（`Action::SnapTo`）：把窗口吸附到目标显示器**工作区**里
/// `zone` 对应的半屏/四分之一屏。
///
/// 用 `SetWindowPlacement` 一次完成"还原 + 摆位"（AltSnap `RestoreWindowToRect` 手法，
/// `hooks.c:1321-1345`）：`showCmd = SW_RESTORE` 处理最大化/最小化态，
/// `rcNormalPosition` 写目标矩形。比"先 Restore 再 SetWindowPos"可靠——后者两步都是
/// 异步的，顺序不保证。
pub fn snap_to_zone(hwnd: isize, zone: SnapZone) {
    let hwnd_ptr: HWND = hwnd as *mut c_void;
    unsafe {
        if IsWindow(hwnd_ptr) == 0 {
            return;
        }
        let mon = MonitorFromWindow(hwnd_ptr, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() {
            return;
        }
        let mut mi = std::mem::zeroed::<MONITORINFO>();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return;
        }
        // 以工作区左上角为原点的局部坐标，再平移到屏幕坐标。
        let w = mi.rcWork.right - mi.rcWork.left;
        let h = mi.rcWork.bottom - mi.rcWork.top;
        let (l, t, r, b) = match zone {
            SnapZone::LeftHalf => (0, 0, w / 2, h),
            SnapZone::RightHalf => (w / 2, 0, w, h),
            SnapZone::TopHalf => (0, 0, w, h / 2),
            SnapZone::BottomHalf => (0, h / 2, w, h),
            SnapZone::TopLeft => (0, 0, w / 2, h / 2),
            SnapZone::TopRight => (w / 2, 0, w, h / 2),
            SnapZone::BottomLeft => (0, h / 2, w / 2, h),
            SnapZone::BottomRight => (w / 2, h / 2, w, h),
        };
        let mut wndpl: WINDOWPLACEMENT = std::mem::zeroed();
        wndpl.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd_ptr, &mut wndpl) == 0 {
            return;
        }
        wndpl.showCmd = SW_RESTORE as u32;
        wndpl.rcNormalPosition = RECT {
            left: mi.rcWork.left + l,
            top: mi.rcWork.top + t,
            right: mi.rcWork.left + r,
            bottom: mi.rcWork.top + b,
        };
        SetWindowPlacement(hwnd_ptr, &wndpl);
    }
}

/// 回退 resize 执行器（engine `Action::SetRect` 的落地，同 fallback_move 范式）：
/// hwnd 有效性守卫 → `SetWindowPos` 置新矩形（异步）。
///
/// **与旧版的差别（2026-10-16 缩放卡顿修复）**：
/// 1. 不再发 `WM_SIZING`——旧代码把 `0x0231` 误标为 WM_SIZING（真值 `0x0214`），
///    实际每帧发的是一条**假的 `WM_ENTERSIZEMOVE`**，且 lParam 传的是本进程栈上 RECT
///    的指针（跨进程毫无意义，目标可能读到垃圾）。尺寸相位改由 `notify_size_move`
///    成对下发，由目标应用自己决定何时 reflow。
/// 2. 不再带 `SWP_FRAMECHANGED`——它每帧强制 `WM_NCCALCSIZE` + 非客户区重算，很贵；
///    AltSnap 也只在特例才用（`hooks.c:4054`）。
/// 3. 补 `SWP_ASYNCWINDOWPOS` + `sync_paint`，理由同 `fallback_move`。
pub fn fallback_resize(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    let hwnd_ptr: HWND = hwnd as *mut c_void;
    unsafe {
        if IsWindow(hwnd_ptr) == 0 {
            return;
        }
        SetWindowPos(
            hwnd_ptr,
            std::ptr::null_mut(),
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
        );
        sync_paint(hwnd_ptr);
    }
}
