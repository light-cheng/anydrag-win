//! 拖动状态机（项目大脑，纯逻辑，零 windows-sys）。
//!
//! `feed` 每事件产出一组 `Action`，由 engine 线程（T3.2）执行 FFI。
//! 规则逐条注释，单测逐条钉死（`engine_*` + GOAL 示例测试）。

use std::time::Instant;

/// 拖动目标（resolve 的产物）。
/// - `frame`：DWM 可见几何 L,T,R,B（物理像素，Per-Monitor-v2 下统一坐标系）。
/// - `is_maximized`：由 resolve 判定（MVP 简化：frame == 所属 monitor 工作区，见 winmod）。
///   双击时用它决定发 SC_MAXIMIZE 还是 SC_RESTORE。
/// - `strategy`：按应用的选择（fallback_apps 命中 → Fallback，否则 cfg.default_strategy；
///   native_excluded 命中 → resolve 直接返回 None，不进状态机）。
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub hwnd: isize,
    pub exe: String,
    pub frame: [i32; 4],
    pub is_maximized: bool,
    pub strategy: Strategy,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    Native,
    Fallback,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// 本事件无动作（占位：每个事件至少返回一条，执行器按"恒有输出"处理）。
    None,
    /// → `PostMessage(hwnd, WM_SYSCOMMAND, SC_DRAGMOVE=0xF012, 0)`，目标应用自己的泵
    ///   跑原生 caption 拖动循环（spike T2.3 验证过才默认启用）。
    BeginNativeDrag,
    /// → 先 `WM_SIZING(ALT)` 后 `SetWindowPos(hwnd, x, y, 保持大小)`；
    /// 同一事件多条时执行器只应用最后一条（合并，见 T3.2）。
    SetPos { x: i32, y: i32 },
    /// → 先 `WM_SIZING(uEdge)` 后 `SetWindowPos(hwnd, x, y, w, h, SWP_FRAMECHANGED)`。
    /// `edges` = RZ_* 位掩码（纯逻辑，执行器映射成 WMSZ_* 发给目标应用）。
    SetRect { x: i32, y: i32, w: i32, h: i32, edges: u8 },
    /// → `PostMessage(hwnd, WM_SYSCOMMAND, SC_MAXIMIZE=0xF030, 0)`。
    Maximize,
    /// → `PostMessage(hwnd, WM_SYSCOMMAND, SC_RESTORE=0xF120, 0)`。
    Restore,
    /// 手势结束：状态回 Idle，执行器清理。
    End,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineEvent {
    Down { x: i32, y: i32, mods: u32 },
    Move { x: i32, y: i32, mods: u32 },
    Up { x: i32, y: i32, mods: u32 },
}

/// 状态机持有的状态。
/// - `Armed`：已按下且修饰齐全，等"位移≥阈值 → 提交拖动"或"500ms 内第二次 Down 无位移 → 双击"。
/// - `Native`/`Fallback`：已提交的拖动进行中（Native 下窗口跟随由目标应用负责，
///   本状态机只等待 Up 收尾；Fallback 下每个 Move 产出一条 SetPos）。
/// `down_seq`：同目标连续 Down 计数（=1 时可升级为双击）。
/// `armed_at`/`start`：双击窗口与位移基准（Fallback 的 SetPos 也需要 `start`，故扩展进状态）。
pub enum DragState {
    Idle,
    Armed {
        target: Target,
        start: [i32; 2],
        down_seq: u8,
        strategy: Strategy,
        armed_at: Instant,
    },
    /// resize 手势已按下、未达位移阈值（触发键组合 = `cfg.resize_mask`，默认 Alt+Ctrl；
    /// 光标须落在目标框边带内才会进入本状态，Down 时刻按 `resize_edges_for` 判定）。
    ArmedResize {
        target: Target,
        start: [i32; 2],
    },
    Native {
        target: Target,
    },
    Fallback {
        target: Target,
        start: [i32; 2],
    },
    /// 已提交的 resize：每个 Move 一条 SetRect（相对 start 累计，与 Fallback 同范式）。
    Resize {
        target: Target,
        start: [i32; 2],
        edges: u8,
    },
}

/// 状态机每次 feed 的配置快照（engine 线程从共享 Config 现取现算，
/// 所以 `enabled=false` 的短路发生在调用方：不调 feed 即恒 None）。
/// `modifiers_mask` 为 u32（位规则 `VK & 0x1F` 产生 bit 16..30，u16 装不下，
/// 见 mods.rs 模块注记——对 GOAL 标注的有意偏差）。
#[derive(Clone, Copy, Debug)]
pub struct EngineCfg {
    /// 触发所需的修饰键 mask（bit = `VK & 0x1F`，见 mods.rs）。
    pub modifiers_mask: u32,
    /// resize 触发键 mask（位规则同 `modifiers_mask`）；0 = 关闭 resize 路径。
    pub resize_mask: u32,
    pub threshold_px: u32,
    /// 双击判定窗口（ms）；None 配置在调用方解析为系统 GetDoubleClickTime()。
    pub double_click_ms: u32,
    /// resize 边缘带（px）：Down 时刻光标到框边 ≤ 此值 ⇒ 提交为调整大小。
    /// 0 = 关闭（GOAL 原示例测试按 0 跑，行为逐字不变）；生产注入 10。
    pub edge_band_px: u32,
}

impl Default for DragState {
    fn default() -> Self {
        DragState::Idle
    }
}

/// 位移²（避免 sqrt；与阈值²比较即可）。
fn dist_sq(a: [i32; 2], b: [i32; 2]) -> i64 {
    let dx = (a[0] - b[0]) as i64;
    let dy = (a[1] - b[1]) as i64;
    dx * dx + dy * dy
}

// ===== 调整大小（Alt + 窗口边缘/角落拖动）=====

/// 边缘位掩码（纯逻辑，执行器映射为 WM_SIZING 的 WMSZ_*）：
/// 光标落在目标 DWM 帧哪条边 10px 带内就置哪位；角落 = 两位。
pub const RZ_TOP: u8 = 1;
pub const RZ_LEFT: u8 = 2;
pub const RZ_RIGHT: u8 = 4;
pub const RZ_BOTTOM: u8 = 8;

/// 调整大小的最小跟踪尺寸（px）：防窗口被拉到 0（简化：不按应用各自
/// 的 min size，MVP 记录于模块注记）。
const MIN_W: i32 = 100;
const MIN_H: i32 = 60;

/// Down 时刻光标到窗口帧边的距离 ≤ `edge_band_px` ⇒ 本手势提交为"调整
/// 大小"而非移动（`edge_band_px=0` 关闭本功能——GOAL 原示例测试按 0 跑，
/// 行为逐字不变；生产由 engine_loop 注入 10px）。
fn resize_edges_for(t: &Target, x: i32, y: i32, band_px: u32) -> u8 {
    if t.is_maximized || band_px == 0 {
        // 最大化窗 OS 语义上不可 resize（双击 Restore 才是出路）→ 走移动手势。
        return 0;
    }
    let [l, tp, r, b] = t.frame;
    // frame 坐标 i32，abs_diff 结果转 u32 与 band_px 比较（物理像素量级远小于 u32 上界）。
    let mut e = 0u8;
    if y.abs_diff(tp) as u32 <= band_px {
        e |= RZ_TOP;
    }
    if y.abs_diff(b) as u32 <= band_px {
        e |= RZ_BOTTOM;
    }
    if x.abs_diff(l) as u32 <= band_px {
        e |= RZ_LEFT;
    }
    if x.abs_diff(r) as u32 <= band_px {
        e |= RZ_RIGHT;
    }
    e
}

/// 由"起点帧 + 起点光标 + 当前光标"算新窗口矩形（相对 start 累计，
/// 与 Fallback 移动同范式）；`edges` 决定哪些边跟随光标。
fn resize_rect(t: &Target, start: [i32; 2], edges: u8, x: i32, y: i32) -> (i32, i32, i32, i32) {
    let [l, tp, r, b] = t.frame;
    let dx = x - start[0];
    let dy = y - start[1];
    let mut nl = l;
    let mut nt = tp;
    let mut nr = r;
    let mut nb = b;
    if edges & RZ_LEFT != 0 {
        nl = l + dx; // 左边界跟随光标（右固定）
    }
    if edges & RZ_RIGHT != 0 {
        nr = r + dx;
    }
    if edges & RZ_TOP != 0 {
        nt = tp + dy;
    }
    if edges & RZ_BOTTOM != 0 {
        nb = b + dy;
    }
    // 最小尺寸钳制（拖过头时把活动边拉回，不动固定边）
    if nr - nl < MIN_W {
        if edges & RZ_LEFT != 0 {
            nl = nr - MIN_W;
        }
        if edges & RZ_RIGHT != 0 {
            nr = nl + MIN_W;
        }
    }
    if nb - nt < MIN_H {
        if edges & RZ_TOP != 0 {
            nt = nb - MIN_H;
        }
        if edges & RZ_BOTTOM != 0 {
            nb = nt + MIN_H;
        }
    }
    (nl, nt, nr - nl, nb - nt)
}

/// 驱动状态机（`DragState` 的方法）。`resolve(x, y)` 由调用方（engine 线程）注入，
/// 负责命中测试/排除表/身份解析/策略选择（winmod::resolve_target）。
impl DragState {
    pub fn feed(
        &mut self,
        ev: &EngineEvent,
        resolve: &dyn Fn(i32, i32) -> Option<Target>,
        cfg: &EngineCfg,
    ) -> Vec<Action> {
        let mods_ok = |m: u32| m & cfg.modifiers_mask == cfg.modifiers_mask;

        match ev {
            EngineEvent::Down { x, y, mods } => {
                // resize 路径（触发键组合 = `cfg.resize_mask`，默认 Alt+Ctrl；mask=0 关闭）：
                // 组合键齐全 + 光标在目标框边带内 ⇒ 本手势提交为"调整大小"；
                // 不贴边则**落到常规路径**（Alt 部分恰好齐全 ⇒ 仍可拖）。
                // 单按 Alt（拖拽键）永远走常规路径，不触发 resize。
                if cfg.resize_mask != 0 && *mods & cfg.resize_mask == cfg.resize_mask {
                    if let Some(target) = resolve(*x, *y) {
                        let edges = resize_edges_for(&target, *x, *y, cfg.edge_band_px);
                        if edges != 0 {
                            *self = DragState::ArmedResize {
                                target,
                                start: [*x, *y],
                            };
                            return vec![Action::None];
                        }
                    } else {
                        *self = DragState::Idle;
                        return vec![Action::None];
                    }
                }
                // 规则 1：修饰不齐 → 忽略（不动状态）。
                if !mods_ok(*mods) {
                    return vec![Action::None];
                }
                // 规则 2：resolve 失败（无窗口/被排除/身份不可查）→ 回 Idle。
                let Some(target) = resolve(*x, *y) else {
                    *self = DragState::Idle;
                    return vec![Action::None];
                };
                // 规则 2/6：双击判定——同目标、窗口内、无位移的第二 Down。
                // （先算完判定再赋 *self：if-let 直接匹配 self 会让借用横跨赋值。）
                let is_double = match self {
                    DragState::Armed {
                        target: prev,
                        start,
                        down_seq,
                        armed_at,
                        ..
                    } => {
                        prev.hwnd == target.hwnd
                            && *down_seq == 1
                            && armed_at.elapsed()
                                <= std::time::Duration::from_millis(cfg.double_click_ms as u64)
                            && dist_sq([*x, *y], *start)
                                < (cfg.threshold_px as i64) * (cfg.threshold_px as i64)
                    }
                    _ => false,
                };
                if is_double {
                    // 双 Down 无位移 = 双击：最大化/还原（由 is_maximized 决定方向）。
                    let action = if target.is_maximized {
                        Action::Restore
                    } else {
                        Action::Maximize
                    };
                    *self = DragState::Idle;
                    return vec![action, Action::End];
                }
                // 新按下 / 双击条件不满足 → (重新) Armed。
                *self = DragState::Armed {
                    target: target.clone(),
                    start: [*x, *y],
                    down_seq: 1,
                    strategy: target.strategy,
                    armed_at: Instant::now(),
                };
                vec![Action::None]
            }

            EngineEvent::Move { x, y, mods } => {
                // 快照取值（match 臂内不赋 *self——借用横跨赋值会 E0506），
                // 决定 "产出的动作 + 下一状态"，match 结束后统一提交。
                let outcome: (Vec<Action>, Option<DragState>) = match self {
                    DragState::Armed {
                        target,
                        start,
                        strategy,
                        armed_at,
                        ..
                    } => {
                        let th = cfg.threshold_px as i64;
                        if !mods_ok(*mods) {
                            (vec![Action::None], None)
                        } else if armed_at.elapsed()
                            > std::time::Duration::from_millis(cfg.double_click_ms as u64)
                        {
                            // 规则 3 前置：超过双击窗口的 Armed = 陈旧单击，不再拉起拖动。
                            (vec![Action::None], Some(DragState::Idle))
                        } else if dist_sq([*x, *y], *start) < th * th {
                            // 未达阈值：仍是"等待双击"，无动作。
                            (vec![Action::None], None)
                        } else {
                            // 规则 3：位移 ≥ 阈值 → 按策略提交。
                            match strategy {
                                Strategy::Native => (
                                    // End 仅表示"armed 阶段结束"，进入 Native 态。
                                    vec![Action::BeginNativeDrag, Action::End],
                                    Some(DragState::Native {
                                        target: target.clone(),
                                    }),
                                ),
                                Strategy::Fallback => (
                                    // 规则 3/4：SetPos = 起点帧 + 位移。
                                    vec![Action::SetPos {
                                        x: target.frame[0] + (*x - start[0]),
                                        y: target.frame[1] + (*y - start[1]),
                                    }],
                                    Some(DragState::Fallback {
                                        target: target.clone(),
                                        start: *start,
                                    }),
                                ),
                            }
                        }
                    }
                    // 规则 4：Fallback 态每个 Move 出一条 SetPos（相对 start 累计；
                    // 同事件多条的合并由执行器做，状态机本身每事件出一条）。
                    DragState::Fallback { target, start } => (
                        vec![Action::SetPos {
                            x: target.frame[0] + (*x - start[0]),
                            y: target.frame[1] + (*y - start[1]),
                        }],
                        None,
                    ),
                    // resize 提交前：达阈值才提交（边掩码在提交时按 Down 位置重算，
                    // 与 Down 时刻判定必然一致）。
                    DragState::ArmedResize { target, start } => {
                        let th = cfg.threshold_px as i64;
                        if *mods & cfg.resize_mask != cfg.resize_mask {
                            // 修饰不齐（松开触发键）→ 本事件无动作、状态保留（与移动路径同语义）。
                            (vec![Action::None], None)
                        } else if dist_sq([*x, *y], *start) < th * th {
                            (vec![Action::None], None)
                        } else {
                            let edges = resize_edges_for(target, start[0], start[1], cfg.edge_band_px);
                            let (rx, ry, rw, rh) = resize_rect(target, *start, edges, *x, *y);
                            (
                                vec![
                                    Action::SetRect {
                                        x: rx,
                                        y: ry,
                                        w: rw,
                                        h: rh,
                                        edges,
                                    },
                                    Action::End,
                                ],
                                Some(DragState::Resize {
                                    target: target.clone(),
                                    start: *start,
                                    edges,
                                }),
                            )
                        }
                    }
                    // 已提交的 resize：每 Move 一条 SetRect（相对 start 累计）。
                    DragState::Resize { target, start, edges } => {
                        let (rx, ry, rw, rh) = resize_rect(target, *start, *edges, *x, *y);
                        (
                            vec![Action::SetRect {
                                x: rx,
                                y: ry,
                                w: rw,
                                h: rh,
                                edges: *edges,
                            }],
                            None,
                        )
                    }
                    // 规则 4 的 Native 侧：拖动循环在目标应用内部，状态机无动作。
                    _ => (vec![Action::None], None),
                };
                if let Some(next) = outcome.1 {
                    *self = next;
                }
                outcome.0
            }

            // 规则 5：任意 Up → [End]。
            // 已提交的拖动（Native/Fallback）与 Idle → 回 Idle。
            // 特殊：未提交的 Armed（一次无位移的"单击"）→ 保留 Armed——
            // 真实双击事件流是 Down/Up/Down/Up，若 Up 也回 Idle，第二次 Down
            // 到达时状态机已失忆，双击永远不可达（GOAL 规则 5 的字面写法会在
            // 生产手势下杀死双击；此处按"Up 结束本 click 相位、保留双击记忆"实现，
            // GOAL 示例测试（连续 Down/Down）与此语义兼容）。
            EngineEvent::Up { .. } => {
                let keep_armed =
                    matches!(self, DragState::Armed { .. } | DragState::ArmedResize { .. });
                if !keep_armed {
                    *self = DragState::Idle;
                }
                vec![Action::End]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Alt (VK 0x12) 的 mask bit（bit 索引 = VK & 0x1F = 18；u32 见模块注记）。
    pub const MOD_ALT: u32 = 1u32 << 18;
    const CTRL_BIT: u32 = 1u32 << 17;

    impl EngineCfg {
        /// 测试固定配置：Alt 触发、阈值 5px、双击窗 500ms、resize 路径关闭
        /// （`resize_mask=0` ⇒ GOAL 原示例测试逐字不变；resize 专项测试用
        /// `cfg_resize()` 打开 Ctrl 路径）。
        pub fn test() -> Self {
            Self {
                modifiers_mask: MOD_ALT,
                threshold_px: 5,
                double_click_ms: 500,
                resize_mask: 0,
                edge_band_px: 10,
            }
        }
    }

    impl Target {
        /// 测试目标：notepad，frame L=100,T=100,R=900,B=700。
        pub fn test_notepad(is_maximized: bool) -> Self {
            Self {
                hwnd: 0x1234,
                exe: "notepad.exe".into(),
                frame: [100, 100, 900, 700],
                is_maximized,
                strategy: Strategy::Native,
            }
        }
    }

    fn down(x: i32, y: i32, mods: u32) -> EngineEvent {
        EngineEvent::Down { x, y, mods }
    }
    fn move_(x: i32, y: i32, mods: u32) -> EngineEvent {
        EngineEvent::Move { x, y, mods }
    }
    fn up(x: i32, y: i32, mods: u32) -> EngineEvent {
        EngineEvent::Up { x, y, mods }
    }
    fn resolve_ok(maximized: bool) -> impl Fn(i32, i32) -> Option<Target> {
        move |_x, _y| Some(Target::test_notepad(maximized))
    }
    fn resolve_fallback() -> impl Fn(i32, i32) -> Option<Target> {
        move |_x, _y| {
            Some(Target {
                strategy: Strategy::Fallback,
                ..Target::test_notepad(false)
            })
        }
    }
    fn resolve_none() -> impl Fn(i32, i32) -> Option<Target> {
        move |_x, _y| None
    }

    /// GOAL 示例（逐字保留）：双 Down 无位移 → 最大化。
    #[test]
    fn double_down_no_motion_maximizes() {
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        let resolve = |_x, _y| Some(Target::test_notepad(false));
        let a1 = s.feed(&down(100, 200, MOD_ALT), &resolve, &cfg);
        assert_eq!(a1, vec![Action::None]);
        let a2 = s.feed(&down(100, 200, MOD_ALT), &resolve, &cfg);
        assert_eq!(a2, vec![Action::Maximize, Action::End]);
    }

    #[test]
    fn engine_down_without_mods_is_ignored() {
        // 规则 1：修饰不齐 → [None]，状态不动（仍是 Idle）。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        let a = s.feed(&down(10, 10, 0), &resolve_ok(false), &cfg);
        assert_eq!(a, vec![Action::None]);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn engine_down_resolves_and_arms() {
        // 规则 2：resolve 成功 → Armed。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        let a = s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        assert_eq!(a, vec![Action::None]);
        assert!(
            matches!(&s, DragState::Armed { target, .. } if target.hwnd == 0x1234),
            "must be Armed with the resolved target"
        );
    }

    #[test]
    fn engine_down_resolve_fail_resets_to_idle() {
        // 规则 2：resolve 失败（排除表/无窗口）→ 回 Idle。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        let a = s.feed(&down(100, 200, MOD_ALT), &resolve_none(), &cfg);
        assert_eq!(a, vec![Action::None]);
        assert!(matches!(s, DragState::Idle));
        // 先 Armed 再 resolve 失败也回 Idle。
        let mut s2 = DragState::Idle;
        s2.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        s2.feed(&down(300, 300, MOD_ALT), &resolve_none(), &cfg);
        assert!(matches!(s2, DragState::Idle));
    }

    #[test]
    fn engine_double_down_maximized_restores() {
        // 规则 2：目标已最大化 → 双击发 Restore。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(true), &cfg);
        let a = s.feed(&down(100, 200, MOD_ALT), &resolve_ok(true), &cfg);
        assert_eq!(a, vec![Action::Restore, Action::End]);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn engine_second_down_outside_window_rearms() {
        // 规则 6：双击窗（注入 0ms → 任何间隔都超窗）外第二次 Down = 重新 Armed，不是双击。
        let mut s = DragState::Idle;
        let mut cfg = EngineCfg::test();
        cfg.double_click_ms = 0;
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        let a = s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        assert_eq!(a, vec![Action::None], "outside the window this is a fresh arm, not a double");
        assert!(matches!(s, DragState::Armed { .. }));
    }

    #[test]
    fn engine_move_below_threshold_stays_armed() {
        // 规则 3（否）：位移 < 阈值 → 仍 Armed（双击候选还活着）。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        let a = s.feed(&move_(103, 200, MOD_ALT), &resolve_ok(false), &cfg);
        assert_eq!(a, vec![Action::None]);
        assert!(matches!(s, DragState::Armed { .. }));
    }

    #[test]
    fn engine_move_commits_native() {
        // 规则 3：位移 ≥ 阈值 + Native 策略 → [BeginNativeDrag, End]，进 Native 态。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        let a = s.feed(&move_(106, 200, MOD_ALT), &resolve_ok(false), &cfg);
        assert_eq!(a, vec![Action::BeginNativeDrag, Action::End]);
        assert!(matches!(s, DragState::Native { .. }));
    }

    #[test]
    fn engine_move_commits_fallback_with_setpos() {
        // 规则 3：位移 ≥ 阈值 + Fallback 策略 → 一条 SetPos（frame + 位移），进 Fallback 态。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        s.feed(&down(100, 200, MOD_ALT), &resolve_fallback(), &cfg);
        let a = s.feed(&move_(106, 203, MOD_ALT), &resolve_fallback(), &cfg);
        // frame L,T = 100,100；位移 +6,+3 → 106,103。
        assert_eq!(a, vec![Action::SetPos { x: 106, y: 103 }]);
        assert!(matches!(s, DragState::Fallback { .. }));
    }

    #[test]
    fn engine_fallback_moves_accumulate_from_start() {
        // 规则 4：Fallback 态每个 Move 一条 SetPos，坐标 = frame + (ev - start)（相对起点累计，
        // 非增量）；执行器对同事件多条只应用最后一条。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        s.feed(&down(100, 200, MOD_ALT), &resolve_fallback(), &cfg);
        s.feed(&move_(106, 200, MOD_ALT), &resolve_fallback(), &cfg); // 提交
        let a1 = s.feed(&move_(140, 230, MOD_ALT), &resolve_fallback(), &cfg);
        assert_eq!(a1, vec![Action::SetPos { x: 100 + 40, y: 100 + 30 }]);
        // 后续 Move 仍相对 start(100,200)：+60,+45。
        let a2 = s.feed(&move_(160, 245, MOD_ALT), &resolve_fallback(), &cfg);
        assert_eq!(a2, vec![Action::SetPos { x: 100 + 60, y: 100 + 45 }]);
    }

    #[test]
    fn engine_up_ends_and_resets() {
        // 规则 5：任意 Up → [End]。Native/Fallback/Idle → 回 Idle；
        // 未提交的 Armed（无位移单击）→ 保留 Armed 作双击记忆（feed 注释）。
        let cfg = EngineCfg::test();

        let mut s = DragState::Idle;
        assert_eq!(s.feed(&up(7, 7, MOD_ALT), &resolve_ok(false), &cfg), vec![Action::End]);
        assert!(matches!(s, DragState::Idle));

        let mut s = DragState::Idle;
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        assert_eq!(s.feed(&up(100, 200, MOD_ALT), &resolve_ok(false), &cfg), vec![Action::End]);
        assert!(
            matches!(s, DragState::Armed { .. }),
            "uncommitted Up keeps the double-click memory"
        );

        let mut s = DragState::Idle;
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        s.feed(&move_(106, 200, MOD_ALT), &resolve_ok(false), &cfg); // → Native
        assert_eq!(s.feed(&up(110, 200, MOD_ALT), &resolve_ok(false), &cfg), vec![Action::End]);
        assert!(matches!(s, DragState::Idle));

        let mut s = DragState::Idle;
        s.feed(&down(100, 200, MOD_ALT), &resolve_fallback(), &cfg);
        s.feed(&move_(106, 200, MOD_ALT), &resolve_fallback(), &cfg); // → Fallback
        s.feed(&move_(120, 200, MOD_ALT), &resolve_fallback(), &cfg);
        assert_eq!(s.feed(&up(120, 200, MOD_ALT), &resolve_fallback(), &cfg), vec![Action::End]);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn engine_double_click_down_up_down_maximizes() {
        // 生产手势 Down/Up/Down：Up 保留双击记忆，第二次 Down 判定双击。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        s.feed(&up(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        let a = s.feed(&down(101, 201, MOD_ALT), &resolve_ok(false), &cfg); // 1px 位移 < 阈值 5
        assert_eq!(a, vec![Action::Maximize, Action::End]);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn engine_extra_modifiers_do_not_block() {
        // 规则 1 的反面：配置的键齐全即可，多按的键不阻挡（YAGNI，见 mods.rs）。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        let a = s.feed(&down(100, 200, MOD_ALT | CTRL_BIT), &resolve_ok(false), &cfg);
        assert_eq!(a, vec![Action::None]);
        assert!(matches!(s, DragState::Armed { .. }));
    }

    #[test]
    fn engine_moves_while_idle_or_native_are_noops() {
        // 规则 1/4：Idle 态的 Move 无事可做；Native 态的 Move 交给目标应用的拖动循环。
        let mut s = DragState::Idle;
        let cfg = EngineCfg::test();
        assert_eq!(s.feed(&move_(5, 5, MOD_ALT), &resolve_ok(false), &cfg), vec![Action::None]);
        s.feed(&down(100, 200, MOD_ALT), &resolve_ok(false), &cfg);
        s.feed(&move_(106, 200, MOD_ALT), &resolve_ok(false), &cfg); // → Native
        assert_eq!(s.feed(&move_(110, 200, MOD_ALT), &resolve_ok(false), &cfg), vec![Action::None]);
        assert!(matches!(s, DragState::Native { .. }));
    }

    // ===== resize（独立触发键 Ctrl = bit17；边带 10px）=====

    fn target_rz() -> Target {
        // frame [100,100,500,400]（400×300），非最大化，Fallback。
        Target {
            hwnd: 0x77,
            exe: "notepad.exe".into(),
            frame: [100, 100, 500, 400],
            is_maximized: false,
            strategy: Strategy::Fallback,
        }
    }
    fn resolve_rz() -> impl Fn(i32, i32) -> Option<Target> {
        move |_, _| Some(target_rz())
    }
    fn cfg_resize() -> EngineCfg {
        let mut c = EngineCfg::test();
        c.resize_mask = CTRL_BIT;
        c.edge_band_px = 10;
        c
    }

    #[test]
    fn resize_right_edge_grows() {
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        assert_eq!(
            s.feed(&down(496, 250, CTRL_BIT), &resolve_rz(), &cfg),
            vec![Action::None]
        );
        assert!(matches!(s, DragState::ArmedResize { .. }));
        let a = s.feed(&move_(516, 250, CTRL_BIT), &resolve_rz(), &cfg); // dx=+20 ≥ 5
        assert_eq!(
            a,
            vec![
                Action::SetRect {
                    x: 100,
                    y: 100,
                    w: 420,
                    h: 300,
                    edges: RZ_RIGHT
                },
                Action::End
            ]
        );
        assert!(matches!(s, DragState::Resize { .. }));
        // 后续 Move 仍相对 start(496,250) 累计：dx=+40。
        let a2 = s.feed(&move_(536, 250, CTRL_BIT), &resolve_rz(), &cfg);
        assert_eq!(
            a2,
            vec![Action::SetRect {
                x: 100,
                y: 100,
                w: 440,
                h: 300,
                edges: RZ_RIGHT
            }]
        );
    }

    #[test]
    fn resize_left_edge_min_clamp() {
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        s.feed(&down(104, 250, CTRL_BIT), &resolve_rz(), &cfg); // 左边（|104-100|=4 ≤ 10）
        // 拖到宽度将低于 MIN_W(100) → 活动边（左）钳回，右固定。
        let a = s.feed(&move_(414, 250, CTRL_BIT), &resolve_rz(), &cfg); // dx=+310 → nl=410, w=90
        assert_eq!(
            a,
            vec![
                Action::SetRect {
                    x: 400,
                    y: 100,
                    w: 100,
                    h: 300,
                    edges: RZ_LEFT
                },
                Action::End
            ]
        );
    }

    #[test]
    fn resize_corner_bottomright() {
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        s.feed(&down(496, 396, CTRL_BIT), &resolve_rz(), &cfg);
        let a = s.feed(&move_(526, 416, CTRL_BIT), &resolve_rz(), &cfg); // dx=+30, dy=+20
        assert_eq!(
            a,
            vec![
                Action::SetRect {
                    x: 100,
                    y: 100,
                    w: 430,
                    h: 320,
                    edges: RZ_RIGHT | RZ_BOTTOM
                },
                Action::End
            ]
        );
    }

    #[test]
    fn resize_client_click_is_noop() {
        // Ctrl + 客户端区（不贴边）→ 无事发生（规则：resize 只在框边带内生效）。
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        assert_eq!(s.feed(&down(300, 250, CTRL_BIT), &resolve_rz(), &cfg), vec![Action::None]);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn resize_both_mods_edge_wins() {
        // 两键同按（Alt+Ctrl）且贴边 → resize 优先于拖动（更特定的意图）。
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        s.feed(&down(496, 250, MOD_ALT | CTRL_BIT), &resolve_rz(), &cfg);
        assert!(matches!(s, DragState::ArmedResize { .. }));
    }

    #[test]
    fn resize_maximized_window_noop() {
        // 最大化窗：边带无效（OS 语义不可 resize）→ Ctrl+贴边点击 = 无动作。
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        let resolve = |_: i32, _: i32| Some(Target {
            is_maximized: true,
            ..target_rz()
        });
        s.feed(&down(496, 250, CTRL_BIT), &resolve, &cfg);
        assert!(matches!(s, DragState::Idle));
    }

    #[test]
    fn alt_double_click_at_edge_still_maximizes() {
        // 回归守卫：Alt 路径不受 resize 影响——贴边 Alt 双击仍发 Maximize
        // （resize 是独立触发键，不抢 Alt 手势）。
        let mut s = DragState::Idle;
        let cfg = cfg_resize();
        s.feed(&down(496, 250, MOD_ALT), &resolve_rz(), &cfg);
        let a = s.feed(&down(496, 250, MOD_ALT), &resolve_rz(), &cfg);
        assert_eq!(a, vec![Action::Maximize, Action::End]);
    }
}
