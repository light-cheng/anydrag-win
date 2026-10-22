//! 界面文案（中文 / English）。
//!
//! **纯逻辑模块**：不 import windows-sys（AGENTS.md §1 不变量 4），因此可单测。
//! FFI 侧只做两件事：把 `tr()` 的返回值塞进控件、按系统语言给出 `system_is_zh`。
//!
//! ## 为什么要"一处定义"
//!
//! 文案散落在 `settings.rs` / `tray.rs` 里各写一遍中文，是坑 #18 的同款形态：
//! 同一语义两处实现，改语言时必然漏掉一处，而漏掉的那处**不会报错**，
//! 只会让界面中英混杂。所以全部文案收在 `Key` 枚举里，`tr()` 是唯一出口；
//! `tr()` 用**穷举 match**，新增 `Key` 而漏写任一语言 = 编译失败（比运行时断言强）。
//!
//! ## 为什么不引 i18n crate
//!
//! 两种语言、约 30 条文案。引 crate 要处理资源文件打包、语言协商、复数规则，
//! 而本工具没有复数需求。手写表反而更小、更可测。

/// 界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    /// 解析配置里的语言字符串。空串/未知值 = **跟随系统**（首次运行、旧配置文件）。
    ///
    /// `system_is_zh` 由调用方从系统 UI 语言算出来（FFI 在 `winmod`），
    /// 这样本函数保持纯逻辑、可单测。
    pub fn from_config(s: &str, system_is_zh: bool) -> Lang {
        match s.trim().to_ascii_lowercase().as_str() {
            "zh" => Lang::Zh,
            "en" => Lang::En,
            _ => {
                if system_is_zh {
                    Lang::Zh
                } else {
                    Lang::En
                }
            }
        }
    }

    /// 存回配置用的代码。
    pub fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    /// 另一个语言（语言切换按钮用）。
    ///
    /// 界面用的是两个独立的分段按钮（点哪个直接选哪个，不是"切到另一个"），
    /// 所以生产路径不调它；保留是因为单测用它钉住"两种语言互为反面"这个性质。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn toggle(self) -> Lang {
        match self {
            Lang::Zh => Lang::En,
            Lang::En => Lang::Zh,
        }
    }
}

/// 全部界面文案的键。**新增键必须同时补两种语言**，否则 `tr` 的穷举 match 编译失败。
///
/// 分组与 SPEC §3.1 的信息架构一一对应（拖动/双击、缩放、手势参数、排除进程），
/// 改布局时照着这里的分组走，别让键名停留在旧版"3 张卡片"的形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // ── 标题区 ───────────────────────────────
    WinTitle,
    Subtitle,
    /// 语言分段按钮的两个标签。**这两种语言下文案相同**（语言名永远用自身语言写，
    /// 否则英文界面里出现"中文"、中文界面里出现"Chinese" 反而看不懂）。
    LangZh,
    LangEn,
    // ── 卡片标题 ─────────────────────────────
    CardDrag,
    CardResize,
    CardParams,
    CardExcluded,
    /// 卡 4 的灰色说明行（取代旧版游离在卡片外的"完全无视手势的进程："）。
    CardExcludedDesc,
    // ── 行标签 ───────────────────────────────
    LblTriggerKeys,
    LblDragThreshold,
    LblDoubleClick,
    LblMiddleSnap,
    LblMiddleThreshold,
    ChkEnable,
    // ── 单位后缀（自绘，跟在输入框后）────────
    UnitPx,
    /// 双击间隔的单位后缀；`{}` 不参与，纯静态。
    UnitMsSystem,
    // ── 按钮 / 提示 ──────────────────────────
    BtnPickProcs,
    BtnSave,
    BtnCancel,
    BtnOk,
    HintImmediate,
    // ── 进程选择弹窗 ─────────────────────────
    ProcPickerTitle,
    ProcNoTitle,
    // ── 托盘 ────────────────────────────────
    TrayTipDisabled,
    MenuEnable,
    MenuDisable,
    MenuSettings,
    MenuUsage,
    MenuConfig,
    MenuQuit,
    /// 用法弹窗正文。含**恰好一个 `{}`**，由调用方填入配置文件路径。
    UsageBody,
    ConfigOpenFailed,
}

/// 取文案。唯一的文案出口。
pub fn tr(lang: Lang, key: Key) -> &'static str {
    match lang {
        Lang::Zh => zh(key),
        Lang::En => en(key),
    }
}

/// 两个语言下**允许**完全相同的键：语言名用自身语言书写是通用做法
/// （Windows 的语言列表、浏览器的语言选择器都这样），不是漏翻译。
#[cfg_attr(not(test), allow(dead_code))]
pub const SAME_IN_BOTH_LANGS: &[Key] = &[Key::LangZh, Key::LangEn];

fn zh(key: Key) -> &'static str {
    match key {
        Key::WinTitle => "anydrag-win 设置",
        Key::Subtitle => "按住 Alt 拖动任意窗口 · 托盘工具",
        Key::LangZh => "中文",
        Key::LangEn => "English",
        Key::CardDrag => "拖动 / 双击",
        Key::CardResize => "缩放",
        Key::CardParams => "手势参数",
        Key::CardExcluded => "排除进程",
        Key::CardExcludedDesc => "这些进程完全无视手势，一行一个，小写。",
        Key::LblTriggerKeys => "触发键",
        Key::LblDragThreshold => "拖动阈值",
        Key::LblDoubleClick => "双击间隔",
        Key::LblMiddleSnap => "中键方向分屏",
        Key::LblMiddleThreshold => "判定阈值",
        Key::ChkEnable => "启用",
        Key::UnitPx => "像素",
        Key::UnitMsSystem => "毫秒（留空 = 跟随系统）",
        Key::BtnPickProcs => "选择进程…",
        Key::BtnSave => "保存",
        Key::BtnCancel => "取消",
        Key::BtnOk => "确定",
        Key::HintImmediate => "保存后立即生效，无需重启。",
        Key::ProcPickerTitle => "选择要无视手势的进程",
        Key::ProcNoTitle => "（无标题）",
        Key::TrayTipDisabled => "anydrag-win（已禁用）",
        Key::MenuEnable => "启用",
        Key::MenuDisable => "禁用",
        Key::MenuSettings => "设置…",
        Key::MenuUsage => "用法…",
        Key::MenuConfig => "配置文件…",
        Key::MenuQuit => "退出",
        Key::UsageBody => {
            "anydrag-win 用法\n\n\
             按住 Alt：\n\
             · 按住鼠标左键拖动 → 移动当前窗口（不限于标题栏，点哪儿拖哪儿）\n\
             · 双击（约 0.5 秒内两次、几乎不位移）→ 窗口最大化；已最大化时再双击 → 还原\n\n\
             按住 Alt + Ctrl：\n\
             · 在窗口任意位置拖动 → 调整窗口大小（方向自动判定，不用瞄边缘）\n\n\
             按住鼠标中键：\n\
             · 朝某个方向拖拽后松开 → 窗口吸附到该方向的半屏 / 四分之一屏\n\n\
             更详细的选项（触发键、阈值、排除名单、中键分屏开关、界面语言）见托盘菜单「设置…」。\n\
             配置文件：{}\n\
             修改后立即生效，无需重启（「设置…」保存或直接手改该文件均可）。"
        }
        Key::ConfigOpenFailed => "无法自动打开配置文件，请手动打开：",
    }
}

fn en(key: Key) -> &'static str {
    match key {
        Key::WinTitle => "anydrag-win Settings",
        Key::Subtitle => "Hold Alt and drag any window · Tray utility",
        Key::LangZh => "中文",
        Key::LangEn => "English",
        Key::CardDrag => "Drag / double-click",
        Key::CardResize => "Resize",
        Key::CardParams => "Gesture parameters",
        Key::CardExcluded => "Excluded processes",
        Key::CardExcludedDesc => {
            "These processes ignore all gestures. One per line, lowercase."
        }
        Key::LblTriggerKeys => "Trigger keys",
        Key::LblDragThreshold => "Drag threshold",
        Key::LblDoubleClick => "Double-click window",
        Key::LblMiddleSnap => "Middle-drag snap",
        Key::LblMiddleThreshold => "Snap threshold",
        Key::ChkEnable => "Enabled",
        Key::UnitPx => "px",
        Key::UnitMsSystem => "ms (blank = system)",
        Key::BtnPickProcs => "Pick processes…",
        Key::BtnSave => "Save",
        Key::BtnCancel => "Cancel",
        Key::BtnOk => "OK",
        Key::HintImmediate => "Changes apply immediately — no restart needed.",
        Key::ProcPickerTitle => "Choose processes to ignore gestures",
        Key::ProcNoTitle => "(untitled)",
        Key::TrayTipDisabled => "anydrag-win (disabled)",
        Key::MenuEnable => "Enable",
        Key::MenuDisable => "Disable",
        Key::MenuSettings => "Settings…",
        Key::MenuUsage => "Usage…",
        Key::MenuConfig => "Config file…",
        Key::MenuQuit => "Exit",
        Key::UsageBody => {
            "anydrag-win usage\n\n\
             Hold Alt:\n\
             · Hold the left mouse button and drag → move the current window \
             (no title bar needed, grab anywhere)\n\
             · Double-click (two clicks within ~0.5s, barely moving) → maximize; \
             double-click again while maximized → restore\n\n\
             Hold Alt + Ctrl:\n\
             · Drag anywhere in the window → resize it \
             (direction is detected automatically, no need to aim for the edge)\n\n\
             Hold the middle button:\n\
             · Drag toward a direction and release → snap the window to that \
             half / quarter of the screen\n\n\
             More options (trigger keys, thresholds, excluded processes, middle-drag snap, \
             UI language) are in the tray menu under \"Settings…\".\n\
             Config file: {}\n\
             Changes apply immediately, no restart needed (either via \"Settings…\" or by \
             editing the file directly)."
        }
        Key::ConfigOpenFailed => "Could not open the config file automatically. Open it manually:",
    }
}

/// 修饰键的显示名。**两套语言相同**：Alt / Ctrl / Shift / Win 是物理键名，
/// 中文界面里也写 Alt（Windows 自己的中文界面就这么写）。
///
/// 放在 i18n 而不是 `settings.rs`：文案只能有一个来源（SPEC D4）。
pub fn modifier_label(vk: u16) -> &'static str {
    match vk {
        0x12 => "Alt",
        0x11 => "Ctrl",
        0x10 => "Shift",
        0x5B => "Win",
        _ => "",
    }
}

/// 四个修饰键（顺序即界面上的显示顺序：Alt / Ctrl / Shift / Win）。
pub const MODIFIER_VKS: [u16; 4] = [0x12, 0x11, 0x10, 0x5B];

/// 所有 Key（单测穷举用；新增 Key 忘加进这里会被 `every_key_translated` 发现）。
#[cfg_attr(not(test), allow(dead_code))]
pub const ALL_KEYS: &[Key] = &[
    Key::WinTitle,
    Key::Subtitle,
    Key::LangZh,
    Key::LangEn,
    Key::CardDrag,
    Key::CardResize,
    Key::CardParams,
    Key::CardExcluded,
    Key::CardExcludedDesc,
    Key::LblTriggerKeys,
    Key::LblDragThreshold,
    Key::LblDoubleClick,
    Key::LblMiddleSnap,
    Key::LblMiddleThreshold,
    Key::ChkEnable,
    Key::UnitPx,
    Key::UnitMsSystem,
    Key::BtnPickProcs,
    Key::BtnSave,
    Key::BtnCancel,
    Key::BtnOk,
    Key::HintImmediate,
    Key::ProcPickerTitle,
    Key::ProcNoTitle,
    Key::TrayTipDisabled,
    Key::MenuEnable,
    Key::MenuDisable,
    Key::MenuSettings,
    Key::MenuUsage,
    Key::MenuConfig,
    Key::MenuQuit,
    Key::UsageBody,
    Key::ConfigOpenFailed,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 每条文案两种语言都必须给全（`tr` 的穷举 match 已经保证编译期不缺，
    /// 但"给了空串"它查不出来 —— 空串在界面上就是一片空白按钮）。
    #[test]
    fn every_key_translated() {
        for &k in ALL_KEYS {
            for lang in [Lang::Zh, Lang::En] {
                let s = tr(lang, k);
                assert!(!s.trim().is_empty(), "{k:?} 在 {lang:?} 下是空串");
            }
        }
        // ALL_KEYS 必须真的覆盖所有变体：用它和 Key 的 Debug 名单核对长度。
        assert_eq!(
            ALL_KEYS.len(),
            33,
            "新增 Key 后请同步 ALL_KEYS 与本断言（当前 {} 条）",
            ALL_KEYS.len()
        );
    }

    /// 两种语言的译文必须**不同**：这是"漏翻译"的唯一可自动检测特征
    /// （复制粘贴中文再忘了改，界面上看不出来，测试能看出来）。
    ///
    /// 例外见 `SAME_IN_BOTH_LANGS`：语言名（"中文"/"English"）按惯例用自身语言写。
    #[test]
    fn translations_actually_differ() {
        for &k in ALL_KEYS {
            if SAME_IN_BOTH_LANGS.contains(&k) {
                continue;
            }
            let (z, e) = (tr(Lang::Zh, k), tr(Lang::En, k));
            assert_ne!(z, e, "{k:?} 中英译文相同，疑似漏翻译");
        }
    }

    /// 中文译文里不该混进整句英文（同上，漏翻译的另一种形态）。
    /// 只查"整条不含任何汉字"的情况，避免误伤 "px" / "Alt" / "Ctrl" 这类术语。
    #[test]
    fn chinese_strings_contain_chinese() {
        for &k in ALL_KEYS {
            // LangEn 的两种译文都是 "English" —— 语言名本就该这样写。
            if k == Key::LangEn {
                continue;
            }
            let s = tr(Lang::Zh, k);
            let has_han = s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
            assert!(has_han, "{k:?} 的中文译文不含汉字：{s:?}");
        }
    }

    #[test]
    fn lang_from_config_and_toggle() {
        // 显式值优先于系统语言。
        assert_eq!(Lang::from_config("zh", false), Lang::Zh);
        assert_eq!(Lang::from_config("en", true), Lang::En);
        // 大小写/空白容错（用户手改配置文件）。
        assert_eq!(Lang::from_config("  EN ", true), Lang::En);
        // 空串 / 未知值 = 跟随系统。
        assert_eq!(Lang::from_config("", true), Lang::Zh);
        assert_eq!(Lang::from_config("", false), Lang::En);
        assert_eq!(Lang::from_config("ja", false), Lang::En);
        assert_eq!(Lang::from_config("ja", true), Lang::Zh);
        // 存回配置的代码可逆。
        assert_eq!(Lang::from_config(Lang::Zh.code(), false), Lang::Zh);
        assert_eq!(Lang::from_config(Lang::En.code(), true), Lang::En);
        assert_eq!(Lang::Zh.toggle(), Lang::En);
        assert_eq!(Lang::En.toggle(), Lang::Zh);
    }

    /// 用法弹窗是多行文本，换行符必须真的存在（否则会挤成一行巨长文本）。
    #[test]
    fn usage_body_is_multiline() {
        for lang in [Lang::Zh, Lang::En] {
            let s = tr(lang, Key::UsageBody);
            assert!(s.lines().count() >= 8, "{lang:?} 用法文本行数太少");
        }
    }

    /// `UsageBody` 用 `{}` 占位配置文件路径。**必须恰好一个**：
    /// 少一个 → 路径丢失（用户找不到文件）；多一个 → `format!` 直接 panic。
    #[test]
    fn usage_body_has_exactly_one_placeholder() {
        for lang in [Lang::Zh, Lang::En] {
            let s = tr(lang, Key::UsageBody);
            assert_eq!(
                s.matches("{}").count(),
                1,
                "{lang:?} 的用法文本必须恰好含一个 {{}} 占位符"
            );
            // 真跑一遍 format，确认没有别的花括号把它撑爆。
            let out = s.replacen("{}", "C:\\x\\config.json", 1);
            assert!(out.contains("config.json"));
        }
    }

    /// 修饰键名是物理键名，两套语言相同；四个都要有、且互不相同。
    #[test]
    fn modifier_labels_are_stable() {
        let labels: Vec<&str> = MODIFIER_VKS.iter().map(|&v| modifier_label(v)).collect();
        assert_eq!(labels, vec!["Alt", "Ctrl", "Shift", "Win"]);
        assert_eq!(modifier_label(0x99), "", "未知 VK 不该编出一个名字");
    }

    /// 语言名必须各自用自身语言书写（这是 `SAME_IN_BOTH_LANGS` 的存在理由）。
    #[test]
    fn language_names_are_self_describing() {
        assert_eq!(tr(Lang::Zh, Key::LangZh), "中文");
        assert_eq!(tr(Lang::En, Key::LangZh), "中文");
        assert_eq!(tr(Lang::Zh, Key::LangEn), "English");
        assert_eq!(tr(Lang::En, Key::LangEn), "English");
    }
}
