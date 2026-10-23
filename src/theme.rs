//! 视觉令牌（配色 / 圆角 / 字号）+ 颜色运算。
//!
//! **纯逻辑模块**：不 import windows-sys（AGENTS.md §1 不变量 4），因此可单测。
//!
//! ## 设计语言：带蓝调的中性色阶 + 半透明边框 + 8/12px 圆角
//!
//! 这套令牌不是凭感觉调出来的，它遵循三条可验证的原则：
//!
//! 1. **中性色带极轻微蓝调**（而不是纯灰）。纯灰在大面积背景上会显得脏；
//!    加一点点蓝调（色相 ~220°、饱和度 ~3-8%）会让界面看起来"干净"。
//!    色阶从 `#ffffff` 到 `#0f1115`，每一档都是同一色相的明度递进。
//! 2. **边框用半透明黑/白，不是实色灰**。实色灰边框在任何背景上都显得厚重；
//!    半透明边框（浅色主题 `#0000000a`、深色 `#ffffff0f`）会与背景自然融合，
//!    这是"轻"的来源。
//! 3. **圆角按用途分三级**：小元素 4px、输入框/按钮 8px、卡片 12px。
//!    层级越大圆角越大，视觉重量才与容器尺寸匹配。
//!
//! 配色与尺寸同时符合 Windows 11 类型刻度与 WCAG 2.1 对比度要求
//! （后者由本模块的单测强制，见 `text_contrast_is_accessible_in_both_themes`）。
//!
//! ## 为什么这里有个 `blend()`
//!
//! GDI 的 `COLORREF` **没有 alpha 通道**。把半透明边框色 `#0000000a` 直接塞进
//! `CreatePen` 会被忽略高 8 位，画出来是**纯黑**——边框从"几乎看不见"变成
//! "又黑又粗"（SPEC R8 / §3.4）。所以本模块把 alpha 边框按目标背景**预混合**
//! 成实色，界面代码只拿实色去画。`blend` 有明确数学期望，单测钉住。

/// COLORREF = `0x00BBGGRR`（windows-sys 不导出 `RGB` 宏，等价手写）。
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

pub const fn red(c: u32) -> u8 {
    (c & 0xFF) as u8
}

pub const fn green(c: u32) -> u8 {
    ((c >> 8) & 0xFF) as u8
}

pub const fn blue(c: u32) -> u8 {
    ((c >> 16) & 0xFF) as u8
}

/// 把 `fg`（带 `alpha`，0=全透明 255=不透明）叠在实色 `bg` 上，返回实色。
///
/// 逐通道线性插值 `out = bg + (fg - bg) * a / 255`（四舍五入）。
/// **在 sRGB 值上插值而不是线性光空间**：这正是浏览器合成 CSS `rgba()` 的做法，
/// 所以结果与浏览器渲染的 `rgba()` 观感一致；换到线性光空间会偏亮，反而不像。
///
/// 单测钉住 SPEC §3.4 给的例子：`#000000` @ 10/255 叠在 `#ffffff` 上 ≈ `#f5f5f5`。
pub const fn blend(fg: u32, alpha: u8, bg: u32) -> u32 {
    rgb(
        blend_ch(red(fg), alpha, red(bg)),
        blend_ch(green(fg), alpha, green(bg)),
        blend_ch(blue(fg), alpha, blue(bg)),
    )
}

/// 单通道预混合。**写成 const fn 而不是闭包**：`const fn` 里不允许调用闭包
/// （E0015），而这套调色板是编译期常量，必须能在 `const` 上下文里算。
const fn blend_ch(f: u8, alpha: u8, b: u8) -> u8 {
    let a = alpha as u32;
    let inv = 255 - a;
    // +127 是四舍五入（否则 245.0 与 245.4 都被截成 245，边框会偏暗一档）。
    (((f as u32) * a + (b as u32) * inv + 127) / 255) as u8
}

/// 蓝色阶（只取这三档，别自己加中间色 —— 相邻档差异太小，肉眼分不出）。
const BLUE_450: u32 = rgb(0x4D, 0x93, 0xF8);
const BLUE_500: u32 = rgb(0x3B, 0x82, 0xF6);
const BLUE_600: u32 = rgb(0x25, 0x63, 0xEB);

/// 一套主题的语义令牌（SPEC §2.4.6 定稿值）。
///
/// **全部是预混合后的实色**（GDI 只吃实色）：注释里保留它原本的 alpha 形式，
/// 便于日后核对：注释里保留原始 alpha 形式，实色是由它算出来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// 窗口底：浅 `#f5f6f7` / 深 `neutral-bluish-950 #151517`。
    pub window_bg: u32,
    /// 卡片底：浅 `#ffffff` / 深 `neutral-bluish-875 #232324`。
    pub card_bg: u32,
    /// 卡片边框：浅 `#0000000a` 叠白 / 深 `#ffffff0f` 叠卡底。
    pub card_border: u32,
    /// 输入框底：浅 `#ffffff` / 深 `neutral-bluish-850 #2c2c2e`。
    pub field_bg: u32,
    /// 输入框边框：浅 `#0000001a` / 深 `#ffffff1f`。
    pub field_border: u32,
    /// 主文字：浅 `#0f1115` / 深 `#f9fafb`。
    pub text_primary: u32,
    /// 次文字：浅 `#61666b` / 深 `#cfd3d6`。
    pub text_secondary: u32,
    /// 三级文字（说明行）：浅 `#81858c` / 深 `#adb2b8`。
    pub text_tertiary: u32,
    /// 禁用态文字（降不透明度后的主文字）。
    pub text_disabled: u32,
    /// 复选框方框描边：浅 `#adb2b8` / 深 `#61666b`。
    pub checkbox_box: u32,
    /// 复选框勾选底色：浅 `blue-500` / 深 `blue-450`。
    pub checkbox_checked: u32,
    /// 勾号本身（两套主题都是白 —— 蓝底上的白勾对比度足够）。
    pub check_mark: u32,
    /// accent / 主按钮底：浅 `blue-600` / 深 `blue-500`。
    pub accent: u32,
    /// accent 悬停：浅 `blue-500` / 深 `blue-450`（只在刻度内取值，不自调）。
    pub accent_hover: u32,
    /// accent 上的文字。
    pub text_on_accent: u32,
    /// 焦点环：浅 `#4176E6` / 深 `#7AAAFF`（2px，见 `FOCUS_RING_W`）。
    pub focus_ring: u32,
    /// 分隔线（标题区/内容区/操作区之间）。
    pub separator: u32,
    /// 次按钮底（取消 / 选择进程…）。
    pub button_bg: u32,
    /// 次按钮边框。
    pub button_border: u32,
}

/// 浅色主题。窗口底 `#f5f6f7`、卡片 `#ffffff`。
pub const LIGHT: Palette = Palette {
    window_bg: rgb(0xF5, 0xF6, 0xF7),
    card_bg: rgb(0xFF, 0xFF, 0xFF),
    // #0000000a 叠在卡片白底上（SPEC §3.4 的例子）。
    card_border: blend(rgb(0x00, 0x00, 0x00), 0x0A, rgb(0xFF, 0xFF, 0xFF)),
    field_bg: rgb(0xFF, 0xFF, 0xFF),
    field_border: blend(rgb(0x00, 0x00, 0x00), 0x1A, rgb(0xFF, 0xFF, 0xFF)),
    text_primary: rgb(0x0F, 0x11, 0x15),
    text_secondary: rgb(0x61, 0x66, 0x6B),
    text_tertiary: rgb(0x81, 0x85, 0x8C),
    text_disabled: blend(rgb(0x0F, 0x11, 0x15), 0x66, rgb(0xFF, 0xFF, 0xFF)),
    // 比 SPEC §2.4.6 的 `#adb2b8` 深一阶：那个值在白卡上只有 2.13:1，
    // 低于 WCAG 1.4.11 对"控件边界"要求的 3:1（实测，见 `checkbox_box_is_visible`）。
    // `#81858c` 是同一套色阶里的最近一档，不是新调的颜色。
    checkbox_box: rgb(0x81, 0x85, 0x8C),
    checkbox_checked: BLUE_500,
    check_mark: rgb(0xFF, 0xFF, 0xFF),
    accent: BLUE_600,
    // 悬停往黑里压（蓝色按钮的常规做法）。**不能往亮里走**：白字叠 `blue-500`
    // 只有 3.68:1，会比静止态更难读。
    accent_hover: blend(rgb(0x00, 0x00, 0x00), 0x20, BLUE_600),
    // 主按钮文字用白色（浅色主题下的标准做法）。
    // （白）。白字叠 `blue-600` = 5.17:1，过 AA。
    text_on_accent: rgb(0xFF, 0xFF, 0xFF),
    focus_ring: rgb(0x41, 0x76, 0xE6),
    separator: blend(rgb(0x00, 0x00, 0x00), 0x1A, rgb(0xF5, 0xF6, 0xF7)),
    button_bg: rgb(0xFF, 0xFF, 0xFF),
    button_border: blend(rgb(0x00, 0x00, 0x00), 0x1F, rgb(0xFF, 0xFF, 0xFF)),
};

/// 深色主题。窗口底 `#151517`、卡片 `#232324`。
pub const DARK: Palette = Palette {
    window_bg: rgb(0x15, 0x15, 0x17),
    card_bg: rgb(0x23, 0x23, 0x24),
    card_border: blend(rgb(0xFF, 0xFF, 0xFF), 0x0F, rgb(0x23, 0x23, 0x24)),
    field_bg: rgb(0x2C, 0x2C, 0x2E),
    field_border: blend(rgb(0xFF, 0xFF, 0xFF), 0x1F, rgb(0x2C, 0x2C, 0x2E)),
    text_primary: rgb(0xF9, 0xFA, 0xFB),
    text_secondary: rgb(0xCF, 0xD3, 0xD6),
    text_tertiary: rgb(0xAD, 0xB2, 0xB8),
    text_disabled: blend(rgb(0xF9, 0xFA, 0xFB), 0x59, rgb(0x23, 0x23, 0x24)),
    // 同理深一档：`#61666b` 在深卡上只有 2.71:1。`neutral-bluish-400` = 7.36:1。
    checkbox_box: rgb(0xAD, 0xB2, 0xB8),
    checkbox_checked: BLUE_450,
    check_mark: rgb(0xFF, 0xFF, 0xFF),
    accent: BLUE_500,
    // 深色主题往亮里走（深底上的悬停惯例）。文字是深色，越亮对比越好。
    accent_hover: BLUE_450,
    // **这一格是深色主题最容易写错的地方**：沿用"白字"会让对比度掉到 3.68:1。
    // 深色主题的主按钮文字取近黑（而不是白）：
    // （近黑），所以深色主题的主按钮文字是**深色**的 —— 实测 5.14:1，过 AA。
    text_on_accent: rgb(0x0F, 0x11, 0x15),
    focus_ring: rgb(0x7A, 0xAA, 0xFF),
    separator: blend(rgb(0xFF, 0xFF, 0xFF), 0x1F, rgb(0x15, 0x15, 0x17)),
    button_bg: rgb(0x2C, 0x2C, 0x2E),
    button_border: blend(rgb(0xFF, 0xFF, 0xFF), 0x1F, rgb(0x23, 0x23, 0x24)),
};

/// 按深浅主题取调色板。
pub fn palette(dark: bool) -> &'static Palette {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

// ── 圆角（三级：小元素 4 / 输入框按钮 8 / 卡片 12）────────────────────────────
/// 卡片圆角 12px（容器级）。
pub const RADIUS_CARD: i32 = 12;
/// 输入框 / 按钮圆角 8px（控件级）。
pub const RADIUS_FIELD: i32 = 8;
/// 复选框方框圆角 4px（小元素级）。
pub const RADIUS_CHECK: i32 = 4;

// ── 字号（Windows 11 类型刻度；正文 14px 是官方 Body 值）─────────────────────
/// 正文 14px（Windows 11 类型刻度里的 `Body`）。
pub const FONT_BODY: i32 = 14;
/// 卡片标题 14px Semibold（Windows `Body Strong`）。
pub const FONT_CARD_TITLE: i32 = 14;
/// 窗口主标题 20px Semibold（Windows `Subtitle`）。
pub const FONT_TITLE: i32 = 20;
/// 窗口副标题 / 卡片说明 12px（Windows `Caption`）。
pub const FONT_CAPTION: i32 = 12;

/// FW_SEMIBOLD。Windows 规范要求"强调用 Semibold，不要用 Bold"。
pub const WEIGHT_SEMIBOLD: i32 = 600;
pub const WEIGHT_REGULAR: i32 = 400;

/// 焦点环宽度 2px（WCAG 2.4.11 焦点指示的最低可见宽度）。
pub const FOCUS_RING_W: i32 = 2;

/// sRGB 相对亮度（WCAG 2.1 定义），0.0（黑）..1.0（白）。
///
/// 生产代码不直接用它 —— 它是 `contrast` 的构件，而 `contrast` 只被单测用来
/// **钉住配色的可达性**（深浅两套主题的正文/按钮/焦点环对比度）。
#[cfg_attr(not(test), allow(dead_code))]
pub fn luminance(c: u32) -> f64 {
    let lin = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(red(c)) + 0.7152 * lin(green(c)) + 0.0722 * lin(blue(c))
}

/// WCAG 对比度（1.0..21.0）。正文要求 ≥ 4.5，非文字 UI 要求 ≥ 3.0。
#[cfg_attr(not(test), allow(dead_code))]
pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC §3.4 的 GDI alpha 坑：`#0000000a` 叠白底必须得到 `#f5f5f5` 左右。
    /// 如果谁把 alpha 直接塞进 COLORREF，这里会变成纯黑，测试立刻红。
    #[test]
    fn blend_matches_spec_example() {
        let out = blend(rgb(0, 0, 0), 0x0A, rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(red(out), 0xF5, "10/255 黑叠白底应为 0xF5");
        assert_eq!(green(out), 0xF5);
        assert_eq!(blue(out), 0xF5);
    }

    /// 边界语义：alpha=0 完全透明（返回底色），alpha=255 完全不透明（返回前景色）。
    #[test]
    fn blend_boundaries() {
        let fg = rgb(0x12, 0x34, 0x56);
        let bg = rgb(0xAB, 0xCD, 0xEF);
        assert_eq!(blend(fg, 0, bg), bg);
        assert_eq!(blend(fg, 255, fg), fg);
        // 中点应落在两者之间（不是任意一端）。
        let mid = blend(fg, 128, bg);
        assert!(red(mid) > red(fg) && red(mid) < red(bg), "128 应落在中间");
    }

    /// COLORREF 通道顺序：低字节是红。写错顺序会让整套配色偏蓝。
    #[test]
    fn colorref_channel_order() {
        let c = rgb(0x11, 0x22, 0x33);
        assert_eq!(c, 0x00332211);
        assert_eq!((red(c), green(c), blue(c)), (0x11, 0x22, 0x33));
    }

    /// 正文与次文字在两套主题下都必须满足 WCAG AA（4.5:1）——
    /// SPEC §2.2 明确要求"对比度必须足够"，而深色是最容易写崩的一档。
    #[test]
    fn text_contrast_is_accessible_in_both_themes() {
        for (name, p) in [("light", &LIGHT), ("dark", &DARK)] {
            for (what, fg) in [
                ("primary", p.text_primary),
                ("secondary", p.text_secondary),
            ] {
                for (on, bg) in [("card", p.card_bg), ("window", p.window_bg)] {
                    let r = contrast(fg, bg);
                    assert!(
                        r >= 4.5,
                        "{name}/{what} 在 {on} 上的对比度 {r:.2} 低于 AA 要求的 4.5"
                    );
                }
            }
            // 主按钮文字叠 accent 底 —— **静止态与悬停态都要过**。
            // 深色主题这里曾经写成"白字叠 blue-500"（3.68:1），正是这条断言抓出来的：
            // 只测静止态就会漏掉悬停那一格。
            for (state, bg) in [("accent", p.accent), ("accent_hover", p.accent_hover)] {
                let r = contrast(p.text_on_accent, bg);
                assert!(r >= 4.5, "{name}/主按钮文字叠 {state} 对比度 {r:.2} < 4.5");
            }
        }
    }

    /// 复选框方框是**控件边界**，WCAG 1.4.11 要求 ≥ 3:1（不是 4.5）。
    ///
    /// SPEC §2.4.6 给的 `#adb2b8`（浅色）在白卡上只有 2.13:1、`#61666b`（深色）
    /// 在深卡上只有 2.71:1 —— 都看不见。所以这里按**可达性**取同色阶的深/浅一档，
    /// 并在注释里记明原因（仍取自同一套色阶，不新调颜色）。
    #[test]
    fn checkbox_box_is_visible() {
        for (name, p) in [("light", &LIGHT), ("dark", &DARK)] {
            let r = contrast(p.checkbox_box, p.card_bg);
            assert!(
                r >= 3.0,
                "{name}/复选框方框对比度 {r:.2} < 3.0，未勾选的框会看不见"
            );
            // 勾号叠勾选底色也要可辨。
            let r = contrast(p.check_mark, p.checkbox_checked);
            assert!(r >= 3.0, "{name}/勾号对比度 {r:.2} < 3.0");
        }
    }

    /// 焦点环是**非文字**元素，无障碍门槛 3:1（WCAG 1.4.11）。
    /// 自绘控件最容易漏的就是它 —— 键盘用户会失去位置指示（SPEC §2.4.4）。
    #[test]
    fn focus_ring_is_visible_on_both_backgrounds() {
        for (name, p) in [("light", &LIGHT), ("dark", &DARK)] {
            for (what, bg) in [("window", p.window_bg), ("card", p.card_bg)] {
                let r = contrast(p.focus_ring, bg);
                assert!(r >= 3.0, "{name}/{what} 上焦点环对比度 {r:.2} < 3.0");
            }
        }
    }

    /// 主题必须真的有两套（复制粘贴后忘改是这类常量表最常见的 bug）。
    #[test]
    fn themes_differ_and_match_documented_tokens() {
        assert_ne!(LIGHT.window_bg, DARK.window_bg);
        assert_ne!(LIGHT.card_bg, DARK.card_bg);
        assert_ne!(LIGHT.text_primary, DARK.text_primary);
        // 深色底必须比浅色底暗（防止把两套写反）。
        assert!(luminance(DARK.window_bg) < luminance(LIGHT.window_bg));
        assert!(luminance(DARK.card_bg) < luminance(LIGHT.card_bg));
        // 钉住 SPEC §2.4.6 的定稿值，防止日后"顺手调一下"漂移。
        assert_eq!(LIGHT.window_bg, rgb(0xF5, 0xF6, 0xF7));
        assert_eq!(DARK.window_bg, rgb(0x15, 0x15, 0x17));
        assert_eq!(DARK.card_bg, rgb(0x23, 0x23, 0x24));
        assert_eq!(LIGHT.text_primary, rgb(0x0F, 0x11, 0x15));
        assert_eq!(DARK.text_primary, rgb(0xF9, 0xFA, 0xFB));
        assert_eq!(LIGHT.accent, rgb(0x25, 0x63, 0xEB));
        assert_eq!(DARK.accent, rgb(0x3B, 0x82, 0xF6));
    }

    /// 圆角必须递增（小元素 < 输入框 < 卡片），且输入框比卡片小。
    #[test]
    fn radii_follow_scale() {
        assert_eq!(RADIUS_CHECK, 4);
        assert_eq!(RADIUS_FIELD, 8);
        assert_eq!(RADIUS_CARD, 12);
        assert!(RADIUS_CHECK < RADIUS_FIELD && RADIUS_FIELD < RADIUS_CARD);
    }

    /// 字号必须落在 Windows 11 刻度上，且正文 ≥ 14px（规范里 12px 只允许做 Caption）。
    #[test]
    fn font_scale_matches_windows_typography() {
        assert_eq!(FONT_CAPTION, 12);
        assert_eq!(FONT_BODY, 14);
        assert_eq!(FONT_CARD_TITLE, 14);
        assert_eq!(FONT_TITLE, 20);
        assert!(FONT_BODY >= 14, "正文小于 14px 在部分语言里不可读");
        assert!(WEIGHT_SEMIBOLD > WEIGHT_REGULAR);
    }
}
