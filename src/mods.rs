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
//! **类型注记（对 GOAL 的偏差，编译器证实）**：GOAL T1.3/T1.4 标注
//! `down_mask: u16`，但其自己的位规则 `VK & 0x1F` 对 Alt(0x12) 产生 bit 18，
//! u16（bit 0..15）装不下——const 求值直接报 E0080。故 mask 全线加宽为 u32
//! （bit 规则本身不变，仍是 `VK & 0x1F`）。

/// `required` 中每个 VK 的 mask bit 是否全部在 `down_mask` 中置位。
/// `required` 为空 = 永不触发（返回 false）。
pub fn matches(down_mask: u32, required: &[u16]) -> bool {
    if required.is_empty() {
        return false;
    }
    let mut mask = 0u32;
    for &vk in required {
        mask |= 1u32 << (vk & 0x1F);
    }
    down_mask & mask == mask
}

#[cfg(test)]
mod tests {
    use super::*;

    // 按 GOAL 位规则：bit 索引 = VK & 0x1F。
    // Alt (VK 0x12) → bit 18；Ctrl (0x11) → bit 17；Shift (0x10) → bit 16。
    const ALT: u32 = 1u32 << 18;
    const CTRL: u32 = 1u32 << 17;
    const SHIFT: u32 = 1u32 << 16;

    #[test]
    fn mods_alt_alone_triggers() {
        assert!(matches(ALT, &[0x12]), "Alt alone must trigger [Alt]");
        assert!(!matches(0, &[0x12]), "no keys must not trigger");
        assert!(!matches(ALT, &[]), "empty required never triggers");
    }

    #[test]
    fn mods_combo_required_and_extra_keys_allowed() {
        // 配置 [Alt, Ctrl]：两者齐全才触发。
        assert!(matches(ALT | CTRL, &[0x12, 0x11]), "Alt+Ctrl must trigger [Alt, Ctrl]");
        assert!(!matches(ALT, &[0x12, 0x11]), "missing Ctrl must not trigger [Alt, Ctrl]");
        assert!(!matches(CTRL, &[0x12, 0x11]), "missing Alt must not trigger [Alt, Ctrl]");
        // 多余按键不阻挡（YAGNI）：配置 [Alt] 时按着 Shift 也照样触发。
        assert!(matches(ALT | SHIFT, &[0x12]), "extra keys must not block (YAGNI)");
    }
}
