//! 设置窗口的几何：**唯一坐标来源**。
//!
//! **纯逻辑模块**：不 import windows-sys（AGENTS.md §1 不变量 4），因此可单测。
//!
//! ## 为什么单独抽一个模块（2026-10-23 的 U1/U2 事故）
//!
//! 旧版 `settings.rs` 里坐标是**散落在 create_controls 里的字面量**，
//! 而卡片矩形是另一个常量表 `CARDS`。两者没有约束关系，于是出现了：
//!
//! - 「完全无视手势的进程：」这个标签写在 `y=290`，而卡片 2 的白底范围是
//!   `y=128..316` —— 标签**画进了上一张卡片**里，与「手势参数」的内容混在一起；
//! - 窗口高 700 而内容只到 636，底部留下约 250px 纯空白。
//!
//! 这两条都不是"调一下间距"能防住的，它们的根因是**同一份布局有两处定义**
//! （坑 #18 的同款形态：同一语义两个实现，生效的那个没测试）。
//!
//! 所以这里把每个控件的矩形都算出来，并让单测钉住三条几何不变量：
//! ① 每个控件都落在**它所属的那张卡片**里（U1）；
//! ② 窗口高度 = 内容自然高度 + 边距（U2，不留死白）；
//! ③ 同一行内的控件互不重叠、且都在客户区内。
//!
//! `settings.rs` 只负责把这里的逻辑坐标按 DPI 缩放后套到窗口上，不再写字面量。

/// 逻辑像素矩形（96 DPI 基准）。宽高不含边框，就是控件本身的尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// `other` 是否完全落在本矩形内（含边界）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    /// 两个矩形是否有面积重叠（仅边界相接不算）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// 按 DPI 缩放（120/144/192 等）。逐字段缩放，与 `settings.rs` 的 `s()` 同式，
    /// 保证"布局里算的"与"窗口里摆的"是同一套算术。
    pub fn scaled(&self, dpi: i32) -> Rect {
        let d = dpi.max(96);
        let s = |v: i32| v * d / 96;
        Rect::new(s(self.x), s(self.y), s(self.w), s(self.h))
    }

    /// 从**物理像素**换回逻辑像素。
    ///
    /// `WM_DRAWITEM` 给的 `rcItem` 已经是设备像素（自绘控件拿到的就是最终矩形），
    /// 而本模块所有坐标都是逻辑像素。绘制函数内部会再 `scaled()` 一次，
    /// 所以必须在入口处先换回来 —— 否则会被**二次缩放**，控件在 150% DPI 下
    /// 画到右下角外面去（第一版就是这样，只有 96 DPI 看着正常）。
    pub fn unscaled(&self, dpi: i32) -> Rect {
        let d = dpi.max(96);
        let s = |v: i32| v * 96 / d;
        Rect::new(s(self.x), s(self.y), s(self.w), s(self.h))
    }
}

/// 一张卡片：外框 + 标题行 + 可选说明行（自绘）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    pub rect: Rect,
    pub title: Rect,
    /// 说明行（只有「排除进程」卡有，对应 U1b：说明不再游离在卡片外）。
    pub desc: Option<Rect>,
}

/// 设置窗口的完整几何（逻辑像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub client_w: i32,
    pub client_h: i32,

    // ── 标题区 ──────────────────────────────
    pub icon: Rect,
    pub title: Rect,
    pub subtitle: Rect,
    /// 语言分段控件的外框（自绘底），两个分段按钮放在它里面。
    pub lang_box: Rect,
    pub lang_zh: Rect,
    pub lang_en: Rect,

    // ── 分隔线（y 坐标，横跨整窗宽）──────────
    pub sep_top: i32,
    pub sep_bottom: i32,

    // ── 四张卡片 ────────────────────────────
    pub cards: [Card; 4],

    // 卡 1：拖动 / 双击
    pub lbl_drag_keys: Rect,
    pub drag_keys: [Rect; 4],
    // 卡 2：缩放
    pub lbl_resize_keys: Rect,
    pub resize_keys: [Rect; 4],
    // 卡 3：手势参数
    pub lbl_threshold: Rect,
    pub edit_threshold: Rect,
    pub unit_threshold: Rect,
    pub lbl_double_click: Rect,
    pub edit_double_click: Rect,
    pub unit_double_click: Rect,
    pub lbl_middle_snap: Rect,
    pub chk_middle_snap: Rect,
    pub lbl_mid_threshold: Rect,
    pub edit_mid_threshold: Rect,
    pub unit_mid_threshold: Rect,
    // 卡 4：排除进程
    pub edit_excluded: Rect,
    pub btn_pick_procs: Rect,

    // ── 操作区 ──────────────────────────────
    pub hint: Rect,
    pub btn_cancel: Rect,
    pub btn_save: Rect,
}

// ── 度量（SPEC §3.2；8px 网格 + 三级圆角）────────────────────────────────────
/// 窗口客户区宽度。按**英文**文案量出来的（`Double-click window` ≈ 130px
/// + 输入框 100 + 单位后缀 + 两侧 16px 内边距），见 SPEC §3.2 与 D5。
pub const CLIENT_W: i32 = 560;
/// 外边距 20（8px 网格）。
pub const MARGIN: i32 = 20;
/// 卡片间距 12。
pub const CARD_GAP: i32 = 12;
/// 卡片左右内边距 16。
pub const CARD_PAD_X: i32 = 16;
/// 卡片上内边距 14。
pub const CARD_PAD_TOP: i32 = 14;
/// 卡片下内边距 16。
pub const CARD_PAD_BOTTOM: i32 = 16;
/// 卡片标题行高（14px Semibold + 行距）。
pub const CARD_TITLE_H: i32 = 20;
/// 卡片标题 → 首行控件的间距。旧版这里是 0，标题与内容"粘成一团"（U3）。
pub const TITLE_GAP: i32 = 12;
/// 控件行高 28（14px 字体 + 上下 7px）。
pub const ROW_H: i32 = 28;
/// 行间距 10。
pub const ROW_GAP: i32 = 10;
/// 标签列宽 150。
pub const LABEL_W: i32 = 150;
/// 单行输入框宽 100。
pub const FIELD_W: i32 = 100;
/// 单位后缀与输入框的间距 8（U6：单位紧跟输入框，不是塞进标签里）。
pub const UNIT_GAP: i32 = 8;
/// 复选框宽 72（16 方框 + 8 间距 + 文字）。
pub const CHECK_W: i32 = 72;
/// 复选框之间的间距 6。
pub const CHECK_GAP: i32 = 6;
/// 按钮尺寸 88×32。
pub const BTN_W: i32 = 88;
pub const BTN_H: i32 = 32;
/// 按钮间距 8。
pub const BTN_GAP: i32 = 8;

/// 标题区：图标 32×32 + 主标题 20px + 副标题 12px。
const ICON_SIZE: i32 = 32;
/// 图标与标题文字的间距。
const ICON_GAP: i32 = 8;
/// 主标题行高（20px Subtitle → 28/36 刻度里的 28）。
const TITLE_H: i32 = 28;
/// 副标题行高（12px Caption → 16）。
const SUBTITLE_H: i32 = 16;
/// 语言分段控件：两段各 63 宽 + 3px 内衬。
const LANG_BOX_W: i32 = 132;
const LANG_BOX_H: i32 = 28;
const LANG_INSET: i32 = 3;

/// 卡 4 的进程名单框高 100（够看 5 行）。
const EXCL_LIST_H: i32 = 100;
/// 「选择进程…」按钮宽 120（英文 `Pick processes…` 需要）。
const PICK_BTN_W: i32 = 120;

/// 算出整套布局。**纯函数**：同样的输入永远同样的输出，可单测。
pub fn compute() -> Layout {
    let left = MARGIN;
    let right = CLIENT_W - MARGIN;
    let inner_left = left + CARD_PAD_X;
    let inner_right = right - CARD_PAD_X;
    // 标签列右侧 = 控件起始 x（U6：标签列宽固定 150，空档收敛到 0）。
    let ctl_x = inner_left + LABEL_W;

    // ── 标题区 ──────────────────────────────
    let icon = Rect::new(left, MARGIN, ICON_SIZE, ICON_SIZE);
    let text_x = icon.right() + ICON_GAP;
    let title = Rect::new(text_x, MARGIN, 300, TITLE_H);
    let subtitle = Rect::new(text_x, MARGIN + TITLE_H, 340, SUBTITLE_H);
    // 语言切换靠右，与"标题 + 副标题"两行的垂直中心对齐。
    let band_center = MARGIN + (TITLE_H + SUBTITLE_H) / 2;
    let lang_box = Rect::new(
        right - LANG_BOX_W,
        band_center - LANG_BOX_H / 2,
        LANG_BOX_W,
        LANG_BOX_H,
    );
    let seg_w = (LANG_BOX_W - LANG_INSET * 2) / 2;
    let seg_h = LANG_BOX_H - LANG_INSET * 2;
    let lang_zh = Rect::new(lang_box.x + LANG_INSET, lang_box.y + LANG_INSET, seg_w, seg_h);
    let lang_en = Rect::new(lang_zh.right(), lang_box.y + LANG_INSET, seg_w, seg_h);

    let band_bottom = MARGIN + TITLE_H + SUBTITLE_H;
    let sep_top = band_bottom + 12;
    let mut y = sep_top + 12;

    // ── 卡 1：拖动 / 双击 ────────────────────
    let card_h = CARD_PAD_TOP + CARD_TITLE_H + TITLE_GAP + ROW_H + CARD_PAD_BOTTOM;
    let card1 = Rect::new(left, y, right - left, card_h);
    let (c1_title, c1_row) = card_geometry(&card1);
    let lbl_drag_keys = Rect::new(inner_left, c1_row, LABEL_W, ROW_H);
    let drag_keys = key_row(ctl_x, c1_row);
    y = card1.bottom() + CARD_GAP;

    // ── 卡 2：缩放 ──────────────────────────
    let card2 = Rect::new(left, y, right - left, card_h);
    let (c2_title, c2_row) = card_geometry(&card2);
    let lbl_resize_keys = Rect::new(inner_left, c2_row, LABEL_W, ROW_H);
    let resize_keys = key_row(ctl_x, c2_row);
    y = card2.bottom() + CARD_GAP;

    // ── 卡 3：手势参数（3 行）────────────────
    let rows3 = ROW_H * 3 + ROW_GAP * 2;
    let card3 = Rect::new(
        left,
        y,
        right - left,
        CARD_PAD_TOP + CARD_TITLE_H + TITLE_GAP + rows3 + CARD_PAD_BOTTOM,
    );
    let (c3_title, r1) = card_geometry(&card3);
    let r2 = r1 + ROW_H + ROW_GAP;
    let r3 = r2 + ROW_H + ROW_GAP;

    let lbl_threshold = Rect::new(inner_left, r1, LABEL_W, ROW_H);
    let edit_threshold = Rect::new(ctl_x, r1, FIELD_W, ROW_H);
    let unit_threshold = Rect::new(
        edit_threshold.right() + UNIT_GAP,
        r1,
        inner_right - (edit_threshold.right() + UNIT_GAP),
        ROW_H,
    );
    let lbl_double_click = Rect::new(inner_left, r2, LABEL_W, ROW_H);
    let edit_double_click = Rect::new(ctl_x, r2, FIELD_W, ROW_H);
    let unit_double_click = Rect::new(
        edit_double_click.right() + UNIT_GAP,
        r2,
        inner_right - (edit_double_click.right() + UNIT_GAP),
        ROW_H,
    );

    // 第 3 行塞了 4 个元素（标签 / 复选框 / 判定阈值标签 / 输入框 + 单位），
    // 所以这一行的后半段用更紧的宽度：复选框 96、次标签 112、输入框 64。
    let lbl_middle_snap = Rect::new(inner_left, r3, LABEL_W, ROW_H);
    let chk_middle_snap = Rect::new(ctl_x, r3, 96, ROW_H);
    let lbl_mid_threshold = Rect::new(chk_middle_snap.right() + 16, r3, 112, ROW_H);
    let edit_mid_threshold = Rect::new(lbl_mid_threshold.right() + 8, r3, 64, ROW_H);
    let unit_mid_threshold = Rect::new(
        edit_mid_threshold.right() + UNIT_GAP,
        r3,
        inner_right - (edit_mid_threshold.right() + UNIT_GAP),
        ROW_H,
    );
    y = card3.bottom() + CARD_GAP;

    // ── 卡 4：排除进程 ──────────────────────
    let desc_h = 16;
    let card4_h =
        CARD_PAD_TOP + CARD_TITLE_H + 6 + desc_h + TITLE_GAP + EXCL_LIST_H + CARD_PAD_BOTTOM;
    let card4 = Rect::new(left, y, right - left, card4_h);
    let c4_title = Rect::new(inner_left, card4.y + CARD_PAD_TOP, LABEL_W * 2, CARD_TITLE_H);
    let c4_desc = Rect::new(inner_left, c4_title.bottom() + 6, inner_right - inner_left, desc_h);
    let list_y = c4_desc.bottom() + TITLE_GAP;
    let btn_pick_procs = Rect::new(inner_right - PICK_BTN_W, list_y, PICK_BTN_W, ROW_H);
    let edit_excluded = Rect::new(
        inner_left,
        list_y,
        btn_pick_procs.x - 12 - inner_left,
        EXCL_LIST_H,
    );

    // ── 操作区 ──────────────────────────────
    let sep_bottom = card4.bottom() + 16;
    let btn_y = sep_bottom + 12;
    let btn_save = Rect::new(right - BTN_W, btn_y, BTN_W, BTN_H);
    let btn_cancel = Rect::new(
        btn_save.x - BTN_GAP - BTN_W,
        btn_y,
        BTN_W,
        BTN_H,
    );
    let hint = Rect::new(
        inner_left - CARD_PAD_X + 4,
        btn_y + (BTN_H - 16) / 2,
        btn_cancel.x - 8 - (inner_left - CARD_PAD_X + 4),
        16,
    );
    let client_h = btn_y + BTN_H + MARGIN;

    Layout {
        client_w: CLIENT_W,
        client_h,
        icon,
        title,
        subtitle,
        lang_box,
        lang_zh,
        lang_en,
        sep_top,
        sep_bottom,
        cards: [
            Card {
                rect: card1,
                title: c1_title,
                desc: None,
            },
            Card {
                rect: card2,
                title: c2_title,
                desc: None,
            },
            Card {
                rect: card3,
                title: c3_title,
                desc: None,
            },
            Card {
                rect: card4,
                title: c4_title,
                desc: Some(c4_desc),
            },
        ],
        lbl_drag_keys,
        drag_keys,
        lbl_resize_keys,
        resize_keys,
        lbl_threshold,
        edit_threshold,
        unit_threshold,
        lbl_double_click,
        edit_double_click,
        unit_double_click,
        lbl_middle_snap,
        chk_middle_snap,
        lbl_mid_threshold,
        edit_mid_threshold,
        unit_mid_threshold,
        edit_excluded,
        btn_pick_procs,
        hint,
        btn_cancel,
        btn_save,
    }
}

/// 卡片内：标题行矩形 + 首行控件的 y。
fn card_geometry(card: &Rect) -> (Rect, i32) {
    let title = Rect::new(
        card.x + CARD_PAD_X,
        card.y + CARD_PAD_TOP,
        card.w - CARD_PAD_X * 2,
        CARD_TITLE_H,
    );
    (title, title.bottom() + TITLE_GAP)
}

/// 一行 4 个修饰键复选框。
fn key_row(x: i32, y: i32) -> [Rect; 4] {
    let mut out = [Rect::new(0, 0, 0, 0); 4];
    for (i, r) in out.iter_mut().enumerate() {
        *r = Rect::new(x + i as i32 * (CHECK_W + CHECK_GAP), y, CHECK_W, ROW_H);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有"控件类"矩形（不含卡片本身与纯文本）。
    fn all_controls(l: &Layout) -> Vec<(&'static str, Rect)> {
        let mut v = vec![
            ("icon", l.icon),
            ("title", l.title),
            ("subtitle", l.subtitle),
            ("lang_box", l.lang_box),
            ("lang_zh", l.lang_zh),
            ("lang_en", l.lang_en),
            ("lbl_drag_keys", l.lbl_drag_keys),
            ("lbl_resize_keys", l.lbl_resize_keys),
            ("lbl_threshold", l.lbl_threshold),
            ("edit_threshold", l.edit_threshold),
            ("unit_threshold", l.unit_threshold),
            ("lbl_double_click", l.lbl_double_click),
            ("edit_double_click", l.edit_double_click),
            ("unit_double_click", l.unit_double_click),
            ("lbl_middle_snap", l.lbl_middle_snap),
            ("chk_middle_snap", l.chk_middle_snap),
            ("lbl_mid_threshold", l.lbl_mid_threshold),
            ("edit_mid_threshold", l.edit_mid_threshold),
            ("unit_mid_threshold", l.unit_mid_threshold),
            ("edit_excluded", l.edit_excluded),
            ("btn_pick_procs", l.btn_pick_procs),
            ("hint", l.hint),
            ("btn_cancel", l.btn_cancel),
            ("btn_save", l.btn_save),
        ];
        for (i, r) in l.drag_keys.iter().enumerate() {
            v.push(if i == 0 { ("drag_key0", *r) } else { ("drag_keyN", *r) });
        }
        for (i, r) in l.resize_keys.iter().enumerate() {
            v.push(if i == 0 {
                ("resize_key0", *r)
            } else {
                ("resize_keyN", *r)
            });
        }
        v
    }

    /// **U1 回归**：每个控件都必须落在它所属的那张卡片里。
    ///
    /// 旧版把「完全无视手势的进程：」写在 y=290 而卡片 2 的白底到 y=316，
    /// 标签画进了上一张卡片 —— 这条断言会当场抓住它。
    #[test]
    fn every_control_lives_in_its_card() {
        let l = compute();
        let in_card = |r: Rect, idx: usize| {
            assert!(
                l.cards[idx].rect.contains(&r),
                "{r:?} 不在卡片 {idx} {:?} 内",
                l.cards[idx].rect
            );
        };
        in_card(l.lbl_drag_keys, 0);
        for r in l.drag_keys {
            in_card(r, 0);
        }
        in_card(l.lbl_resize_keys, 1);
        for r in l.resize_keys {
            in_card(r, 1);
        }
        for r in [
            l.lbl_threshold,
            l.edit_threshold,
            l.unit_threshold,
            l.lbl_double_click,
            l.edit_double_click,
            l.unit_double_click,
            l.lbl_middle_snap,
            l.chk_middle_snap,
            l.lbl_mid_threshold,
            l.edit_mid_threshold,
            l.unit_mid_threshold,
        ] {
            in_card(r, 2);
        }
        in_card(l.edit_excluded, 3);
        in_card(l.btn_pick_procs, 3);
        in_card(l.cards[3].desc.unwrap(), 3);
        // 卡片标题也必须在自己卡内（且不压到内容行）。
        for c in &l.cards {
            assert!(c.rect.contains(&c.title));
            let content_top = c.title.bottom() + TITLE_GAP;
            assert!(
                content_top <= c.rect.bottom() - CARD_PAD_BOTTOM + 1,
                "卡片标题把内容挤出卡外"
            );
        }
    }

    /// **U2 回归**：窗口高度 = 内容自然高度 + 20px 底边距，不留死白。
    ///
    /// 旧版内容到 636 而窗口 700（底部 250px 空白，因为窗口尺寸是手填的）。
    #[test]
    fn window_height_is_content_driven() {
        let l = compute();
        let content_bottom = l.btn_save.bottom();
        assert_eq!(
            l.client_h,
            content_bottom + MARGIN,
            "客户区高度必须正好等于按钮底边 + 20px 边距"
        );
        // 也不许反过来把内容裁掉。
        assert!(l.client_h > content_bottom);
    }

    /// 所有控件都在客户区内（V5：任何 DPI 下都不被裁切）。
    #[test]
    fn all_controls_inside_client_area() {
        let l = compute();
        let client = Rect::new(0, 0, l.client_w, l.client_h);
        for (name, r) in all_controls(&l) {
            assert!(
                client.contains(&r),
                "{name} {r:?} 越出客户区 {client:?}"
            );
        }
    }

    /// 同一行内的控件互不重叠（U6：标签/输入框/单位之间不留 200px 空档，
    /// 但也不能叠在一起）。
    #[test]
    fn no_overlapping_controls() {
        let l = compute();
        let items = all_controls(&l);
        for (i, (n1, r1)) in items.iter().enumerate() {
            for (n2, r2) in items.iter().skip(i + 1) {
                // 语言分段控件是"容器 + 两个子按钮"，天然包含关系。
                if (*n1 == "lang_box" && (*n2 == "lang_zh" || *n2 == "lang_en"))
                    || (*n2 == "lang_box" && (*n1 == "lang_zh" || *n1 == "lang_en"))
                {
                    continue;
                }
                assert!(!r1.overlaps(r2), "{n1} {r1:?} 与 {n2} {r2:?} 重叠");
            }
        }
    }

    /// 卡片之间不重叠，且按顺序自上而下排列（信息架构的顺序就是阅读顺序）。
    #[test]
    fn cards_are_stacked_in_order() {
        let l = compute();
        for i in 0..l.cards.len() - 1 {
            let (a, b) = (l.cards[i].rect, l.cards[i + 1].rect);
            assert!(!a.overlaps(&b), "卡片 {i} 与 {} 重叠", i + 1);
            assert_eq!(b.y, a.bottom() + CARD_GAP, "卡片间距必须统一为 12");
            assert_eq!(a.x, b.x);
            assert_eq!(a.w, b.w);
        }
    }

    /// 单位后缀必须紧跟输入框（U6/V9）：间距就是 UNIT_GAP，不是几百像素。
    #[test]
    fn units_sit_next_to_their_fields() {
        let l = compute();
        assert_eq!(l.unit_threshold.x, l.edit_threshold.right() + UNIT_GAP);
        assert_eq!(l.unit_double_click.x, l.edit_double_click.right() + UNIT_GAP);
        assert_eq!(
            l.unit_mid_threshold.x,
            l.edit_mid_threshold.right() + UNIT_GAP
        );
        // 单位后缀必须有正宽度（否则文字根本画不出来）。
        for u in [
            l.unit_threshold,
            l.unit_double_click,
            l.unit_mid_threshold,
        ] {
            assert!(u.w > 24, "单位后缀宽度 {} 太小，英文放不下", u.w);
        }
    }

    /// 分隔线必须把"标题区 / 内容区 / 操作区"分开（U7），且不穿过任何控件。
    ///
    /// 判据是"**没有控件跨过**分隔线"，而不是"所有控件都在线的一侧"——
    /// 卡片里的控件本来就该落在两条线之间。写成后者会把正常布局判成失败
    /// （第一版就写错了，被这条断言当场抓住）。
    #[test]
    fn separators_bracket_the_content() {
        let l = compute();
        assert!(l.sep_top > l.subtitle.bottom(), "上分隔线压到副标题");
        assert!(l.sep_top < l.cards[0].rect.y, "上分隔线跑到第一张卡下面");
        assert!(
            l.sep_bottom > l.cards[3].rect.bottom(),
            "下分隔线压到最后一张卡"
        );
        assert!(l.sep_bottom < l.btn_save.y, "下分隔线跑到按钮下面");
        for (name, r) in all_controls(&l) {
            assert!(
                !(r.y < l.sep_top && r.bottom() > l.sep_top),
                "{name} 跨过上方分隔线 y={}",
                l.sep_top
            );
            assert!(
                !(r.y < l.sep_bottom && r.bottom() > l.sep_bottom),
                "{name} 跨过下方分隔线 y={}",
                l.sep_bottom
            );
        }
        // 标题区的控件必须在**上**线之前，操作区的必须在**下**线之后。
        for (name, r) in [
            ("title", l.title),
            ("subtitle", l.subtitle),
            ("lang_box", l.lang_box),
        ] {
            assert!(r.bottom() <= l.sep_top, "{name} 应在上分隔线之上");
        }
        for (name, r) in [
            ("hint", l.hint),
            ("btn_save", l.btn_save),
            ("btn_cancel", l.btn_cancel),
        ] {
            assert!(r.y >= l.sep_bottom, "{name} 应在下分隔线之下");
        }
    }

    /// 语言切换按钮在标题区、靠右，且两段等宽相邻（分段控件观感）。
    #[test]
    fn language_segments_are_adjacent_and_right_aligned() {
        let l = compute();
        assert_eq!(l.lang_en.x, l.lang_zh.right(), "两段之间不能有缝");
        assert_eq!(l.lang_zh.w, l.lang_en.w, "两段必须等宽");
        assert_eq!(l.lang_en.right(), l.lang_box.right() - LANG_INSET);
        assert!(l.lang_box.right() <= l.client_w - MARGIN);
        // 不与标题/副标题文字重叠（英文标题较长，靠右的分段控件不能压上去）。
        assert!(!l.lang_box.overlaps(&l.title));
        assert!(!l.lang_box.overlaps(&l.subtitle));
    }

    /// 操作区：按钮右对齐、等宽等高，提示在左侧且不压按钮（U9）。
    #[test]
    fn action_area_buttons_are_right_aligned() {
        let l = compute();
        assert_eq!(l.btn_save.w, BTN_W);
        assert_eq!(l.btn_save.h, BTN_H);
        assert_eq!(l.btn_cancel.w, l.btn_save.w);
        assert_eq!(l.btn_cancel.h, l.btn_save.h);
        assert_eq!(l.btn_save.y, l.btn_cancel.y);
        assert_eq!(l.btn_save.right(), l.client_w - MARGIN);
        assert_eq!(l.btn_cancel.right() + BTN_GAP, l.btn_save.x);
        assert!(l.hint.right() <= l.btn_cancel.x);
    }

    /// 按 DPI 缩放后，所有控件仍在缩放后的客户区内（V5：120/144/192 不裁切）。
    ///
    /// 这条在纯逻辑层就能验证 —— 不必真的把系统 DPI 调成 192。
    #[test]
    fn scales_without_clipping_at_common_dpis() {
        let l = compute();
        for dpi in [96, 120, 144, 168, 192, 240] {
            let client = Rect::new(0, 0, l.client_w.scaled_w(dpi), l.client_h.scaled_w(dpi));
            for (name, r) in all_controls(&l) {
                let sr = r.scaled(dpi);
                assert!(
                    client.contains(&sr),
                    "DPI {dpi}: {name} {sr:?} 越出 {client:?}"
                );
            }
            // 卡片也必须完整放下。
            for (i, c) in l.cards.iter().enumerate() {
                assert!(client.contains(&c.rect.scaled(dpi)), "DPI {dpi}: 卡片 {i} 越界");
            }
        }
    }

    /// 缩放必须是单调的（DPI 越大，像素越多），否则 150% 下会变小。
    #[test]
    fn scaling_is_monotonic() {
        let l = compute();
        let mut prev = 0;
        for dpi in [96, 120, 144, 192] {
            let w = l.edit_threshold.scaled(dpi).w;
            assert!(w >= prev, "DPI {dpi} 下输入框反而变窄了");
            prev = w;
        }
        assert_eq!(l.edit_threshold.scaled(144).w, FIELD_W * 144 / 96);
    }
}

/// 单独缩放一个标量（`Rect::scaled` 的内部算术，测试里也要用）。
#[cfg_attr(not(test), allow(dead_code))]
trait ScaleW {
    fn scaled_w(&self, dpi: i32) -> i32;
}

impl ScaleW for i32 {
    fn scaled_w(&self, dpi: i32) -> i32 {
        *self * dpi.max(96) / 96
    }
}
