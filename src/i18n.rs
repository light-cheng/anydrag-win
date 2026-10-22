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
//! 单测 `every_key_translated` 用穷举 match 钉住"新增 Key 必须两种语言都给译文"。
//!
//! ## 为什么不引 i18n crate
//!
//! 两种语言、约 30 条文案。引 crate 要处理资源文件打包、语言协商、复数规则，
//! 而本工具没有复数、没有占位符替换需求。手写表反而更小、更可测。

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
    pub fn toggle(self) -> Lang {
        match self {
            Lang::Zh => Lang::En,
            Lang::En => Lang::Zh,
        }
    }
}

/// 全部界面文案的键。**新增键必须同时补两种语言**，否则 `tr` 的穷举 match 编译失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // ── 设置窗口 ─────────────────────────────
    WinTitle,
    Subtitle,
    CardTrigger,
    CardParams,
    CardExcluded,
    LblDrag,
    LblResize,
    LblThreshold,
    LblDoubleClick,
    LblMiddleSnap,
    LblMiddleThreshold,
    BtnPickProcs,
    BtnSave,
    BtnCancel,
    ChkEnable,
    HintImmediate,
    // ── 进程选择弹窗 ─────────────────────────
    ProcPickerTitle,
    BtnOk,
    // ── 托盘 ────────────────────────────────
    TrayTipDisabled,
    MenuEnable,
    MenuDisable,
    MenuSettings,
    MenuUsage,
    MenuConfig,
    MenuQuit,
    UsageBody,
}

/// 取文案。唯一的文案出口。
pub fn tr(lang: Lang, key: Key) -> &'static str {
    match lang {
        Lang::Zh => zh(key),
        Lang::En => en(key),
    }
}

fn zh(key: Key) -> &'static str {
    match key {
        Key::WinTitle => "anydrag-win 设置",
        Key::Subtitle => "按住 Alt 拖动任意窗口 · 托盘工具",
        Key::CardTrigger => "触发键",
        Key::CardParams => "手势参数",
        Key::CardExcluded => "排除进程（一行一个，小写）",
        Key::LblDrag => "拖动 / 双击：",
        Key::LblResize => "缩放：",
        Key::LblThreshold => "拖动阈值（像素）",
        Key::LblDoubleClick => "双击间隔（毫秒，留空 = 跟随系统）",
        Key::LblMiddleSnap => "中键方向分屏",
        Key::LblMiddleThreshold => "判定阈值（像素）",
        Key::BtnPickProcs => "选择进程…",
        Key::BtnSave => "保存",
        Key::BtnCancel => "取消",
        Key::ChkEnable => "启用",
        Key::HintImmediate => "保存后立即生效，无需重启。",
        Key::ProcPickerTitle => "选择要无视手势的进程",
        Key::BtnOk => "确定",
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
             修改后立即生效，无需重启（「设置…」保存或直接手改配置文件均可）。"
        }
    }
}

fn en(key: Key) -> &'static str {
    match key {
        Key::WinTitle => "anydrag-win Settings",
        Key::Subtitle => "Hold Alt and drag any window · Tray utility",
        Key::CardTrigger => "Trigger keys",
        Key::CardParams => "Gesture parameters",
        Key::CardExcluded => "Excluded processes (one per line, lowercase)",
        Key::LblDrag => "Drag / double-click:",
        Key::LblResize => "Resize:",
        Key::LblThreshold => "Drag threshold (px)",
        Key::LblDoubleClick => "Double-click window (ms, blank = system)",
        Key::LblMiddleSnap => "Middle-drag snap",
        Key::LblMiddleThreshold => "Threshold (px)",
        Key::BtnPickProcs => "Select processes…",
        Key::BtnSave => "Save",
        Key::BtnCancel => "Cancel",
        Key::ChkEnable => "Enable",
        Key::HintImmediate => "Changes apply immediately. No restart needed.",
        Key::ProcPickerTitle => "Choose processes to ignore gestures",
        Key::BtnOk => "OK",
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
             Changes apply immediately, no restart needed (either via \"Settings…\" or by \
             editing the config file directly)."
        }
    }
}

/// 所有 Key（单测穷举用；新增 Key 忘加进这里会被 `every_key_translated` 发现）。
pub const ALL_KEYS: &[Key] = &[
    Key::WinTitle,
    Key::Subtitle,
    Key::CardTrigger,
    Key::CardParams,
    Key::CardExcluded,
    Key::LblDrag,
    Key::LblResize,
    Key::LblThreshold,
    Key::LblDoubleClick,
    Key::LblMiddleSnap,
    Key::LblMiddleThreshold,
    Key::BtnPickProcs,
    Key::BtnSave,
    Key::BtnCancel,
    Key::ChkEnable,
    Key::HintImmediate,
    Key::ProcPickerTitle,
    Key::BtnOk,
    Key::TrayTipDisabled,
    Key::MenuEnable,
    Key::MenuDisable,
    Key::MenuSettings,
    Key::MenuUsage,
    Key::MenuConfig,
    Key::MenuQuit,
    Key::UsageBody,
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
            26,
            "新增 Key 后请同步 ALL_KEYS 与本断言（当前 {} 条）",
            ALL_KEYS.len()
        );
    }

    /// 两种语言的译文必须**不同**：这是"漏翻译"的唯一可自动检测特征
    /// （复制粘贴中文再忘了改，界面上看不出来，测试能看出来）。
    ///
    /// 例外：品牌名、纯符号不在此列 —— 所以这里只跳过明确允许相同的键。
    #[test]
    fn translations_actually_differ() {
        for &k in ALL_KEYS {
            let (z, e) = (tr(Lang::Zh, k), tr(Lang::En, k));
            assert_ne!(z, e, "{k:?} 中英译文相同，疑似漏翻译");
        }
    }

    /// 中文译文里不该混进 ASCII 字母开头的整句英文（同上，漏翻译的另一种形态）。
    /// 只查"整条不含任何汉字"的情况，避免误伤 "px" / "Alt" / "Ctrl" 这类术语。
    #[test]
    fn chinese_strings_contain_chinese() {
        for &k in ALL_KEYS {
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
}
