# 测试说明（交给 Windows 侧的 agent 执行）

- **文档版本**：2026-10-22
- **被测改动**：见 §1「这次改了什么」
- **预计耗时**：单元测试 2 分钟；真机冒烟 10–15 分钟（含人工确认项）
- **前置环境**：Windows 10/11 x64、Visual Studio 2022 BuildTools（MSVC x64）、
  Windows SDK（含 `rc.exe`）、git-bash 或 cmd

> 本文件是**操作手册**，不是验收记录。执行完请按 §7 的模板回填结果，
> 把「实测输出」原样贴回来 —— 不要写「已通过」这种没有原始输出的结论。

---

## 1. 这次改了什么（测试要盯的点）

改动分两块，**第二块只影响外观，第一块有行为影响**。

### 1.1 代码修复（重点）

| # | 改动 | 影响 | 必须验证 |
|---|---|---|---|
| A | `mods::matches_mask` 成为**唯一**判定入口；`required_mask == 0` 恒返回 false | 旧实现在 `modifiers: []` 时**任何一次鼠标点击都会拖动窗口**。已修 | §4.1 |
| B | `config::snapshot` / `replace` 的 `if let Ok(..)` 改为就地拆锁 | 锁中毒时不再静默跳过（旧行为：故障永久化且零报错） | §4.2（难触发，跑通即可） |
| C | 删除 `mods::matches(&[u16])` 平行实现 | 消除「同一语义两个实现」，其中带测试的那个没被生产调用 | `cargo test` 编译通过即证明 |

### 1.2 外观（不影响行为，但要确认能构建）

| # | 改动 | 验证 |
|---|---|---|
| D | 新 `assets/anydrag-win.ico`（7 尺寸，错位双方块） | §5.1 exe 图标、§5.2 托盘图标 |
| E | 新 `assets/hero.png`（README 头图） | §5.3 渲染 |
| F | 新增 `.gitattributes`（统一 LF） | §3 |

---

## 2. 环境准备

```bash
# 全部命令都在仓库根目录执行（含 build.bat 的那一层）
cd /path/to/anydrag-win

# 确认工具链
rustc --version      # 期望 1.9x
cargo --version
git --version
```

**若 `cargo build` 报 `LNK1181: cannot open input file 'kernel32.lib'`**，
说明 `vcvarsall.bat` 没注入环境（本机曾因沙箱拦 `reg.exe` 出现）。绕法：

```bash
# 手工指向 MSVC + SDK（版本号按本机实际改）
export PATH="/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207/bin/Hostx64/x64:$PATH"
export LIB="/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207/lib/x64;/c/Program Files (x86)/Windows Kits/10/Lib/10.0.22621.0/um/x64;/c/Program Files (x86)/Windows Kits/10/Lib/10.0.22621.0/ucrt/x64"
cargo build --release
```

### 2.1 **先查工作区的完整性标签**（不查会得到满屏假 FAIL）

2026-10-22 实测（SEAMARK 机）：工作区目录被打了 **Low 完整性标签**，从工作区直接
启动的 exe 会继承 Low IL，后果是**功能全部"坏掉"但代码没问题**：

| 现象 | 真因 |
|---|---|
| 双击 exe 一闪而过 / 没反应 | 任务栏（explorer，Medium IL）拒绝 Low 进程的 `Shell_NotifyIconW(NIM_ADD)`，返回 err=5，程序打印 `NIM_ADD failed` 后 `return` |
| 四项手势冒烟全部 `dx=0` 假 FAIL | Low 的 `probe_gesture.exe` 用 `SendInput` 注入的事件不被引擎收到 |
| `cargo test` 固定 3 条 config 测试失败 | Low 进程对 `%TEMP%` 无写权限，`config.rs` 的 `tmp_dir()` panic（err=5 拒绝访问） |

**先查**：

```powershell
icacls . | Select-String "Mandatory Label"
# 若出现 Low Mandatory Level → 本机就是这种情况，必须按下面绕开
```

**绕法一（推荐）：把 exe 复制到 `%TEMP%` 并显式提权到 Medium**

```powershell
$run = "$env:TEMP\anydrag-run"; New-Item -ItemType Directory -Force $run | Out-Null
Copy-Item target\release\anydrag-win.exe, target\release\probe_gesture.exe $run
# 只复制不够！Copy-Item 会把源文件的 Low 标签一起带过去，必须显式设
icacls "$run\anydrag-win.exe"   /setintegritylevel Medium
icacls "$run\probe_gesture.exe" /setintegritylevel Medium
# 之后所有真机测试都从这个目录跑
```

**绕法二：给 `%TEMP%` 一个 Low 可写的子目录**（只为 `cargo test`）

```powershell
$t = "$env:TEMP\anydrag-lowtmp"; New-Item -ItemType Directory -Force $t | Out-Null
icacls $t /setintegritylevel Low
$env:TEMP = $t; $env:TMP = $t
cargo test    # → 全绿
```

> 这两个都是**环境**问题，与被测代码无关。若不做绕法，看到失败先对照本表，
> **不要当成代码缺陷报**。

---

## 3. 第 0 步：确认工作区状态（30 秒）

```bash
git status --short
```

**期望**：恰好这些条目（多出来的要问清楚再继续）

```
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
?? docs/acceptance-2026-10-22.md
?? docs/testing.md
```

> 若你用的是补丁包（`patch/`），打完后 `git status` 可能显示 `A`（intent-to-add）
> 而不是 `??`——那是 `git add -N` 的正常结果，内容无差别，不影响判定。

```bash
# 检查有没有整文件被行尾重写（判据见下方说明，不要用"输出为空"当判据）
git diff --ignore-all-space --stat
```

**正确判据**：看**增删行数是否接近文件总行数**。本次改动的正常形态是
`+133 / −35` 量级（真实新增的注释、测试、文档）。若某个文件出现
`+N / −N` 且 N ≈ 该文件行数（例如 `+739/−739`），那才是行尾污染。

> **为什么不能用"输出为空"当判据**（2026-10-22 订正）：`--ignore-all-space`
> 只忽略**空白差异**，真实的实质改动当然还会显示出来。上一版文档把判据写成
> "必须为空"是错的，会让执行者误以为正常改动是噪声。
>
> 真正要防的是上一轮发生的事：4 个文件被 Windows 编辑器整体改成 CRLF，
> `git diff` 看起来像整文件重写但实际零改动。逐文件确认行尾更直接：
>
> ```bash
> # 期望每个文件都是 CRLF=0（.bat 除外，它按 .gitattributes 保留 CRLF）
> for f in AGENTS.md README.md src/config.rs src/engine.rs src/mods.rs \
>          .gitattributes docs/testing.md; do
>   printf "%-22s CRLF=%s\n" "$f" "$(grep -c $'\r' "$f" || true)"
> done
> ```
>
> `.gitattributes` 已兜底：仓库内一律 LF，`*.bat` 保留 CRLF，
> `*.ico/*.exe/*.res` 不做转换。

---

## 4. 第 1 步：单元测试（必跑，2 分钟）

```bash
cargo test 2>&1 | tail -30
```

**期望**：`test result: ok. 92 passed; 0 failed`

分布（用编译器真实输出分组统计，**不要**用 `grep -c '#[test]'`，见 AGENTS 坑 #20）：

```
coalesce 6 / config 6 / engine 28 / hook 2 / i18n 8 / icon 1 /
layout 11 / mods 3 / ring 3 / settings 6 / theme 9 / throttle 9   = 92
```

> 2026-10-23 新增 `theme`（配色令牌 + `blend()` + WCAG 对比度）、
> `layout`（唯一坐标来源 + 几何不变量）、`i18n`（全部文案，中英双语）、
> `settings`（控件 id / 卡片标题映射）四个模块的 34 条测试；
> `config` 新增 1 条（旧配置缺 `lang` 字段仍能加载）。
> 测试数**只增不减**，改完请用 `cargo test` 的真实汇总行核对。

### 4.1 重点测试 A：空 modifiers 不得触发手势

```bash
cargo test empty_required_mask_never_triggers -- --nocapture
cargo test engine_empty_modifiers_mask_never_triggers -- --nocapture
```

**期望**：两条都 `1 passed`。

**这两条是本次修复的核心**。旧代码 `m & 0 == 0` 恒真，导致用户把设置界面四个
触发键复选框**全取消**后，随手点一下鼠标就把窗口拖走，且毫无报错。
若这两条失败 → **立刻停止**，说明修复没生效或编译到了旧代码。

> 注意：`cargo test` 会跑**所有**测试，其中 `ring_concurrent_conservation`
> 是 10 万次并发压测，正常 1 秒内完成。若整体超过 60 秒或卡住，
> 大概率是 `--release` 和 debug 混跑导致的，重跑一次即可。

### 4.2 测试 B：config 锁处理（跑通即可，无需单独验证）

没有专门的单元测试（锁中毒无法在安全 Rust 里人为制造）。
只要 `cargo test` 全绿 + 编译无 warning 涉及 `snapshot`/`replace` 即可。

---

## 5. 第 2 步：构建 + 外观验证（5 分钟）

```bash
cmd //c "build.bat" 2>&1 | tail -20
```

**期望**：`Finished release profile [optimized] target(s)`，且**没有**
`rc.exe not found or failed` 这条 warning。

> 如果出现 `cargo:warning=rc.exe not found or failed; icon/manifest not embedded`，
> 说明图标**没有**嵌进 exe，§5.1 必然失败。此时要装 Windows SDK 或修正 `build.rs`
> 里的 rc.exe 搜索路径，不要跳过。

### 5.1 exe 文件图标（人工，1 分钟）

```bash
ls -la target/release/anydrag-win.exe
```

在资源管理器里看 `target/release/anydrag-win.exe` 的图标：

- [ ] 显示为**两个错位圆角方块**（后层灰描边空心、前层白色填充 + 炭黑描边）
- [ ] 不是旧的「蓝底白色四向箭头」
- [ ] 不是 Windows 通用默认图标（若为此，见上方 rc.exe 警告）

放大到「大图标」视图再看一次，确认圆角和描边没有锯齿崩坏。

### 5.2 托盘图标（2 分钟）

```bash
# 启动（GUI 子系统，从 cmd 启动不弹黑框）
# 若本机工作区带 Low 完整性标签，请从 %TEMP% 的提权副本启动（见 §2.1）
target/release/anydrag-win.exe
```

**先看客观判据**（16px 下肉眼判不准颜色，别靠眼睛）：

```
# 启动日志里这行必须出现且 hIcon 非空（坑 #2 的守卫）
[tray] NIM_ADD ok, hIcon=0x...
```

| 状态 | 期望主色 | 判据 |
|---|---|---|
| 启用 | `#2D7FF9`（亮蓝） | 见 `src/icon.rs` 的 `tray_icon(true)` |
| 禁用 | `#969CA6`（中灰） | 见 `src/icon.rs` 的 `tray_icon(false)` |

形状由单测客观核对（不靠人眼）：

```bash
cargo test icon_shape_and_coverage -- --nocapture
# 会打印 ASCII 图；期望是四向箭头，且断言上下/左右镜像对称、四臂都接近边缘
```

**再做人眼确认**（这几项无法自动化）：

- [ ] 托盘出现图标，是**亮蓝色四向箭头**（不是新 exe 图标那样的双方块）
- [ ] 鼠标悬停提示 `anydrag-win`
- [ ] 右键菜单 →「禁用」，图标变**灰色**，提示变 `anydrag-win（已禁用）`
- [ ] 再点「启用」，恢复亮蓝
- [ ] 切换系统主题（浅色 ↔ 深色），两种主题下图标都看得清

> 托盘 tooltip 没有公开的读取通道，只能人眼悬停确认（代码路径 `NIF_TIP` 在
> `src/tray.rs`）。`config.json` 的 `enabled` 翻转可客观验证，见 §7.1 的同类手法。

> **为什么托盘图标和 exe 图标不一样，这是有意的**：托盘是单色剪影，深灰在深色
> 任务栏上对比度仅 1.3:1（几乎不可见），亮蓝在深浅两种任务栏上都 ≥3.4:1。
> 详见 `assets/icon-src/README.md`。**不要**把这条当成 bug 报。

### 5.3 README 头图（人工，30 秒）

打开 `assets/hero.png`：

- [ ] 1600×560，白底
- [ ] 左侧图标 + 标题 `anydrag-win` + 两行英文说明
- [ ] 底部四个手势图示与文字（Alt + drag / Alt + double-click / Alt + Ctrl + drag / Middle-drag）
- [ ] **文字是清晰的矢量渲染，不是乱码**（若乱码说明用了 AI 生图，是错的）

---

## 6. 第 3 步：真机手势冒烟（必跑，10 分钟）

### 6.0 前置守卫（**不要跳过**）

这台机器的桌面是**通过远程桌面客户端连进来的**，该窗口全屏置顶。
任何「操作光标下窗口」的动作都可能误伤它 —— 历史上真实发生过把用户的桌面窗口
吸附成半屏。

```bash
# 1) 确认只有一个实例（多实例是排查第一嫌疑）
tasklist | grep -i anydrag-win

# 2) 确认冒烟驱动已构建
ls -la target/release/probe_gesture.exe

# 3) 确认 exe 不是 Low 完整性（Low 会让 SendInput 注入失效 → 四项全假 FAIL）
icacls target/release/probe_gesture.exe | grep -i "Mandatory Label"
# 若显示 Low Mandatory Level → 按 §2.1 复制到 %TEMP% 并 /setintegritylevel Medium
```

如果 `probe_gesture.exe` 不存在：

```bash
cargo build --release --bin probe_gesture
```

**冒烟前必须新开一个记事本作为目标窗口**，并让它在前台：

```bash
notepad &
sleep 2
```

> 探针自己有 `raise_target` 守卫（用 `WindowFromPoint` + `GetAncestor` 校验
> 目标窗真的在最前，否则直接 `exit 3` 中止）。但**不要依赖它兜底** ——
> 它只能确认"中心点下是目标窗"，不能防止你自己脚本里的其他误操作。

### 6.1 四项手势（逐条执行，每条都要看输出）

```bash
cd target/release

# ① 移动：Alt + 客户区拖动，期望 dx ≥ 40
./probe_gesture.exe alt-drag notepad
# 期望末行：ALT_DRAG=PASS

# ② 双击最大化 / 还原
./probe_gesture.exe alt-dbl notepad
# 期望末行：ALT_DBL=PASS (maximized=true, restored=true)

# ③ 缩放：Alt+Ctrl 在窗口内部（不贴边）拖动，期望宽度 +80
./probe_gesture.exe ctrl-resize notepad
# 期望末行：CTRL_RESIZE=PASS

# ④ 中键方向分屏：向左拖 200px 松手 → 吸附左半屏
./probe_gesture.exe middle-snap notepad
# 期望末行：MIDDLE_SNAP=PASS (left_ok=true, width_ok=true)
```

**每条都要把完整输出贴回来**，不要只贴 PASS/FAIL。

> 若 `alt-dbl` 失败并显示 `maximized=false`：先看是不是窗口处在
> 「记录=最大化、矩形=普通尺寸」的不一致态（坑 #14）。清态顺序是
> `SW_RESTORE` → `SW_SHOWNORMAL` → `SetWindowPos`，用 `probe_gesture maximize`
> 复位后再测。

### 6.2 手势结束后必须复位

```bash
./probe_gesture.exe maximize notepad   # 避免留下半屏窗口
./probe_gesture.exe close notepad      # 关掉测试窗
```

### 6.3 设置界面外观与交互验收（2026-10-23 新增，必跑）

对应 `docs/spec-2026-10-23-gui-i18n.md` §5.2 的 V1–V15。**截图不接受"看着还行"**，
四张都要有：中文浅色 / 英文浅色 / 中文深色 / 英文深色。

```powershell
# 前置：exe 必须是 Medium IL（见 §2.1），否则托盘装不上、窗口也起不来
# 四张截图（窗口置顶后截屏；PrintWindow 抓不到子控件，必须用 CopyFromScreen）
```

| # | 检查 | 判据 | 怎么验 |
|---|---|---|---|
| V1 | 无控件压在错误的卡片上 | 每个标签/控件都落在其所属卡片的白色区域内 | 目视 + `layout.rs` 的 `every_control_lives_in_its_card` 单测 |
| V2 | 无死空白 | 窗口底边距 ≈ 20px | 目视；`window_height_is_content_driven` 单测 |
| V3 | 卡片标题与首行不相贴 | 能分辨"标题行"与"内容行" | 目视 |
| V4 | 正文 14px / 标题 Semibold | `grep -n DEFAULT_GUI_FONT src/settings.rs` **必须为空** | 命令 |
| V5 | 120/144/192 DPI 均不被裁切 | 客户区宽度 == `560 * dpi / 96` | 见下方实测值 |
| V6 | **四张**截图 | 英文下任一标签不被截断成 "…" | 目视 |
| V7 | 语言切换不需重开窗口 | 点按钮立即变 | 点「English」看标题/按钮是否立刻变 |
| V8 | 点「取消」后语言回滚 | 重新打开仍是旧语言 | 切语言 → 取消 → 重开 |
| V9 | 单位后缀跟随输入框 | "5 像素"成一组 | 目视 |
| V10 | **深色下没有"白底浅字/白底黑字"控件** | EDIT 深底浅字、复选框深底浅字、按钮深底浅字 | 深色截图逐控件核对 |
| V11 | 切系统主题后窗口跟着变（不重启） | 系统设置里切深浅 → 窗口立即重绘 | 见下方脚本 |
| V12 | 复选框自绘后**键盘可用** | Tab 聚焦 → 空格能切换；焦点环可见 | 见下方脚本 |
| V13 | 配色符合设计原则 | 中性色带蓝调；**不得出现纯灰 `#808080` 这类自调色** | 目视 + `theme.rs` 对比度单测 |
| V14 | 圆角符合刻度 | 卡片 12 / 输入框 8 / 复选框 4 | 截图放大目视 |
| V15 | 深色下卡片边框可辨但不刺眼 | 不是纯黑粗线（说明 alpha 预混合了） | 深色截图目视 |

**V5 实测值（本机 144 DPI，2026-10-23）**：

```
[settings] dpi=144 client=840x1101     # 进程自报
GetClientRect  = 840x1101              # 外部测量一致
期望 = 560*144/96 = 840                # ✅
```

> **注意：测 DPI/截图的那一侧也必须声明 PerMonitorV2 感知**。
> PowerShell 默认是 DPI-unaware，`GetWindowRect` 会返回被系统虚拟化过的
> 96 DPI 值（窗口明明 840x1101，它说 575x771），`CopyFromScreen` 也只取到
> 左上角一块 —— 看起来像"界面被裁掉"，其实是被截断的。
> 脚本开头必须 `SetProcessDpiAwarenessContext(-4)`。
> （同款坑还有：`FindWindowW(cls, $null)` 从 PowerShell 传 `$null` 会被封送成
> 空串而非 NULL，于是永远找不到窗口 —— 改用 `EnumWindows` 按类名匹配。）

**V11 手动验法**：打开设置窗口 → 系统设置 → 个性化 → 颜色 → 「选择默认应用模式」
切浅/深 → **设置窗口应当立即变色**，不需要关掉重开。
（实现：`WM_SETTINGCHANGE` + `lParam == "ImmersiveColorSet"` → 重取
`AppsUseLightTheme` → 重套 EDIT/BUTTON 主题 + `InvalidateRect` 全窗重绘。）

**V12 手动验法**：点一下设置窗口 → 按 Tab（焦点环应出现在第一个复选框上）→
再按 Tab（应移到第二个）→ 按空格（勾选状态应翻转）。

---

## 7. 第 4 步：本次修复的**真实场景**回归（重点，3 分钟）

单元测试覆盖了逻辑，但**必须真机验证一次**，因为这是用户能实际踩到的坑。

### 7.1 复现「触发键全取消 → 手势必须完全失效」

这是改动 A 的现场验证。**旧版在这里会失败**（点一下鼠标就拖窗口）。

1. 托盘右键 →「设置…」
2. 把「拖动 / 双击」下的 **Alt / Ctrl / Shift / Win 四个复选框全部取消勾选**
3. 点「保存」
4. **不要按任何键**，直接用鼠标左键点一下记事本的客户区并小幅拖动
5. 观察：

- [ ] 窗口**纹丝不动**（修复后的正确行为）
- [ ] 若窗口被拖动了 → **修复失败，立刻停止并报告**

6. 恢复配置：设置界面重新勾选 **Alt**（拖动）与 **Ctrl + Alt**（缩放），保存
7. 再次 `probe_gesture alt-drag notepad`，确认 `ALT_DRAG=PASS` 恢复

> 该状态（`modifiers: []`）是设置界面的**合法状态**，不是异常输入。
> 旧代码 `down_mask & 0 == 0` 恒真 → 任意点击都触发拖动。

### 7.2 确认配置热重载仍正常（防止改动 B 引入回归）

改动 B 动了 `config::snapshot` / `replace`，而热重载完全依赖这两个函数，
所以要确认「保存后立即生效」这条链路没坏。

**不要用 `drag_threshold_px` 来验证**（探针固定拖 80px，无论阈值改成 5 还是 40
都会 `PASS`，测不出区别）。改用**触发键**验证，差异是二元的、没有歧义：

```bash
# 前提：程序在跑，记事本开着
# 1) 基线：默认 Alt 触发
./probe_gesture.exe alt-drag notepad
# 期望：ALT_DRAG=PASS

# 2) 手改配置，把拖动触发键从 Alt 改成 Ctrl
#    把 "modifiers": [18]  改成  "modifiers": [17]
notepad "$APPDATA/anydrag-win/config.json"

# 3) 不发任何鼠标事件，直接再跑一次（探针注入的是 Alt）
./probe_gesture.exe alt-drag notepad
# 期望：ALT_DRAG=FAIL —— 因为现在要 Ctrl 才触发，注入 Alt 不该生效
```

- [ ] 第 3 步得到 `ALT_DRAG=FAIL` → 热重载生效（配置真的换了）
- [ ] 若仍是 `PASS` → 热重载坏了，**报告**

```bash
# 4) 改回 [18]，再测一次确认恢复
#    期望：ALT_DRAG=PASS
```

> **这一步同时验证了坑 #13 的内容级比较**：`[18]` → `[17]` 是「个数不变」的修改，
> 旧实现只比长度会误判「没变」，导致钩子继续按旧键表算 mods。
> 现在用 `hook::mods_match` 做集合比较，能正确刷新。

**用调试日志确认更直接**（可选）：

```bash
taskkill //F //IM anydrag-win.exe
ANYDRAG_DEBUG=1 target/release/anydrag-win.exe 2> diag.log &
# 手改 config.json 保存后：
grep "\[config\]" diag.log
# 期望：[config] 已重载（file_changed=true）mods=[...]
```

---

## 8. 第 5 步：跑完清理

```bash
taskkill //F //IM anydrag-win.exe
rm -f diag.log
git status --short   # 确认没有测试残留文件被误提交
```

如果测试过程中改过 `%APPDATA%\anydrag-win\config.json`，
**把它恢复成测试前的样子**（`git status` 看不到它，它在仓库外，容易漏）。

---

## 9. 结果回填模板

执行完请照这个格式回填（**原始输出要贴，不要只写结论**）：

```markdown
## 测试结果 — <日期> <机器名>

### 环境
rustc --version: <粘贴>
cargo --version: <粘贴>
Windows 版本: <粘贴>

### §3 工作区状态
git status --short: <粘贴>
git diff --ignore-all-space --stat: <粘贴，期望为空>

### §4 单元测试
cargo test 汇总行: <粘贴 "test result: ok. ...">
empty_required_mask_never_triggers: <粘贴>
engine_empty_modifiers_mask_never_triggers: <粘贴>

### §5 构建与外观
build.bat 结果: <粘贴 Finished 行>
是否有 rc.exe warning: <有/无>
exe 图标: <截图或描述>
托盘图标（浅色主题）: <截图或描述>
托盘图标（深色主题）: <截图或描述>
hero.png 文字是否清晰: <是/否>

### §6 手势冒烟
alt-drag: <粘贴完整输出>
alt-dbl: <粘贴完整输出>
ctrl-resize: <粘贴完整输出>
middle-snap: <粘贴完整输出>

### §7 真实场景回归
7.1 触发键全取消后拖动窗口: <纹丝不动 / 被拖动了>
7.1 恢复 Alt 后 alt-drag: <PASS/FAIL>
7.2 热重载日志: <粘贴 [config] 行>

### 结论
- [ ] 全部通过
- [ ] 有失败（列出条目 + 原始输出）

### 备注
<任何异常现象、截图路径、不确定的地方>
```

---

## 10. 已知的「不是 bug」

测试时如果遇到以下现象，**不要当缺陷报**：

| 现象 | 原因 |
|---|---|
| 托盘图标是**亮蓝**，与 exe 图标（深灰）不一致 | 有意为之：托盘是单色剪影，深灰在深色任务栏对比度 1.3:1 几乎不可见。见 `assets/icon-src/README.md` |
| 高权限（管理员）窗口无法拖动 | 非提权进程对 elevated 窗口 UIPI 不可见，安全限制 |
| UAC 弹窗（Secure Desktop）上手势无效 | 同上，钩子进不了安全桌面 |
| Alt 双击**标题栏**把窗口关了 | Windows 自带「Alt+双击标题栏 = 关闭」，系统行为，任何全局钩子都拦不住。测试请点**客户区** |
| 多显示器下最大化判定偶尔偏差 | 按「窗口所属显示器工作区」判定，简化实现，已知边界 |
| `cargo test` 里 `ring_concurrent_conservation` 偶发慢 | 10 万次并发压测，属正常 |

---

## 11. 出问题时的第一手资料

```bash
# 带调试日志跑（诊断手势问题必用）
ANYDRAG_DEBUG=1 target/release/anydrag-win.exe 2> diag.log &

# 关键日志行：
grep "\[config\]" diag.log   # 热重载是否发生
grep "\[engine\]" diag.log   # 每个事件的动作
grep "\[resolve\]" diag.log  # 命中测试逐候选跳过原因
grep "\[heartbeat\]" diag.log # 每秒消息计数，含 dropped= 与 state=
grep "\[watchdog\]" diag.log # 手势停滞强制收尾
```

`[heartbeat]` 里的 `dropped=` 持续增长 = 事件环溢出；`state=` 长时间停在
`Fallback`/`Resize` = 状态机卡住。这两个字段是排查手势类故障的第一手指标。

**先查实例数量**：`tasklist | grep -i anydrag-win`。多实例会让旧进程的钩子
继续跑，症状看起来像「改了没生效」。
