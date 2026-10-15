//! T3.3 — 系统托盘（main 线程）。
//!
//! - 隐藏 **message-only 窗**（`RegisterClassExW` + `CreateWindowExW(WS_POPUP, HWND_MESSAGE)`）
//!   作为托盘回调窗，收 `u32` 自定义消息 `TRAY_MSG = WM_APP + 100`；
//! - `Shell_NotifyIconW(NIM_ADD)`：图标用 `LoadImageW(0, IDI_APPLICATION, …)`（不内嵌
//!   资源文件，YAGNI——GOAL T3.3 原文）；
//! - 右键/左键单击 → `CreatePopupMenu` 三项：
//!   - **Enable/Disable**：翻转 `config::ENABLED` 原子镜像并 `Config::save()` 写回磁盘
//!     （`Arc<Config>` 不可原地改 bool，故走镜像，见 config.rs ENABLED 注释）；
//!   - **Config…**：`MessageBoxW` 显示配置文件路径（MVP 不做 GUI 编辑）；
//!   - **Quit**：`NIM_DELETE` + `PostQuitMessage` → 泵退出 → main 返回 → 进程退出
//!     （LL 钩子随进程退出由系统自动卸载）。
//!
//! `install(&cfg)` 在 main 线程跑托盘消息泵（阻塞至 Quit）。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{GetLastError, POINT, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetMessageW,
    GetCursorPos, HMENU, IMAGE_ICON, IDI_APPLICATION, IDC_ARROW, LR_DEFAULTSIZE,
    LR_SHARED, LoadCursorW, LoadImageW, MB_ICONINFORMATION, MB_OK, MF_SEPARATOR, MF_STRING,
    MessageBoxW, PostQuitMessage, RegisterClassExW, SetForegroundWindow, TPM_RIGHTALIGN,
    TranslateMessage, WNDCLASSEXW, DispatchMessageW, HWND_MESSAGE, MSG,
    TrackPopupMenu, WM_APP, WM_COMMAND, WM_LBUTTONUP, WM_RBUTTONUP, WS_POPUP,
};

use crate::config;

/// 托盘回调消息（GOAL T3.3：`u32` 自定义 `WM_APP+100`）。
const TRAY_MSG: u32 = WM_APP + 100;
/// 菜单命令 id。
const MENU_TOGGLE: usize = 1;
const MENU_CONFIG: usize = 2;
const MENU_QUIT: usize = 3;
/// 托盘图标 uID。
const TRAY_ICON_ID: u32 = 1;

/// NUL 结尾 wide 串（局部持有，FFI 调用期间有效）。
fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// main 线程入口：装托盘 + 跑消息泵（阻塞至 Quit 菜单）。
pub fn install(cfg: &Arc<config::Config>) {
    // 启动时把磁盘配置的 enabled 载入原子镜像（engine 线程读它）。
    config::ENABLED.store(cfg.enabled, Ordering::Release);

    unsafe {
        let instance = GetModuleHandleW(0 as PCWSTR);
        let class = wstr("anydrag-win-tray");
        let cls: WNDCLASSEXW = WNDCLASSEXW {
            cbSize: core::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(instance, IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: 0 as PCWSTR,
            lpszClassName: class.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        let atom = RegisterClassExW(&cls);
        if atom == 0 {
            eprintln!("anydrag-win: RegisterClassExW failed, GetLastError={}", GetLastError());
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wstr("anydrag-win").as_ptr(),
            WS_POPUP,
            0, 0, 0, 0,
            HWND_MESSAGE,
            0 as HMENU,
            instance,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            eprintln!("anydrag-win: CreateWindowExW(HWND_MESSAGE) failed");
            return;
        }
        // GOAL T3.3：图标 = LoadImageW(0, IDI_APPLICATION, …)，不内嵌资源文件。
        let icon = LoadImageW(
            instance,
            IDI_APPLICATION,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        );
        let mut ni = NOTIFYICONDATAW::default();
        ni.cbSize = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
        ni.hWnd = hwnd;
        ni.uID = TRAY_ICON_ID;
        ni.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        ni.uCallbackMessage = TRAY_MSG;
        ni.hIcon = icon;
        let tip = wstr("anydrag-win");
        ni.szTip[..tip.len()].copy_from_slice(&tip);
        if Shell_NotifyIconW(NIM_ADD, &ni) == 0 {
            eprintln!("anydrag-win: Shell_NotifyIconW(NIM_ADD) failed");
            return;
        }

        // 托盘消息泵（main 线程，阻塞至 Quit）。
        let mut msg: MSG = core::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // 退泵前摘掉托盘图标（Quit 路径）。
        Shell_NotifyIconW(NIM_DELETE, &ni);
    }
}

/// message-only 窗 WndProc（`unsafe extern "system"` = stdcall）。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        TRAY_MSG => {
            let event = lparam as u32; // 托盘回调 lParam = 鼠标消息
            if event == WM_LBUTTONUP || event == WM_RBUTTONUP {
                let mut pt: POINT = core::mem::zeroed();
                let _ = GetCursorPos(&mut pt);
                show_menu(hwnd, pt.x, pt.y);
            }
            0
        }
        WM_COMMAND => {
            let id = wparam as usize;
            match id {
                MENU_TOGGLE => toggle_enabled(),
                MENU_CONFIG => {
                    let p = config::file_path();
                    let path = p.to_string_lossy();
                    let text = format!(
                        "Config file:\n{path}\n\nMVP: no GUI editor — edit by hand,\nrestart anydrag-win to apply."
                    );
                    let wide = wstr(&text);
                    let cap = wstr("anydrag-win");
                    MessageBoxW(
                        hwnd,
                        wide.as_ptr(),
                        cap.as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                MENU_QUIT => PostQuitMessage(0),
                _ => {}
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 弹三项菜单（Enable/Disable、Config…、Quit）。
fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let hmenu = unsafe { CreatePopupMenu() };
    if hmenu.is_null() {
        return;
    }
    let enabled = config::ENABLED.load(Ordering::Acquire);
    unsafe {
        let toggle = wstr(if enabled { "Disable" } else { "Enable" });
        let config_lbl = wstr("Config\u{2026}");
        let quit_lbl = wstr("Quit");
        AppendMenuW(hmenu, MF_STRING, MENU_TOGGLE, toggle.as_ptr());
        AppendMenuW(hmenu, MF_SEPARATOR, 0, 0 as PCWSTR);
        AppendMenuW(hmenu, MF_STRING, MENU_CONFIG, config_lbl.as_ptr());
        AppendMenuW(hmenu, MF_STRING, MENU_QUIT, quit_lbl.as_ptr());
        // 先置前再弹，保证菜单点击-away 正确消失（经典托盘模式）。
        SetForegroundWindow(hwnd);
        TrackPopupMenu(
            hmenu,
            TPM_RIGHTALIGN,
            x,
            y,
            0,
            hwnd,
            std::ptr::null(),
        );
        DestroyMenu(hmenu);
    }
}

/// 翻转 enabled 原子镜像并写回磁盘（新值对 engine 线程立即生效；
/// 其他字段沿用磁盘值，`load()` 现读）。
fn toggle_enabled() {
    let new = !config::ENABLED.load(Ordering::Relaxed);
    config::ENABLED.store(new, Ordering::Release);
    let mut c = config::load();
    c.enabled = new;
    c.save();
}
