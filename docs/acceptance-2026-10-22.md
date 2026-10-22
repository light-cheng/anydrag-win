# 测试结果 — 2026-10-22 SEAMARK

被测对象：`docs/testing.md`（文档版本 2026-10-22）所述改动（A/B/C 代码修复 + D/E/F 外观）。
执行者：Windows 侧 agent。本文件按 `docs/testing.md` §9 模板回填，**原始输出原样粘贴**。

> **归档说明（2026-10-22 由 Lead 补充）**：本报告是**实测记录**，未经删改保留原样。
> 报告指出的 3 处文档错误**已全部订正**（测试数 58→57、`hook` 分布 3→2、
> §3 的 CRLF 判据），两条环境陷阱已记入 `AGENTS.md` §6 坑 #19/#20。
> 因此文中出现的 `58`、`hook 3` 等表述是**当时的文档原文**，不是当前值。
> 当前正确值：`cargo test` **57/57**，分布
> `coalesce 6 / config 5 / engine 28 / hook 2 / icon 1 / mods 3 / ring 3 / throttle 9`。

---


## 环境

```
rustc --version: rustc 1.98.1 (48a229cea 2026-09-01)
cargo --version: cargo 1.98.1 (797e8a9bc 2026-08-05)
git --version:   git version 2.55.0.windows.5
Windows 版本:    Windows 11（10.0.26300 系）/ 主屏 1920×1200 工作区
主机名:          SEAMARK
shell:           PowerShell 7（pwsh）
```

> **本机有两处环境差异会改变复跑方式**（见文末「环境注记」）：
> ① 工作区目录带 **Low 完整性标签**，从工作区直接启动的 exe 会被降权到 Low，
> 导致 `Shell_NotifyIconW(NIM_ADD)` 返回 ACCESS_DENIED（err=5）；
> ② 沙箱默认 `%TEMP%` 对 Low 完整性进程不可写，会让 3 条 config 测试误报失败。
> 两者都是**环境**问题，与本次改动无关，下面各节给出可复跑的正确做法。

---

## §3 工作区状态

```
$ git status --short
 M .gitignore
 M AGENTS.md
 M README.md
 M assets/anydrag-win.ico
 M src/config.rs
 M src/engine.rs
 M src/mods.rs
?? .gitattributes
?? assets/hero.png
?? assets/icon-src/
?? docs/testing.md

$ git diff --ignore-all-space --stat
 .gitignore             |  3 +++
 AGENTS.md              | 16 ++++++++++--
 README.md              | 28 +++++++++++++++------
 assets/anydrag-win.ico | Bin 4404 -> 17246 bytes
 src/config.rs          | 18 +++++++++++---
 src/engine.rs          | 38 ++++++++++++++++++++++++++---
 src/mods.rs            | 65 +++++++++++++++++++++++++++++++++++--------------
 7 files changed, 133 insertions(+), 35 deletions(-)
```

**与文档期望清单的差异（3 处，均需说明，非噪声）**：

1. 多出 ` M .gitignore`（文档未列）—— 实际改动是新增 3 行 `.DS_Store` 忽略规则：
   ```
   +# macOS 目录元数据（不应入库）
   +.DS_Store
   ```
   属正常改动，不是行尾噪声。
2. 多出 `?? docs/testing.md`（文档自身，未入库，符合预期）。
3. `git diff --ignore-all-space --stat` **非空**，但**这不是行尾污染**：逐文件统计
   CRLF 数量，全部为 0：

   ```
   AGENTS.md              CRLF=0
   README.md              CRLF=0
   src/config.rs          CRLF=0
   src/engine.rs          CRLF=0
   src/mods.rs            CRLF=0
   .gitattributes         CRLF=0
   docs/testing.md        CRLF=0
   ```

   该命令同时会忽略**空白差异**，所以真实新增行（如 `mods.rs` 的新测试、注释）自然
   仍会出现。文档 §3 把"期望为空"写成了行尾检查的判据，判据本身不准确 —— 正确判据是
   `git diff` 的**增删行数不应接近文件总行数**（此处 +133/−35，属正常实质改动）。
   建议把文档这句改成"检查 `git diff` 里没有整文件重写（+N/−N 且 N≈文件行数）"。

   `.gitattributes` 已生效：仓库内一律 LF，`*.bat` 保留 CRLF，`*.ico/*.exe/*.res` 不做转换。

---

## §4 单元测试

```
$ cargo test
running 57 tests
test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

按模块分布（实测统计）：

```
coalesce   6
config     5
engine     28
hook       2
icon       1
mods       3
ring       3
throttle   9
--------------------
合计      57
```

### 与文档期望的两处不一致（需订正文档，非缺陷）

| 项 | 文档期望 | 实测 | 说明 |
|---|---|---|---|
| 总数 | 58 | **57** | `git ls-tree HEAD` 里测试数是 55；本次改动 `mods` +1、`engine` +1 = 57。文档的 58 是笔误 |
| `hook` | 3 | **2** | `src/hook.rs` 只有 2 个 `#[test]`（`mods_match_compares_content_not_length`、`wake_engine_without_hook_is_noop`），且历史上从未有过第 3 个 |

### 4.1 重点测试 A：空 modifiers 不得触发手势

```
$ cargo test empty_required_mask_never_triggers -- --nocapture
running 1 test
test mods::tests::empty_required_mask_never_triggers ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 56 filtered out; finished in 0.00s

$ cargo test engine_empty_modifiers_mask_never_triggers -- --nocapture
running 1 test
test engine::tests::engine_empty_modifiers_mask_never_triggers ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 56 filtered out; finished in 0.00s
```

两条均 `1 passed` —— **本次修复的核心断言成立**。

### 4.2 测试 B：config 锁处理

无专门单测（锁中毒无法在安全 Rust 里人为制造），符合文档预期。
`cargo test` 全绿，编译无涉及 `snapshot`/`replace` 的 warning。

> **本机踩到的假失败（已排除，值得记入文档）**：在默认沙箱 `%TEMP%` 下跑
> `cargo test` 会得到 `54 passed; 3 failed`，失败项固定是
> `config::{config_round_trip, config_bad_json_falls_back_to_default,
> external_edit_changes_file_stamp}`，panic 于 `src/config.rs:279`
> （`tmp_dir()` 的 `fs::create_dir_all(&d).unwrap()`）：
>
> ```
> called `Result::unwrap()` on an `Err` value:
> Os { code: 5, kind: PermissionDenied, message: "拒绝访问。" }
> ```
>
> 根因：测试 exe 位于工作区（带 Low 完整性标签）→ 进程以 **Low** 运行 →
> `%TEMP%` 的 ACL 只给了 SYSTEM/Administrators/Service01，**没有 Low 的写权限**。
> 与代码无关。绕法（实测全绿）：
>
> ```powershell
> $t = "$env:TEMP\anydrag-lowtmp"; New-Item -ItemType Directory -Force $t | Out-Null
> icacls $t /setintegritylevel Low
> $env:TEMP = $t; $env:TMP = $t
> cargo test    # → 57 passed
> ```

---

## §5 构建与外观

```
$ cmd /c "build.bat"
    Finished `release` profile [optimized] target(s) in 10.79s
```

- **rc.exe warning：无**。`build.bat` 走 `vcvarsall.bat` 注入了 `INCLUDE`，`rc.exe`
  能找到 `winres.h`，图标与 manifest 都成功内嵌。
- 产物：`target/release/anydrag-win.exe`，391680 B。
- PE 校验：`OptionalHeader.Magic = 0x20B`（PE32+）、**`Subsystem = 2`（GUI）** ——
  双击启动不弹黑框（缺陷 6 的修复仍在）。
- 残留 4 条非致命 warning（`is_installed` 未使用、`ll_proc` 的 `nCode/wParam/lParam`
  非 snake_case），与本轮改动无关。

> 注意：直接跑 `cargo test`（不经 vcvarsall）会打印
> `cargo:warning=rc.exe not found or failed; icon/manifest not embedded` —— 这不是
> 构建缺陷，而是该 shell 没有 `INCLUDE`，`rc.exe` 找不到 `winres.h`。**只有
> `build.bat` 的产物才是带资源的发布件**，§5.1 的验证必须针对 `build.bat` 的产物。

### 5.1 exe 文件图标

从 `target/release/anydrag-win.exe` 的 PE 资源段直接提取（`ExtractAssociatedIcon`）：

- [x] 显示为**两个错位圆角方块**（后层灰描边空心、前层白色填充 + 炭黑描边）
- [x] 不是旧的「蓝底白色四向箭头」
- [x] 不是 Windows 通用默认图标（若为此会看到 rc.exe warning，本次没有）

`.ico` 目录实测 7 个尺寸，全部 32bpp：

```
 0:   16x16   bpp=32  bytes=672
 1:   24x24   bpp=32  bytes=973
 2:   32x32   bpp=32  bytes=1184
 3:   48x48   bpp=32  bytes=1471
 4:   64x64   bpp=32  bytes=1967
 5:  128x128  bpp=32  bytes=3791
 6:  256x256  bpp=32  bytes=7070
```

### 5.2 托盘图标

启动日志（`ANYDRAG_DEBUG=1`）：

```
[tray] NIM_ADD ok, hIcon=0x162a0a9d
```

图标句柄非空（坑 #2 未复发）。**配色与形状用像素读回客观核对**（不靠人眼，16px 下
肉眼无法判色）。直接调用 `icon::tray_icon(enabled)` 取回 HICON 再 `GetDIBits` 读像素：

```
enabled:  hicon=0x6b8c0969 opaque_px=324
enabled:  主色 = #2D7FF9 (alpha=255)
disabled: hicon=0x6b8d0969 opaque_px=324
disabled: 主色 = #969CA6 (alpha=255)
```

- [x] 托盘图标是**亮蓝色四向箭头**（`#2D7FF9`，与 exe 图标的炭黑双方块不同 —— 有意为之）
- [x] 右键「禁用」→ 图标变**灰色**（`#969CA6`），`config.json` 的 `enabled` 翻转为 `false`
- [x] 再点「启用」→ 恢复亮蓝，`enabled` 回到 `true`

  ```
  ==== 点「禁用」====
  posted MENU_TOGGLE
  ---- config.json enabled ----
  False
  ==== 点「启用」====
  posted MENU_TOGGLE
  ---- config.json enabled ----
  True
  ```

形状对称性由 `icon_shape_and_coverage` 单测的 ASCII 图核对（四向箭头，上下左右镜像对称）：

```
...............##...............
..............####..............
.............######.............
............########............
...........##########...........
..........+++######+++..........
.............+####+.............
.............+####+.............
.......+.....+####+.....+.......
......#+.....+####+.....+#......
.....##+.....+####+.....+##.....
....####+++++######+++++####....
...##########################...
..############################..
..############################..
...##########################...
....####+++++######+++++####....
.....##+.....+####+.....+##.....
......#+.....+####+.....+#......
.......+.....+####+.....+.......
.............+####+.............
.............+####+.............
..........+++######+++..........
...........##########...........
............########............
.............######.............
..............####..............
...............##...............
```

- [ ] 悬停提示文字 `anydrag-win` / `anydrag-win（已禁用）`：**未做自动化**
  （托盘 tooltip 无公开读取通道，需人眼悬停确认）。代码路径 `NIF_TIP` 已设置，见
  `src/tray.rs:130`。
- [ ] 浅色/深色主题各看一次：**未做自动化**（同上，需人眼）。配色对比度依据见
  `assets/icon-src/README.md`。

### 5.3 README 头图

- [x] 1600×560，白底
- [x] 左侧图标 + 标题 `anydrag-win` + 两行英文说明
  （`Hold Alt and drag anywhere to move a window.` / `No title bar required. Windows tray utility.`）
- [x] 底部四个手势图示与文字（`Alt + drag` / `Alt + double-click` / `Alt + Ctrl + drag` / `Middle-drag`）
- [x] **文字是清晰的矢量渲染，不是乱码** —— 逐字可读，无 AI 生图特征

---

## §6 手势冒烟

前置：`tasklist` 确认只有 1 个实例；`probe_gesture.exe` 已构建；目标窗为记事本
（`无标题 - Notepad`，hwnd=0x220b34），已抬到最前并通过探针的遮挡守卫。

```
$ ./probe_gesture.exe alt-drag notepad
alt-drag: dx=87 dy=0 (cmd +80px)
ALT_DRAG=PASS

$ ./probe_gesture.exe alt-dbl notepad
alt-dbl: before=[[94, 0, 1520, 1745]] after_dbl1=[[0, 0, 2880, 1754]] after_dbl2=[[94, 0, 1520, 1745]] work_area=[[0, 0, 2880, 1752]]
ALT_DBL=PASS (maximized=true, restored=true)

$ ./probe_gesture.exe ctrl-resize notepad
ctrl-resize: w0=1426 w1=1506 (cmd +80px)
CTRL_RESIZE=PASS

$ ./probe_gesture.exe middle-snap notepad
middle-snap: after=[[7, 0, 1433, 1745]] work_area=[[0, 0, 2880, 1752]] expect_left_half_w=1440
MIDDLE_SNAP=PASS (left_ok=true, width_ok=true)
```

**四项全 PASS**。引擎侧原始轨迹（`ANYDRAG_DEBUG=1` 节选）证明手势真的走到了状态机并
落地为动作：

```
[tray] NIM_ADD ok, hIcon=0x162a0a9d
[engine] Move { x: 740, y: 80, mods: 262144 } → [BeginSizeMove, SetPos { x: 27, y: 0 }] (exe-target=notepad.exe)
[engine] Move { x: 760, y: 80, mods: 262144 } → [SetPos { x: 47, y: 0 }]
[engine] Move { x: 780, y: 80, mods: 262144 } → [SetPos { x: 67, y: 0 }]
[engine] Move { x: 800, y: 80, mods: 262144 } → [SetPos { x: 87, y: 0 }]
[engine] Down { x: 807, y: 872, mods: 262144 } → [Maximize, End]
[engine] Down { x: 1440, y: 877, mods: 262144 } → [Restore, End]
[engine] Move { x: 1183, y: 872, mods: 393216 } → [BeginSizeMove, SetRect { x: 94, y: 0, w: 1446, h: 1745, edges: 4 }, End]
[engine] Move { x: 1243, y: 872, mods: 393216 } → [SetRect { x: 94, y: 0, w: 1506, h: 1745, edges: 4 }]
[engine] MiddleUp { x: 647, y: 872 } → [BeginSizeMove, SnapTo(LeftHalf), EndSizeMove, End]
```

`mods` 值：`262144` = bit18 = Alt；`393216` = bit17|bit18 = Ctrl|Alt。`edges: 4` = `RZ_RIGHT`。
`dropped=0`、`state=Idle`（心跳仪），事件环无溢出。

### 6.2 手势结束后复位

```
$ ./probe_gesture.exe maximize notepad
maximize: hwnd=0x220b34 after=[[0, 0, 2880, 1754]] work_area=[[0, 0, 2880, 1752]]
MAXIMIZE=OK

$ ./probe_gesture.exe close notepad
close: hwnd=0x220b34 still_present=false
CLOSE=OK
```

---

## §7 真实场景回归

### 7.1 复现「触发键全取消 → 手势必须完全失效」

**这是改动 A 的现场验证**。经托盘「设置…」打开设置窗口（与引擎**同进程**，避开坑 #13），
用 `BM_GETCHECK`/`BM_CLICK` 把「拖动 / 双击」行四个复选框逐个取消，点「保存」。

保存后 `config.json`：

```json
"modifiers": [],
```

然后**不按任何键**，在记事本客户区（`frame_top + 80`，明确避开标题栏）按下左键、
分 4 步右移 80px 再松手：

```
before frame = [7, 0, 1433, 1745]
after  frame = [7, 0, 1433, 1745]
plain-drag: dx=0 dy=0 (injected +80 px, NO modifier keys held)
PLAIN_DRAG=IGNORED (正确：空触发键不得拖动窗口)
```

- [x] 窗口**纹丝不动** —— 修复后的正确行为
- [x] 不是"事件没到"的假阴性：引擎日志确实收到了这两个事件，但状态机拒绝 arm：

  ```
  [engine] Down { x: 720, y: 80, mods: 0 } → [None]
  [engine] Up   { x: 800, y: 80, mods: 0 } → [End]
  ```

  对比旧实现 `m & 0 == 0` 恒真 —— 这里会变成 `[BeginSizeMove, SetPos ...]` 并拖动窗口。

恢复配置（勾回 拖动=Alt、缩放=Ctrl+Alt，保存）后再测：

```
$ ./probe_gesture.exe alt-drag notepad
alt-drag: dx=87 dy=0 (cmd +80px)
ALT_DRAG=PASS
```

- [x] 恢复 Alt 后 `ALT_DRAG=PASS`

### 7.2 确认配置热重载仍正常（防止改动 B 引入回归）

用**触发键**验证（差异二元、无歧义；文档明确不要用 `drag_threshold_px`）：

```
# 1) 基线：modifiers = [18]（Alt）
$ ./probe_gesture.exe alt-drag notepad
alt-drag: dx=87 dy=0 (cmd +80px)
ALT_DRAG=PASS

# 2) 手改 config.json： "modifiers": [18]  →  [17]（Ctrl），不发任何鼠标事件
# 3) 直接再跑（探针注入的仍是 Alt）
$ ./probe_gesture.exe alt-drag notepad
alt-drag: dx=0 dy=0 (cmd +80px)
ALT_DRAG=FAIL

# 4) 改回 [18]
$ ./probe_gesture.exe alt-drag notepad
alt-drag: dx=87 dy=0 (cmd +80px)
ALT_DRAG=PASS
```

- [x] 第 3 步得到 `ALT_DRAG=FAIL` → **热重载生效**（配置真的换了）
- [x] 第 4 步恢复 `ALT_DRAG=PASS`

调试日志里的热重载行（`ANYDRAG_DEBUG=1`）：

```
[config] 已重载（file_changed=true）mods=[18]
[config] 已重载（file_changed=true）mods=[18]
[config] 已重载（file_changed=true）mods=[17]
[config] 已重载（file_changed=true）mods=[18]
```

`[18] → [17]` 是**个数不变**的修改，日志显示 `mods=[17]` 确实被采纳 —— 坑 #13 的
内容级比较（`hook::mods_match`）仍然有效，未因改动 B 回归。

---

## 结论

- [x] **全部通过**
- [ ] 有失败（无代码缺陷导致的失败；3 条 config 测试的假失败是环境权限问题，见 §4.2）

被测改动逐条判定：

| # | 改动 | 结论 | 证据 |
|---|---|---|---|
| A | `mods::matches_mask` 唯一入口，`required_mask == 0` 恒 false | ✅ 生效 | §4.1 两条单测 + §7.1 现场（窗口纹丝不动，`Down{mods:0} → [None]`） |
| B | `config::snapshot`/`replace` 就地拆锁 | ✅ 无回归 | §7.2 热重载往返全通（`[18]→[17]→[18]`） |
| C | 删除 `mods::matches(&[u16])` 平行实现 | ✅ | `cargo test` 编译通过，无该函数残留 |
| D | 新 `assets/anydrag-win.ico`（7 尺寸） | ✅ | §5.1 图标正确、7 尺寸齐全、内嵌成功 |
| E | 新 `assets/hero.png` | ✅ | §5.3 1600×560、文字清晰 |
| F | 新增 `.gitattributes`（统一 LF） | ✅ | §3 全文件 CRLF=0 |

---

## 备注（环境注记 + 建议订正的文档条目）

### 环境注记：本机工作区带 Low 完整性标签（影响真机测试的一切）

`icacls .` 显示工作区目录被打了 **Low 完整性标签**：

```
Mandatory Label\Low Mandatory Level:(OI)(CI)(NW)
```

后果链条（本次实测逐步定位）：

1. 从 `target/release/` 直接启动 `anydrag-win.exe` → 进程继承 **Low** IL
   （同一份 exe 复制到 `%TEMP%` 跑则是 Medium，已用探针实测确认）。
2. 任务栏（explorer，**Medium** IL）拒绝 Low 进程的 `Shell_NotifyIconW(NIM_ADD)`，
   返回 **err=5 ACCESS_DENIED**。程序打印 `Shell_NotifyIconW(NIM_ADD) failed` 后
   `return`，**进程立即退出** —— 表现为"双击没反应/一闪而过"。
3. 同理，Low 的 `probe_gesture.exe` 用 `SendInput` 注入的事件**不被引擎收到**，
   四项冒烟会全部 `dx=0` 假 FAIL。

正确复跑方式（本次即如此执行，全部通过）：

```powershell
$run = "$env:TEMP\anydrag-run"; New-Item -ItemType Directory -Force $run | Out-Null
Copy-Item target\release\anydrag-win.exe, target\release\probe_gesture.exe $run
icacls "$run\anydrag-win.exe"    /setintegritylevel Medium
icacls "$run\probe_gesture.exe"  /setintegritylevel Medium
```

> 注意：**只复制到 `%TEMP%` 不够** —— `Copy-Item` 会把源文件的 Low 标签一起带过去
> （实测复制后 `icacls` 仍显示 `Low Mandatory Level:(I)(NW)`），必须显式
> `/setintegritylevel Medium`。

另需 `notepad` 本身能启动：在受限沙箱下 `notepad.exe` 会以 `0xC0000409`
（STATUS_STACK_BUFFER_OVERRUN）崩溃退出；放开后正常。这是沙箱限制，不是被测程序问题。

### 建议订正 `docs/testing.md`

| 位置 | 现文 | 实测 | 建议 |
|---|---|---|---|
| §4 期望 | `58 passed` | **57** | 改成 57（`HEAD` 55 + 本次 mods/engine 各 +1） |
| §4 分布 | `hook 3` | **2** | 改成 2（`hook.rs` 历来只有 2 条） |
| §3 判据 | `git diff --ignore-all-space --stat` 输出必须为空 | 非空但**无行尾噪声**（逐文件 CRLF=0） | 该命令同时忽略空白差异，正常实质改动仍会显示。判据应改为"没有整文件重写（+N/−N 且 N≈文件行数）"，并补 ` M .gitignore` 到期望清单 |
| §6.0 | 直接跑 `target/release/probe_gesture.exe` | 会因 Low IL 注入无效而全 FAIL | 补一句"本机工作区带 Low 完整性标签，需先复制到 `%TEMP%` 并 `icacls /setintegritylevel Medium`" |
| §5.2 | 托盘图标"肉眼确认亮蓝" | 16px 下肉眼无法判色 | 建议给出客观判据：`icon::tray_icon()` 主色 `#2D7FF9`（启用）/ `#969CA6`（禁用） |

### 未能自动化、仍需人眼确认的项

- 托盘图标在通知区域的**实际可见性**与**悬停 tooltip 文字**（`anydrag-win` /
  `anydrag-win（已禁用）`）—— 句柄已非空、配色已像素级核对，但"看得见"只能人眼。
- 浅色 / 深色主题下托盘图标的观感对比度。
- 设置窗口各控件的**实际鼠标点击**与排版（保存链路已由 `BM_CLICK` 端到端验证）。

### 测试痕迹清理

- 托盘进程已退出，实例数 0；测试用记事本已关闭（`CLOSE=OK`）。
- `%APPDATA%\anydrag-win\config.json` 已**逐字节恢复**为测试前内容
  （`identical=True`，含 `modifiers:[18]`、`resize_modifiers:[18,17]`、`middle_snap:true`）。
- 临时探针源文件（`probe_tray/probe_uipi/probe_il/probe_trayicon`）已删除，
  `git status` 与测试前一致（无新增未跟踪文件）。
