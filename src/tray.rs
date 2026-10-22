//! T3.3 — 系统托盘（main 线程）。
//!
//! - 隐藏 **message-only 窗**（`RegisterClassExW` + `CreateWindowExW(WS_POPUP, HWND_MESSAGE)`）
//!   作为托盘回调窗，收 `u32` 自定义消息 `TRAY_MSG = WM_APP + 100`；
//! - `Shell_NotifyIconW(NIM_ADD)`：图标用 `LoadImageW(0, IDI_APPLICATION, …)`（不内嵌
//!   资源文件，YAGNI）；
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
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{GetLastError, POINT, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    ShellExecuteW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, HMENU, HWND_MESSAGE, IDC_ARROW, LoadCursorW, MB_ICONINFORMATION,
    MB_OK, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostQuitMessage, RegisterClassExW,
    SetForegroundWindow, TPM_RIGHTALIGN, TrackPopupMenu, TranslateMessage, WM_APP, WM_COMMAND,
    WM_LBUTTONUP, WM_RBUTTONUP, WNDCLASSEXW, WS_POPUP,
};

use crate::config;
use crate::i18n::{self, Key, Lang};
use crate::icon;

/// 托盘回调消息（`u32` 自定义 `WM_APP+100`）。
const TRAY_MSG: u32 = WM_APP + 100;
/// 菜单命令 id。
const MENU_TOGGLE: usize = 1;
const MENU_CONFIG: usize = 2;
const MENU_QUIT: usize = 3;
const MENU_USAGE: usize = 4;
const MENU_SETTINGS: usize = 5;
/// 托盘图标 uID。
const TRAY_ICON_ID: u32 = 1;

/// 当前托盘图标句柄 + 它是否是**自绘**的（自绘的要手动销毁；系统 `LR_SHARED`
/// 兜底图标不能销毁）。切换启用态时先销毁旧的，避免每次切换泄漏一个 HICON。
static PREV_ICON: AtomicIsize = AtomicIsize::new(0);
static PREV_ICON_OWNED: AtomicBool = AtomicBool::new(false);

/// 取一个新的托盘图标（自绘优先，失败退回系统图标），并回收上一个自绘图标。
fn next_icon(enabled: bool) -> *mut c_void {
    let mut h = icon::tray_icon(enabled);
    let owned = h != 0;
    if !owned {
        h = icon::fallback();
    }
    let prev = PREV_ICON.swap(h, Ordering::AcqRel);
    if PREV_ICON_OWNED.swap(owned, Ordering::AcqRel) && prev != 0 {
        icon::destroy(prev);
    }
    h as *mut c_void
}

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
        // 图标 = **运行时自绘**（icon::tray_icon，四向移动箭头），不内嵌资源文件，
        // 也不再借用系统默认的 IDI_APPLICATION（跟谁都撞脸、认不出是哪个工具）。
        let icon = next_icon(true);
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
        // 启动即反映启用状态（config.json enabled=false 时直接是灰色"已禁用"图标）。
        if !config::ENABLED.load(Ordering::Acquire) {
            set_icon_state(hwnd, false);
        }

        // 托盘消息泵（main 线程，阻塞至 Quit）。
        //
        // **`IsDialogMessageW` 不能省**：设置窗口是普通 `WS_OVERLAPPED` 窗口而非
        // `DialogBox` 建的对话框，系统不会自动处理 Tab 焦点循环 / 回车默认按钮 /
        // Esc。自绘控件虽有 `WS_TABSTOP`，但焦点转移必须由这个函数驱动 ——
        // 少了它，用户按 Tab 毫无反应，键盘用户等于用不了设置界面（SPEC V12）。
        // 返回 true 表示消息已被对话框逻辑消费，不能再 Dispatch（否则控件会被点两次）。
        let mut msg: MSG = core::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if crate::settings::is_dialog_message(&msg) {
                continue;
            }
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
                MENU_SETTINGS => crate::settings::open(),
                MENU_USAGE => {
                    let p = config::file_path().display().to_string();
                    // 文案统一走 i18n（SPEC D4）：`UsageBody` 含一个 `{}` 占位符，
                    // 这里把配置文件路径填进去。语言每次现算，保证手改配置后
                    // 下次点菜单就变（SPEC §4.5）。
                    let body = i18n::tr(lang(), Key::UsageBody).replacen("{}", &p, 1);
                    let text = wstr(&body);
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
    let l = lang();
    unsafe {
        let toggle = wstr(i18n::tr(
            l,
            if enabled { Key::MenuDisable } else { Key::MenuEnable },
        ));
        let usage_lbl = wstr(i18n::tr(l, Key::MenuUsage));
        let settings_lbl = wstr(i18n::tr(l, Key::MenuSettings));
        let config_lbl = wstr(i18n::tr(l, Key::MenuConfig));
        let quit_lbl = wstr(i18n::tr(l, Key::MenuQuit));
        AppendMenuW(hmenu, MF_STRING, MENU_TOGGLE, toggle.as_ptr());
        AppendMenuW(hmenu, MF_SEPARATOR, 0, 0 as PCWSTR);
        AppendMenuW(hmenu, MF_STRING, MENU_SETTINGS, settings_lbl.as_ptr());
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
    // 唤醒引擎让它立即按新开关跑（它可能正阻塞在 GetMessageW，见 hook::wake_engine）。
    crate::hook::wake_engine();
    new
}

/// 图标 + tip 反映启用状态（`NIM_MODIFY`，同 uID/uCallbackMessage）：
/// 启用 = **自绘四向移动箭头（亮蓝）** + "anydrag-win"；
/// 禁用 = **同一箭头（灰）** + "anydrag-win（已禁用）"。
///
/// 为什么改成自绘：原先借用系统图标（`IDI_APPLICATION` / `IDI_QUESTION`），
/// 前者是个通用窗口图标、跟一堆程序撞脸，用户反馈"还是这个默认的图标"认不出来。
/// 自绘的箭头直接表达"拖窗口"，且配色即状态，托盘上一眼可辨。
fn set_icon_state(hwnd: HWND, enabled: bool) {
    unsafe {
        let icon = next_icon(enabled);
        // tooltip 也双语（SPEC §4.5）。启用态只显示品牌名（两语言相同），
        // 禁用态才需要"已禁用/disabled"的说明。
        let tip = if enabled {
            wstr("anydrag-win")
        } else {
            wstr(i18n::tr(lang(), Key::TrayTipDisabled))
        };
        let mut ni = notify_icon_base(hwnd, icon);
        ni.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(NIM_MODIFY, &ni);
    }
}

/// 当前界面语言。**每次弹菜单/换 tooltip 时现算**，保证手改 `config.json`
/// 的 `lang` 后下次点击就变（SPEC §4.5）。
fn lang() -> Lang {
    crate::winmod::ui_lang(&config::snapshot())
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
        let text = wstr(&format!(
            "{}\n{}",
            i18n::tr(lang(), Key::ConfigOpenFailed),
            p.display()
        ));
        let cap = wstr("anydrag-win");
        unsafe {
            MessageBoxW(hwnd, text.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONINFORMATION);
        }
    }
}
