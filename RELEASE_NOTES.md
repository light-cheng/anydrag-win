A small Windows tray utility: hold **Alt** and drag **anywhere** on any window to move it —
the title bar is not required.

## Gestures

| Gesture | Effect |
| --- | --- |
| **Alt + left-drag** | Move the window under the cursor |
| **Alt + double-click** | Maximize; double-click again to restore |
| **Alt + Ctrl + drag** | Resize from any point — the direction is inferred, no edge aiming |
| **Middle-drag**, then release | Snap to that half or quarter of the screen |

## Highlights

- **Single exe, no installer, no runtime dependencies** — the icon and the DPI manifest are
  embedded in the PE resource section.
- **Settings window** with a **Chinese / English** switch (applies instantly, Cancel rolls
  back) and a dark theme that follows the system.
- **Save applies immediately** — the config hot-reloads, no restart.
- Exclude specific processes (`native_excluded`); middle-drag snapping can be turned off.
- **Keyboard accessible**: Tab / Shift+Tab move focus, Space toggles a checkbox.

## Install

Download `anydrag-win.exe` and double-click it. It lives in the notification area. The
interface language follows your system language on first run.

Config file: `%APPDATA%\anydrag-win\config.json` (reachable from the tray menu).

## Known limits

- Windows 10/11 x64 only.
- Under high-contrast themes the custom-drawn controls in the settings window fall back to
  the native look (no custom colours are applied).
- 120 / 192 DPI have no on-device screenshots yet — the geometry is covered by unit tests
  across 96/120/144/168/192/240.
- The binary is **not code-signed**, so SmartScreen may warn on first run. Choose
  "More info → Run anyway".

## 中文说明

[中文 README](https://github.com/light-cheng/anydrag-win/blob/main/README.zh-CN.md) ·
[中文发布说明](https://github.com/light-cheng/anydrag-win/blob/main/RELEASE_NOTES.zh-CN.md)

MIT License.
