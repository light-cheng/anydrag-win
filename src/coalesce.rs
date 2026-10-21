//! 事件合并（纯逻辑，零 windows-sys）。
//!
//! **为什么需要它（2026-10-21 "一直高频缩放才卡死"的根因）**：
//! `WH_MOUSE_LL` 回调按鼠标硬件率推事件（实测注入 4183 次/秒时全部入环），
//! 而 engine 线程每批要花真实时间在 `SetWindowPos` + `sync_paint`
//! （`SendMessageTimeoutW(WM_SYNCPAINT, …, 120ms)`）上。消费跟不上生产，
//! 64 槽环溢出 → 心跳仪实测 `dropped=7172/12000`（**60% 事件被丢弃**）。
//!
//! 丢 Move 本身无害（几何是**绝对值**，下一个 Move 直接覆盖），但丢 `Down`/`Up`
//! 是灾难：`Up` 丢了状态机永远停在 `Fallback`/`Resize` 态 →
//! `Action::EndSizeMove` 永不发出 → 目标应用**永久卡在尺寸调整相位**
//! （正是 MEMORY 坑 #1 描述的死法），且此后所有手势都受影响。
//!
//! 对策分两层：
//! 1. **生产者侧扩容**（`hook::RING_SLOTS` 64 → 512）：吸收突发，让 `Up` 活下来；
//! 2. **消费者侧合并**（本模块）：一批 drain 出来的事件里，连续 `Move` 只保留
//!    **最后一个**——状态机对 Move 的处理是纯函数（`frame + (cur - start)`），
//!    跳过中间态不改变结果，却能把每批的工作量从 O(事件数) 降到 O(1)。
//!
//! 合并规则（顺序即语义，不可交换）：
//! - `Move` 后紧跟 `Move` → 前者可丢（后者覆盖它）；
//! - 任何 `Down`/`Up`/`MiddleDown`/`MiddleUp` 都是**相位边界**，两侧的 Move 不可跨越；
//! - 合并只删「被后续 Move 完全覆盖」的事件，绝不重排、绝不丢非 Move 事件。

use crate::engine::EngineEvent;

/// 原地压缩事件序列：连续 `Move` 只保留最后一个。
///
/// 返回压缩后的长度（结果写回 `evs` 的前 `n` 项）。**零分配**——engine 线程
/// 每批调用一次，热路径上不该再要一块 Vec。
pub fn coalesce_moves(evs: &mut [EngineEvent]) -> usize {
    if evs.len() < 2 {
        return evs.len();
    }
    let mut w = 0usize; // 写指针
    for r in 0..evs.len() {
        let cur = evs[r];
        // 当前是 Move，且写指针的前一项也是 Move → 覆盖它（后者覆盖前者）。
        // 注意：只有**相邻** Move 才能合并；被 Down/Up 隔开的两段必须各留一个。
        if w > 0 && is_move(&cur) && is_move(&evs[w - 1]) {
            evs[w - 1] = cur;
        } else {
            evs[w] = cur;
            w += 1;
        }
    }
    w
}

fn is_move(e: &EngineEvent) -> bool {
    matches!(e, EngineEvent::Move { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(x: i32) -> EngineEvent {
        EngineEvent::Move { x, y: 0, mods: 1 }
    }
    fn dn(x: i32) -> EngineEvent {
        EngineEvent::Down { x, y: 0, mods: 1 }
    }
    fn up(x: i32) -> EngineEvent {
        EngineEvent::Up { x, y: 0, mods: 1 }
    }

    #[test]
    fn consecutive_moves_collapse_to_last() {
        let mut v = vec![mv(1), mv(2), mv(3), mv(4)];
        let n = coalesce_moves(&mut v);
        assert_eq!(n, 1);
        assert_eq!(v[0], mv(4), "only the newest move matters (absolute geometry)");
    }

    #[test]
    fn down_and_up_are_never_dropped() {
        // 关键回归：Down/Up 是相位边界，丢一个就会让应用永久卡在尺寸调整相位。
        let mut v = vec![dn(0), mv(1), mv(2), mv(3), up(4)];
        let n = coalesce_moves(&mut v);
        assert_eq!(n, 3);
        assert_eq!(v[0], dn(0));
        assert_eq!(v[1], mv(3), "moves between Down and Up collapse to the last");
        assert_eq!(v[2], up(4));
    }

    #[test]
    fn moves_do_not_cross_phase_boundaries() {
        // Down 两侧的 Move 不可合并（否则会把 Down 之前的几何带过相位边界）。
        let mut v = vec![mv(1), mv(2), dn(3), mv(4), mv(5)];
        let n = coalesce_moves(&mut v);
        assert_eq!(n, 3);
        assert_eq!(v[0], mv(2));
        assert_eq!(v[1], dn(3));
        assert_eq!(v[2], mv(5));
    }

    #[test]
    fn middle_buttons_are_boundaries_too() {
        let mut v = vec![
            EngineEvent::MiddleDown { x: 1, y: 1 },
            mv(2),
            mv(3),
            EngineEvent::MiddleUp { x: 4, y: 4 },
        ];
        let n = coalesce_moves(&mut v);
        assert_eq!(n, 3);
        assert!(matches!(v[0], EngineEvent::MiddleDown { .. }));
        assert_eq!(v[1], mv(3));
        assert!(matches!(v[2], EngineEvent::MiddleUp { .. }));
    }

    #[test]
    fn alternating_events_are_untouched() {
        let mut v = vec![dn(1), up(2), dn(3), up(4)];
        let n = coalesce_moves(&mut v);
        assert_eq!(n, 4, "no moves = nothing to merge");
        assert_eq!(v, vec![dn(1), up(2), dn(3), up(4)]);
    }

    #[test]
    fn empty_and_single_are_noops() {
        let mut v: Vec<EngineEvent> = vec![];
        assert_eq!(coalesce_moves(&mut v), 0);
        let mut v1 = vec![mv(9)];
        assert_eq!(coalesce_moves(&mut v1), 1);
        assert_eq!(v1[0], mv(9));
    }
}
