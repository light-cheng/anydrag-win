Windows 托盘小工具：按住 Alt 在**任意窗口的任意位置**拖动即可移动窗口（不限于标题栏）。

## 手势

| 手势 | 效果 |
|---|---|
| Alt + 左键拖动 | 移动光标下的窗口 |
| Alt + 双击 | 最大化；已最大化时再双击 → 还原 |
| Alt + Ctrl 拖动 | 调整大小（任意位置，方向自动判定，不用瞄边缘） |
| 中键朝某方向拖动后松手 | 吸附到该方向的半屏 / 四分之一屏 |

## 特性

- **单个 exe，无安装、无运行时依赖**（图标与 DPI 清单内嵌在 PE 资源段）
- **设置界面**：中英双语切换（点了立即生效、取消可回滚）、深色主题跟随系统
- **保存即生效**，无需重启（配置热重载）
- 可排除特定进程（`native_excluded`），中键分屏可关
- 键盘可用：Tab / Shift+Tab 移动焦点，空格切换复选框

## 安装

下载 `anydrag-win.exe` 双击运行即可，常驻托盘。首次运行按系统语言选择界面语言。

配置文件：`%APPDATA%\anydrag-win\config.json`（托盘「配置文件…」可直达）。

## 已知限制

- 仅 Windows 10/11 x64
- 高对比度主题下设置界面的自绘控件会退回原生外观（不做自定义配色）
- 120 / 192 DPI 未经真机截图验证（几何逻辑已按 96/120/144/168/192/240 六档单测覆盖）
- 未做代码签名，首次运行可能触发 SmartScreen 提示，选「更多信息 → 仍要运行」即可

## English

[English README](https://github.com/light-cheng/anydrag-win/blob/main/README.md) ·
[English release notes](https://github.com/light-cheng/anydrag-win/blob/main/RELEASE_NOTES.md)

MIT License.
