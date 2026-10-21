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
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
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
    CreateWindowExW, DefWindowProcW, DestroyWindow, EnumWindows, GetClientRect,
    GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, IsWindow, IsWindowVisible, LoadCursorW,
    RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow, BM_GETCHECK,
    BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CW_USEDEFAULT,
    ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, IDCANCEL, IDC_ARROW, SW_SHOW,
    WM_CLOSE, WM_COMMAND, WM_CTLCOLORSTATIC, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT,
    WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_TOOLWINDOW,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE, WS_VSCROLL,
    // ListBox（进程选择弹窗用）：多选 + 系统原生选中态，省掉自绘 owner-draw 列表。
    GWL_EXSTYLE, LB_ADDSTRING, LB_ERR, LB_GETCOUNT, LB_GETSEL, LB_GETTEXT, LB_GETTEXTLEN,
    LB_SETSEL, LBS_HASSTRINGS, LBS_MULTIPLESEL, LBS_NOTIFY,
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
/// 设置窗口内「选择进程…」按钮。
const ID_PICK_PROCS: usize = 3;
/// 进程选择弹窗内的控件/按钮。
const ID_PROC_LIST: usize = 10;
const ID_PROC_OK: usize = 11;

/// 设置窗口句柄（0 = 未打开）。只允许一个实例。
static WIN: AtomicIsize = AtomicIsize::new(0);

/// 底部灰色提示控件句柄（WM_CTLCOLORSTATIC 里给它上灰色文字）。
static HINT_HWND: AtomicIsize = AtomicIsize::new(0);

/// 进程选择弹窗的父窗口（设置窗口）句柄；None = 未打开。
///
/// **ListBox 句柄为什么单独放一个静态**：`CreateWindowExW` 会**同步**触发
/// `WM_CREATE`，而此时外层还没执行到"把弹窗句柄存进 PROC_PICK"那一步——
/// 第一版在 WM_CREATE 里写 `PROC_PICK` 的 `Some((_, lb))`，结果永远写不进去，
/// `lb` 恒为 0，点"确定"时 `apply_proc_selection` 直接 return，名单纹丝不动。
/// 拆成两个静态后 WM_CREATE 只管写自己那个，没有时序依赖。
static PROC_PICK: Mutex<Option<isize>> = Mutex::new(None);
static PROC_LIST: AtomicIsize = AtomicIsize::new(0);

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
    excl: isize,
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
    excl: 0,
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
                ID_PICK_PROCS => open_proc_picker(),
                ID_SAVE => {
                    let cfg = collect();
                    // 热重载：engine 线程下一个事件批次就会用新值；同时写盘持久化。
                    config::replace(cfg.clone());
                    cfg.save();
                    // **必须显式唤醒引擎（2026-10-21）**：引擎的重载检查由消息驱动，
                    // 而它此刻很可能正阻塞在 `GetMessageW`（用户点完保存并没动鼠标）。
                    // 不补这一条，新配置要等下一次鼠标事件才生效；若改的是触发键，
                    // 钩子仍按旧键表算 mods，**第一次手势必失效**。
                    crate::hook::wake_engine();
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
    // ── 卡片三：进程名单 ────────────────────────────
    label("完全无视手势的进程：", 30, 290, 300);
    // 「选择进程…」：列出当前可见窗口勾选，用户不必去猜 exe 名称
    // （2026-10-21 用户反馈：手填 exe 名等于不可用）。下面的多行框保留给
    // 高级用法——手填当前没开着的进程（如没启动的游戏）。
    mk(
        "BUTTON",
        "选择进程…",
        BS_PUSHBUTTON as u32,
        0,
        420,
        288,
        110,
        26,
        ID_PICK_PROCS,
    );
    c.excl = editn(30, 316, 560, 110);

    // ── 底部按钮区 ──────────────────────────────────
    HINT_HWND.store(
        label("保存后立即生效，无需重启。", 16, 606, 300),
        Ordering::Release,
    );
    mk("BUTTON", "保存", BS_DEFPUSHBUTTON as u32, 0, 330, 602, 110, 34, ID_SAVE);
    mk("BUTTON", "取消", BS_PUSHBUTTON as u32, 0, 452, 602, 110, 34, ID_CANCEL);

    *CTL.lock().unwrap() = c;
}

// ─────────────────────────────────────────────────────────────────────────────
// 进程选择弹窗
//
// **为什么要有它（2026-10-21 用户反馈）**：排除表原来只让用户手填 exe 名称
// （"一行一个，如 game.exe"）。但用户不可能知道目标是 `msedge.exe` 还是
// `MicrosoftEdge.exe`，也不会为了填一个名字去开任务管理器查——这个交互等于
// 不可用。改成：点「选择进程…」→ 列出**当前所有可见窗口**（标题 + exe）→
// 勾选 → 确定即写入名单。
//
// 实现取舍：用 **ListBox 多选**（`LBS_MULTIPLESEL`）而不是自绘复选框列表。
// Win32 自绘 owner-draw 列表要处理 WM_DRAWITEM + 焦点框 + 高亮 + 键盘导航，
// 几百行且容易出视觉 bug；ListBox 多选是系统原生行为，选中态/键盘/滚动全免费，
// 视觉也与系统一致。代价是"已选项"不显示勾号而显示蓝色高亮——可接受。
// ─────────────────────────────────────────────────────────────────────────────

/// EnumWindows 的收集体（回调里不能分配/加锁以外的重活，故只存句柄）。
static VIS_HWNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// 当前可见窗口（弹窗 ListBox 的数据源）：(显示文本, exe 名)。
/// exe 名取不到（提权进程等）的条目照样显示，但 exe 字段为空串。
fn visible_windows() -> Vec<(String, String)> {
    *VIS_HWNDS.lock().unwrap() = Vec::new();
    unsafe {
        EnumWindows(Some(collect_visible), 0);
    }
    let hwnds = std::mem::take(&mut *VIS_HWNDS.lock().unwrap());

    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for h in hwnds {
        let exe = crate::winmod::exe_name(h).unwrap_or_default();
        // 同一 exe 的多个窗口只保留第一个（排除表按 exe 生效，列 N 个
        // 相同 exe 只会让用户困惑）。
        if !exe.is_empty() && seen.contains(&exe) {
            continue;
        }
        let title = window_title(h);
        // 标题与 exe 都为空 = 没什么可展示的（纯消息窗）。
        if title.is_empty() && exe.is_empty() {
            continue;
        }
        let shown = if title.is_empty() {
            format!("（无标题）  [{exe}]")
        } else {
            format!("{title}  [{exe}]")
        };
        if !exe.is_empty() {
            seen.push(exe.clone());
        }
        out.push((shown, exe));
    }
    // 按显示文本排序：稳定的列表顺序，用户第二次打开时条目不会乱跳。
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// EnumWindows 回调：只收"可见 + 非工具窗"的顶层窗。
unsafe extern "system" fn collect_visible(hwnd: HWND, _lparam: isize) -> i32 {
    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    // 过滤工具窗（WS_EX_TOOLWINDOW）：它们不该出现在用户可见列表里。
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    // 自己的两个窗口不进列表（设置窗与弹窗本身，选了也没意义）。
    if hwnd as isize == WIN.load(Ordering::Acquire) {
        return 1;
    }
    if let Some(p) = *PROC_PICK.lock().unwrap() {
        if p == hwnd as isize {
            return 1;
        }
    }
    VIS_HWNDS.lock().unwrap().push(hwnd as isize);
    1
}

/// 窗口标题（取不到返回空串）。
fn window_title(hwnd: isize) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd as HWND);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(hwnd as HWND, buf.as_mut_ptr(), buf.len() as i32);
        if got <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

/// 打开进程选择弹窗（模态：禁用父窗口，关闭时恢复）。
/// 必须在 main 线程调用（与 `open()` 同源）。
fn open_proc_picker() {
    unsafe {
        let parent = WIN.load(Ordering::Acquire);
        if parent == 0 {
            return;
        }
        // 已打开则前置，不重开第二个。
        if let Some(p) = *PROC_PICK.lock().unwrap() {
            if IsWindow(p as HWND) != 0 {
                ShowWindow(p as HWND, SW_SHOW);
                SetForegroundWindow(p as HWND);
                return;
            }
        }
        let instance = GetModuleHandleW(0 as PCWSTR);
        let class = wstr("anydrag-win-procpick");
        let cls = WNDCLASSEXW {
            cbSize: core::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(proc_pick_wnd_proc),
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
        if RegisterClassExW(&cls) == 0 && GetLastError() != 1410 {
            eprintln!("anydrag-win: RegisterClassExW(procpick) failed");
            return;
        }
        let dpi = GetDpiForSystem().max(96) as i32;
        let sc = |v: i32| v * dpi / 96;
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class.as_ptr(),
            wstr("选择要无视手势的进程").as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            sc(560),
            sc(440),
            parent as HWND,
            0 as _,
            instance,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            eprintln!("anydrag-win: CreateWindowExW(procpick) failed");
            return;
        }
        *PROC_PICK.lock().unwrap() = Some(hwnd as isize);
        // 模态：禁用父窗口，避免用户在弹窗开着的期间改主界面造成状态错乱。
        EnableWindow(parent as HWND, 0);
        SetForegroundWindow(hwnd);
    }
}

/// 进程选择弹窗的窗口过程。
unsafe extern "system" fn proc_pick_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let instance = GetModuleHandleW(0 as PCWSTR);
            // ListBox 多选，占满客户区上部。
            let list = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                wstr("LISTBOX").as_ptr(),
                std::ptr::null_mut() as PCWSTR,
                WS_CHILD
                    | WS_VISIBLE
                    | WS_VSCROLL
                    | LBS_MULTIPLESEL as u32
                    | LBS_HASSTRINGS as u32
                    | LBS_NOTIFY as u32,
                12,
                12,
                516,
                320,
                hwnd,
                ID_PROC_LIST as _,
                instance,
                std::ptr::null_mut(),
            );
            let dpi = GetDpiForSystem().max(96) as i32;
            let sc = |v: i32| v * dpi / 96;
            CreateWindowExW(
                0,
                wstr("BUTTON").as_ptr(),
                wstr("确定").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_DEFPUSHBUTTON as u32,
                sc(300),
                sc(348),
                sc(110),
                sc(30),
                hwnd,
                ID_PROC_OK as _,
                instance,
                std::ptr::null_mut(),
            );
            CreateWindowExW(
                0,
                wstr("BUTTON").as_ptr(),
                wstr("取消").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as u32,
                sc(420),
                sc(348),
                sc(110),
                sc(30),
                hwnd,
                IDCANCEL as _,
                instance,
                std::ptr::null_mut(),
            );
            PROC_LIST.store(list as isize, Ordering::Release);
            // 灌数据。已在名单里的 exe 默认选中。
            let current = config::snapshot().native_excluded;
            for (shown, exe) in visible_windows() {
                let t = wstr(&shown);
                let idx = SendMessageW(list, LB_ADDSTRING, 0, t.as_ptr() as isize);
                if !exe.is_empty() && current.iter().any(|e| e == &exe) {
                    SendMessageW(list, LB_SETSEL, 1, idx);
                }
            }
            0
        }
        WM_COMMAND => {
            let id = wp & 0xFFFF;
            if id == ID_PROC_OK {
                apply_proc_selection();
                close_proc_picker(hwnd);
            } else if id == IDCANCEL as usize {
                close_proc_picker(hwnd);
            }
            0
        }
        WM_CLOSE => {
            close_proc_picker(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// 把 ListBox 的选中项写回设置窗口的多行输入框（**不写盘**——用户还要点"保存"）。
///
/// 合并而非替换：弹窗里勾掉的项会从名单消失，但用户手填的、当前不可见的进程
/// （例如没开着的游戏）必须保留。所以是"选中集合 ∪ 手填集合里不在可见集合中的项"。
fn apply_proc_selection() {
    let lb = PROC_LIST.load(Ordering::Acquire);
    if lb == 0 {
        return;
    }
    let c = *CTL.lock().unwrap();
    // 整个函数体都在 unsafe 里：text_of/set_text 与 SendMessageW 都是 FFI。
    unsafe {
    // 手填部分：现有名单里，属于"当前不可见窗口"的条目要保留
    //（例如没开着的游戏，用户提前手填好了）。
    let mut keep: Vec<String> = parse_list(&text_of(c.excl));
    let visible_exes: Vec<String> = visible_windows()
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty())
        .collect();
    keep.retain(|e| !visible_exes.iter().any(|v| v == e));

    // 勾选部分：从 ListBox 的选中项里取 exe（显示文本形如 "标题  [exe]"）。
    let mut picked: Vec<String> = Vec::new();
    {
        let count = SendMessageW(lb as HWND, LB_GETCOUNT, 0, 0);
        for i in 0..count {
            if SendMessageW(lb as HWND, LB_GETSEL, i as usize, 0) <= 0 {
                continue;
            }
            let len = SendMessageW(lb as HWND, LB_GETTEXTLEN, i as usize, 0) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = SendMessageW(lb as HWND, LB_GETTEXT, i as usize, buf.as_mut_ptr() as isize);
            if got == LB_ERR as isize {
                continue;
            }
            let text = String::from_utf16_lossy(&buf[..got as usize]);
            if let (Some(s), Some(e)) = (text.rfind('['), text.rfind(']')) {
                if e > s {
                    picked.push(text[s + 1..e].to_string());
                }
            }
        }
    }
    let mut all = std::mem::take(&mut keep);
    for e in &picked {
        if !e.is_empty() && !all.contains(e) {
            all.push(e.clone());
        }
    }
    all.sort();
    all.dedup();
    set_text(c.excl, &all.join("\r\n"));
    }
}

/// 关闭进程选择弹窗并恢复父窗口。
fn close_proc_picker(hwnd: HWND) {
    unsafe {
        if let Some(parent) = PROC_PICK.lock().unwrap().take() {
            if IsWindow(parent as HWND) != 0 {
                EnableWindow(parent as HWND, 1);
            }
        }
        PROC_LIST.store(0, Ordering::Release);
        DestroyWindow(hwnd);
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
        cfg.native_excluded = parse_list(&text_of(c.excl));
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
