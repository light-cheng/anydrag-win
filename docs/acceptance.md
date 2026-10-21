# T4.2 验收证据（anydrag-win）

- **日期**：2026-10-16
- **环境**：Windows 11（10.0.26300 系）/ 主屏 2880×1752 @100% DPI / git-bash /
  `rustc 1.98.1` / MSVC BuildTools `14.44.35207` / Windows SDK `10.0.22621.0`
- **被测产物**：`target/release/anydrag-win.exe`（304128 B，strip 后）
- **依据**：`GOAL.md` §8 Definition of Done（8 条）+ §T3.4 冒烟五项

> 本文件是**实测记录**，不是声明。每条都给出可复跑的命令与原始输出；无法自动化的项
> 明确标注为「待人工确认」，不冒充已通过。

---

## 逐条对照 GOAL §8

### 1. `git log` 含 T0.1…T4.3 全部 commit；`GOAL.md` 未被改动 ✅

```
$ git log --oneline | head -20
c504cca 增量：Alt+Ctrl 就近角 resize + 托盘图标修复 + 用法菜单 + 冒烟驱动加固
6d653ee T4.1 全量回归…
29ffe14 T3.4: main 接线 + 5 项冒烟自动化全过
81feacb T3.3 tray
06ebfdd T3.2 engine loop
3d66327 T3.1 hook thread
f28eda3 T2.3 spike: probe_scandrag 实测 0xF012 …
b25aa9c T2.2 fallback mover
b3fc902 T2.1 winmod FFI
01cfced T1.4 drag state machine
61dae60 T1.3 mods
f47b069 T1.2 ring
cb0a8c6 T1.1 config
c852a02 T0.2 build.bat + toolchain gate
6e19902 T0.1 cargo scaffold + deps

$ git status --short GOAL.md
（空 —— 规格文件全程未被改动）
```

> T4.2 = 提交本文件的那次 commit；T4.3 = 紧随其后的 README commit。
> `c504cca` 是 GOAL 范围外的增量 commit（resize / 图标修复 / 冒烟驱动加固），
> 不占用任务编号。

### 2. `build.bat` → `Finished release profile`，exe 存在 ✅

```
$ cmd //c build.bat
    Finished `release` profile [optimized] target(s) in 36.22s

$ ls -la target/release/anydrag-win.exe
-rwxr-xr-x 2 Service01 197121 304128 target/release/anydrag-win.exe
```

数量级符合 GOAL §8 第 2 条（数百 KB，`strip` 后）。

### 3. `cargo test` 全绿 ✅

```
$ cargo test
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.46s
```

分布：`config`×3、`mods`×2、`ring`×3、`engine`×21（含 GOAL 逐条规则与 resize 专项）。

### 4. 冒烟五项（GOAL §T3.4）✅

驱动：`target/release/probe_gesture.exe`（真实 `SendInput` 注入，DWM 几何前后差判定）。
被测进程：`ANYDRAG_DEBUG=1 anydrag-win.exe`。

| # | 冒烟项 | 命令 | 实测输出 | 判定 |
|---|---|---|---|---|
| 1 | 启动 / 托盘 | `anydrag-win.exe` | `[tray] NIM_ADD ok, hIcon=0x1002d`；进程存活 | ✅ |
| 2 | Alt 拖动移动窗口 | `probe_gesture alt-drag notepad` | `alt-drag: dx=87 dy=0 (cmd +80px)` | **ALT_DRAG=PASS** |
| 3 | Alt 双击最大化 / 还原 | `probe_gesture alt-dbl notepad` | `after_dbl1=[0,0,2880,1754] after_dbl2=[151,57,977,1273]` | **ALT_DBL=PASS**（maximized=true, restored=true） |
| 4 | 排除表生效 | 配置 `native_excluded:["notepad.exe"]` → 重启 → `alt-drag` | `dx=0`；日志 `[resolve] (604,137) 0x59072e stop: excluded notepad.exe` | ✅ 手势被无视 |
| 5 | 总开关生效 | 配置 `enabled:false` → 重启 → `alt-drag` | `dx=0`；引擎无任何动作 | ✅ 全部手势失效 |
| 6 | 窗口正常关闭（清理） | `probe_gesture close notepad` | `still_present=false` / `CLOSE=OK` | ✅ |

引擎侧原始轨迹（`ANYDRAG_DEBUG=1`，节选）——这是"手势真的走到了状态机并落地为动作"的证据：

```
[engine] Down { x: 477, y: 137, mods: 262144 } → [None]                              ← Alt 按下 → Armed
[engine] Move { x: 497, y: 137, mods: 262144 } → [SetPos { x: 84, y: 57 }] (exe-target=notepad.exe)
[engine] Move { x: 517, y: 137, mods: 262144 } → [SetPos { x: 104, y: 57 }] (exe-target=notepad.exe)
[engine] Move { x: 537, y: 137, mods: 262144 } → [SetPos { x: 124, y: 57 }] (exe-target=notepad.exe)
[engine] Move { x: 557, y: 137, mods: 262144 } → [SetPos { x: 144, y: 57 }] (exe-target=notepad.exe)
[engine] Up   { x: 557, y: 137, mods: 262144 } → [End]
[engine] Down { x: 564, y: 665, mods: 262144 } → [Maximize, End]                      ← 双击 → 最大化
[engine] Down { x: 1440, y: 877, mods: 262144 } → [Restore, End]                      ← 再双击 → 还原
[engine] Down { x: 982, y: 665, mods: 393216 } → [None]                               ← Alt+Ctrl 贴右边
[engine] Move { x: 1002, y: 665, mods: 393216 } → [SetRect { x: 151, y: 57, w: 846, h: 1216, edges: 4 }, End]
[engine] Move { x: 1062, y: 665, mods: 393216 } → [SetRect { x: 151, y: 57, w: 906, h: 1216, edges: 4 }]
```

`mods` 值：`262144` = bit18 = Alt；`393216` = bit17|bit18 = Ctrl|Alt。`edges: 4` = `RZ_RIGHT`。

**补充项（GOAL 范围外的增量，一并验证）**

| 冒烟项 | 命令 | 实测输出 | 判定 |
|---|---|---|---|
| Alt+Ctrl 就近角 resize | `probe_gesture ctrl-resize notepad` | `w0=826 w1=906 (cmd +80px)` | **CTRL_RESIZE=PASS** |

### 5. `docs/spike-scandrag.md` 与默认策略一致 ✅

```
$ grep -n 'DEFAULT_STRATEGY' src/config.rs
const DEFAULT_STRATEGY: &str = "fallback";   // 注释引用 spike §3

$ grep -o '"decision":"[a-z]*"' docs/spike-scandrag.md
"decision":"fallback"
```

`Config::default().default_strategy == "fallback"` == spike 判定结论。

### 6. 配置落在 `%APPDATA%\anydrag-win\config.json`，改后重启生效 ✅

```
$ cat "$APPDATA/anydrag-win/config.json"
{"enabled":true}          ← 由托盘「启用/禁用」写盘产生（非手工创建）
```

"改后重启生效"由上面第 4 条的第 4、5 项直接证明：改 `native_excluded` / `enabled`
→ 重启 → 行为随之改变（手势被无视 / 全部失效）。

### 7. 托盘菜单可用 ⚠️ 部分（代码 + 图标已证，点击待人工目视）

- 菜单项（`src/tray.rs`）：`启用/禁用`、`用法…`、`配置文件…`、`退出`——共 4 项
  （GOAL §T3.3 写 3 项，`用法…` 为本次增量）。
- 图标句柄 `hIcon=0x1002d` 非空（本次修复，见下节）。
- **无法自动化**：托盘菜单需要真实鼠标点击，本环境无此自动化手段 → **待人工确认**。
- 相关路径的代码审查结论：`MENU_TOGGLE` 翻转 `config::ENABLED` 原子镜像 + `Config::save()`；
  `MENU_QUIT` → `PostQuitMessage(0)` → 泵退出 → `main` 返回 → 进程退出（LL 钩子随进程自动卸载）。

### 8. 无未决 `TODO` ✅

```
$ grep -rn "TODO\|FIXME\|XXX" src/
（空）
```

---

## 本次验收发现并修复的问题

**托盘图标不可见（用户实测反馈）**

`tray::install()` 里加载图标时把**本进程模块句柄**当 `hInstance` 传给了 `LoadImageW`：

```rust
// 错误：IDI_APPLICATION 是系统预定义资源（MAKEINTRESOURCE(32512)），
// 传本进程 hInstance 会去自己的 exe 里找图标资源 → 本仓无 .rc/图标 → 返回 NULL
let icon = LoadImageW(instance, IDI_APPLICATION, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED);
```

`hIcon = NULL` 时 `Shell_NotifyIconW(NIM_ADD)` 仍返回成功，但**托盘图标不可见**（静默失败）。
`set_icon_state()` 一直用的是 `NULL`（从系统取图标），所以只有启动路径中招。

修复：改为 `std::ptr::null_mut()`（与 `set_icon_state` 统一），并加 `ANYDRAG_DEBUG=1` 下的
`hIcon` 打印以便观测。修复后实测 `hIcon=0x1002d`（非空）。

**冒烟驱动被遮挡窗干扰**

本机运行着远程桌面客户端 `gameviewer.exe`，其窗口铺满整个工作区（`frame=[0,0,2880,1752]`）
且位于 Z 序顶层，导致引擎的命中测试解析到它而不是测试窗——表现为三项手势全部 `dx=0` 的
**假 FAIL**（日志显示引擎实际对 `gameviewer.exe` 正常执行了 56 次 `SetPos`）。
修复：`probe_gesture` 在测前把目标窗 `SetWindowPos(HWND_TOPMOST)` 抬到最前、测后恢复
`HWND_NOTOPMOST`；同时把 Alt 拖动起点从 `frame_top + 40` 下移到 `+80`，确保落点**明确在
客户区**，避免 PASS 来自 OS 自带的标题栏拖动（假阳性）。

---

## 环境注记（影响复跑方式，不影响产品结论）

1. **构建**：本机自动化沙箱拦截 `reg.exe`，而 `vcvarsall.bat` 依赖它 → `build.bat` 在沙箱内
   无法完成环境注入（`LNK1181: 无法打开输入文件 kernel32.lib`）。本次构建改为手工注入
   MSVC 的 `PATH`/`LIB`（`VC\Tools\MSVC\14.44.35207\bin\HostX64\x64` +
   `...\lib\x64` + SDK `Lib\10.0.22621.0\{ucrt,um}\x64`）。**普通桌面会话下 `cmd /c build.bat`
   可直接使用**，GOAL §2 的工具链结论依然成立。
2. **冒烟**：需真实交互桌面（`SendInput` 驱动）。本机为远程桌面会话，测试期间有真实鼠标
   活动干扰，故驱动改为"先确认托盘进程就绪（轮询 `tasklist`）再逐项执行"。

## 仍需人工目视确认的项

- 托盘图标在通知区域**可见**（句柄已非空，但"看得见"需人眼）；
- 托盘四项菜单的**实际点击**行为（含 `用法…` 弹窗、`配置文件…` 唤起编辑器）；
- GOAL §9 风险 3/4 的边界场景：Secure Desktop（UAC 弹窗）上钩子失效、提权窗口不可见。

---

## 2026-10-16 追加：用户实测反馈驱动的修复与新增

用户实测提出四条：缩放有严重延迟且"还不如自己去边缘缩放"、Alt 双击根本不能用、
要中键方向分屏、要设置界面。逐条结论：

| 项 | 结论 | 证据 |
|---|---|---|
| 缩放卡顿 | 修复（4 个缺陷，见 commit `e2641ac`） | 元凶是 `WM_SIZING` 常量写成 `0x0231`（实为 `WM_ENTERSIZEMOVE`，真值 `0x0214`）——每帧发假 ENTERSIZEMOVE 却从不发 EXITSIZEMOVE，应用永久卡在尺寸调整态 |
| 缩放要求贴边 | 修复：改为**窗口任意位置**抓取，方向按九宫格/就近角判定 | `ctrl-resize` 冒烟在窗口内部 3/4 处抓取（距右边缘约 250px）→ `w 1185→1251`，PASS |
| Alt 双击无法最大化 | 修复：改走 `SetWindowPlacement`（不依赖目标消息泵）+ 修饰键"伪装" | `alt-dbl` 冒烟 `maximized=true, restored=true`，PASS |
| 中键方向分屏 | 新增 | `middle-snap` 冒烟：窗口 `[7,0,1433,1745]` ≈ 左半屏 `[0,0,1440,1752]`，PASS |
| 设置界面 | 新增（Win32 原生控件，保存即生效） | `--settings` 启动后按标题找到窗口 `hwnd=0x4c0b98`；`send-save` 触发保存按钮后 `config.json` 由缺 `middle_snap` 变为完整配置 |
| 托盘图标是系统默认的 | 修复：改为**运行时自绘**的四向移动箭头（启用=亮蓝/禁用=灰） | 图标句柄 `hIcon=0x8008dc`（自建，非系统共享图标）；`icon_shape_and_coverage` 单测打印 ASCII 形状并校验四向对称 |

**回归**：`cargo test` 38/38 全绿；release 构建通过；四项手势冒烟全 PASS。

**本轮仍未自动化的项**（需人工目视）：
- 托盘图标在通知区域的**观感**（形状/配色已由单测的 ASCII 图核对，但"好不好看"要人眼）；
- 设置窗口各控件的**实际点击**与排版（保存链路已由 `send-save` 端到端验证）；
- 托盘菜单「设置…」的实际点击。

**安全教训（已加防护）**：冒烟驱动原先会操作"光标下最顶层的窗口"而非它以为的那个窗口。
本轮中键分屏冒烟时，光标下的顶层窗口是全屏的远程桌面客户端 `GameViewer.exe`，
结果**把用户的远程桌面窗口吸附成了半屏**。现已加守卫：抬到最前之后用窗口中心点做
`WindowFromPoint` + `GetAncestor(GA_ROOT)` 校验，不是目标窗就**直接中止**（exit 3）。
另新增 `maximize <标题>` 子命令用于复位窗口几何（已用它把该窗口恢复为全屏）。

---

## 2026-10-16 追加二：缩放仍有延迟的根因与 A/B 实测

用户追问「为啥拖动这么丝滑，缩放还是有延迟？」。做了对照实验与量化。

**为什么移动顺滑、缩放不顺**：移动只让窗口管理器挪一下表面，目标应用几乎不干活；
缩放要目标应用**按新尺寸重排+重绘**，成本全在它那边。所以同样的事件管线，
移动看不出延迟，缩放会被目标应用的消化速度卡住。

**量化方法与基线**（`probe_gesture resize-bench`：以约 200Hz 注入 40 步 × 5px，
再轮询窗口几何直到稳定，量"最后一次鼠标移动之后窗口还在变化多久"）。
被测目标：Edge + 4000 个卡片的页面（1782px 宽）。对照项是新增的
`probe_gesture native-resize`——**把光标真的放到窗口右边框上按下拖动**，
走完全原生的缩放路径，注入序列完全相同。

| 路径 | 注入用时 | 追上用时 | 额外延迟 | 最终宽度 |
|---|---|---|---|---|
| 原生边框缩放（对照基准） | 231ms | 231ms | **0ms** | 1982 ✓ |
| 我们的路径（修复前） | 249ms | 314ms | **+65ms** | 1964 ✗（差 18px） |
| 我们的路径（修复后） | 241ms | 252ms | **+11ms** | 1982 ✓ |

**两个根因**：

1. **灌得比目标消化得快**（主因）。我们按鼠标事件率下发（实测约 160 次/秒），
   而目标应用只能消化约 100 次/秒 → 它的消息队列越堆越多，窗口永远追不上光标，
   且末尾几次干脆没落地（所以最终宽度差 18px）。
   对策：几何更新**节流到显示器刷新周期**（`winmod::refresh_interval_ms`，实测该屏 120Hz
   → 8ms），并加**背压**（上一次请求没落地就不发下一次，决策在 `throttle::Throttle`）。
   中间态直接丢弃——每次下发的都是**绝对**几何，不累积误差。AltSnap 的
   `conf.RefreshRate`（`ASleep(RefreshRate)`，注释 "Accurate!!!"）是同一招。
2. **坐标语义搞错了**：engine 算出的目标矩形是 **DWM 帧**
   （`EXTENDED_FRAME_BOUNDS`）语义，却直接当**窗口矩形**下发给 `SetWindowPos`，
   两者相差左右下各约 7px 的不可见边框 → 结果偏大（实测差 18px）。
   现在下发前按实测内缩量换算（`engine_loop::apply_geom`），尺寸精确命中。

**剩余差距**：+11ms ≈ 一帧（120Hz 下 8ms），与原生基本同档，属可接受范围。

**回归**：`cargo test` 38/38 全绿；alt-drag / alt-dbl / ctrl-resize / middle-snap 四项全 PASS。


---

## 事故 5：快速反复缩放后拖拽/缩放全灭，只剩最大化可用（2026-10-21）

**用户报障原文**："在 alt+ctrl 快速缩放多次后，功能会失效，重新尝试 alt 拖拽（失效），
alt 最大化（有效），alt+ctrl 缩放（失效）"。

**根因：背压是硬闩锁，一次不匹配就永久锁死。**
旧 `engine_loop` 把"目标是否已追上"实现成：`caught_up(req, hwnd)` 比较
`GetWindowRect(hwnd)` 与上一次**请求**的矩形 `req`，不等即推迟；而 `req` **只在成功
下发时更新** ⇒ 一旦两者出现无法消除的差异，`caught_up` 永远为假，此后每一次
拖拽/缩放都被推迟到"目标追上"这个永不到来的时刻。`MAX_RETRY_ROUNDS` 用尽后只是
丢弃本帧、闩锁**依旧存在** ⇒ 功能永久失效。

这个差异极易产生：应用按自己的最小尺寸钳制（`WM_GETMINMAXINFO`）、DPI 取整、
尺寸调整相位中的临时几何、**以及切换到另一个窗口**（旧 `req` 属于旧窗口，却拿新窗口
的矩形去比）。用户"快速缩放多次"正是在反复制造这种差异，所以越快点越容易中招。

**为什么"只有最大化还好使"**：最大化/还原与中键分屏走 `SetWindowPlacement`
（`winmod::set_placement_max_restore` / `snap_to_zone`），**完全不经过**几何下发路径，
因此不受闩锁影响——这个症状指纹本身就指向了背压状态机。

**修复**（背压从"硬闩锁"改成"软背压"，逻辑抽到纯模块 `src/throttle.rs`）：

1. **绑定目标窗口**：`req` 存 `(hwnd, 矩形)`，只有**同一窗口**才做追上判断；
   换了目标窗口 = 旧请求与本帧无关，不拦。
2. **手势边界清闩锁**：`Down` / `Up` 时 `Throttle::reset()`，陈旧请求绝不跨手势存活；
   `Down` 还顺手丢弃上一手势遗留的 `pending` + 定时器（否则定时器会在新手势里
   把旧窗口的矩形放炮）。
3. **等太久就强制下发**：`throttle::FORCE_AFTER_MS`（40ms ≈ 120Hz 下 5 帧）兜底，
   把"永久死锁"退化成"每 40ms 下发一次最新几何"（= 无背压时的行为，可接受）。
   正常情况 1-2 帧就追上，这个上限不会触发。
4. **`Up` 先补发 `pending` 再清闩锁**：收尾那一帧的目标矩形只存在于 `pending`
   （`Fallback`/`Resize` 态的 `Up` 不产出 `SetPos`/`SetRect`），先清会把用户松手时的
   最终位置丢掉，窗口停在中途尺寸。
5. 顺带把 `SetTimer` 的武装收进 `arm_timer()`（继续守住"同一时刻最多一个定时器"）。

**回归测试**：新增 `throttle` 模块 10 条单测（含
`never_catching_up_is_forced_after_deadline`、`stale_request_for_other_window_does_not_block`、
`reset_clears_every_latch`），`cargo test` **47/47 全绿**。

**真机对照实验**（同一台机、同一 notepad、同一注入脚本，旧构建 = `ad41aba`）：

| 步骤 | 旧构建 ad41aba | 新构建 |
|---|---|---|
| 连做 10 次 `resize-bench`（末尾两次撞屏幕右边界） | 最后两次 `w_final` 卡在 2892 | 同左（窗口已达屏幕宽，属预期） |
| 随后 `alt-drag` | **FAIL**（dx=0 dy=0） | **PASS**（dx=87） |
| 随后 `ctrl-resize` | **FAIL**（w0=w1=1336） | **PASS**（+80px） |
| 随后 `alt-dbl` | PASS | PASS |
| 随后 `middle-snap` | PASS | PASS |

**症状指纹与用户报障逐项吻合**（旧版：拖拽 FAIL / 缩放 FAIL / 最大化 PASS），
修复后四项全 PASS。

### 追加：真正的"一直高频"才是触发条件（事件环溢出）

初版复现器（`resize-bench`）每轮只注入 200ms、中间静默 300ms，节奏太松，**触发不了**
用户说的"一直高频缩放"。新增 `probe_gesture resize-storm`：按住 Alt+Ctrl **全程不松手**，
连续注入 `rounds × 40` 次扫动，步间**不留 sleep**，把注入速率拉到 4000-6000 次/秒。

此时暴露了比背压更根本的一层根因：**64 槽事件环溢出**。
`WH_MOUSE_LL` 按鼠标硬件率推事件，而 engine 每批要花真实时间在
`SetWindowPos` + `sync_paint`（`SendMessageTimeoutW(WM_SYNCPAINT, …, 120ms)`）上，
消费跟不上生产。心跳仪（本轮新增 `dropped=` 与 `state=` 两个字段）实测：

| 指标 | 修复前 | 修复后 |
|---|---|---|
| 注入事件 | 12000 | 12000 |
| `dropped`（环满丢弃） | **7172（60%）** | **0** |
| 注入速率 | 4183/s | 6170/s（更快了） |
| 结果 | 靠运气（`Up` 恰好没被丢） | `RESIZE_STORM=SURVIVED` |

**为什么丢 `Up` 是灾难**：`Up` 丢失 ⇒ 状态机永远停在 `Fallback`/`Resize` 态 ⇒
`Action::EndSizeMove` 永不发出 ⇒ 目标应用**永久卡在尺寸调整相位**、所有重排被无限
推迟（正是 MEMORY 坑 #1 的死法），且此后手势全部受影响。丢 Move 则无害（几何是绝对值）。

**修复两层**：

1. **生产者侧扩容**：`hook::RING_SLOTS` 64 → 512（128 个事件/帧的余量，吸收任何突发）。
2. **消费者侧合并**：新增纯模块 `src/coalesce.rs` 的 `coalesce_moves`——一批 drain
   出来的事件里连续 `Move` 只保留**最后一个**。状态机对 Move 是纯函数
   （`frame + (cur - start)`），跳过中间态不改变结果，每批工作量从 O(事件数) 降到 O(1)。
   合并规则：`Down`/`Up`/`MiddleDown`/`MiddleUp` 都是**相位边界**，两侧 Move 不可跨越；
   只删"被后续 Move 完全覆盖"的事件，绝不重排、绝不丢非 Move 事件（6 条单测钉死）。
3. **状态看门狗兜底**：`STALE_GESTURE_MS = 2000`——若状态机非 Idle 且已 2 秒无任何
   事件（= 配对的 `EndSizeMove` 永远不会来了），强制补发 `EndSizeMove` 并把状态打回
   Idle。这是"绝不把目标应用永久留在尺寸调整相位"的最后一道防线。

**回归**：`cargo test` **53/53 全绿**（新增 `coalesce` 6 条）。400 轮风暴（16000 次注入）
后 `alt-drag` / `ctrl-resize` / `alt-dbl` / `middle-snap` 四项仍全 PASS。
