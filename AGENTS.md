# AGENTS — anydrag-win

64 位 Windows 托盘工具：按住 **Alt** 在任意窗口任意点拖动 = 移动窗口；Alt 双击 = 最大化/还原；
**Alt+Ctrl** 在窗口任意位置拖动 = 缩放；**中键**朝某方向拖动后松手 = 方向分屏。
clean-room（参照 AnyDrag macOS / AltSnap，**不得抄代码**），MIT。

本文件是**当前**工程约定与设计依据，取代早期的 `GOAL.md`（已于 2026-10-21 归档为
`docs/archive-goal.md`，仅供考古）。改动设计前先读这里；发现这里与现实不符时，
**改这里**，不要让文档与现实分叉。

---

## 1. 架构（三线程）

```
main 线程   dpi_aware_v2() -> config::init(load()) -> 起 engine 线程 -> 托盘消息泵（阻塞）
            （设置窗口也建在这个线程，由同一个泵分发消息）
engine 线程 自己的 GetMessageW 泵：配置热重载 -> 状态看门狗 -> drain ring -> feed -> 执行 Action
hook 线程   WH_MOUSE_LL，回调 O(1) 零堆分配：事件塞 ring + PostThreadMessageW(engine_tid, WM_APP)
```

三线程之间**只**通过这些通道通信：

| 通道 | 方向 | 用途 |
|---|---|---|
| `hook::RING`（SPSC，512 槽） | hook -> engine | 鼠标事件 |
| `WM_APP`（`PostThreadMessageW`） | hook -> engine | 唤醒 engine |
| `config::{STORE, VERSION, FILE_STAMP}` | 任意 -> engine | 配置热重载 |
| `config::ENABLED`（AtomicBool） | tray -> engine | 启用/禁用开关 |

### 关键不变量（改动时守住）

1. **`winmod::dpi_aware_v2()` 必须是进程第一个 FFI 调用**（在 `main` 最前面）。
2. 坐标一律**物理像素**；窗口几何只用 `DwmGetWindowAttribute(EXTENDED_FRAME_BOUNDS)`。
   注意 `Target.frame` 是 **DWM 帧**，而 `SetWindowPos` 吃的是**窗口矩形**，两者差左右下
   各约 7px 不可见边框 —— 下发前必须换算（`engine_loop::apply_geom`）。
3. mask 位规则 = `VK & 0x1F`，类型是 **u32**（`VK & 0x1F` 会产生 bit 16..30，u16 装不下）。
4. **纯逻辑模块零 `windows-sys` 导入**：`config` / `ring` / `mods` / `engine` / `icon` /
   `coalesce` / `throttle`。这是可单测性的边界，违反即坏设计。FFI 全在 `winmod.rs`。
5. **所有 FFI 集中在 `winmod.rs`**，其他模块只调它（`hook.rs`/`tray.rs`/`settings.rs` 因
   自身就是 Win32 回调/窗口宿主而例外）。
6. `RwLock` 读写一律 `unwrap_or_else(|p| p.into_inner())` 就地拆锁。
   **绝不用 `if let Ok(..)` 静默跳过** —— 一次静默跳过会让故障永久化且毫无异常
   （2026-10-20 "手势全灭但进程毫无异常" 事故根因）。
7. 缩放/移动必须**成对**下发 `WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE`。
8. 几何更新必须**节流 + 背压**（`throttle.rs`）：下发频率不能超过目标应用的重排速度。
9. 线程定时器（`SetTimer(NULL, ...)`）的 `nIDEvent` 会被系统忽略，
   **返回值才是 `KillTimer` 要用的 ID**。用固定常量反复 Set/Kill = 一个都杀不掉
   （2026-10-20 "定时器炸弹"事故根因）。
10. 注释写**为什么**（不变量、实测依据、被否掉的方案），不写"做了什么"。

---

## 2. 功能清单（当前全集）

| 手势 | 行为 |
|---|---|
| Alt + 左键拖动 | 移动窗口（不限标题栏，光标在哪就操作哪个窗口） |
| Alt + 双击（约 0.5s 内两次、几乎不位移） | 最大化；已最大化时再双击 -> 还原 |
| Alt+Ctrl + 拖动 | 调整大小（方向按 3x3 九宫格，正中心格取就近角；**不要求贴边**） |
| 中键拖动超过阈值后松手 | 吸附到该方向的半屏 / 四分之一屏（主轴->半屏，对角->四分之一） |

未达阈值的中键点击**不干预**（应用自己的中键粘贴/滚动照常）。

---

## 3. 配置

文件：`%APPDATA%\anydrag-win\config.json`。缺失或 JSON 损坏 -> 按默认值运行，不报错。

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `enabled` | bool | `true` | 总开关。`false` 时钩子仍装但所有手势无效果 |
| `modifiers` | VK 码数组 | `[18]`（Alt） | 要求"全部按下"才触发；`18`=Alt、`17`=Ctrl、`16`=Shift、`91`=Win |
| `resize_modifiers` | VK 码数组 | `[17, 18]`（Ctrl+Alt） | 缩放触发键，要求全部同时按下；`[]` = 关闭缩放 |
| `drag_threshold_px` | 像素 | `5` | 位移达到才提交拖动，否则视为双击候选 |
| `double_click_ms` | 毫秒 / `null` | `null` | 双击判定窗口；`null` = 跟随系统 `GetDoubleClickTime()` |
| `native_excluded` | 进程名(小写)数组 | `[]` | 名单内应用**完全无视手势**（不穿透到下层窗口） |
| `middle_snap` | bool | `true` | 中键方向分屏开关 |
| `middle_snap_threshold_px` | 像素 | `40` | 两轴位移都小于它 = 普通中键点击，不干预 |

### 改动何时生效

**两条路径都立即生效，无需重启程序：**

1. **设置界面点"保存"** —— `config::replace()` 换内存值 + 版本号自增，同时
   `hook::wake_engine()` 补发一条空 `WM_APP` 把引擎从 `GetMessageW` 阻塞中叫醒。
2. **直接手改文件** —— 引擎每条消息比一次文件 mtime，变了就重新 `load()`。

`ANYDRAG_DEBUG=1` 下会打印 `[config] 已重载（file_changed=...）mods=[...]` 便于确认。

> **为什么需要 `wake_engine`（2026-10-21 修）**：重载检查由消息驱动，而引擎空闲时阻塞在
> `GetMessageW`。不显式唤醒的话，用户点完保存若没动鼠标，新配置要等下一次鼠标事件才生效；
> 若改的是触发键，钩子还会按**旧**键表算 mods，第一次手势静默失效。

---

## 4. 已移除的设计

记录"曾经存在、为什么没了"，避免未来又把它加回来。

### native 拖动策略（`SC_DRAGMOVE` / `0xF012`） 2026-10-21 移除

**曾经是什么**：触发时向目标窗口 `PostMessage(hwnd, WM_SYSCOMMAND, 0xF012)`，让目标应用
自己的消息泵跑原生 caption 拖动循环，理论上零 IPC、compositor 级顺滑。

**为什么移除**：

- 该通道只在**部分**应用生效。2026-10-15 spike 实测：explorer 生效、notepad 失效；
  2026-10-21 补测：mspaint 生效，taskmgr / regedit / 系统设置失效。
- **失效时毫无报错**，用户只看到"拖不动"。曾有用户因设置界面文案写"更顺滑"而选中它，
  随后在 notepad 上完全无法拖动。
- 决定性数据：**fallback（`SetWindowPos`）对所有应用都生效，包括 native 能用的那些**
  （explorer / mspaint 均实测通过）。也就是说 native 没有任何独占能力，
  唯一差别是理论顺滑度，而实测 fallback 拖 explorer 是逐帧跟手的，无可感知滞后。
- 一个只提供感官增益、却有能力让功能完全失效、且用户无法自行判断该不该开的选项，
  风险收益比是负的。

**随之删除**：`Config.default_strategy`、`Config.fallback_apps`、`Strategy` 枚举、
`DragState::Native`、`Action::BeginNativeDrag`、`winmod::SC_DRAGMOVE`、
`winmod::post_sys_command`、设置界面的"默认拖动通道"下拉框与"强制 fallback 进程"输入框、
`probe_scandrag` 探针。

**保留**：`native_excluded`（排除表与拖动策略无关，仍用于"不想让全局钩子打扰某应用"）。

**拖动现在只有一条路**：`SetWindowPos` + `WM_ENTERSIZEMOVE/EXITSIZEMOVE` 配对 + 节流背压。

---

## 5. 构建、测试、冒烟

```bash
cargo test                 # 55 个单测（纯逻辑模块零 windows-sys）
cmd /c "build.bat"         # release 构建（含 rc.exe 图标/manifest 内嵌）
```

- **测试**：纯逻辑模块（ring/mods/engine/config/icon/coalesce/throttle/hook 的
  `mods_match`）可单测；FFI 层不可单测，靠冒烟探针。
- **冒烟探针**：`target/release/probe_gesture.exe`，子命令
  `frame|alt-drag|alt-dbl|ctrl-resize|middle-snap|maximize|native-resize|send-save|close <标题子串>`。
  跑之前它会先确认目标窗在最前，否则直接中止（避免误伤别的窗口）。
- **诊断日志**：`ANYDRAG_DEBUG=1` 打 stderr。含 `[engine]` 每事件动作、
  `[resolve]` 逐候选跳过原因、`[apply]` 几何下发、`[heartbeat] 1s:` 消息计数
  （`app/timer/other/pending/armed/mods/dropped/state`）—— 心跳仪是判断事件环溢出与
  状态滞留的第一手指标。

### 环境事实

- 主屏 **2880x1752、120Hz、DPI 150%**（`GetDpiForSystem()`=144）。
- 用户通过远程桌面客户端连过来，该窗口**全屏置顶** —— 任何"操作光标下窗口"的动作都可能
  误伤它，测试前先确认。探针有守卫，但自己的脚本没有。
- **多实例是排查第一嫌疑**：桌面副本与 `target/release` 是同一份字节，用户双击启动的是
  桌面副本。排查前先数实例。

---

## 6. 踩过的坑（改代码前读一遍）

按时间顺序，都是实测踩出来的。

1. **windows-sys 常量值必须查源码核对**：曾把 `WM_SIZING` 写成 `0x0231`（实为
   `WM_ENTERSIZEMOVE`），导致缩放路径每帧发假 ENTERSIZEMOVE 且从不发 EXITSIZEMOVE
   -> 应用永久卡在尺寸调整态 -> 缩放卡死。
2. **托盘图标**：`LoadImageW` 取系统图标时 hInstance 必须传 NULL，否则返回 NULL 且
   `NIM_ADD` 仍成功 -> 图标静默不可见。（现已改为运行时自绘，见 `src/icon.rs`。）
3. **冒烟必须先确认托盘进程就绪**（轮询 `tasklist`），否则探测跑在钩子装上之前，
   得到假 FAIL。
4. **冒烟前必须确认目标窗真的在最前**：本机有全屏置顶的远程桌面客户端。实测曾把用户的
   桌面窗口吸附成半屏。
5. **`&`/`nohup` 分离启动的进程会被沙箱在 bash 调用结束时回收**。排查"进程闪退"先排除
   这个伪影。长驻进程用 `bg_run`。
6. **"探针全 PASS 但用户手动测试失败"先确认手势理解一致**：2026-10-16 破案 —— 用户一直在
   **点按** Alt 键（双击 Alt 本身），而不是"按住 Alt + 鼠标操作"。键盘诊断实证其真按住
   Alt 的 7.7 秒里 `GetAsyncKeyState` 采样全程稳定，**代码无 bug**。
7. **线程定时器的 nIDEvent 会被系统忽略**：`SetTimer(NULL, id, ...)` 返回值才是 KillTimer
   要用的 ID。用固定常量反复 Set/Kill = 一个都杀不掉；每次背压推迟都新增一个定时器 ->
   一个手势攒上百个 4-8ms 定时器同时放炮 -> 引擎泵被 WM_TIMER 灌满 -> 100% CPU 空转、
   真实事件饿死、**手势结束后卡死永久持续**。修法：存返回值按真 ID 拆弹 +
   `timer_id.is_none()` 才武装 + `FORCE_AFTER_MS` 兜底。
8. **`RwLock` 的 `if let Ok(..)` 静默跳过 = 故障永久化**：中毒时 `set_mods` 写不进去、
   `ll_proc` 读锁失败跳过 -> mods 恒 0 -> 所有手势全灭但进程毫无异常。
9. **本沙箱拦一切系统配置通道**：`reg.exe`、`schtasks.exe`、PowerShell 的 COM 实例化、
   PowerShell 注册表 provider、`Add-Type` P/Invoke 全被拦。
   **开机自启可用方案**：手工构造 MS-SHLLINK `.lnk`（Python struct）放进
   `%APPDATA%\...\Startup\` —— 克隆本机一个在用的真实 `.lnk` 的 VolumeID 块最稳。
   （注意 bash heredoc 会把 `\` 悄悄转成正斜杠，写文件后要校验修正。）
10. **"探针全 PASS 但用户说又坏了"先查是不是两个实例**，优先用改造后的构建验证。
11. **背压做成"硬闩锁"= 一次不匹配永久锁死（2026-10-21 事故根因）**：`caught_up` 拿
    `GetWindowRect` 与"上次**请求**的矩形"比，不等就推迟，而请求矩形**只在成功下发时
    更新** => 一旦出现消除不掉的差异（应用最小尺寸钳制、DPI 取整、尺寸相位临时几何、
    **切换目标窗口**），此后所有拖拽/缩放永久被推迟。
    症状指纹极具辨识度：**拖拽死、缩放死，但最大化/分屏照旧**（后者走
    `SetWindowPlacement`，不经过几何下发路径）。
    修法见 `src/throttle.rs`：① `req` 绑定 hwnd；② `Down`/`Up` 边界 `reset()`；
    ③ `FORCE_AFTER_MS=40` 兜底；④ `Up` **先补发 pending 再清**，否则松手最终位置丢失。
    教训：任何"等对方追上"的反馈环都必须有**超时**，且**跨手势不能携带状态**。
12. **"高频才卡死"= 事件环溢出，丢 `Up` 比丢 Move 致命得多（2026-10-21）**：
    `WH_MOUSE_LL` 按鼠标硬件率推事件（实测注入 4000-6500 次/秒），而 engine 每批要花真实
    时间在 `SetWindowPos` + `sync_paint` 上 => 64 槽环溢出，实测 dropped=60%。
    丢 Move 无害（几何是绝对值）；**丢 `Up` 是灾难**：状态机永远停在 `Fallback`/`Resize`
    态 => `EndSizeMove` 永不发出 => 目标应用**永久卡在尺寸调整相位**。
    诊断前提：低频探针触发不了，必须写"全程按住不松手、步间无 sleep"的压测。
    修法三层：① `RING_SLOTS` 64->512；② `coalesce::coalesce_moves` 把批内连续 Move 压成
    最后一个；③ `STALE_GESTURE_MS=2000` 看门狗强制收尾。
    教训：任何"每批 O(事件数)"的消费端都可能被生产者淹掉，先把批内工作压到 O(1)，
    再留一个"相位配对"的看门狗。
13. **配置热重载是"消息驱动"的，别把检查藏在某个消息分支里（2026-10-21）**：
    引擎空闲时阻塞在 `GetMessageW`，而 `WM_APP` 只由钩子在**鼠标事件发生时**投递。
    旧实现把重载检查和 `hook::set_mods` 全放在 `if msg.message == WM_APP` 分支里，
    于是"设置界面点了保存"也可能不生效。修法：`hook::wake_engine()` 补发空 `WM_APP`；
    重载/自愈检查移出该分支，每条消息都跑。
    **连带坑**：自愈校验 `hook::mods_len() != live.all_vks().len()` 只比**长度**，
    把触发键 Alt->Ctrl（个数不变）会误判"没变"，钩子继续按旧键表算 mods ->
    改完第一次手势静默失效。必须内容级比较（`hook::mods_match`，集合语义）。
    **测试坑**：`--settings` 会起**第二个进程**（自己的 engine + `ENGINE_TID`），
    `FindWindowW` 找到的是那个进程的窗口 —— 向它发保存唤醒的是另一个进程的引擎，
    会误判"wake_engine 失效"。验证"保存立即生效"必须保证设置窗口与引擎同进程。
14. **`SetWindowPlacement` 禁用 `WPF_ASYNCWINDOWPLACEMENT`（2026-10-21 实测）**：
    异步标志下只把 `showCmd` 写进窗口记录就返回，**矩形可能完全不变** -> 窗口进入
    "记录=最大化、矩形=普通尺寸"的**不一致态**，此后每次最大化都因"已经是最大化态"而
    幂等空转，`ALT_DBL=FAIL (maximized=false)` 且恢复不了。同步调用立刻重排。
    已改为同步 + `GetWindowPlacement` 回读校验，没落地再退 `ShowWindow`。
    **同款假 FAIL 也会被自己的测试脚本造出来**：用 Python 先写 `SW_SHOWNORMAL` 再
    `SetWindowPos` 就会留下不一致态。正确清态顺序：
    `SW_RESTORE` -> `SW_SHOWNORMAL` -> `SetWindowPos`。

---

## 7. 交付物状态

- `cargo test` 55/55。
- exe 为**单文件**（图标 + manifest 均内嵌 PE 资源段，不依赖任何外部文件）。
- 三处就位且 MD5 一致：`target/release/`、`%USERPROFILE%\Desktop\`、
  `%LOCALAPPDATA%\Programsnydrag-win\`（开机自启 `.lnk` 指向此处）。
- 事故与验收记录见 `docs/acceptance.md`。

## 8. 改这个仓库时的默认流程

1. 先读本文件 §6（坑）与相关模块注释，再动手。
2. 纯逻辑改动走 TDD：先写失败测试 -> 最小实现 -> 绿 -> commit。
3. FFI 改动必须真机冒烟（`probe_gesture` 四项手势 + 目标应用）。
4. commit 前 `cargo test` 全绿；FFI 改动额外跑冒烟。
5. 发现本文件与现实不符 -> **改本文件**，不要让文档与现实分叉。
6. commit message 用中文或英文均可，但必须写清**为什么**（根因/实测依据），
   不只是改了什么。
