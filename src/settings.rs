//! 设置界面（托盘「设置…」打开）：**Win32 原生控件**，不引任何 GUI 框架依赖。
//!
//! 设计要点：
//! - 窗口建在 **main 线程**（托盘消息泵所在线程），由同一个 `GetMessageW` 泵分发消息——
//!   不需要额外线程，也不会阻塞托盘交互（`config.rs` 的注释记过：`GetMessageW` 的
//!   `hwnd` 传 `null` 会取整个线程的消息队列，子窗口消息自然会被派发到）。
//! - **保存即生效**：读控件 → 组装 `Config` → `config::replace()`（版本号自增，
//!   engine 线程下一个事件批次刷新快照并同步钩子监视键）→ `Config::save()` 写盘。
//!   不需要重启。
//! - 坐标按系统 DPI 缩放（本进程是 Per-Monitor-v2 感知，不缩放的话 150% 下会很小）。
//! - 本窗口属于本进程，`winmod::resolve_target` 已跳过 `anydrag-win.exe`，
//!   所以它自己不会被手势拖动/缩放。

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowTextLengthW, GetWindowTextW,
    IsWindow, LoadCursorW, RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowTextW,
    ShowWindow, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL,
    CB_SETCURSEL, CBS_DROPDOWNLIST, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_WANTRETURN, IDC_ARROW, SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT,
    WNDCLASSEXW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    WS_VISIBLE, WS_VSCROLL,
};

use crate::config;

/// 复选框勾选值（windows-sys 0.61 只导出了 BST_FOCUS/BST_PUSHED，勾选态自己定义）。
const BST_CHECKED_V: usize = 1;

const ID_SAVE: usize = 1;
const ID_CANCEL: usize = 2;

/// 设置窗口句柄（0 = 未打开）。只允许一个实例。
static WIN: AtomicIsize = AtomicIsize::new(0);

/// 控件句柄表。`HWND` 是裸指针（非 `Send`），放不进 `static Mutex`，故统一存 `isize`。
#[derive(Clone, Copy)]
struct Ctls {
    drag_alt: isize,
    drag_ctrl: isize,
    drag_shift: isize,
    drag_win: isize,
    rz_alt: isize,
    rz_ctrl: isize,
    rz_shift: isize,
    rz_win: isize,
    thr: isize,
    dbl: isize,
    mid_on: isize,
    mid_thr: isize,
    strategy: isize,
    excl: isize,
    fallback: isize,
}

const ZERO: Ctls = Ctls {
    drag_alt: 0,
    drag_ctrl: 0,
    drag_shift: 0,
    drag_win: 0,
    rz_alt: 0,
    rz_ctrl: 0,
    rz_shift: 0,
    rz_win: 0,
    thr: 0,
    dbl: 0,
    mid_on: 0,
    mid_thr: 0,
    strategy: 0,
    excl: 0,
    fallback: 0,
};

static CTL: Mutex<Ctls> = Mutex::new(ZERO);

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 打开（或前置）设置窗口。**必须在 main 线程调用**（托盘菜单回调就在该线程）。
pub fn open() {
    unsafe {
        let existing = WIN.load(Ordering::Acquire);
        if existing != 0 && IsWindow(existing as HWND) != 0 {
            ShowWindow(existing as HWND, SW_SHOW);
            SetForegroundWindow(existing as HWND);
            return;
        }
        let instance = GetModuleHandleW(0 as PCWSTR);
        let class = wstr("anydrag-win-settings");
        let cls = WNDCLASSEXW {
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
        // 重复注册会返回 0 且 GetLastError=ERROR_CLASS_ALREADY_EXISTS(1410)，属正常。
        if RegisterClassExW(&cls) == 0 && GetLastError() != 1410 {
            eprintln!("anydrag-win: RegisterClassExW(settings) failed");
            return;
        }
        let dpi = GetDpiForSystem().max(96) as i32;
        let s = |v: i32| v * dpi / 96;
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wstr("anydrag-win 设置").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            s(580),
            s(566),
            0 as HWND,
            0 as _,
            instance,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            eprintln!("anydrag-win: CreateWindowExW(settings) failed");
            return;
        }
        WIN.store(hwnd as isize, Ordering::Release);
        // WM_CREATE 里建控件并灌初值。
        populate(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            create_controls(hwnd);
            0
        }
        WM_COMMAND => {
            match wp & 0xFFFF {
                ID_SAVE => {
                    let cfg = collect();
                    // 热重载：engine 线程下一个事件批次就会用新值；同时写盘持久化。
                    config::replace(cfg.clone());
                    cfg.save();
                    DestroyWindow(hwnd);
                }
                ID_CANCEL => {
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            WIN.store(0, Ordering::Release);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// 建控件（WM_CREATE）。坐标单位是物理像素，已按 DPI 缩放。
unsafe fn create_controls(parent: HWND) {
    let dpi = GetDpiForSystem().max(96) as i32;
    let s = |v: i32| v * dpi / 96;
    let font = GetStockObject(DEFAULT_GUI_FONT);
    let instance = GetModuleHandleW(0 as PCWSTR);
    let mk = |class: &str,
              text: &str,
              style: u32,
              x: i32,
              y: i32,
              w: i32,
              h: i32,
              id: usize|
     -> isize {
        let c = wstr(class);
        let t = wstr(text);
        let hw = CreateWindowExW(
            0,
            c.as_ptr(),
            t.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            s(x),
            s(y),
            s(w),
            s(h),
            parent,
            id as _,
            instance,
            std::ptr::null_mut(),
        );
        if hw.is_null() {
            return 0;
        }
        SendMessageW(hw, WM_SETFONT, font as usize, 1);
        hw as isize
    };
    let label = |text: &str, x: i32, y: i32, w: i32| -> isize {
        mk("STATIC", text, 0, x, y, w, 18, 0)
    };
    let check = |text: &str, x: i32, y: i32, w: i32, id: usize| -> isize {
        mk("BUTTON", text, BS_AUTOCHECKBOX as u32, x, y, w, 20, id)
    };
    let edit = |text: &str, x: i32, y: i32, w: i32, h: i32, id: usize| -> isize {
        mk("EDIT", text, WS_BORDER | ES_AUTOHSCROLL as u32, x, y, w, h, id)
    };

    let mut c = ZERO;
    // 拖动/双击触发键
    label("触发键：拖动 / 双击", 16, 16, 200);
    c.drag_alt = check("Alt", 220, 14, 60, 0);
    c.drag_ctrl = check("Ctrl", 285, 14, 60, 0);
    c.drag_shift = check("Shift", 350, 14, 64, 0);
    c.drag_win = check("Win", 420, 14, 60, 0);
    // 缩放触发键
    label("触发键：缩放", 16, 48, 200);
    c.rz_alt = check("Alt", 220, 46, 60, 0);
    c.rz_ctrl = check("Ctrl", 285, 46, 60, 0);
    c.rz_shift = check("Shift", 350, 46, 64, 0);
    c.rz_win = check("Win", 420, 46, 60, 0);
    // 阈值 / 双击间隔
    label("拖动阈值（像素）", 16, 84, 200);
    c.thr = edit("", 220, 82, 80, 22, 0);
    label("双击间隔（毫秒，留空=跟随系统）", 16, 116, 300);
    c.dbl = edit("", 320, 114, 80, 22, 0);
    // 中键分屏
    label("中键方向分屏", 16, 152, 200);
    c.mid_on = check("启用", 220, 150, 70, 0);
    label("判定阈值(px)", 300, 152, 90);
    c.mid_thr = edit("", 395, 150, 70, 22, 0);
    // 默认策略
    label("默认拖动通道", 16, 186, 200);
    c.strategy = mk(
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        220,
        184,
        160,
        200,
        0,
    );
    // 名单
    label("完全无视手势的进程（一行一个，小写）", 16, 226, 400);
    c.excl = mk(
        "EDIT",
        "",
        WS_BORDER
            | WS_VSCROLL
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_WANTRETURN as u32,
        16,
        248,
        530,
        96,
        0,
    );
    label("强制走 fallback 通道的进程（一行一个，小写）", 16, 354, 420);
    c.fallback = mk(
        "EDIT",
        "",
        WS_BORDER
            | WS_VSCROLL
            | ES_MULTILINE as u32
            | ES_AUTOVSCROLL as u32
            | ES_WANTRETURN as u32,
        16,
        376,
        530,
        96,
        0,
    );
    // 按钮 + 提示
    mk("BUTTON", "保存", BS_PUSHBUTTON as u32, 340, 492, 96, 30, ID_SAVE);
    mk("BUTTON", "取消", BS_PUSHBUTTON as u32, 450, 492, 96, 30, ID_CANCEL);
    label("保存后立即生效，无需重启。", 16, 498, 300);

    *CTL.lock().unwrap() = c;

    // 下拉框内容（COMBOBOX 的字符串必须在控件建好后加）。
    let items = ["fallback（SetWindowPos，兼容性最好）", "native（原生拖动循环，更顺滑）"];
    for it in items.iter() {
        let t = wstr(it);
        SendMessageW(c.strategy as HWND, CB_ADDSTRING, 0, t.as_ptr() as isize);
    }
}

/// 把当前生效配置灌进控件（打开窗口时调用）。
unsafe fn populate(_hwnd: HWND) {
    let cfg = config::snapshot();
    let c = *CTL.lock().unwrap();
    let set_check = |h: isize, on: bool| {
        if h != 0 {
            SendMessageW(h as HWND, BM_SETCHECK, if on { BST_CHECKED_V } else { 0 }, 0);
        }
    };
    let has = |list: &[u16], vk: u16| list.contains(&vk);
    set_check(c.drag_alt, has(&cfg.modifiers, 0x12));
    set_check(c.drag_ctrl, has(&cfg.modifiers, 0x11));
    set_check(c.drag_shift, has(&cfg.modifiers, 0x10));
    set_check(c.drag_win, has(&cfg.modifiers, 0x5B));
    set_check(c.rz_alt, has(&cfg.resize_modifiers, 0x12));
    set_check(c.rz_ctrl, has(&cfg.resize_modifiers, 0x11));
    set_check(c.rz_shift, has(&cfg.resize_modifiers, 0x10));
    set_check(c.rz_win, has(&cfg.resize_modifiers, 0x5B));
    set_check(c.mid_on, cfg.middle_snap);

    set_text(c.thr, &cfg.drag_threshold_px.to_string());
    set_text(
        c.dbl,
        &cfg.double_click_ms.map(|v| v.to_string()).unwrap_or_default(),
    );
    set_text(c.mid_thr, &cfg.middle_snap_threshold_px.to_string());
    set_text(c.excl, &cfg.native_excluded.join("\r\n"));
    set_text(c.fallback, &cfg.fallback_apps.join("\r\n"));
    let sel = if cfg.default_strategy == "native" { 1 } else { 0 };
    SendMessageW(c.strategy as HWND, CB_SETCURSEL, sel, 0);
}

/// 读控件 → 组装新配置。以磁盘值为基底，保留本界面不编辑的字段（如 enabled）。
fn collect() -> config::Config {
    unsafe {
        let c = *CTL.lock().unwrap();
        let mut cfg = config::load();
        cfg.enabled = config::ENABLED.load(Ordering::Acquire);
        cfg.modifiers = collect_vks(c.drag_alt, c.drag_ctrl, c.drag_shift, c.drag_win);
        cfg.resize_modifiers = collect_vks(c.rz_alt, c.rz_ctrl, c.rz_shift, c.rz_win);
        cfg.drag_threshold_px = text_of(c.thr).trim().parse().unwrap_or(5);
        let dbl = text_of(c.dbl);
        cfg.double_click_ms = if dbl.trim().is_empty() {
            None
        } else {
            dbl.trim().parse().ok()
        };
        cfg.middle_snap = is_checked(c.mid_on);
        cfg.middle_snap_threshold_px = text_of(c.mid_thr).trim().parse().unwrap_or(40);
        cfg.default_strategy = if combo_sel(c.strategy) == 1 {
            "native".into()
        } else {
            "fallback".into()
        };
        cfg.native_excluded = parse_list(&text_of(c.excl));
        cfg.fallback_apps = parse_list(&text_of(c.fallback));
        cfg
    }
}

unsafe fn collect_vks(alt: isize, ctrl: isize, shift: isize, win: isize) -> Vec<u16> {
    let mut v = Vec::new();
    if is_checked(alt) {
        v.push(0x12); // VK_MENU
    }
    if is_checked(ctrl) {
        v.push(0x11); // VK_CONTROL
    }
    if is_checked(shift) {
        v.push(0x10); // VK_SHIFT
    }
    if is_checked(win) {
        v.push(0x5B); // VK_LWIN
    }
    v
}

/// 多行文本框 → 进程名列表：按行切、去空白、转小写、去空行、去重。
fn parse_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let n = line.trim().to_lowercase();
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

unsafe fn is_checked(h: isize) -> bool {
    h != 0 && SendMessageW(h as HWND, BM_GETCHECK, 0, 0) as usize == BST_CHECKED_V
}

unsafe fn combo_sel(h: isize) -> isize {
    if h == 0 {
        return 0;
    }
    SendMessageW(h as HWND, CB_GETCURSEL, 0, 0)
}

unsafe fn text_of(h: isize) -> String {
    if h == 0 {
        return String::new();
    }
    let hw = h as HWND;
    let n = GetWindowTextLengthW(hw).max(0) as usize;
    let mut buf = vec![0u16; n + 1];
    let got = GetWindowTextW(hw, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&buf[..got])
}

unsafe fn set_text(h: isize, s: &str) {
    if h == 0 {
        return;
    }
    let t = wstr(s);
    SetWindowTextW(h as HWND, t.as_ptr());
}

/// 供 `tray.rs` 判断窗口是否已开（暂未使用，保留给后续"打开时前置"逻辑）。
#[allow(dead_code)]
pub fn is_open() -> bool {
    let h = WIN.load(Ordering::Acquire);
    h != 0 && unsafe { IsWindow(h as HWND) != 0 }
}

/// 占位：让 `c_void` 的导入在无其他用途时不报警（CreateWindowExW 的 lpparam 用到）。
#[allow(dead_code)]
fn _unused(_: *mut c_void) {}
