//! T3.3 — 系统托盘（main 线程）。
//!
//! - 隐藏 **message-only 窗**（`RegisterClassExW` + `CreateWindowExW(WS_POPUP, HWND_MESSAGE)`）
//!   作为托盘回调窗，收 `u32` 自定义消息 `TRAY_MSG = WM_APP + 100`；
//! - `Shell_NotifyIconW(NIM_ADD)`：图标用 `LoadImageW(0, IDI_APPLICATION, …)`（不内嵌
//!   资源文件，YAGNI——GOAL T3.3 原文）；
//! - **图标 = 启用状态反馈**（NIM_MODIFY）：启用 = IDI_APPLICATION + tip
//!   "anydrag-win"；禁用 = IDI_QUESTION（蓝圈问号）+ tip "anydrag-win（已禁用）"——
//!   不点开菜单也能一眼看出当前手势是否生效；
//! - 右键/左键单击 → `CreatePopupMenu` 四项：
//!   - **启用/禁用**：翻转 `config::ENABLED` 原子镜像并 `Config::save()` 写回磁盘
//!     （`Arc<Config>` 不可原地改 bool，故走镜像，见 config.rs ENABLED 注释）+ 换图标；
//!   - **用法…**：MessageBox 说明手势（Alt 拖动 / Alt 双击）与配置文件路径；
//!   - **配置文件…**：ShellExecuteW("open") 用默认编辑器打开 config.json（文件
//!     不存在时先写当前值当模板）；
//!   - **退出**：`NIM_DELETE` + `PostQuitMessage` → 泵退出 → main 返回 → 进程退出
//!     （LL 钩子随进程退出由系统自动卸载）。
//!
//! `install(&cfg)` 在 main 线程跑托盘消息泵（阻塞至 Quit）。

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{GetLastError, POINT, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    ShellExecuteW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetMessageW,
    GetCursorPos, HMENU, IMAGE_ICON, IDI_APPLICATION, IDI_QUESTION, IDC_ARROW, LR_DEFAULTSIZE,
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
const MENU_USAGE: usize = 4;
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
        // **hInstance 必须是 NULL**：IDI_APPLICATION 是系统预定义资源（MAKEINTRESOURCE(32512)），
        // 传本进程模块句柄会从自己的 exe 里找图标资源 → 本仓没有 .rc/图标 → 返回 NULL →
        // NIM_ADD 时 hIcon=NULL 的托盘图标**不可见**（2026-10-16 用户实测"托盘没图标"即此因；
        // set_icon_state 一直用的就是 NULL，故只有启动路径中招）。
        let icon = LoadImageW(
            std::ptr::null_mut(),
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
        // 图标句柄可观测（ANYDRAG_DEBUG=1）：NULL 即托盘不可见——启动路径最易踩的坑。
        if std::env::var_os("ANYDRAG_DEBUG").is_some() {
            eprintln!("[tray] NIM_ADD ok, hIcon=0x{:x}", icon as usize);
        }
        // 启动即反映启用状态（config.json enabled=false 启动时 = 黄手 + "已禁用" tip）。
        if !config::ENABLED.load(Ordering::Acquire) {
            set_icon_state(hwnd, false);
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
                MENU_TOGGLE => {
                    let new = toggle_enabled();
                    set_icon_state(hwnd, new);
                }
                MENU_USAGE => {
                    let p = config::file_path().display().to_string();
                    let text = wstr(&format!(
                        "anydrag-win 用法\n\n\
                         按住 Alt：\n\
                         · 按住鼠标左键拖动 → 移动当前窗口（不限于标题栏，点哪儿拖哪儿）\n\
                         · 双击（约 0.5 秒内两次、几乎不位移）→ 窗口最大化；已最大化时再双击 → 还原\n\n\
                         配置文件：{p}\n\
                         修改后重启 anydrag-win 生效（本菜单「启用/禁用」可即时切换）。",
                    ));
                    let cap = wstr("anydrag-win");
                    MessageBoxW(hwnd, text.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONINFORMATION);
                }
                MENU_CONFIG => open_config_file(hwnd),
                MENU_QUIT => PostQuitMessage(0),
                _ => {}
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 弹菜单（启用/禁用、用法…、配置文件…、退出）。
fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let hmenu = unsafe { CreatePopupMenu() };
    if hmenu.is_null() {
        return;
    }
    let enabled = config::ENABLED.load(Ordering::Acquire);
    unsafe {
        let toggle = wstr(if enabled { "禁用" } else { "启用" });
        let usage_lbl = wstr("用法…");
        let config_lbl = wstr("配置文件…");
        let quit_lbl = wstr("退出");
        AppendMenuW(hmenu, MF_STRING, MENU_TOGGLE, toggle.as_ptr());
        AppendMenuW(hmenu, MF_SEPARATOR, 0, 0 as PCWSTR);
        AppendMenuW(hmenu, MF_STRING, MENU_USAGE, usage_lbl.as_ptr());
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
/// 其他字段沿用磁盘值，`load()` 现读）。返回翻转后的新值（调用方据此换图标）。
fn toggle_enabled() -> bool {
    let new = !config::ENABLED.load(Ordering::Relaxed);
    config::ENABLED.store(new, Ordering::Release);
    let mut c = config::load();
    c.enabled = new;
    c.save();
    new
}

/// 图标 + tip 反映启用状态（`NIM_MODIFY`，同 uID/uCallbackMessage）：
/// 启用 = IDI_APPLICATION + "anydrag-win"；禁用 = IDI_QUESTION + "anydrag-win（已禁用）"。
fn set_icon_state(hwnd: HWND, enabled: bool) {
    unsafe {
        // IDI_* 在 windows-sys 是 PCWSTR（系统图标名句柄），LoadImageW 的 pName 参数。
        // hInstance 传 NULL = 从 system32 取系统图标（GOAL T3.3 的 LoadImageW(0, …) 语义）。
        // 禁用态用 IDI_QUESTION（蓝圈问号 = "暂停中"）：本版本 windows-sys 无
        // IDI_OUTLAW（禁止圈）；而 IDI_HAND 与 IDI_ERROR 同为 32513，Win11 下
        // 即红圈白叉——16px 托盘上被用户误读为"出错"（2026-10-16 反馈
        // "托盘图标怎么是个 X"，根因即此）。
        let icon = if enabled {
            LoadImageW(std::ptr::null_mut(), IDI_APPLICATION, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED)
        } else {
            LoadImageW(std::ptr::null_mut(), IDI_QUESTION, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED)
        };
        let tip = if enabled {
            wstr("anydrag-win")
        } else {
            wstr("anydrag-win（已禁用）")
        };
        let mut ni = notify_icon_base(hwnd, icon);
        ni.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(NIM_MODIFY, &ni);
    }
}

/// `NIM_ADD`/`NIM_MODIFY` 共用的 NOTIFYICONDATAW 基础字段（hIcon/tip 由调用方补）。
fn notify_icon_base(hwnd: HWND, icon: *mut c_void) -> NOTIFYICONDATAW {
    let mut ni = NOTIFYICONDATAW::default();
    ni.cbSize = core::mem::size_of::<NOTIFYICONDATAW>() as u32;
    ni.hWnd = hwnd;
    ni.uID = TRAY_ICON_ID;
    ni.uFlags = NIF_ICON | NIF_TIP;
    ni.uCallbackMessage = TRAY_MSG;
    ni.hIcon = icon;
    ni
}

/// "配置文件…"：用系统默认编辑器打开 config.json。文件不存在时先写当前值当
/// 模板（含 enabled 镜像——`load()` 只读磁盘，镜像值须显式回填，否则用户
/// 刚在菜单里切过的开关会被模板覆盖回去）；ShellExecuteW 失败（返回值 ≤ 32
/// = 错误码）则退回 MessageBox 显示路径。
fn open_config_file(hwnd: HWND) {
    let p = config::file_path();
    if !std::path::Path::exists(&p) {
        let mut c = config::load();
        c.enabled = config::ENABLED.load(Ordering::Acquire);
        let _ = config::save_to(&c, &p);
    }
    let path_w = wstr(p.to_string_lossy().as_ref());
    let verb = wstr("open");
    let r = unsafe {
        ShellExecuteW(hwnd, verb.as_ptr(), path_w.as_ptr(), 0 as PCWSTR, 0 as PCWSTR, 0)
    };
    if (r as isize) <= 32 {
        let text = wstr(&format!("无法自动打开配置文件，请手动打开：\n{}", p.display()));
        let cap = wstr("anydrag-win");
        unsafe {
            MessageBoxW(hwnd, text.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONINFORMATION);
        }
    }
}
