//! P/Invoke 安全封装（GOAL §7：所有 FFI 集中于此，其他模块只调它）。
//! 纯逻辑模块（config/ring/mods/engine）零 windows-sys 导入，本文件是唯一例外。
//!
//! windows-sys 0.61 符号位置注记（§5 逃生门实测）：
//! QueryFullProcessImageNameW 在 Win32_System_Threading（非 ProcessStatus）；
//! MonitorFromWindow/GetMonitorInfoW/MONITORINFO 在 Win32_Graphics_Gdi；
//! GetDoubleClickTime 在 Win32_UI_Input_KeyboardAndMouse；HWND/HANDLE 是
//! `*mut c_void` 的 type alias（不是 tuple struct，不能 `HWND(...)` 构造）。

use crate::config::Config;
use crate::engine::{Strategy, Target};
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
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsWindow,
    IsWindowVisible, PostMessageW, SetWindowPos, GWL_EXSTYLE, SWP_NOSIZE, SWP_NOACTIVATE,
    SWP_NOZORDER, WM_SYSCOMMAND, WS_EX_TOOLWINDOW,
};

// ---- 未文档化但数十年稳定的 syscommand 常量（GOAL §3：0xF012 由 T2.3 spike 背书）----
pub const SC_DRAGMOVE: u32 = 0xF012; // SC_MOVE | HTCAPTION：让目标应用自己跑 caption 拖动循环
pub const SC_MAXIMIZE: u32 = 0xF030;
pub const SC_RESTORE: u32 = 0xF120;
/// wParam 0x0001（HTCAPTION 族）：先于 SetWindowPos 发给目标，
/// 让终端类应用 reflow。开放问题（GOAL §9b）：AltSnap 传 0 亦工作，MVP 按 GOAL 传 0x0001 并注释。
pub const WM_SIZING: u32 = 0x0231;

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
    let h: HWND = hwnd as *mut c_void;
    unsafe {
        let mut pid: u32 = 0;
        if GetWindowThreadProcessId(h, &mut pid) == 0 || pid == 0 {
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
    let mut st: std::cell::RefCell<(i32, i32, Vec<isize>)> =
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

/// 发 syscommand（Maximize/Restore/BeginNativeDrag 共用通道）。
/// PostMessage 跨进程投递：SC_MAXIMIZE/SC_RESTORE 是标准命令，目标 WndProc
/// 正常处理；SC_DRAGMOVE 的跨进程生效性由 T2.3 spike 实测背书。
pub fn post_sys_command(hwnd: isize, cmd: u32) {
    unsafe {
        PostMessageW(hwnd as *mut c_void, WM_SYSCOMMAND, cmd as usize, 0);
    }
}

/// T2.2 回退执行器：hwnd 有效性守卫 + WM_SIZING(ALT 标记) + SetWindowPos 保持大小。
pub fn fallback_move(hwnd: isize, x: i32, y: i32) {
    let h: HWND = hwnd as *mut c_void;
    unsafe {
        if IsWindow(h) == 0 {
            return;
        }
        PostMessageW(h, WM_SIZING, 0x0001, 0);
        SetWindowPos(
            h,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

/// 系统双击间隔（ms）：配置 double_click_ms=None 时由调用方用此值解析。
pub fn system_double_click_ms() -> u32 {
    unsafe { GetDoubleClickTime() }
}
