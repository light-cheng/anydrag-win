//! 配置：`%APPDATA%\anydrag-win\config.json`。
//!
//! 纯逻辑模块——不 import windows-sys（可单测边界，见 GOAL §7）。
//! 序列化用 serde；缺失字段/损坏文件一律回退默认值并打警告，绝不 panic。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 触发修饰键默认值：VK_MENU (Alt) = 0x12。
pub const DEFAULT_MODS: [u16; 1] = [0x12];

/// 默认拖动策略。T2.3 spike（2026-10-15）实测定稿："fallback"——
/// 远程 0xF012 在 explorer 上生效（B 按键变体 dx=160）但 notepad 失效，
/// 按 GOAL 判定规则（两者均生效才可用 native）落 fallback。
/// 证据与复跑方法见 docs/spike-scandrag.md。
const DEFAULT_STRATEGY: &str = "fallback";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    /// 总开关。false 时钩子仍装，但引擎对一切手势返回 None（§T3.2）。
    pub enabled: bool,
    /// VK 码列表，要求"全部按下"才算触发；默认 [Alt]。
    pub modifiers: Vec<u16>,
    /// 位移达到该像素数才提交拖动，否则视为双击候选。默认 5。
    pub drag_threshold_px: u32,
    /// 双击判定窗口。None = 跟随系统 GetDoubleClickTime()；默认 None。
    pub double_click_ms: Option<u32>,
    /// "native" | "fallback"——T2.3 spike 结论写入的默认值。
    pub default_strategy: String,
    /// 进程文件名（小写）→ 该应用完全无视手势。
    pub native_excluded: Vec<String>,
    /// 进程文件名 → 强制 fallback 策略。
    pub fallback_apps: Vec<String>,
}

// Default 手写而非 derive：derive 会给 enabled=false / modifiers=[]，
// 与产品默认（启用 + Alt 触发）相反。
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            modifiers: DEFAULT_MODS.to_vec(),
            drag_threshold_px: 5,
            double_click_ms: None,
            default_strategy: DEFAULT_STRATEGY.to_string(),
            native_excluded: Vec::new(),
            fallback_apps: Vec::new(),
        }
    }
}

/// 配置路径：`%APPDATA%\anydrag-win\config.json`（手拼，不引 directories crate）。
fn config_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join("anydrag-win").join("config.json")
}

/// 读取配置。文件不存在/JSON 损坏 → 返回默认值并打印警告（不 panic、不退出）。
pub fn load() -> Config {
    load_from(&config_path())
}

impl Config {
    /// 原子写：先写同目录 `.json.tmp` 再 rename——避免托盘写回时半截文件毁掉配置。
    pub fn save(&self) {
        if let Err(e) = save_to(self, &config_path()) {
            eprintln!("anydrag-win: failed to save config: {e}");
        }
    }
}

/// 显式路径版本（单测 hermetic 用；行为与 load/save 相同）。
pub fn load_from(path: &std::path::Path) -> Config {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("anydrag-win: config {path:?} is corrupt ({e}), using defaults");
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => {
            eprintln!("anydrag-win: cannot read config {path:?} ({e}), using defaults");
            Config::default()
        }
    }
}

/// 显式路径版本的原子保存。创建父目录 → 写 .tmp → rename。
pub fn save_to(cfg: &Config, path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = {
        let mut p = std::path::PathBuf::from(path);
        p.set_extension("json.tmp");
        p
    };
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 每个测试独立子目录（同进程 PID 相同，共享目录会让并发测试互踩文件）。
    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("anydrag-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn config_defaults() {
        let c = Config::default();
        assert!(c.enabled, "enabled defaults to true");
        assert_eq!(c.modifiers, DEFAULT_MODS, "modifiers default to [Alt]");
        assert_eq!(c.drag_threshold_px, 5);
        assert_eq!(c.double_click_ms, None);
        assert!(!c.default_strategy.is_empty());
        assert!(c.native_excluded.is_empty());
        assert!(c.fallback_apps.is_empty());
    }

    #[test]
    fn config_round_trip() {
        let dir = tmp_dir("round_trip");
        let path = dir.join("config.json");
        let mut c = Config::default();
        c.native_excluded.push("notepad.exe".into());
        c.fallback_apps.push("msedge.exe".into());
        c.double_click_ms = Some(350);
        save_to(&c, &path).unwrap();
        let back = load_from(&path);
        assert_eq!(c, back, "save→load must be identity");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_bad_json_falls_back_to_default() {
        let dir = tmp_dir("bad_json");
        let path = dir.join("config.json");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"{ this is : not json ").unwrap();
        let c = load_from(&path);
        assert_eq!(c, Config::default(), "corrupt file must yield defaults, not panic");
        let _ = fs::remove_dir_all(&dir);
    }
}
