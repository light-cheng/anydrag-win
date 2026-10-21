//! 几何下发的节流 + 背压决策（纯逻辑，零 windows-sys）。
//!
//! 背景（`docs/acceptance.md` "缩放延迟"一节）：几何更新不能按鼠标事件率下发，
//! 否则目标应用的消息队列越堆越多、窗口永远追不上光标。两道闸门：
//! 1. **节流**：两次下发间隔 >= 显示器刷新周期（`frame_ms`）；
//! 2. **背压**：上一次请求还没被目标窗口落地（`GetWindowRect` 仍不等于请求值）
//!    就先不发——等它消化完。
//!
//! **背压必须是"软"的（2026-10-21 事故根因）**：旧实现把背压做成了硬闩锁——
//! 上一次请求的窗口矩形 `req` 与窗口实际矩形一旦不等，`caught_up` 就永远为假，
//! 而 `req` 只在**成功下发**时才更新 ⇒ **一次不匹配就永久锁死**，此后所有拖拽/
//! 缩放都被推迟到"目标追上"这个永不到来的时刻；`MAX_RETRY_ROUNDS` 用尽后只是
//! 丢弃本帧、闩锁依旧 ⇒ 拖拽与缩放全灭，而最大化/分屏走 `SetWindowPlacement`
//! 完全不经过这里，所以"只有最大化还好使"。
//!
//! 触发一次不匹配非常容易：应用按自己的最小尺寸钳制（`WM_GETMINMAXINFO`）、
//! DPI 取整、尺寸调整相位里的临时几何、以及**切换目标窗口**（旧 `req` 属于另一个
//! 窗口，却拿新窗口的矩形去比）。
//!
//! 本模块的三条自愈规则（每一条都对应一个回归测试）：
//! - **目标窗口变了**（hwnd 不同）⇒ 旧请求与本次无关，不拦；
//! - **手势开始/结束** ⇒ `reset()` 清空闩锁（陈旧请求绝不允许跨手势存活）；
//! - **等太久**（`FORCE_AFTER_MS`）⇒ **强制下发**。宁可多灌一帧，
//!   也不能让一个陈旧的背压判断把整个功能永久关掉。

use std::time::Instant;

/// 一次待落地的几何更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geom {
    Pos { x: i32, y: i32 },
    Rect { x: i32, y: i32, w: i32, h: i32 },
}

/// 单次决策结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// 立即下发（调用方执行 `apply_geom` 并 `commit`）。
    Apply,
    /// 推迟（调用方存 pending、武装定时器；中间态直接丢弃）。
    Defer,
}

/// 背压等待上限（ms）：超过它就算目标还没追上也要强制下发。
///
/// 取 40ms ≈ 120Hz 下的 5 帧。正常情况目标在 1-2 帧内就追上，这个上限**不会被
/// 触发**；它只在"目标永远追不上"（被钳制 / 卡死 / 已最小化）时兜底，把死锁
/// 退化成"每 40ms 下发一次最新几何"——这正是没有背压时的行为，可以接受。
pub const FORCE_AFTER_MS: u64 = 40;

/// 节流 + 背压状态机（engine 线程独占，无锁）。
pub struct Throttle {
    /// 显示器刷新周期（ms）。
    frame_ms: u64,
    /// 上一次成功下发的时刻（节流闸门）。
    last_apply: Option<Instant>,
    /// 上一次成功下发的 `(hwnd, 请求的窗口矩形)`（背压闸门）。
    req: Option<(isize, [i32; 4])>,
    /// 当前 pending 第一次被推迟的时刻（强制放行兜底）。
    deferred_since: Option<Instant>,
}

impl Throttle {
    pub fn new(frame_ms: u64) -> Self {
        Self {
            frame_ms,
            last_apply: None,
            req: None,
            deferred_since: None,
        }
    }

    /// 手势开始/结束：清空全部闩锁。
    /// **陈旧请求绝不允许跨手势存活**——这是"快速缩放几次后全灭"的直接解药。
    pub fn reset(&mut self) {
        self.last_apply = None;
        self.req = None;
        self.deferred_since = None;
    }

    /// 是否有"针对该窗口、尚未落地"的请求。
    /// 调用方据此决定是否值得花一次 `GetWindowRect`（没有就跳过，热路径零系统调用）。
    pub fn outstanding(&self, hwnd: Option<isize>) -> bool {
        matches!((self.req, hwnd), (Some((rh, _)), Some(h)) if rh == h)
    }

    /// 决策。`current` = 目标窗口**当前**矩形（仅当 `outstanding(hwnd)` 为真时才需要
    /// 查询，其余情况传 `None`）。`force` = 本帧必须落地（鼠标抬起，手势收尾不能留
    /// 中间态）。
    pub fn decide(
        &self,
        now: Instant,
        hwnd: Option<isize>,
        current: Option<[i32; 4]>,
        force: bool,
    ) -> Decision {
        if force {
            return Decision::Apply;
        }
        let due = match self.last_apply {
            None => true,
            Some(t) => elapsed_ms(now, t) >= self.frame_ms,
        };
        if !due {
            return Decision::Defer;
        }
        if self.caught_up(hwnd, current) {
            return Decision::Apply;
        }
        // 目标还没追上：等太久就强制放行（软背压，见模块注释）。
        match self.deferred_since {
            Some(t) if elapsed_ms(now, t) >= FORCE_AFTER_MS => Decision::Apply,
            _ => Decision::Defer,
        }
    }

    /// 目标是否已经把上一次请求落地了。
    /// 不等 = 应用还在消化上一帧（重排/重绘没做完），此时再灌只会让队列越堆越长。
    fn caught_up(&self, hwnd: Option<isize>, current: Option<[i32; 4]>) -> bool {
        match (self.req, hwnd) {
            // 同一窗口：当前矩形等于上次请求 = 已追上；查不到（窗口已销毁等）= 不拦。
            (Some((rh, r)), Some(h)) if rh == h => current.map(|c| c == r).unwrap_or(true),
            // 没有历史请求 / 目标窗口换了 → 与本帧无关，不拦。
            _ => true,
        }
    }

    /// 记录"本帧被推迟了"（只在第一次推迟时打点，用于强制放行计时）。
    pub fn note_deferred(&mut self, now: Instant) {
        if self.deferred_since.is_none() {
            self.deferred_since = Some(now);
        }
    }

    /// 记录一次成功下发：更新节流时刻 + 背压基准，并清掉推迟打点。
    /// `req` = 本次请求的窗口矩形（`apply_geom` 的返回值；拿不到就传 `None`）。
    pub fn commit(&mut self, now: Instant, hwnd: Option<isize>, req: Option<[i32; 4]>) {
        self.last_apply = Some(now);
        self.req = match (hwnd, req) {
            (Some(h), Some(r)) => Some((h, r)),
            _ => None,
        };
        self.deferred_since = None;
    }
}

/// 单调时钟差（ms）。`Instant` 保证 `now >= t`，这里仍用 saturating 兜底。
fn elapsed_ms(now: Instant, t: Instant) -> u64 {
    now.saturating_duration_since(t).as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const FRAME: u64 = 8;
    const HWND_A: isize = 0xA;
    const HWND_B: isize = 0xB;
    /// 一次请求的窗口矩形。
    const REQ: [i32; 4] = [100, 100, 500, 400];

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    /// 造一个"刚下发过 REQ"的状态。
    fn after_apply(t0: Instant) -> Throttle {
        let mut th = Throttle::new(FRAME);
        th.commit(t0, Some(HWND_A), Some(REQ));
        th
    }

    #[test]
    fn first_apply_is_immediate() {
        let t0 = Instant::now();
        let th = Throttle::new(FRAME);
        assert_eq!(
            th.decide(t0, Some(HWND_A), None, false),
            Decision::Apply,
            "no history = nothing to wait for"
        );
    }

    #[test]
    fn throttle_defers_within_one_frame() {
        let t0 = Instant::now();
        let th = after_apply(t0);
        // 同一帧内、目标已追上 → 仍然要等刷新周期（节流闸门）。
        assert_eq!(th.decide(at(t0, 2), Some(HWND_A), Some(REQ), false), Decision::Defer);
        // 过了一帧 → 放行。
        assert_eq!(th.decide(at(t0, 9), Some(HWND_A), Some(REQ), false), Decision::Apply);
    }

    #[test]
    fn backpressure_defers_until_target_catches_up() {
        let t0 = Instant::now();
        let th = after_apply(t0);
        // 目标还在 [.., 420]（没落地 400）→ 推迟。
        assert_eq!(
            th.decide(at(t0, 10), Some(HWND_A), Some([100, 100, 500, 420]), false),
            Decision::Defer
        );
        // 目标追上了 → 放行。
        assert_eq!(th.decide(at(t0, 12), Some(HWND_A), Some(REQ), false), Decision::Apply);
    }

    /// 回归：2026-10-21 "快速缩放几次后拖拽/缩放全灭"。
    /// 目标永远追不上（应用按最小尺寸钳制）时，旧实现会永久锁死；
    /// 现在必须在 `FORCE_AFTER_MS` 后强制下发。
    #[test]
    fn never_catching_up_is_forced_after_deadline() {
        let t0 = Instant::now();
        let mut th = after_apply(t0);
        let never = Some([100, 100, 500, 999]);
        assert_eq!(th.decide(at(t0, 10), Some(HWND_A), never, false), Decision::Defer);
        th.note_deferred(at(t0, 10));
        // 还没到上限：继续等。
        assert_eq!(
            th.decide(at(t0, 10 + FORCE_AFTER_MS - 1), Some(HWND_A), never, false),
            Decision::Defer
        );
        // 到上限：强制放行（绝不无限等）。
        assert_eq!(
            th.decide(at(t0, 10 + FORCE_AFTER_MS), Some(HWND_A), never, false),
            Decision::Apply,
            "stale backpressure must never dead-lock the geometry path"
        );
    }

    /// 回归：旧 `req` 属于窗口 A，新手势拖的是窗口 B —— 不许拦。
    #[test]
    fn stale_request_for_other_window_does_not_block() {
        let t0 = Instant::now();
        let th = after_apply(t0);
        assert!(!th.outstanding(Some(HWND_B)), "a request for A is not outstanding for B");
        assert_eq!(
            th.decide(at(t0, 10), Some(HWND_B), Some([0, 0, 300, 300]), false),
            Decision::Apply,
            "another window's stale request must not gate this one"
        );
    }

    /// 回归：手势边界 `reset()` 必须清空闩锁（陈旧请求不跨手势存活）。
    #[test]
    fn reset_clears_every_latch() {
        let t0 = Instant::now();
        let mut th = after_apply(t0);
        th.note_deferred(t0);
        th.reset();
        assert!(!th.outstanding(Some(HWND_A)));
        assert_eq!(
            th.decide(t0, Some(HWND_A), Some([0, 0, 1, 1]), false),
            Decision::Apply,
            "after reset the first event of a new gesture applies immediately"
        );
    }

    #[test]
    fn force_overrides_every_gate() {
        let t0 = Instant::now();
        let th = after_apply(t0);
        // 既没过节流周期、目标也没追上，但鼠标抬起必须落地。
        assert_eq!(
            th.decide(at(t0, 1), Some(HWND_A), Some([0, 0, 1, 1]), true),
            Decision::Apply
        );
    }

    #[test]
    fn commit_clears_deferral_timer() {
        let t0 = Instant::now();
        let mut th = after_apply(t0);
        th.note_deferred(t0);
        th.commit(at(t0, 10), Some(HWND_A), Some(REQ));
        // 重新开始计时：不该被上一次的推迟打点直接顶到强制放行。
        assert_eq!(
            th.decide(at(t0, 11), Some(HWND_A), Some([0, 0, 1, 1]), false),
            Decision::Defer
        );
    }

    #[test]
    fn missing_hwnd_never_blocks() {
        let t0 = Instant::now();
        let th = after_apply(t0);
        assert!(!th.outstanding(None));
        assert_eq!(th.decide(at(t0, 10), None, None, false), Decision::Apply);
    }
}
