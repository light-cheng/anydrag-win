# T2.3 Spike 记录：跨进程 `SC_DRAGMOVE (0xF012)` 原生拖动可行性

- **日期**：2026-10-15
- **环境**：Windows 10.0.26300.9711（Windows 11 26300 系），交互桌面会话（用户 Service01），
  主屏 DPI 100%。
- **探针**：`src/bin/probe_scandrag.rs`（release：`target/release/probe_scandrag.exe`）。
  复跑：`cmd /c "…\vc_cargo.bat" build --release --bin probe_scandrag`，然后直接运行
  `./target/release/probe_scandrag.exe`（会真实移动鼠标约 15–20 s，并开/关 notepad、
  explorer、msedge 测试窗口；运行前请离开桌面或注意光标移动）。

## 1. 目的与判定规则

按 GOAL T2.3：验证“远程 PostMessage(hwnd, WM_SYSCOMMAND, 0xF012) 是否能让第三方
窗口跟随鼠标移动”（即应用自己的 caption 拖动循环能否被外部消息触发）。
**判定规则（GOAL 原文）**：notepad 与 explorer **都**判“生效”→ 默认策略 `native`；
**否则** → `fallback`。msedge 仅在安装时附加测试，不参与判定。

“生效”标准：命令移动 +80 px 后，DWM 扩展边框（`DWMWA_EXTENDED_FRAME_BOUNDS`）
位移 `|dx| ≥ 40 px`（阈值 `WORKS_THRESHOLD`）。

## 2. 测试方法（双变体）

| 变体 | 序列 | 含义 |
|---|---|---|
| **A 无键** | 鼠标移到客户区中心 → `PostMessage 0xF012` → 50 ms → 鼠标 +80 px → 50 ms → 读几何；补一次 LDOWN+LUP 收尾（解除可能挂起的原生循环） | GOAL T2.3 原文序列 |
| **B 按键** | 鼠标到中心 → `LDOWN` → `PostMessage 0xF012` → 180 ms → 鼠标 +80 px → 80 ms → `LUP` → 读几何 | **产品保真**：引擎在 Down 时刻发 0xF012，用户此刻必按住按键 |

几何测量前均先 `SetForegroundWindow` + 150 ms 稳定；探针自身首个 FFI 调用为
`SetProcessDpiAwarenessContext(PerMonitorV2)`（GOAL 不变量 1）。

## 3. 结果（2026-10-15 实测）

| 应用 | 启动 | A 无键 dxa | B 按键 dxb | 判定 |
|---|---|---|---|---|
| notepad | `notepad.exe`（pid 认窗口，失败时标题兜底） | 0 | 0 | **失效** |
| explorer | `explorer.exe C:\Windows`（`CabinetWClass`+标题 "Windows"） | 0 | **+160** | **生效** |
| msedge | `msedge --window-size=800,600 about:blank` | — | — | N/A：本机未安装 msedge（`CreateProcessW` 返回 0，`GetLastError=2` `ERROR_FILE_NOT_FOUND`） |

机器可读行（本文件的权威数据来源）：

```
RESULT_JSON={"notepad":"失效 (A无键 dxa=0 / B按键 dxb=0)","explorer":"生效 (A无键 dxa=0 / B按键 dxb=160)","msedge":"未找到窗口/拖测失败","decision":"fallback"}
```

### 判定

`notepad_works=false`、`explorer_works=true` ⇒ 按 GOAL 规则（需两者均生效才可用
native）⇒ **默认策略定稿为 `"fallback"`**（`src/config.rs` 的 `DEFAULT_STRATEGY`）。

## 4. 观察与坑（实测记录）

1. **本机 `CreateProcessW` 必须给非 NULL 的 `STARTUPINFOW`**（`cb = sizeof`）。
   全 NULL 参数会返回 `FALSE` + `GetLastError=87`（`ERROR_INVALID_PARAMETER`）。
   用 Python ctypes 逐参隔离验证（诊断脚本
   `~\AppData\Local\hermes\cache\scratch\probe_spawn_diag3.py`）：
   全 NULL→87；带有效 STARTUPINFO→成功。文档（MSDN）声称 `lpStartupInfo` 可为 NULL，
   但本机行为不是——探针已按实测修复并注释在 `spawn_wide`。
2. **notepad 的 pid 不认自己的窗口**：`CreateProcessW("notepad.exe")` 成功的 pid 名下
   0 个可见顶层窗口（Win11 26300 系 notepad 为打包应用，窗口由宿主承载）。
   探针因此有“pid 匹配失败 → 标题兜底（'notepad'）”的调试路径，
   并在失败时打印候选计数。notepad 的 0xF012 失效结论基于真实 notepad 窗口
   （标题兜底命中），依然有效。
3. **explorer B 变体 dxb=160 ≈ 2×80**：命令 +80 px，实测位移 160 px。可能因素：
   explorer caption 拖动按物理像素积分、DWM 边框含阴影补偿、或 180 ms 等待窗口
   内多次采样。对判定无影响（阈值 40），数值如实记录。
4. **msedge 缺席**：`ERROR_FILE_NOT_FOUND(2)`。GOAL 允许（“若装了 Edge 就测”）。

## 5. 对实现的落地影响

- `config.rs`：`DEFAULT_STRATEGY = "fallback"`（spike 定稿，见本文件 §3）。
- `engine_loop` 执行 `Native` 动作时：对 notepad 这类“0xF012 不响应”的应用，
  引擎应按 GOAL 的 `native_excluded` 名单走 fallback；本 spike 建议默认把
  `notepad` 放入排除名单的候选（最终名单在 T3.4/T4.2 依据 GOAL §6 表定稿，
  此处仅记录实测证据）。
- 探针二变体（B 按键）证明：**explorer 能跟随**，说明 fallback 之外 native 通道
  并非全灭——T4.2 验收时可附本记录作对照证据。
