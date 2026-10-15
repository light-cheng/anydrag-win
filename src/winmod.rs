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
use windows_sys::Win32::Foundation::{CloseHandle, HWND, POINT, RECT};
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
    GetClassNameW, GetWindowLongW, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    PostMessageW, SetWindowPos, WindowFromPoint, GWL_EXSTYLE, SWP_NOSIZE, SWP_NOACTIVATE,
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
pub fn resolve_target(x: i32, y: i32, cfg: &Config) -> Option<Target> {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.is_null() {
            return None;
        }
        if IsWindowVisible(hwnd) == 0 {
            return None;
        }
        // 工具窗（WS_EX_TOOLWINDOW）不参与。
        if GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW as i32 != 0 {
            return None;
        }
        // 桌面/WorkerW 不是可拖目标。
        let mut class = [0u16; 64];
        if GetClassNameW(hwnd, class.as_mut_ptr() as PWSTR, 64) > 0 {
            let cls = String::from_utf16_lossy(&class[..]);
            if cls == "Progman" || cls == "WorkerW" {
                return None;
            }
        }
        let exe = exe_name(hwnd as isize)?;
        // 排除表：完全无视手势。
        if cfg.native_excluded.iter().any(|n| n == &exe) {
            return None;
        }
        let frame = frame_bounds(hwnd as isize)?;
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
        Some(Target {
            hwnd: hwnd as isize,
            exe,
            frame,
            is_maximized,
            strategy,
        })
    }
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
    frame[0] == mi.rcWork.left
        && frame[1] == mi.rcWork.top
        && frame[2] == mi.rcWork.right
        && frame[3] == mi.rcWork.bottom
}

/// 发 syscommand（Maximize/Restore/BeginNativeDrag 共用通道）。
/// PostMessage 跨进程投递：SC_MAXIMIZE/SC_RESTORE 是标准命令，目标 WndProc
/// 正常处理；SC_DRAGMOVE 的跨进程生效性由 T2.3 spike 实测背书。
pub fn post_sys_command(hwnd: isize, cmd: u32) {
    unsafe {
        PostMessageW(hwnd as *mut c_void, WM_SYSCOMMAND, cmd as usize, 0);
    }
}

/// 系统双击间隔（ms）：配置 double_click_ms=None 时由调用方用此值解析。
pub fn system_double_click_ms() -> u32 {
    unsafe { GetDoubleClickTime() }
}
