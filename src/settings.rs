//! 设置界面（托盘「设置…」打开）：**Win32 原生窗口 + 自绘控件**，不引任何 GUI 框架。
//!
//! ## 为什么大部分控件是自绘的（2026-10-23 重做）
//!
//! 旧版全用原生控件（`BS_AUTOCHECKBOX` / `WS_EX_CLIENTEDGE` 的 EDIT /
//! `BS_PUSHBUTTON` / `STATIC`），观感是 Windows 95 —— 用户原话"控件样式也很丑"。
//! 实测（SPEC §2.3）确认了三条硬事实，决定了这里的控件选型：
//!
//! | 控件 | 原生能否跟随深色 | 本文件的处理 |
//! |---|---|---|
//! | `EDIT` | **能**（套 `DarkMode_Explorer`） | 保留原生（键盘/光标/选区全免费），但去掉 `WS_EX_CLIENTEDGE`，边框与底色由父窗口自绘（8px 圆角） |
//! | `BUTTON` | 能 | **改自绘**：原生按钮的圆角是系统固定的 4px，做不到 SPEC §3.2 要求的 8px（V14） |
//! | `CHECKBOX` | **不能**，套主题后是"白底浅字"（比不套更糟） | 自绘 `BS_OWNERDRAW`（方框 4px 圆角 + 焦点环） |
//! | `STATIC` | **不能** | 全部改自绘 `DrawTextW` |
//!
//! > **与 SPEC §3.3 的偏差（已回报到 SPEC）**：§3.3 写"主按钮 `BS_DEFPUSHBUTTON`
//! > + `DarkMode_Explorer`"，但 §3.2 与 V14 又要求按钮圆角 8px。原生按钮做不到
//! > 8px 圆角，两条硬约束冲突，取 §3.2/V14（可见的视觉结果优先），
//! > 所以按钮也是自绘的。复选框的键盘空格、`BN_CLICKED`、焦点态仍由系统控件
//! > 骨架提供 —— `BS_OWNERDRAW` 只替换"怎么画"，不替换行为。
//!
//! ## 布局
//!
//! 所有坐标来自 [`crate::layout`]（纯逻辑、可单测）。本文件**不写坐标字面量** ——
//! 旧版把标签坐标和卡片矩形分别硬编码，导致标签画进上一张卡片（SPEC U1）。
//!
//! ## 语言
//!
//! 文案全部走 [`crate::i18n::tr`]。切换语言只更新本窗口的 `UI_LANG` + 重绘，
//! **不重建窗口**（重建会丢用户已输入未保存的内容，SPEC D3）。
//! 语言**只在点「保存」时写进配置** —— 点「取消」自然回滚（SPEC V8）。
//!
//! ## 保存即生效
//!
//! 读控件 → 组装 `Config` → `config::replace()`（版本号自增，engine 下一个事件批次
//! 刷新快照并同步钩子监视键）→ `Config::save()` 写盘 → `hook::wake_engine()` 唤醒引擎。
//! 最后那步**不能省**（2026-10-21 事故）：引擎空闲时阻塞在 `GetMessageW`，
//! 不显式唤醒的话，用户点完保存若没动鼠标，新配置要等下一次鼠标事件才生效。
//!
//! ## 本窗口不会被自己的手势拖动
//!
//! `winmod::resolve_target` 已跳过 `anydrag-win.exe`。

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

use windows_sys::core::PCWSTR;
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, FillRect, GetStockObject, LineTo, MoveToEx, RoundRect,
    SelectObject, SetBkColor, SetBkMode, SetTextColor, DEFAULT_CHARSET, DT_CENTER,
    DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HDC, NULL_BRUSH, PS_SOLID, SRCCOPY,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DrawIconEx, EnumWindows, GetClientRect,
    GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, IsWindow, IsWindowVisible, LoadCursorW,
    RegisterClassExW, SendMessageW, SetForegroundWindow, SetWindowTextW, ShowWindow,
    BS_DEFPUSHBUTTON, BS_OWNERDRAW, BS_PUSHBUTTON, CW_USEDEFAULT, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_NUMBER, ES_WANTRETURN, GWL_EXSTYLE, IDCANCEL, IDC_ARROW,
    SW_SHOW, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_DESTROY, WM_DRAWITEM,
    WM_ERASEBKGND, WM_SETFONT, WM_SETTINGCHANGE, MSG, WNDCLASSEXW, WS_CAPTION,
    WM_KEYDOWN, WM_KEYUP, WM_CHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_SYSCHAR, WM_NEXTDLGCTL,
    WS_CHILD, WS_EX_DLGMODALFRAME, WS_EX_TOOLWINDOW, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
    WS_VISIBLE, WS_VSCROLL,
    // ListBox（进程选择弹窗用）：多选 + 系统原生选中态，省掉自绘 owner-draw 列表。
    LB_ADDSTRING, LB_ERR, LB_GETCOUNT, LB_GETSEL, LB_GETTEXT, LB_GETTEXTLEN, LB_SETSEL,
    LBS_HASSTRINGS, LBS_MULTIPLESEL, LBS_NOTIFY,
};

use crate::config;
use crate::i18n::{self, Key, Lang};
use crate::layout::{self, Rect};
use crate::theme::{self, Palette};

/// 复选框勾选值（windows-sys 0.61 只导出了 BST_FOCUS/BST_PUSHED，勾选态自己定义）。
/// **owner-draw 的 BUTTON 不再维护勾选位**（SPEC R11），这个常量只给
/// 高对比度退回原生外观的那条路径用。
const BST_CHECKED_V: usize = 1;

const ID_SAVE: usize = 1;
const ID_CANCEL: usize = 2;
/// 设置窗口内「选择进程…」按钮。
const ID_PICK_PROCS: usize = 3;
/// 语言分段按钮。
const ID_LANG_ZH: usize = 4;
const ID_LANG_EN: usize = 5;
/// 复选框 id 起点：`ID_CHECK_BASE + i`，`i` 对应 [`CheckIdx`] 的顺序。
const ID_CHECK_BASE: usize = 20;
/// 复选框数量（4 个拖动键 + 4 个缩放键 + 1 个中键开关）。
const CHECK_COUNT: usize = 9;

/// 进程选择弹窗内的控件/按钮。
const ID_PROC_LIST: usize = 10;
const ID_PROC_OK: usize = 11;

/// 设置窗口句柄（0 = 未打开）。只允许一个实例。
static WIN: AtomicIsize = AtomicIsize::new(0);
/// 窗口当前是否用深色主题（`WM_SETTINGCHANGE` 时重取）。
static DARK: AtomicBool = AtomicBool::new(false);
/// 高对比度开关（创建控件时决定"自绘还是退回原生"，见 `high_contrast`）。
static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);
/// 界面语言。**窗口本地状态**，只在点「保存」时写进配置 ——
/// 这样点「取消」天然回滚，不需要额外的回滚逻辑（SPEC V8）。
static UI_LANG: Mutex<Lang> = Mutex::new(Lang::Zh);

/// 用户在这次打开窗口期间**是否点过语言按钮**。
///
/// 决定「保存」时往配置里写什么：点过 → 写显式 `"zh"`/`"en"`；
/// 没点过 → **保留原值**（可能是空串 = 跟随系统）。
///
/// 少这一位就会静默丢失"跟随系统"：用户打开设置窗口、什么都没改、点保存，
/// 配置里的 `lang: ""` 被固化成 `"zh"`，此后他改系统语言界面也不跟着变了 ——
/// 界面上完全看不出来，只是某天开始"不跟随系统了"。
static LANG_TOUCHED: AtomicBool = AtomicBool::new(false);

/// 复选框的勾选状态。owner-draw 的 BUTTON 不维护 `BM_GETCHECK`，
/// 所以状态由我们自己存（SPEC R11 / §3.3 最后一行）。
static CHECKS: Mutex<[bool; CHECK_COUNT]> = Mutex::new([false; CHECK_COUNT]);

/// 复选框在 [`CHECKS`] 里的下标。**顺序即界面顺序**，与 `create_controls` 一致。
#[derive(Clone, Copy)]
enum CheckIdx {
    DragAlt = 0,
    DragCtrl = 1,
    DragShift = 2,
    DragWin = 3,
    RzAlt = 4,
    RzCtrl = 5,
    RzShift = 6,
    RzWin = 7,
    MidOn = 8,
}

/// 窗口字体（创建一次，`WM_DESTROY` 时销毁）。
#[derive(Clone, Copy, Default)]
struct Fonts {
    title: isize,
    card: isize,
    body: isize,
    caption: isize,
}

static FONTS: Mutex<Fonts> = Mutex::new(Fonts {
    title: 0,
    card: 0,
    body: 0,
    caption: 0,
});

/// 进程选择弹窗的父窗口（设置窗口）句柄；None = 未打开。
///
/// **ListBox 句柄为什么单独放一个静态**：`CreateWindowExW` 会**同步**触发
/// `WM_CREATE`，而此时外层还没执行到"把弹窗句柄存进 PROC_PICK"那一步——
/// 第一版在 WM_CREATE 里写 `PROC_PICK` 的 `Some((_, lb))`，结果永远写不进去，
/// `lb` 恒为 0，点"确定"时 `apply_proc_selection` 直接 return，名单纹丝不动。
/// 拆成两个静态后 WM_CREATE 只管写自己那个，没有时序依赖。
static PROC_PICK: Mutex<Option<isize>> = Mutex::new(None);
static PROC_LIST: AtomicIsize = AtomicIsize::new(0);

/// 控件句柄表。`HWND` 是裸指针（非 `Send`），放不进 `static Mutex`，故统一存 `isize`。
#[derive(Clone, Copy)]
struct Ctls {
    thr: isize,
    dbl: isize,
    mid_thr: isize,
    excl: isize,
    /// 复选框：下标同 [`CheckIdx`]。
    checks: [isize; CHECK_COUNT],
    btn_save: isize,
    btn_cancel: isize,
    btn_pick: isize,
    lang_zh: isize,
    lang_en: isize,
}

const ZERO: Ctls = Ctls {
    thr: 0,
    dbl: 0,
    mid_thr: 0,
    excl: 0,
    checks: [0; CHECK_COUNT],
    btn_save: 0,
    btn_cancel: 0,
    btn_pick: 0,
    lang_zh: 0,
    lang_en: 0,
};

static CTL: Mutex<Ctls> = Mutex::new(ZERO);

// ── Win11 DWM 窗口效果常量（未文档化但数十年稳定的属性 id）──
/// DWMWA_USE_IMMERSIVE_DARK_MODE：标题栏明暗（Win10 1809+）。
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
/// DWMWA_WINDOW_CORNER_PREFERENCE：圆角窗口（Win11 22000+）。DWMWCP_ROUND = 2。
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 取当前界面语言（窗口打开期间用 `UI_LANG`，否则按配置 + 系统语言现算）。
fn lang() -> Lang {
    *UI_LANG.lock().unwrap_or_else(|p| p.into_inner())
}

fn palette() -> &'static Palette {
    theme::palette(DARK.load(Ordering::Acquire))
}

// ─────────────────────────────────────────────────────────────────────────────
// 字体
// ─────────────────────────────────────────────────────────────────────────────

/// 建一套字体。**字体名只写 `Segoe UI`**：Windows 的 font linking 默认已配好，
/// 汉字会自动落到 Microsoft YaHei UI 渲染（SPEC §2.1 / R2）。实测 144DPI 下
/// 中文显示正常；若某机器上出现方块，把 face 改成 `Microsoft YaHei UI` 即可。
unsafe fn make_font(size_px: i32, weight: i32, dpi: i32) -> isize {
    let face = wstr("Segoe UI");
    // lfHeight 取负 = 字符高度（不含内部行距），与 CSS 的 font-size 语义接近。
    let h = -(size_px * dpi / 96);
    let f = CreateFontW(
        h,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        0,
        0,
        face.as_ptr(),
    );
    f as isize
}

unsafe fn build_fonts(dpi: i32) {
    let mut f = Fonts {
        title: make_font(theme::FONT_TITLE, theme::WEIGHT_SEMIBOLD, dpi),
        card: make_font(theme::FONT_CARD_TITLE, theme::WEIGHT_SEMIBOLD, dpi),
        body: make_font(theme::FONT_BODY, theme::WEIGHT_REGULAR, dpi),
        caption: make_font(theme::FONT_CAPTION, theme::WEIGHT_REGULAR, dpi),
    };
    let mut slot = FONTS.lock().unwrap_or_else(|p| p.into_inner());
    // 先销毁旧的（DPI 变化时重建），避免每次切 DPI 泄漏 4 个 HFONT。
    for old in [slot.title, slot.card, slot.body, slot.caption] {
        if old != 0 {
            DeleteObject(old as *mut c_void);
        }
    }
    *slot = f;
    f = *slot;
    let _ = f;
}

unsafe fn destroy_fonts() {
    let mut slot = FONTS.lock().unwrap_or_else(|p| p.into_inner());
    for old in [slot.title, slot.card, slot.body, slot.caption] {
        if old != 0 {
            DeleteObject(old as *mut c_void);
        }
    }
    *slot = Fonts::default();
}

fn font(which: fn(&Fonts) -> isize) -> isize {
    let f = *FONTS.lock().unwrap_or_else(|p| p.into_inner());
    which(&f)
}

// ─────────────────────────────────────────────────────────────────────────────
// 自绘
// ─────────────────────────────────────────────────────────────────────────────

/// 画一段文字（单行、垂直居中、可省略号）。`rect` 是**逻辑**坐标，内部按 DPI 缩放。
unsafe fn text(
    hdc: HDC,
    f: isize,
    color: u32,
    r: Rect,
    dpi: i32,
    s: &str,
    center: bool,
) {
    let sr = r.scaled(dpi);
    let mut rc = RECT {
        left: sr.x,
        top: sr.y,
        right: sr.right(),
        bottom: sr.bottom(),
    };
    let old = SelectObject(hdc, f as *mut c_void);
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, color);
    let w = wstr(s);
    let mut flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
    if center {
        flags |= DT_CENTER;
    }
    DrawTextW(hdc, w.as_ptr(), w.len() as i32 - 1, &mut rc, flags);
    SelectObject(hdc, old);
}

/// 圆角矩形（填充 + 描边）。`border_w` = 0 表示不描边。
unsafe fn round_rect(hdc: HDC, r: Rect, dpi: i32, fill: u32, border: u32, border_w: i32, radius: i32) {
    let sr = r.scaled(dpi);
    let brush = CreateSolidBrush(fill);
    let pen = if border_w > 0 {
        CreatePen(PS_SOLID, border_w, border)
    } else {
        // NULL_PEN 需要 GetStockObject；这里用同色画笔绕过（少一个 import）。
        CreatePen(PS_SOLID, 1, fill)
    };
    let ob = SelectObject(hdc, brush as *mut c_void);
    let op = SelectObject(hdc, pen as *mut c_void);
    // RoundRect 的椭圆宽高是**直径**，所以传 2×radius。
    let d = (radius * dpi / 96) * 2;
    RoundRect(hdc, sr.x, sr.y, sr.right(), sr.bottom(), d, d);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    DeleteObject(brush as *mut c_void);
    DeleteObject(pen as *mut c_void);
}

/// 一条 1px 水平线（分隔线）。
unsafe fn hline(hdc: HDC, x1: i32, x2: i32, y: i32, dpi: i32, color: u32) {
    let s = |v: i32| v * dpi / 96;
    let pen = CreatePen(PS_SOLID, 1, color);
    let op = SelectObject(hdc, pen as *mut c_void);
    MoveToEx(hdc, s(x1), s(y), std::ptr::null_mut());
    LineTo(hdc, s(x2), s(y));
    SelectObject(hdc, op);
    DeleteObject(pen as *mut c_void);
}

/// 自绘整个客户区：窗口底 → 分隔线 → 卡片 → 标题/说明/标签/单位 → 输入框边框 → 提示。
///
/// **顺序很重要**：先铺底再画卡片，最后画文字，避免文字被后画的矩形盖住。
unsafe fn paint_background(hwnd: HWND, hdc: HDC) {
    let mut rc: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rc);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let dpi = crate::winmod::dpi_for_window(hwnd as isize);
    let p = palette();
    let l = layout::compute();
    let lang = lang();
    let t = |k: Key| i18n::tr(lang, k);

    // 双缓冲：先画进内存 DC，再一次 BitBlt —— 否则自绘 + 子控件会明显闪烁。
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem, bmp);

    let bg_brush = CreateSolidBrush(p.window_bg);
    FillRect(mem, &rc, bg_brush);
    DeleteObject(bg_brush as *mut c_void);

    // ── 标题区 ──────────────────────────────
    // 图标（与托盘同源的自绘箭头，启用态配色）。
    let hicon = crate::icon::tray_icon(true);
    if hicon != 0 {
        let ir = l.icon.scaled(dpi);
        DrawIconEx(
            mem,
            ir.x,
            ir.y,
            hicon as *mut c_void,
            ir.w,
            ir.h,
            0,
            std::ptr::null_mut(),
            0,
        );
        crate::icon::destroy(hicon);
    }
    let f_title = font(|f| f.title);
    let f_card = font(|f| f.card);
    let f_body = font(|f| f.body);
    let f_cap = font(|f| f.caption);

    text(mem, f_title, p.text_primary, l.title, dpi, t(Key::WinTitle), false);
    text(
        mem,
        f_cap,
        p.text_secondary,
        l.subtitle,
        dpi,
        t(Key::Subtitle),
        false,
    );

    // 语言分段控件的容器（两个分段按钮是子窗口，画在它上面）。
    round_rect(
        mem,
        l.lang_box,
        dpi,
        p.field_bg,
        p.field_border,
        1,
        theme::RADIUS_FIELD,
    );

    // ── 分隔线 ──────────────────────────────
    hline(mem, l.cards[0].rect.x, l.cards[0].rect.right(), l.sep_top, dpi, p.separator);
    hline(
        mem,
        l.cards[0].rect.x,
        l.cards[0].rect.right(),
        l.sep_bottom,
        dpi,
        p.separator,
    );

    // ── 四张卡片 ────────────────────────────
    for card in &l.cards {
        round_rect(
            mem,
            card.rect,
            dpi,
            p.card_bg,
            p.card_border,
            1,
            theme::RADIUS_CARD,
        );
        text(
            mem,
            f_card,
            p.text_primary,
            card.title,
            dpi,
            card_title_text(lang, card_index(&l.cards, card)),
            false,
        );
    }
    // 卡 4 的说明行（取代旧版游离在卡片外的那个标签 —— SPEC U1/U1b）。
    if let Some(desc) = l.cards[3].desc {
        text(
            mem,
            f_cap,
            p.text_tertiary,
            desc,
            dpi,
            t(Key::CardExcludedDesc),
            false,
        );
    }

    // ── 行标签 ──────────────────────────────
    for (r, k) in [
        (l.lbl_drag_keys, Key::LblTriggerKeys),
        (l.lbl_resize_keys, Key::LblTriggerKeys),
        (l.lbl_threshold, Key::LblDragThreshold),
        (l.lbl_double_click, Key::LblDoubleClick),
        (l.lbl_middle_snap, Key::LblMiddleSnap),
        (l.lbl_mid_threshold, Key::LblMiddleThreshold),
    ] {
        text(mem, f_body, p.text_primary, r, dpi, t(k), false);
    }

    // ── 单位后缀（跟着输入框，SPEC U6 / V9）──
    for (r, k) in [
        (l.unit_threshold, Key::UnitPx),
        (l.unit_double_click, Key::UnitMsSystem),
        (l.unit_mid_threshold, Key::UnitPx),
    ] {
        text(mem, f_body, p.text_secondary, r, dpi, t(k), false);
    }

    // ── 输入框边框（EDIT 自身无边框，见 create_controls）──
    for r in [
        l.edit_threshold,
        l.edit_double_click,
        l.edit_mid_threshold,
        l.edit_excluded,
    ] {
        round_rect(
            mem,
            r,
            dpi,
            p.field_bg,
            p.field_border,
            1,
            theme::RADIUS_FIELD,
        );
    }

    // ── 操作区提示 ──────────────────────────
    text(
        mem,
        f_cap,
        p.text_secondary,
        l.hint,
        dpi,
        t(Key::HintImmediate),
        false,
    );

    BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old_bmp);
    DeleteObject(bmp as *mut c_void);
    DeleteDC(mem);
}

/// 卡片下标（`paint_background` 里给每张卡取标题用）。
fn card_index(cards: &[layout::Card; 4], card: &layout::Card) -> usize {
    cards
        .iter()
        .position(|c| c.rect == card.rect)
        .unwrap_or(0)
}

/// 卡片标题文案。顺序与 `layout::compute` 里的 4 张卡一致。
fn card_title_text(lang: Lang, idx: usize) -> &'static str {
    let key = match idx {
        0 => Key::CardDrag,
        1 => Key::CardResize,
        2 => Key::CardParams,
        _ => Key::CardExcluded,
    };
    i18n::tr(lang, key)
}

/// 控件背后该用什么底色。
///
/// **为什么需要**：`BS_OWNERDRAW` 按钮在发 `WM_DRAWITEM` **之前**，系统会先用
/// `COLOR_BTNFACE`（浅灰 `#F0F0F0`）把控件矩形填一遍。我们的卡片是白色的，
/// 于是复选框文字背后出现一块**灰方块**（第一版截图里一眼可见）—— 那正是
/// Windows 95 观感的残留。修法是在 `draw_item` 开头自己把整块矩形按真实背景
/// 重填一遍，顺带让圆角外侧也是对的颜色。
fn control_bg(id: usize) -> u32 {
    let p = palette();
    match id {
        // 语言分段在 field_bg 的容器里。
        ID_LANG_ZH | ID_LANG_EN => p.field_bg,
        // 复选框全部落在卡片上。
        id if (ID_CHECK_BASE..ID_CHECK_BASE + CHECK_COUNT).contains(&id) => p.card_bg,
        // 「选择进程…」在卡片 4 上。
        ID_PICK_PROCS => p.card_bg,
        // 取消 / 保存 在窗口底的按钮行上。
        _ => p.window_bg,
    }
}

/// `WM_DRAWITEM`：自绘按钮 / 复选框 / 语言分段。
///
/// `BS_OWNERDRAW` 只替换**怎么画**，不替换行为 —— 鼠标点击、键盘空格、
/// `BN_CLICKED`、`WS_TABSTOP` 焦点全由系统按钮骨架负责（SPEC §3.3）。
unsafe fn draw_item(lp: LPARAM) -> LRESULT {
    let dis = &*(lp as *const DRAWITEMSTRUCT);
    let hdc = dis.hDC;
    let dpi = {
        let w = WIN.load(Ordering::Acquire);
        if w != 0 {
            crate::winmod::dpi_for_window(w)
        } else {
            96
        }
    };
    let p = palette();
    let lang = lang();
    let id = dis.CtlID as usize;
    // `rcItem` 是**设备像素**（自绘控件拿到的就是最终矩形），而本文件所有绘制
    // 辅助函数都吃逻辑像素并在内部再缩放一次 —— 所以这里必须先换回逻辑像素，
    // 否则 150% DPI 下会被二次放大、画到控件外面去。
    let item = Rect::new(
        dis.rcItem.left,
        dis.rcItem.top,
        dis.rcItem.right - dis.rcItem.left,
        dis.rcItem.bottom - dis.rcItem.top,
    )
    .unscaled(dpi);
    let disabled = dis.itemState & ODS_DISABLED != 0;
    let focused = dis.itemState & ODS_FOCUS != 0;
    let selected = dis.itemState & ODS_SELECTED != 0;

    // 高对比度下不自绘（保留用户配色，SPEC §9）。创建时已按 HC 选了原生样式，
    // 这里只是兜底：万一样式没换成功，也别用我们的调色板盖掉系统配色。
    if HIGH_CONTRAST.load(Ordering::Acquire) {
        return 1;
    }

    // **先把控件矩形按真实背景重填**：系统在 WM_DRAWITEM 之前已经用
    // COLOR_BTNFACE（浅灰）刷过一遍，不覆盖的话卡片上会出现灰方块。
    {
        let bg = CreateSolidBrush(control_bg(id));
        FillRect(hdc, &dis.rcItem, bg);
        DeleteObject(bg as *mut c_void);
    }

    match id {
        // ── 语言分段（选中态用 accent 填充）──
        ID_LANG_ZH | ID_LANG_EN => {
            let is_selected = (id == ID_LANG_ZH && lang == Lang::Zh)
                || (id == ID_LANG_EN && lang == Lang::En);
            if is_selected {
                round_rect(
                    hdc,
                    item,
                    dpi,
                    p.accent,
                    p.accent,
                    0,
                    theme::RADIUS_CHECK + 2,
                );
            }
            let label = i18n::tr(
                lang,
                if id == ID_LANG_ZH {
                    Key::LangZh
                } else {
                    Key::LangEn
                },
            );
            let color = if is_selected {
                p.text_on_accent
            } else {
                p.text_secondary
            };
            text(hdc, font(|f| f.body), color, item, dpi, label, true);
            if focused {
                focus_ring(hdc, item, dpi, p.focus_ring);
            }
            1
        }
        // ── 复选框（方框 4px 圆角 + 勾 + 焦点环）──
        id if (ID_CHECK_BASE..ID_CHECK_BASE + CHECK_COUNT).contains(&id) => {
            let idx = id - ID_CHECK_BASE;
            let on = CHECKS.lock().unwrap_or_else(|x| x.into_inner())[idx];
            draw_checkbox(hdc, item, dpi, p, lang, idx, on, focused, disabled);
            1
        }
        // ── 普通按钮 ──
        _ => {
            let primary = id == ID_SAVE;
            let (fill, border, fg) = if disabled {
                (p.button_bg, p.button_border, p.text_disabled)
            } else if primary {
                // 悬停/按下的视觉差异用 accent_hover —— 没有鼠标追踪时至少让
                // "按下去"有反馈（ODS_SELECTED）。
                let fill = if selected { p.accent_hover } else { p.accent };
                (fill, fill, p.text_on_accent)
            } else {
                (p.button_bg, p.button_border, p.text_primary)
            };
            round_rect(hdc, item, dpi, fill, border, 1, theme::RADIUS_FIELD);
            let label = match id {
                ID_SAVE => i18n::tr(lang, Key::BtnSave),
                ID_CANCEL => i18n::tr(lang, Key::BtnCancel),
                ID_PICK_PROCS => i18n::tr(lang, Key::BtnPickProcs),
                _ => "",
            };
            text(hdc, font(|f| f.body), fg, item, dpi, label, true);
            if focused {
                focus_ring(hdc, item, dpi, p.focus_ring);
            }
            1
        }
    }
}

/// 2px 焦点环（DSH `--dsw-focus-ring-width`）。**自绘控件必须画它** ——
/// 原生控件有、自绘没有 = 键盘用户失去位置指示（SPEC §2.4.4 / V12）。
unsafe fn focus_ring(hdc: HDC, r: Rect, dpi: i32, color: u32) {
    let inset = r;
    // 用描边画笔的 RoundRect 画环：比手画四条线简单，且圆角一致。
    let sr = inset.scaled(dpi);
    let brush = CreateSolidBrush(color);
    let pen = CreatePen(PS_SOLID, theme::FOCUS_RING_W, color);
    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    let op = SelectObject(hdc, pen as *mut c_void);
    let d = (theme::RADIUS_FIELD * dpi / 96) * 2;
    RoundRect(hdc, sr.x, sr.y, sr.right(), sr.bottom(), d, d);
    SelectObject(hdc, ob);
    SelectObject(hdc, op);
    DeleteObject(brush as *mut c_void);
    DeleteObject(pen as *mut c_void);
}

/// 画一个复选框：方框（16×16，4px 圆角）+ 勾 + 文字。
///
/// 方框用 `theme::RADIUS_CHECK`（4px，DSH `--dsw-radius-xs`），文字用正文色 ——
/// 深浅两套主题下都可读（配色见 `theme::Palette`，对比度有单测钉住）。
unsafe fn draw_checkbox(
    hdc: HDC,
    r: Rect,
    dpi: i32,
    p: &Palette,
    lang: Lang,
    idx: usize,
    on: bool,
    focused: bool,
    disabled: bool,
) {
    let box_px = 16;
    let gap = 8;
    // 方框在行内垂直居中（逻辑坐标算，最后统一缩放）。
    let by = r.y + (r.h - box_px) / 2;
    let bx = Rect::new(r.x, by, box_px, box_px);

    let (box_fill, box_border) = if disabled {
        (p.card_bg, p.button_border)
    } else if on {
        (p.checkbox_checked, p.checkbox_checked)
    } else {
        (p.field_bg, p.checkbox_box)
    };
    round_rect(
        hdc,
        bx,
        dpi,
        box_fill,
        box_border,
        if on || disabled { 0 } else { 1 },
        theme::RADIUS_CHECK,
    );

    if on && !disabled {
        // 勾：两段折线。用逻辑坐标算再缩放，保证各 DPI 下比例一致。
        let s = |v: i32| v * dpi / 96;
        let (x0, y0) = (bx.x + 4, bx.y + 8);
        let (x1, y1) = (bx.x + 7, bx.y + 11);
        let (x2, y2) = (bx.x + 12, bx.y + 5);
        let pen = CreatePen(PS_SOLID, 2, p.check_mark);
        let op = SelectObject(hdc, pen as *mut c_void);
        MoveToEx(hdc, s(x0), s(y0), std::ptr::null_mut());
        LineTo(hdc, s(x1), s(y1));
        LineTo(hdc, s(x2), s(y2));
        SelectObject(hdc, op);
        DeleteObject(pen as *mut c_void);
    }

    // 文字：方框右侧，垂直居中。
    let label_r = Rect::new(bx.right() + gap, r.y, r.w - box_px - gap, r.h);
    let color = if disabled {
        p.text_disabled
    } else {
        p.text_primary
    };
    text(hdc, font(|f| f.body), color, label_r, dpi, check_label(lang, idx), false);

    if focused {
        // 焦点环画在**整个控件**外框上（含文字），与原生复选框的焦点虚线一致。
        focus_ring(hdc, r, dpi, p.focus_ring);
    }
}

/// 复选框的可读文字（`draw_checkbox` 的调用方负责画，见 `draw_check`）。
fn check_label(lang: Lang, idx: usize) -> &'static str {
    match idx {
        0..=3 => i18n::modifier_label(i18n::MODIFIER_VKS[idx]),
        4..=7 => i18n::modifier_label(i18n::MODIFIER_VKS[idx - 4]),
        8 => i18n::tr(lang, Key::ChkEnable),
        _ => "",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 窗口
// ─────────────────────────────────────────────────────────────────────────────

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

        // 语言与主题在开窗前定好：控件创建时就按它们选样式/文字。
        let cfg = config::snapshot();
        *UI_LANG.lock().unwrap_or_else(|p| p.into_inner()) = crate::winmod::ui_lang(&cfg);
        // 每次打开窗口都从"没碰过语言"开始：取消/关闭即回滚（SPEC V8）。
        LANG_TOUCHED.store(false, Ordering::Release);
        DARK.store(crate::winmod::system_dark_mode(), Ordering::Release);
        HIGH_CONTRAST.store(crate::winmod::high_contrast(), Ordering::Release);

        // 建窗口**之前**只能用系统 DPI（此刻还没有 hwnd）。
        // 这里曾经写成 `dpi_for_window(0)`，而那个函数当时对 0 返回 96 ——
        // 在 150% 的机器上窗口按 96 算尺寸、WM_CREATE 按 144 布局，
        // 结果右半边和下半边被窗口边缘裁掉（截图里一眼可见）。
        let dpi = crate::winmod::system_dpi();
        let l = layout::compute();
        if std::env::var_os("ANYDRAG_DEBUG").is_some() {
            eprintln!(
                "[settings] dpi={dpi} client={}x{}",
                l.client_w * dpi / 96,
                l.client_h * dpi / 96
            );
        }
        // CreateWindowExW 的宽高是**含标题栏+边框的整个窗口**尺寸，不是客户区。
        // 用 AdjustWindowRectExForDpi 从客户区反推，才不会像旧版那样靠手填
        // 700 这种魔数（旧版内容只到 636，底部留了 250px 死白 —— SPEC U2）。
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: l.client_w * dpi / 96,
            bottom: l.client_h * dpi / 96,
        };
        let _ = windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut frame,
            style,
            0,
            0,
            dpi as u32,
        );
        let title = wstr(i18n::tr(lang(), Key::WinTitle));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            frame.right - frame.left,
            frame.bottom - frame.top,
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

/// 给窗口套上 Win11 现代效果（WM_CREATE 里调用，窗口已存在、尚未显示）。
///
/// - 圆角窗口（Win11 22000+；旧系统/远程桌面被 DWM 忽略时无副作用）
/// - 标题栏明暗跟随系统（深色主题下不跟随就是"深内容 + 浅标题栏"的撕裂）
unsafe fn apply_modern_window_style(hwnd: HWND) {
    let corner = DWMWCP_ROUND;
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const u32 as *const _,
        std::mem::size_of::<u32>() as u32,
    );
    let dark: u32 = if DARK.load(Ordering::Acquire) { 1 } else { 0 };
    DwmSetWindowAttribute(
        hwnd,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
        &dark as *const u32 as *const _,
        std::mem::size_of::<u32>() as u32,
    );
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            apply_modern_window_style(hwnd);
            build_fonts(crate::winmod::dpi_for_window(hwnd as isize));
            create_controls(hwnd);
            0
        }
        // 自绘整窗背景（卡片、标题、标签、单位后缀、输入框边框）。
        WM_ERASEBKGND => {
            paint_background(hwnd, wp as HDC);
            1
        }
        // 自绘控件（按钮 / 复选框 / 语言分段）。
        WM_DRAWITEM => draw_item(lp),
        // EDIT 的底色与文字色（深色下必须是深底浅字，否则就是"白底浅字"——SPEC V10）。
        WM_CTLCOLOREDIT => {
            let p = palette();
            let hdc = wp as HDC;
            SetTextColor(hdc, p.text_primary);
            SetBkColor(hdc, p.field_bg);
            field_brush(p.field_bg)
        }
        // 键盘 Tab 焦点循环由消息泵里的 `IsDialogMessageW` 驱动（见
        // `settings::is_dialog_message`）。主窗口**不**处理 `WM_GETDLGCODE` ——
        // 那是子控件对对话框管理器的协议，窗口过程里处理它没有意义，
        // 落到下面的 `_ => DefWindowProcW` 即可。
        // **`WM_NEXTDLGCTL` 必须自己实现**（2026-10-23 实测，这是 Tab 焦点循环的
        // 关键一环）：`IsDialogMessageW` 遇到 Tab 时不直接调 `SetFocus`，而是
        // 给父窗口发 `WM_NEXTDLGCTL`。**真正的对话框**（`DialogBox` 建的）由
        // 系统内部的对话框过程处理这条消息；我们这种普通 `WS_OVERLAPPED` 窗口
        // 落到 `DefWindowProcW` 就是**什么都不做** ——
        // 症状：`IsDialogMessageW` 返回 1（消息被"消费"了），但焦点一动不动。
        // 实测就是卡在这里：第一次 Tab 能把焦点送进第一个复选框（系统初始化时
        // 顺手做的），之后再按完全不动，键盘用户走不出第一个控件。
        WM_NEXTDLGCTL => {
            next_dlg_ctl(hwnd, wp, lp);
            0
        }
        WM_COMMAND => {
            let id = wp & 0xFFFF;
            let code = (wp >> 16) & 0xFFFF;
            match id {
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
                    // 语言只存在窗口本地状态里，销毁窗口即回滚（SPEC V8）。
                    DestroyWindow(hwnd);
                }
                ID_LANG_ZH | ID_LANG_EN => {
                    if code == 0 {
                        // BN_CLICKED
                        let new = if id == ID_LANG_ZH { Lang::Zh } else { Lang::En };
                        if new != lang() {
                            *UI_LANG.lock().unwrap_or_else(|p| p.into_inner()) = new;
                            LANG_TOUCHED.store(true, Ordering::Release);
                            retranslate(hwnd);
                        }
                    }
                }
                id if (ID_CHECK_BASE..ID_CHECK_BASE + CHECK_COUNT).contains(&id) => {
                    // BN_CLICKED：翻转我们自己维护的勾选位（owner-draw 不维护 BM_GETCHECK）。
                    let idx = id - ID_CHECK_BASE;
                    {
                        let mut c = CHECKS.lock().unwrap_or_else(|p| p.into_inner());
                        c[idx] = !c[idx];
                    }
                    // 让控件重画（BS_OWNERDRAW 收到 WM_DRAWITEM）。
                    windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                        id as HWND,
                        std::ptr::null(),
                        1,
                    );
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
            destroy_fonts();
            // 回收 WM_CTLCOLOREDIT 缓存的画刷（GDI 对象不会随窗口销毁自动释放）。
            drop_field_brush();
            0
        }
        // 系统主题/语言/高对比度变化：重取并重绘（SPEC V11：不重启就跟着变）。
        WM_SETTINGCHANGE => {
            let immersive = lp != 0 && {
                // lParam 指向 "ImmersiveColorSet" 时才是配色变化（其它 settingchange
                // 很频繁，每条都重绘会闪）。
                let p = lp as *const u16;
                let mut ok = true;
                for (i, want) in "ImmersiveColorSet".encode_utf16().enumerate() {
                    if *p.add(i) != want {
                        ok = false;
                        break;
                    }
                }
                ok
            };
            if immersive {
                let new_dark = crate::winmod::system_dark_mode();
                let changed = new_dark != DARK.load(Ordering::Acquire);
                DARK.store(new_dark, Ordering::Release);
                if changed {
                    // 标题栏明暗 + 已有 EDIT 的主题切换**都不会自动发生**，
                    // 必须显式重套（SPEC §3.4）。
                    apply_modern_window_style(hwnd);
                    retheme_children(new_dark);
                    windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                        hwnd,
                        std::ptr::null(),
                        1,
                    );
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// EDIT 的背景画刷缓存（**必须缓存**）。
///
/// `WM_CTLCOLOREDIT` 每帧都会来一次，每次 `CreateSolidBrush` 而不释放就是**每帧泄漏
/// 一个 GDI 对象**（每进程 GDI 句柄上限 10000，打开设置窗口几十秒就会耗尽 ——
/// 那时整个进程的绘制都会失败，且毫无报错）。所以按颜色缓存一个，颜色变了才换。
///
/// 存 `isize` 而不是 `HBRUSH`（裸指针非 `Send`，放不进 `static Mutex`）。
static BRUSH: Mutex<(isize, u32)> = Mutex::new((0, u32::MAX));

/// 取一个"当前主题输入框底色"的画刷，顺带把上一个回收掉。
unsafe fn field_brush(color: u32) -> isize {
    let mut c = BRUSH.lock().unwrap_or_else(|p| p.into_inner());
    if c.0 != 0 && c.1 == color {
        return c.0;
    }
    if c.0 != 0 {
        DeleteObject(c.0 as *mut c_void);
    }
    let b = CreateSolidBrush(color) as isize;
    *c = (b, color);
    b
}

/// 进程退出/窗口销毁时回收缓存的画刷。
unsafe fn drop_field_brush() {
    let mut c = BRUSH.lock().unwrap_or_else(|p| p.into_inner());
    if c.0 != 0 {
        DeleteObject(c.0 as *mut c_void);
    }
    *c = (0, u32::MAX);
}

/// 语言切换：更新窗口标题 + 所有控件文字 + 重绘。
///
/// **不重建窗口**（SPEC D3）：重建会丢用户已输入未保存的其它字段，也会让窗口闪。
///
/// 自绘控件（卡片标题/标签/单位后缀/复选框文字）在 `WM_ERASEBKGND` 里按当前语言
/// 现取，重绘一次就全变了；但**控件自己的窗口文字也必须同步** ——
/// 那是屏幕阅读器（UIA）读到的内容，也是 `GetWindowText` 能看到的唯一东西。
/// 只重绘不 `SetWindowTextW` 的话，视觉上变了、辅助技术读到的还是旧语言。
unsafe fn retranslate(hwnd: HWND) {
    let lang = lang();
    let t = wstr(i18n::tr(lang, Key::WinTitle));
    SetWindowTextW(hwnd, t.as_ptr());

    let c = *CTL.lock().unwrap_or_else(|p| p.into_inner());
    let set = |h: isize, s: &str| {
        if h != 0 {
            let w = wstr(s);
            SetWindowTextW(h as HWND, w.as_ptr());
        }
    };
    // 复选框：文字由 `check_label` 给出（修饰键名两语言相同，第 9 个是"启用/Enabled"）。
    for (i, h) in c.checks.iter().enumerate() {
        set(*h, check_label(lang, i));
    }
    set(c.btn_save, i18n::tr(lang, Key::BtnSave));
    set(c.btn_cancel, i18n::tr(lang, Key::BtnCancel));
    set(c.btn_pick, i18n::tr(lang, Key::BtnPickProcs));
    set(c.lang_zh, i18n::tr(lang, Key::LangZh));
    set(c.lang_en, i18n::tr(lang, Key::LangEn));

    // 进程选择弹窗（若开着）的标题也一并更新。
    if let Some(p) = *PROC_PICK.lock().unwrap_or_else(|x| x.into_inner()) {
        if p != 0 && IsWindow(p as HWND) != 0 {
            let pt = wstr(i18n::tr(lang, Key::ProcPickerTitle));
            SetWindowTextW(p as HWND, pt.as_ptr());
        }
    }
    windows_sys::Win32::Graphics::Gdi::InvalidateRect(hwnd, std::ptr::null(), 1);
}

/// 给已有的 EDIT / BUTTON 重新套主题（切深浅色时**必须**显式调用，
/// 控件主题不会自己跟着系统变 —— SPEC §3.4）。
unsafe fn retheme_children(dark: bool) {
    let c = *CTL.lock().unwrap_or_else(|p| p.into_inner());
    for h in [c.thr, c.dbl, c.mid_thr, c.excl] {
        if h != 0 {
            crate::winmod::set_window_theme(h, dark);
        }
    }
}

/// 建控件（WM_CREATE）。
///
/// **只有 EDIT 是原生控件**（键盘/光标/选区/IME 全免费），且不带 `WS_EX_CLIENTEDGE`：
/// 边框与底色由 `paint_background` 自绘成 8px 圆角，输入区向内缩 1px 边框。
/// 其余全部 `BS_OWNERDRAW` 自绘（见模块注释的控件选型表）。
///
/// **高对比度**：`HIGH_CONTRAST` 为真时退回原生样式（`BS_AUTOCHECKBOX` /
/// `BS_PUSHBUTTON`），并且 `draw_item` 直接返回 —— 不破坏用户自己的系统配色。
unsafe fn create_controls(parent: HWND) {
    let dpi = crate::winmod::dpi_for_window(parent as isize);
    let l = layout::compute();
    let instance = GetModuleHandleW(0 as PCWSTR);
    let hc = HIGH_CONTRAST.load(Ordering::Acquire);
    let body = font(|f| f.body);
    let lang = lang();

    // 通用建控件：坐标是逻辑像素，这里统一缩放。
    let mk = |class: &str,
              text: &str,
              style: u32,
              ex: u32,
              r: Rect,
              id: usize|
     -> isize {
        let c = wstr(class);
        let t = wstr(text);
        let sr = r.scaled(dpi);
        let hw = CreateWindowExW(
            ex,
            c.as_ptr(),
            t.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            sr.x,
            sr.y,
            sr.w,
            sr.h,
            parent,
            id as _,
            instance,
            std::ptr::null_mut(),
        );
        if hw.is_null() {
            eprintln!("anydrag-win: CreateWindowExW({class}, id={id}) failed");
            return 0;
        }
        SendMessageW(hw, WM_SETFONT, body as usize, 1);
        hw as isize
    };

    // ── 单行输入框：内缩 1px 边框 + 8px 横向内边距 ──
    let edit1 = |r: Rect| -> isize {
        let inner = Rect::new(r.x + 1 + 8, r.y + 1, r.w - 2 - 16, r.h - 2);
        mk(
            "EDIT",
            "",
            ES_AUTOHSCROLL as u32 | ES_NUMBER as u32,
            0,
            inner,
            0,
        )
    };
    // ── 多行输入框（排除名单）：不要 ES_NUMBER ──
    let editn = |r: Rect| -> isize {
        let inner = Rect::new(r.x + 1 + 8, r.y + 1 + 6, r.w - 2 - 16, r.h - 2 - 12);
        mk(
            "EDIT",
            "",
            WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | ES_WANTRETURN as u32,
            0,
            inner,
            0,
        )
    };
    // ── 按钮 / 复选框：自绘（高对比度时退回原生）──
    let button = |text: &str, r: Rect, id: usize, primary: bool| -> isize {
        let style = if hc {
            if primary {
                BS_DEFPUSHBUTTON as u32
            } else {
                BS_PUSHBUTTON as u32
            }
        } else {
            BS_OWNERDRAW as u32
        };
        mk("BUTTON", text, style | 0x10000 /* WS_TABSTOP */, 0, r, id)
    };
    let check = |text: &str, r: Rect, id: usize| -> isize {
        // 高对比度用 BS_AUTOCHECKBOX（系统会维护勾选位）；否则 BS_OWNERDRAW
        // 由我们自己维护（SPEC R11）。
        let style = if hc {
            0x00000003u32 // BS_AUTOCHECKBOX
        } else {
            BS_OWNERDRAW as u32
        };
        mk("BUTTON", text, style | 0x10000, 0, r, id)
    };

    let mut c = ZERO;

    // ── 卡 1 / 卡 2：触发键复选框 ──
    for (i, r) in l.drag_keys.iter().enumerate() {
        c.checks[i] = check(
            i18n::modifier_label(i18n::MODIFIER_VKS[i]),
            *r,
            ID_CHECK_BASE + i,
        );
    }
    for (i, r) in l.resize_keys.iter().enumerate() {
        c.checks[4 + i] = check(
            i18n::modifier_label(i18n::MODIFIER_VKS[i]),
            *r,
            ID_CHECK_BASE + 4 + i,
        );
    }

    // ── 卡 3：三个数字框 + 中键开关 ──
    c.thr = edit1(l.edit_threshold);
    c.dbl = edit1(l.edit_double_click);
    c.mid_thr = edit1(l.edit_mid_threshold);
    c.checks[CheckIdx::MidOn as usize] = check(
        i18n::tr(lang, Key::ChkEnable),
        l.chk_middle_snap,
        ID_CHECK_BASE + CheckIdx::MidOn as usize,
    );

    // ── 卡 4：排除名单 ──
    c.excl = editn(l.edit_excluded);
    c.btn_pick = button(
        i18n::tr(lang, Key::BtnPickProcs),
        l.btn_pick_procs,
        ID_PICK_PROCS,
        false,
    );

    // ── 标题区：语言分段 ──
    c.lang_zh = button(i18n::tr(lang, Key::LangZh), l.lang_zh, ID_LANG_ZH, false);
    c.lang_en = button(i18n::tr(lang, Key::LangEn), l.lang_en, ID_LANG_EN, false);

    // ── 操作区：取消 / 保存 ──
    c.btn_cancel = button(
        i18n::tr(lang, Key::BtnCancel),
        l.btn_cancel,
        ID_CANCEL,
        false,
    );
    c.btn_save = button(i18n::tr(lang, Key::BtnSave), l.btn_save, ID_SAVE, true);

    // 深色下 EDIT 必须套主题（否则白底浅字 —— SPEC V10 的最大风险）。
    let dark = DARK.load(Ordering::Acquire);
    for h in [c.thr, c.dbl, c.mid_thr, c.excl] {
        if h != 0 {
            crate::winmod::set_window_theme(h, dark);
        }
    }

    *CTL.lock().unwrap_or_else(|p| p.into_inner()) = c;
    let _ = CheckIdx::DragAlt;
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
    *VIS_HWNDS.lock().unwrap_or_else(|p| p.into_inner()) = Vec::new();
    unsafe {
        EnumWindows(Some(collect_visible), 0);
    }
    let hwnds = std::mem::take(&mut *VIS_HWNDS.lock().unwrap_or_else(|p| p.into_inner()));

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
        let no_title = i18n::tr(lang(), Key::ProcNoTitle);
        let shown = if title.is_empty() {
            format!("{no_title}  [{exe}]")
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
    if let Some(p) = *PROC_PICK.lock().unwrap_or_else(|x| x.into_inner()) {
        if p == hwnd as isize {
            return 1;
        }
    }
    VIS_HWNDS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(hwnd as isize);
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
        if let Some(p) = *PROC_PICK.lock().unwrap_or_else(|x| x.into_inner()) {
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
        let dpi = crate::winmod::dpi_for_window(parent as isize);
        let sc = |v: i32| v * dpi / 96;
        let title = wstr(i18n::tr(lang(), Key::ProcPickerTitle));
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class.as_ptr(),
            title.as_ptr(),
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
        *PROC_PICK.lock().unwrap_or_else(|x| x.into_inner()) = Some(hwnd as isize);
        // 模态：禁用父窗口，避免用户在弹窗开着的期间改主界面造成状态错乱。
        EnableWindow(parent as HWND, 0);
        SetForegroundWindow(hwnd);
    }
}

/// 进程选择弹窗的窗口过程。
unsafe extern "system" fn proc_pick_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let instance = GetModuleHandleW(0 as PCWSTR);
            let dpi = crate::winmod::dpi_for_window(hwnd as isize);
            let sc = |v: i32| v * dpi / 96;
            let body = font(|f| f.body);
            // ListBox 多选，占满客户区上部。
            let list = CreateWindowExW(
                0,
                wstr("LISTBOX").as_ptr(),
                std::ptr::null_mut() as PCWSTR,
                WS_CHILD
                    | WS_VISIBLE
                    | WS_VSCROLL
                    | LBS_MULTIPLESEL as u32
                    | LBS_HASSTRINGS as u32
                    | LBS_NOTIFY as u32,
                sc(12),
                sc(12),
                sc(516),
                sc(320),
                hwnd,
                ID_PROC_LIST as _,
                instance,
                std::ptr::null_mut(),
            );
            SendMessageW(list, WM_SETFONT, body as usize, 1);
            let dark = DARK.load(Ordering::Acquire);
            crate::winmod::set_window_theme(list as isize, dark);
            let ok = wstr(i18n::tr(lang(), Key::BtnOk));
            let cancel = wstr(i18n::tr(lang(), Key::BtnCancel));
            for (text, x, id, style) in [
                (&ok, 300, ID_PROC_OK, BS_DEFPUSHBUTTON as u32),
                (&cancel, 420, IDCANCEL as usize, BS_PUSHBUTTON as u32),
            ] {
                let b = CreateWindowExW(
                    0,
                    wstr("BUTTON").as_ptr(),
                    text.as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    sc(x),
                    sc(348),
                    sc(110),
                    sc(30),
                    hwnd,
                    id as _,
                    instance,
                    std::ptr::null_mut(),
                );
                SendMessageW(b, WM_SETFONT, body as usize, 1);
                crate::winmod::set_window_theme(b as isize, dark);
            }
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
    let c = *CTL.lock().unwrap_or_else(|p| p.into_inner());
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
        if let Some(parent) = PROC_PICK.lock().unwrap_or_else(|x| x.into_inner()).take() {
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
    let c = *CTL.lock().unwrap_or_else(|p| p.into_inner());
    let has = |list: &[u16], vk: u16| list.contains(&vk);
    {
        let mut chk = CHECKS.lock().unwrap_or_else(|p| p.into_inner());
        chk[CheckIdx::DragAlt as usize] = has(&cfg.modifiers, 0x12);
        chk[CheckIdx::DragCtrl as usize] = has(&cfg.modifiers, 0x11);
        chk[CheckIdx::DragShift as usize] = has(&cfg.modifiers, 0x10);
        chk[CheckIdx::DragWin as usize] = has(&cfg.modifiers, 0x5B);
        chk[CheckIdx::RzAlt as usize] = has(&cfg.resize_modifiers, 0x12);
        chk[CheckIdx::RzCtrl as usize] = has(&cfg.resize_modifiers, 0x11);
        chk[CheckIdx::RzShift as usize] = has(&cfg.resize_modifiers, 0x10);
        chk[CheckIdx::RzWin as usize] = has(&cfg.resize_modifiers, 0x5B);
        chk[CheckIdx::MidOn as usize] = cfg.middle_snap;
    }
    // 高对比度下复选框是原生的，得用 BM_SETCHECK 同步一次（自绘那条路读 CHECKS）。
    if HIGH_CONTRAST.load(Ordering::Acquire) {
        let chk = *CHECKS.lock().unwrap_or_else(|p| p.into_inner());
        for (i, h) in c.checks.iter().enumerate() {
            if *h != 0 {
                SendMessageW(
                    *h as HWND,
                    windows_sys::Win32::UI::WindowsAndMessaging::BM_SETCHECK,
                    if chk[i] { BST_CHECKED_V } else { 0 },
                    0,
                );
            }
        }
    }

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
        let c = *CTL.lock().unwrap_or_else(|p| p.into_inner());
        let mut cfg = config::load();
        cfg.enabled = config::ENABLED.load(Ordering::Acquire);
        let chk = *CHECKS.lock().unwrap_or_else(|p| p.into_inner());
        // 高对比度时勾选位在原生控件里，以控件为准（自绘那条路以 CHECKS 为准）。
        let on = |i: usize| -> bool {
            if HIGH_CONTRAST.load(Ordering::Acquire) && c.checks[i] != 0 {
                SendMessageW(
                    c.checks[i] as HWND,
                    windows_sys::Win32::UI::WindowsAndMessaging::BM_GETCHECK,
                    0,
                    0,
                ) as usize
                    == BST_CHECKED_V
            } else {
                chk[i]
            }
        };
        cfg.modifiers = collect_vks(on(0), on(1), on(2), on(3));
        cfg.resize_modifiers = collect_vks(on(4), on(5), on(6), on(7));
        cfg.drag_threshold_px = text_of(c.thr).trim().parse().unwrap_or(5);
        let dbl = text_of(c.dbl);
        cfg.double_click_ms = if dbl.trim().is_empty() {
            None
        } else {
            dbl.trim().parse().ok()
        };
        cfg.middle_snap = on(CheckIdx::MidOn as usize);
        cfg.middle_snap_threshold_px = text_of(c.mid_thr).trim().parse().unwrap_or(40);
        // 语言：**只写窗口里实际选中的那一个**，不碰"跟随系统"的语义。
        //
        // 注意别写成"总是写 lang().code()"：那样用户第一次打开设置窗口点保存，
        // 配置里的 `lang: ""`（跟随系统）就会被固化成显式的 "zh"，
        // 此后他改系统语言界面也不再跟着变 —— 一个静默的行为丢失。
        if LANG_TOUCHED.load(Ordering::Acquire) {
            cfg.lang = lang().code().to_string();
        }
        cfg
    }
}

fn collect_vks(alt: bool, ctrl: bool, shift: bool, win: bool) -> Vec<u16> {
    let mut v = Vec::new();
    if alt {
        v.push(0x12); // VK_MENU
    }
    if ctrl {
        v.push(0x11); // VK_CONTROL
    }
    if shift {
        v.push(0x10); // VK_SHIFT
    }
    if win {
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

/// `WM_NEXTDLGCTL` 的落地：把焦点移到"下一个"可聚焦子控件。
///
/// 参数语义（Win32）：
/// - `lParam != 0` → `wParam` 就是目标控件句柄，直接设焦点；
/// - `lParam == 0` → 按 `wParam` 决定方向：0 = 下一个，1 = 上一个。
///
/// **为什么必须自己写**：见 `wnd_proc` 里 WM_NEXTDLGCTL 的注释 —— 普通窗口
/// 的 `DefWindowProcW` 对这条消息不做任何事，于是 `IsDialogMessageW` 虽然
/// "消费"了 Tab，焦点却永远不动。真正的对话框有系统内置的处理，我们没有。
unsafe fn next_dlg_ctl(parent: HWND, wp: WPARAM, lp: LPARAM) {
    if lp != 0 {
        // 明确指定目标：SetFocus 即可。
        windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(wp as HWND);
        return;
    }
    let forward = wp == 0;
    let Some(next) = next_focusable(parent, forward) else {
        return;
    };
    windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(next);
}

/// 找下一个可聚焦的子控件（`WS_TABSTOP` + 可见 + 启用）。
///
/// 顺序按 Z 序（`GetWindow(GW_CHILD)` 然后 `GW_HWNDNEXT`），这与创建顺序一致，
/// 也就是用户在界面上从上到下看到的顺序 —— Tab 顺序必须和视觉顺序一致，
/// 否则键盘用户会被"跳着走"的焦点搞晕（SPEC §9 把它列为非验收项，但既然
/// 已经实现了焦点循环，顺序就该是对的）。
///
/// 到末尾后回绕到第一个（Tab 是环形的）。当前没有焦点时从头开始。
unsafe fn next_focusable(parent: HWND, forward: bool) -> Option<HWND> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindow, GW_CHILD, GW_HWNDNEXT, GWL_STYLE, WS_TABSTOP,
    };
    // 收集全部可聚焦子控件。
    let mut list: Vec<HWND> = Vec::new();
    let mut h = GetWindow(parent, GW_CHILD);
    while !h.is_null() {
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        if style & WS_TABSTOP != 0
            && IsWindowVisible(h) != 0
            && IsWindowEnabled(h) != 0
        {
            list.push(h);
        }
        h = GetWindow(h, GW_HWNDNEXT);
    }
    if list.is_empty() {
        return None;
    }
    let cur = windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus();
    let idx = list.iter().position(|&x| x == cur);
    let n = list.len();
    let next = match idx {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        // 当前焦点不在列表里（首次 Tab 或焦点在窗口本身）：正向从头，反向从尾。
        None => {
            if forward {
                0
            } else {
                n - 1
            }
        }
    };
    Some(list[next])
}

/// 把一条消息先交给设置窗口的对话框键盘逻辑（**消息泵里每轮调用**）。
///
/// 为什么必须这样：本窗口是普通 `WS_OVERLAPPED` 窗口，不是 `DialogBox` 建的
/// 对话框，所以系统**不会**自动处理 Tab 焦点循环、回车默认按钮、Esc 取消。
/// 自绘控件（`BS_OWNERDRAW`）本身有 `WS_TABSTOP`，但把焦点送过去要靠
/// `IsDialogMessageW` —— 少了它，用户按 Tab 完全没反应，键盘用户等于用不了
/// 这个界面（SPEC V12 要求"Tab 聚焦到复选框 → 空格能切换"）。
///
/// 返回 true 表示"这条消息已被对话框逻辑吃掉"，调用方不要再
/// `TranslateMessage`/`DispatchMessageW`（否则会重复处理，表现为控件被点两次）。
pub fn is_dialog_message(msg: *const MSG) -> bool {
    let h = WIN.load(Ordering::Acquire);
    if h == 0 {
        return false;
    }
    unsafe {
        if IsWindow(h as HWND) == 0 {
            return false;
        }
        let m = (*msg).message;
        let is_key = matches!(
            m,
            WM_KEYDOWN | WM_KEYUP | WM_CHAR | WM_SYSKEYDOWN | WM_SYSKEYUP | WM_SYSCHAR
        );
        if !is_key {
            return false;
        }
        // **Tab / Shift+Tab 自己处理**（2026-10-23 实测）：
        // `IsDialogMessageW` 对 Tab 返回 TRUE（"我处理了"），但焦点**一动不动**。
        // 原因是它内部靠给父窗口发 `WM_NEXTDLGCTL` 来完成移动，而那条消息
        // 只有**真正的对话框过程**（`DialogBox` 内置的那个）才认；普通窗口的
        // `DefWindowProcW` 对它什么都不做 —— 我们也补了 WM_NEXTDLGCTL 的处理，
        // 但实测这条消息根本没被发出来（IsDialogMessageW 在非对话框窗口上
        // 提前返回了）。
        //
        // 所以不走那条依赖内部实现的路径，直接自己算下一个可聚焦控件并
        // `SetFocus`。这是**确定性**的做法：不依赖"窗口是不是对话框"。
        if m == WM_KEYDOWN {
            let vk = (*msg).wParam as u16;
            if vk == 0x09 {
                // VK_TAB。Shift 按下时反向。
                let shift = windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) < 0;
                next_dlg_ctl(h as HWND, if shift { 1 } else { 0 }, 0);
                return true;
            }
        }
        let r = windows_sys::Win32::UI::WindowsAndMessaging::IsDialogMessageW(h as HWND, msg);
        r != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复选框数量必须与 `CheckIdx` 的枚举一致 —— 少一个就是某个勾选框
    /// 点了没反应（而且不会报错）。`CHECK_COUNT` 是数组长度，`MidOn` 是最后一个
    /// 下标的断言。
    #[test]
    fn check_count_matches_index_enum() {
        assert_eq!(CheckIdx::MidOn as usize, CHECK_COUNT - 1);
        assert_eq!(CheckIdx::DragAlt as usize, 0);
        assert_eq!(CheckIdx::RzAlt as usize, 4);
    }

    /// 每个复选框都必须有非空文字（空白勾选框 = 用户不知道勾的是什么）。
    #[test]
    fn every_checkbox_has_a_label() {
        for lang in [Lang::Zh, Lang::En] {
            for i in 0..CHECK_COUNT {
                let s = check_label(lang, i);
                assert!(!s.trim().is_empty(), "{lang:?} 的第 {i} 个复选框没文字");
            }
        }
    }

    /// 卡 1 与卡 2 的复选框文字必须都是四个修饰键名（顺序一致），
    /// 卡 3 那个必须是"启用/Enabled" —— 混了就是行对错了卡。
    #[test]
    fn checkbox_labels_map_to_the_right_rows() {
        for lang in [Lang::Zh, Lang::En] {
            for i in 0..4 {
                assert_eq!(
                    check_label(lang, i),
                    i18n::modifier_label(i18n::MODIFIER_VKS[i]),
                    "卡 1 第 {i} 个复选框"
                );
                assert_eq!(
                    check_label(lang, 4 + i),
                    i18n::modifier_label(i18n::MODIFIER_VKS[i]),
                    "卡 2 第 {i} 个复选框"
                );
            }
            assert_eq!(check_label(lang, 8), i18n::tr(lang, Key::ChkEnable));
        }
    }

    /// 卡片标题必须四张各不相同（复制粘贴漏改 = 两张卡同名）。
    #[test]
    fn card_titles_are_distinct() {
        for lang in [Lang::Zh, Lang::En] {
            let titles: Vec<&str> = (0..4).map(|i| card_title_text(lang, i)).collect();
            for (i, a) in titles.iter().enumerate() {
                for (j, b) in titles.iter().enumerate() {
                    if i != j {
                        assert_ne!(a, b, "{lang:?} 卡 {i} 与卡 {j} 标题相同");
                    }
                }
            }
        }
    }

    /// 每个控件 id 都有唯一归属：复选框 id 落在 `[ID_CHECK_BASE, +CHECK_COUNT)`
    /// 区间内，且不与按钮/语言分段的 id 相撞。
    ///
    /// 撞了会怎样：`WM_COMMAND` 按 id 分派，撞了就是"点 A 触发了 B"，
    /// 而且**不会报错**（id 只是个整数）。
    #[test]
    fn control_ids_do_not_collide() {
        let mut ids = vec![ID_SAVE, ID_CANCEL, ID_PICK_PROCS, ID_LANG_ZH, ID_LANG_EN];
        let check_ids: Vec<usize> = (ID_CHECK_BASE..ID_CHECK_BASE + CHECK_COUNT).collect();
        for c in &check_ids {
            assert!(!ids.contains(c), "复选框 id {c} 与按钮 id 相撞");
        }
        ids.extend(check_ids);
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "控件 id 有重复");
        // 进程选择弹窗的 id 属于另一个窗口，但仍不该与主窗口的重合
        // （弹窗的消息不会路由到主窗口，但保持编号空间清晰更省心）。
        assert!(!ids.contains(&ID_PROC_LIST));
        assert!(!ids.contains(&ID_PROC_OK));
    }

    /// 卡片标题的下标映射必须覆盖 0..4 且与 layout 的卡片顺序一致。
    ///
    /// 顺序写错（例如把"缩放"放到卡 1）不会报错，只会让界面上的分组标题
    /// 与下面的控件对不上 —— 这正是 SPEC U1 那类"标签画进别的卡"的翻版。
    #[test]
    fn card_title_order_matches_layout() {
        let l = layout::compute();
        assert_eq!(l.cards.len(), 4);
        let zh: Vec<&str> = (0..4).map(|i| card_title_text(Lang::Zh, i)).collect();
        assert_eq!(
            zh,
            vec!["拖动 / 双击", "缩放", "手势参数", "排除进程"],
            "卡片标题顺序必须与 layout 的 4 张卡一一对应"
        );
    }
}
