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
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, FillRect, GetStockObject, RoundRect, SelectObject, SetBkMode,
    SetTextColor, TextOutW, DEFAULT_GUI_FONT, HDC, HFONT, LOGFONTW, NULL_BRUSH, PS_SOLID, SRCCOPY,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowTextLengthW,
    GetWindowTextW, IsWindow, LoadCursorW, RegisterClassExW, SendMessageW, SetForegroundWindow,
    SetWindowTextW, ShowWindow, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON,
    BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CW_USEDEFAULT,
    ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, IDC_ARROW, SW_SHOW, WM_CLOSE,
    WM_COMMAND, WM_CTLCOLORSTATIC, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT, WNDCLASSEXW,
    WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    WS_VISIBLE, WS_VSCROLL,
};

use crate::config;

/// 复选框勾选值（windows-sys 0.61 只导出了 BST_FOCUS/BST_PUSHED，勾选态自己定义）。
const BST_CHECKED_V: usize = 1;

/// COLORREF = 0x00BBGGRR（RGB 宏未由 windows-sys 导出，等价手写）。
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

const ID_SAVE: usize = 1;
const ID_CANCEL: usize = 2;

/// 设置窗口句柄（0 = 未打开）。只允许一个实例。
static WIN: AtomicIsize = AtomicIsize::new(0);

/// 底部灰色提示控件句柄（WM_CTLCOLORSTATIC 里给它上灰色文字）。
static HINT_HWND: AtomicIsize = AtomicIsize::new(0);

// ── Win11 DWM 现代窗口效果常量（未文档化但数十年稳定的属性 id）──
/// DWMWA_USE_IMMERSIVE_DARK_MODE：标题栏明暗（Win10 1809+）。
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
/// DWMWA_WINDOW_CORNER_PREFERENCE：圆角窗口（Win11 22000+）。DWMWCP_ROUND = 2。
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;
/// DWMWA_SYSTEMBACKDROP_TYPE：系统背景材质（Win11 22621+）。DWMSBT_MAINWINDOW = 2（Mica）。
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMSBT_MAINWINDOW: u32 = 2;

/// 卡片区域（96dpi 基准坐标，与 create_controls 的布局一一对应）+ 标题。
/// 自绘卡片取代 GROUPBOX：GROUPBOX 标题颜色改不了（主题引擎自绘，不吃
/// WM_CTLCOLORSTATIC），且方角灰框没有现代感；自绘卡片在远程桌面下也稳定显示
/// （不依赖 DWM Mica 材质转发）。
///
/// **锁浅色设计**：Win32 原生控件（EDIT/COMBO/CHECKBOX）不跟随系统深色，
/// 硬跟深色会标题栏深/内容浅或反之的撕裂（2026-10-16 用户实测）。浅色卡片 +
/// 系统浅色控件 + 浅色标题栏全套一致。深色要等全套自绘控件时再做。
const CARDS: [(&str, i32, i32, i32, i32); 3] = [
    ("触发键", 12, 12, 596, 104),
    ("手势参数", 12, 128, 596, 188),
    ("进程名单（一行一个，小写）", 12, 328, 596, 256),
];

/// 双缓冲自绘窗口背景（WM_ERASEBKGND 调用）。
///
/// Win11 设置应用同款层次：浅灰窗口底 + 白色圆角卡片 + 蓝色半粗体卡片标题。
/// 控件是 sibling 子窗口，绘制在背景之上，天然分层。
unsafe fn paint_background(hwnd: HWND, hdc: HDC) {
    let mut rc = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w <= 0 || h <= 0 {
        return;
    }
    // 内存 DC 双缓冲，避免闪烁。
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem, bmp);

    // 浅色卡片设计（锁定浅色，见 CARDS 注释）。
    let bg = rgb(0xF0, 0xF0, 0xF0);
    let card = rgb(0xFF, 0xFF, 0xFF);
    let border = rgb(0xE3, 0xE3, 0xE3);
    let title_c = rgb(0x2D, 0x7F, 0xF9);

    // 窗口底
    let bg_brush = CreateSolidBrush(bg);
    FillRect(mem, &rc, bg_brush);
    DeleteObject(bg_brush);

    let dpi = GetDpiForSystem().max(96) as i32;
    let s = |v: i32| v * dpi / 96;

    // 卡片标题字体：Segoe UI 半粗体（比正文大一级，形成层级）。
    let mut lf: LOGFONTW = std::mem::zeroed();
    lf.lfHeight = -s(15);
    lf.lfWeight = 600; // FW_SEMIBOLD
    lf.lfCharSet = 1; // DEFAULT_CHARSET
    let face = "Segoe UI".encode_utf16().chain(std::iter::once(0));
    for (i, c) in face.enumerate().take(32) {
        lf.lfFaceName[i] = c;
    }
    let title_font: HFONT = CreateFontW(
        lf.lfHeight,
        lf.lfWidth,
        lf.lfEscapement,
        lf.lfOrientation,
        lf.lfWeight,
        0,
        0,
        0,
        lf.lfCharSet as u32,
        0,
        0,
        0,
        0,
        lf.lfFaceName.as_ptr(),
    );
    let old_font = SelectObject(mem, title_font);
    SetBkMode(mem, TRANSPARENT as i32);
    SetTextColor(mem, title_c);

    for (title, x, y, cw, ch) in CARDS {
        let (l, t, r, b) = (s(x), s(y), s(x + cw), s(y + ch));
        // 圆角卡片（白/深卡 + 细边框）
        let card_brush = CreateSolidBrush(card);
        let border_pen = CreatePen(PS_SOLID, 1, border);
        let old_brush = SelectObject(mem, card_brush);
        let old_pen = SelectObject(mem, border_pen);
        RoundRect(mem, l, t, r, b, s(24), s(24));
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        DeleteObject(card_brush);
        DeleteObject(border_pen);
        // 卡片标题（左上内缩）
        let tw = wstr(title);
        TextOutW(mem, l + s(18), t + s(9), tw.as_ptr(), tw.len() as i32 - 1);
    }

    SelectObject(mem, old_font);
    DeleteObject(title_font);

    BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp);
    DeleteDC(mem);
}

/// 给窗口套上 Win11 现代效果（WM_CREATE 里调用，窗口已存在、尚未显示）。
///
/// - Mica 云母背景（Win11 22621+；旧系统/远程桌面被 DWM 忽略时无副作用）
/// - 圆角窗口（Win11 22000+）
/// - 标题栏固定浅色（与浅色卡片设计一致，锁浅色见 CARDS 注释）
///
/// 配套 wnd_proc 的 WM_ERASEBKGND：自绘卡片背景。
unsafe fn apply_modern_window_style(hwnd: HWND) {
    let backdrop = DWMSBT_MAINWINDOW;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE,
        &backdrop as *const u32 as *const _,
        std::mem::size_of::<u32>() as u32,
    );
    let corner = DWMWCP_ROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const u32 as *const _,
        std::mem::size_of::<u32>() as u32,
    );
    // 标题栏固定浅色（0 = light），与整套浅色卡片一致。
    let light: u32 = 0;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &light as *const u32 as *const _,
        std::mem::size_of::<u32>() as u32,
    );
}

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
        // 窗口图标 = 运行时自绘的四向箭头（与托盘同源，启用=亮蓝）。
        // 类注册的图标句柄随进程退出由系统回收，无需手动销毁。
        let icon = crate::icon::tray_icon(true);
        let hicon = if icon != 0 {
            icon as *mut c_void
        } else {
            std::ptr::null_mut()
        };
        let cls = WNDCLASSEXW {
            cbSize: core::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: hicon,
            hCursor: LoadCursorW(instance, IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: 0 as PCWSTR,
            lpszClassName: class.as_ptr(),
            hIconSm: hicon,
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
            s(620),
            // 注意：CreateWindowExW 的宽高是**含标题栏+边框的整个窗口**尺寸，
            // 不是客户区。布局内容底边（按钮行 636）+ 非客户区(~40@150%) + 底边距
            // → 700 基准（150% 下 1050px 总高）。660 会把按钮裁掉一半。
            s(700),
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
            // 先套 Win11 现代效果（Mica + 圆角 + 深色标题栏），再建控件。
            apply_modern_window_style(hwnd);
            create_controls(hwnd);
            0
        }
        // 自绘卡片式背景（浅灰底 + 白圆角卡片 + 蓝色标题），不依赖 Mica 是否生效。
        WM_ERASEBKGND => {
            paint_background(hwnd, wp as HDC);
            1
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
            HINT_HWND.store(0, Ordering::Release);
            0
        }
        // 控件文字着色（锁浅色，与整套浅色卡片一致）：
        // - 底部提示 → 灰色；其余 STATIC 保持默认黑字
        // 统一返回 NULL_BRUSH：文字背景透明，白色卡片透出。
        WM_CTLCOLORSTATIC => {
            let target = lp as HWND;
            if target == HINT_HWND.load(Ordering::Acquire) as HWND {
                SetTextColor(wp as HDC, rgb(0x5A, 0x5A, 0x5A));
            }
            GetStockObject(NULL_BRUSH) as isize
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// 建控件（WM_CREATE）。坐标单位是物理像素，已按 DPI 缩放。
///
/// **布局（2026-10-16 美化）**：三个分组框（触发键 / 手势参数 / 进程名单）+ 底部按钮区。
/// 之前所有控件平铺无层次，用户反馈"gui 需要美化"。分组框只画框和标题（BS_GROUPBOX），
/// 子控件仍是 sibling 用绝对坐标摆放；输入框加 WS_EX_CLIENTEDGE 立体边框；
/// 「保存」设为默认按钮（BS_DEFPUSHBUTTON，回车可触发）。
unsafe fn create_controls(parent: HWND) {
    let dpi = GetDpiForSystem().max(96) as i32;
    let s = |v: i32| v * dpi / 96;
    let font = GetStockObject(DEFAULT_GUI_FONT);
    let instance = GetModuleHandleW(0 as PCWSTR);
    let mk = |class: &str,
              text: &str,
              style: u32,
              ex: u32,
              x: i32,
              y: i32,
              w: i32,
              h: i32,
              id: usize|
     -> isize {
        let c = wstr(class);
        let t = wstr(text);
        let hw = CreateWindowExW(
            ex,
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
        mk("STATIC", text, 0, 0, x, y, w, 20, 0)
    };
    let check = |text: &str, x: i32, y: i32, w: i32| -> isize {
        mk("BUTTON", text, BS_AUTOCHECKBOX as u32, 0, x, y, w, 24, 0)
    };
    // 单行输入框（立体边框）。
    let edit1 = |x: i32, y: i32, w: i32| -> isize {
        mk(
            "EDIT",
            "",
            ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            x,
            y,
            w,
            26,
            0,
        )
    };
    // 多行输入框（立体边框 + 滚动条）。
    let editn = |x: i32, y: i32, w: i32, h: i32| -> isize {
        mk(
            "EDIT",
            "",
            WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | ES_WANTRETURN as u32,
            WS_EX_CLIENTEDGE,
            x,
            y,
            w,
            h,
            0,
        )
    };
    let mut c = ZERO;

    // ── 卡片一：触发键（卡片框与标题由 paint_background 自绘，见 CARDS）──
    label("拖动 / 双击：", 30, 42, 110);
    c.drag_alt = check("Alt", 150, 40, 64);
    c.drag_ctrl = check("Ctrl", 218, 40, 64);
    c.drag_shift = check("Shift", 290, 40, 68);
    c.drag_win = check("Win", 362, 40, 64);
    label("缩放：", 30, 80, 110);
    c.rz_alt = check("Alt", 150, 78, 64);
    c.rz_ctrl = check("Ctrl", 218, 78, 64);
    c.rz_shift = check("Shift", 290, 78, 68);
    c.rz_win = check("Win", 362, 78, 64);

    // ── 卡片二：手势参数 ────────────────────────────
    label("拖动阈值（像素）", 30, 160, 200);
    c.thr = edit1(240, 158, 90);
    label("双击间隔（毫秒，留空 = 跟随系统）", 30, 198, 280);
    c.dbl = edit1(320, 196, 90);
    label("中键方向分屏", 30, 236, 130);
    c.mid_on = check("启用", 168, 234, 76);
    label("判定阈值(px)", 260, 236, 110);
    c.mid_thr = edit1(378, 234, 90);
    label("默认拖动通道", 30, 274, 130);
    c.strategy = mk(
        "COMBOBOX",
        "",
        CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
        0,
        168,
        272,
        260,
        220,
        0,
    );

    // ── 卡片三：进程名单 ────────────────────────────
    label("完全无视手势的进程：", 30, 358, 300);
    c.excl = editn(30, 384, 560, 76);
    label("强制走 fallback 通道的进程：", 30, 476, 300);
    c.fallback = editn(30, 502, 560, 76);

    // ── 底部按钮区 ──────────────────────────────────
    HINT_HWND.store(
        label("保存后立即生效，无需重启。", 16, 606, 300),
        Ordering::Release,
    );
    mk("BUTTON", "保存", BS_DEFPUSHBUTTON as u32, 0, 330, 602, 110, 34, ID_SAVE);
    mk("BUTTON", "取消", BS_PUSHBUTTON as u32, 0, 452, 602, 110, 34, ID_CANCEL);

    *CTL.lock().unwrap() = c;

    // 下拉框内容（COMBOBOX 的字符串必须在控件建好后加）。
    // 文案必须如实反映实测结论（docs/spike-scandrag.md）：native 走
    // PostMessage(SC_DRAGMOVE)，让目标应用自己跑 caption 拖动循环——**在 notepad 等
    // 应用上直接失效**（spike 实测 dx=0），只有 explorer 这类有效。旧文案写"更顺滑"
    // 会诱导用户选中它，然后"Alt 完全拖不动"（2026-10-21 用户报障即此）。
    let items = [
        "fallback（SetWindowPos，兼容性最好，推荐）",
        "native（原生拖动循环；仅部分应用有效，可能完全拖不动）",
    ];
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
