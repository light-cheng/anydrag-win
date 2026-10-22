//! 修饰键匹配（纯逻辑，零 windows-sys）。
//!
//! 语义（YAGNI，注释写明）：只要求"配置的键全部按下"即触发；
//! 多按了其他键**不**阻挡（用户按 Alt 的同时恰好按着 Ctrl 不算失误）。
//!
//! mask 约定：`down_mask` 的每一位 = 一个修饰键的"按下"状态，
//! bit 索引 = `VK & 0x1F`。Windows 修饰键 VK 码（SHIFT 0x10 / CONTROL 0x11 /
//! MENU 0x12 / LSHIFT 0xA0 / LCONTROL 0xA3 / LMENU 0xA5 …）取 `& 0x1F`
//! 后得到 bit 16/17/18/0/3/5…… 左侧/右侧修饰键与基础码会落到不同 bit，
//! MVP 按"配置哪个 VK 就检查哪个 bit"处理（配置 [0x12] = 只认 MENU 位）。
//!
//! **类型注记（对早期 GOAL 的偏差，编译器证实）**：早期 GOAL 曾标注
//! `down_mask: u16`，但其自己的位规则 `VK & 0x1F` 对 Alt(0x12) 产生 bit 18，
//! u16（bit 0..15）装不下——const 求值直接报 E0080。故 mask 全线加宽为 u32
//! （bit 规则本身不变，仍是 `VK & 0x1F`）。

/// 修饰键是否满足要求（本模块**唯一**的判定入口）。
///
/// **`required_mask == 0` 必须返回 false**（= 永不触发）。这一条是**安全底线**，
/// 不是口味问题：`down_mask & 0 == 0` 恒真，若照位运算直译，空列表就等于
/// "任何一次鼠标点击都拖动窗口"。2026-10-22 审查发现生产路径正是这么写的
/// （`engine::feed` 内联闭包 `m & mask == mask`），而这里带测试的判定函数
/// 反而没有调用点——两个实现给出相反语义，生效的恰好是没测试的那个。
///
/// 现在 `engine` 的三条路径（拖动 Down / resize Down / ArmedResize 提交）全部走本函数，
/// 语义只有一处定义。**不要再提供一个接受 `&[u16]` 的平行版本**：那正是本坑的成因
/// （VK 列表 → mask 的折算已由 `config::mask_of` 负责，不必在此重复）。
pub fn matches_mask(down_mask: u32, required_mask: u32) -> bool {
    required_mask != 0 && down_mask & required_mask == required_mask
}

#[cfg(test)]
mod tests {
    use super::*;

    // 位规则：bit 索引 = VK & 0x1F。
    // Alt (VK 0x12) → bit 18；Ctrl (0x11) → bit 17；Shift (0x10) → bit 16。
    const ALT: u32 = 1u32 << 18;
    const CTRL: u32 = 1u32 << 17;
    const SHIFT: u32 = 1u32 << 16;

    /// 把 VK 列表折算成 mask——**与生产路径同一个函数**（`config::mask_of`）。
    /// 测试若自带一份折算逻辑，就又会变成"两个实现"（见 `matches_mask` 注记）。
    fn mask(vks: &[u16]) -> u32 {
        crate::config::mask_of(vks)
    }

    #[test]
    fn mods_alt_alone_triggers() {
        assert!(matches_mask(ALT, mask(&[0x12])), "Alt alone must trigger [Alt]");
        assert!(!matches_mask(0, mask(&[0x12])), "no keys must not trigger");
        assert!(!matches_mask(ALT, mask(&[])), "empty required never triggers");
    }

    #[test]
    fn mods_combo_required_and_extra_keys_allowed() {
        // 配置 [Alt, Ctrl]：两者齐全才触发。
        let ac = mask(&[0x12, 0x11]);
        assert!(matches_mask(ALT | CTRL, ac), "Alt+Ctrl must trigger [Alt, Ctrl]");
        assert!(!matches_mask(ALT, ac), "missing Ctrl must not trigger [Alt, Ctrl]");
        assert!(!matches_mask(CTRL, ac), "missing Alt must not trigger [Alt, Ctrl]");
        // 多余按键不阻挡（YAGNI）：配置 [Alt] 时按着 Shift 也照样触发。
        assert!(
            matches_mask(ALT | SHIFT, mask(&[0x12])),
            "extra keys must not block (YAGNI)"
        );
    }

    /// 回归（2026-10-22）：`required_mask == 0` 绝不能被位运算直译成"恒真"。
    ///
    /// 生产路径（`engine::feed`）历史上用的是内联 `m & mask == mask`，于是
    /// `modifiers: []` 让每一次鼠标点击都变成拖动窗口。这条断言钉住它。
    #[test]
    fn empty_required_mask_never_triggers() {
        // 0 永远不匹配，哪怕 down_mask 也是 0（`0 & 0 == 0` 的陷阱就在这里）。
        assert!(!matches_mask(0, 0), "empty required mask must never match");
        assert!(!matches_mask(ALT | CTRL, 0), "empty required mask ignores down keys");
        // 经 VK 列表折算后同样为 0，语义必须一致。
        assert_eq!(mask(&[]), 0, "empty VK list must fold to mask 0");
        assert!(!matches_mask(ALT | CTRL, mask(&[])), "empty list must not trigger");
        // 非空 mask 的正常路径不受影响。
        assert!(matches_mask(ALT, mask(&[0x12])));
        assert!(!matches_mask(ALT, mask(&[0x11])), "Alt must not satisfy a Ctrl requirement");
    }
}
