//! 配置：`%APPDATA%\anydrag-win\config.json`。
//!
//! 纯逻辑模块——不 import windows-sys（可单测边界，见 GOAL §7）。
//! 序列化用 serde；缺失字段/损坏文件一律回退默认值并打警告，绝不 panic。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

/// 触发修饰键默认值：VK_MENU (Alt) = 0x12。
pub const DEFAULT_MODS: [u16; 1] = [0x12];
/// resize 触发键默认值：**Alt+Ctrl 同时按住**（VK_MENU=0x12 与 VK_CONTROL=0x11；
/// mask 匹配要求两 bit 同时置位，缺一即不触发）。单按 Alt 仍是拖拽/双击专属
/// （用户 2026-10-16 定稿：resize 不单独占用 Ctrl，改为 Alt+Ctrl 组合）；
/// 可配置 `resize_modifiers`，`[]` = 关闭 resize 功能。
pub const DEFAULT_RESIZE_MODS: [u16; 2] = [0x11, 0x12];

/// 内存"启用"开关（`Config.enabled` 的 AtomicBool 镜像，T3.3 引入）：
/// 托盘线程要翻转它、engine 线程要读它——`Arc<Config>` 内的 bool 不能原地
/// 改（共享引用 UB），故开关走原子镜像；磁盘持久化仍由 `Config::save()` 承担。
/// main 启动时按 `Config::load()` 初始化。
pub static ENABLED: AtomicBool = AtomicBool::new(true);

/// 当前生效配置的全局存放点（2026-10-16 设置界面引入，用于**热重载**）。
///
/// 为什么需要它：engine 线程持的是 `Arc<Config>`（不可变），设置界面保存的新值
/// 没法"塞回去"。改用一个全局 `RwLock<Config>` + 版本号：
/// 写方（设置界面）`replace()` 后版本号 +1；读方（engine 线程）每个事件批次
/// 比一次版本号，变了才 clone 一份新快照 —— 热路径上没有锁竞争、也不做无谓克隆。
static STORE: OnceLock<RwLock<Config>> = OnceLock::new();
static VERSION: AtomicU64 = AtomicU64::new(0);

/// 启动时装入初始配置（main 调用一次）。
pub fn init(cfg: Config) {
    let _ = STORE.set(RwLock::new(cfg));
}

/// 配置版本号：每次 `replace()` 自增。engine 线程用它判断是否需要刷新快照。
pub fn version() -> u64 {
    VERSION.load(Ordering::Acquire)
}

/// 取当前配置的快照（未 `init` 时返回默认值，保证任何调用顺序都不 panic）。
pub fn snapshot() -> Config {
    match STORE.get() {
        Some(lock) => lock.read().map(|c| c.clone()).unwrap_or_default(),
        None => Config::default(),
    }
}

/// 用新配置替换当前生效值（热重载），并自增版本号唤醒读方。
/// **不写盘**——写盘由调用方显式 `Config::save()`（设置界面保存按钮负责）。
pub fn replace(cfg: Config) {
    if let Some(lock) = STORE.get() {
        if let Ok(mut w) = lock.write() {
            *w = cfg;
        }
    }
    VERSION.fetch_add(1, Ordering::Release);
}

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
    /// resize 触发键 VK 码列表，要求"全部同时按下"；默认 [Ctrl, Alt]（Alt+Ctrl
    /// 组合，2026-10-16 用户定稿；单按 Alt 仍为拖拽/双击专属）。
    /// **在窗口任意位置按下即进入缩放**，方向按九宫格/就近角自动判定，不需要贴边
    /// （2026-10-16 废弃旧的 10px 边带触发——那等于把原生缩放重做一遍且更难瞄）；
    /// `[]` = 关闭 resize。
    pub resize_modifiers: Vec<u16>,
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
    /// 中键方向分屏开关（2026-10-16 用户要求）。true 时：按住中键拖动超过
    /// `middle_snap_threshold_px` 再松开 → 窗口吸附到该方向对应的半屏/四分之一屏。
    /// 未达阈值的中键点击**不干预**（应用自己的中键粘贴/滚动照常）。
    pub middle_snap: bool,
    /// 中键分屏的方向判定阈值（px）：拖拽位移两个轴都小于它 = 普通中键点击。
    pub middle_snap_threshold_px: u32,
}

// Default 手写而非 derive：derive 会给 enabled=false / modifiers=[]，
// 与产品默认（启用 + Alt 触发）相反。
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            modifiers: DEFAULT_MODS.to_vec(),
            resize_modifiers: DEFAULT_RESIZE_MODS.to_vec(),
            drag_threshold_px: 5,
            double_click_ms: None,
            default_strategy: DEFAULT_STRATEGY.to_string(),
            native_excluded: Vec::new(),
            fallback_apps: Vec::new(),
            middle_snap: true,
            middle_snap_threshold_px: 40,
        }
    }
}

/// 配置路径：`%APPDATA%\anydrag-win\config.json`（手拼，不引 directories crate）。
/// VK 列表 → mask：bit = `VK & 0x1F`（与 mods.rs / hook.rs 同一位规则；
/// u32 加宽见 mods.rs 模块注记）。空列表 → 0。
/// `modifiers_mask()` / `resize_mask()` 共用，避免两套位规则漂移。
pub fn mask_of(vks: &[u16]) -> u32 {
    vks.iter().fold(0u32, |m, &vk| m | 1u32 << (vk & 0x1F))
}

fn config_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(appdata).join("anydrag-win").join("config.json")
}

/// 配置路径（T3.3 "Config…" 菜单项展示用）：`%APPDATA%\anydrag-win\config.json`。
pub fn file_path() -> PathBuf {
    config_path()
}

/// 读取配置。文件不存在/JSON 损坏 → 返回默认值并打印警告（不 panic、不退出）。
pub fn load() -> Config {
    load_from(&config_path())
}

impl Config {
    /// 构造"触发所需修饰键"mask：bit = `VK & 0x1F`（与 mods.rs / hook.rs 同一位规则；
    /// u32 加宽见 mods.rs 模块注记）。engine_loop 每次 feed 现算（O(≤4) 位运算）。
    pub fn modifiers_mask(&self) -> u32 {
        mask_of(&self.modifiers)
    }

    /// resize 触发键 mask（`resize_modifiers`，位规则同 `modifiers_mask`）；
    /// 空列表 = 0 = 关闭 resize 路径。
    pub fn resize_mask(&self) -> u32 {
        mask_of(&self.resize_modifiers)
    }

    /// 钩子线程需要监视的全部 VK：拖动触发键 ∪ 缩放触发键（去重）。
    /// 设置界面热重载后要用它更新 `hook::set_mods`——否则改了触发键也不生效。
    pub fn all_vks(&self) -> Vec<u16> {
        let mut v = self.modifiers.clone();
        for &vk in self.resize_modifiers.iter() {
            if !v.contains(&vk) {
                v.push(vk);
            }
        }
        v
    }

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

    #[test]
    fn config_store_hot_reload_roundtrip() {
        // 设置界面"保存即生效"的机制：replace() 换值 + 版本号自增；
        // engine 线程靠版本号决定是否刷新快照（见 engine_loop::run 的热重载检查）。
        // 放在同一个测试里做，避免与其他用例并发争抢全局存放点。
        init(Config::default());
        let v0 = version();
        let mut c = Config::default();
        c.drag_threshold_px = 77;
        replace(c);
        assert!(version() > v0, "replace must bump the version so readers notice");
        assert_eq!(
            snapshot().drag_threshold_px,
            77,
            "snapshot must observe the replaced value"
        );
    }
}
