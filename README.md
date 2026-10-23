![anydrag-win](assets/hero.png)

**English** · [中文](README.zh-CN.md)

A Windows tray utility: hold **Alt** and drag **anywhere** on any window to move it —
no title bar required. **Alt + double-click** maximizes/restores. **Alt + Ctrl + drag**
resizes from anywhere, with the direction inferred automatically. **Middle-drag** in a
direction snaps the window to that half or quarter of the screen.

Interaction modelled on AnyDrag (macOS) and AltSnap (Windows); the code is a clean-room
implementation and shares nothing with either. MIT licensed.

Runs as a tray icon. No installer, no runtime dependencies — a single ~420 KB `.exe`.

## Quick start

```bat
:: Requires a Rust toolchain + MSVC x64 (Visual Studio 2022 BuildTools or Community)
cargo build --release

:: Run the release build
target\release\anydrag-win.exe
```

> `cargo build --release` is all you need. The icon and the DPI manifest are embedded by
> `build.rs`, which locates `rc.exe` and the Windows SDK include directories by itself —
> **no `vcvarsall.bat` required**. `build.bat` is only a convenience wrapper for machines
> with a fixed BuildTools install; if you have the Community edition or a custom path,
> use the cargo command above.

The program icon (two offset rounded squares) appears in the notification area.
There is no main window: everything is gestures plus the tray menu.

## Usage

### Gestures (work on **any** window — whatever the cursor is over)

| Gesture | Effect |
| --- | --- |
| Hold **Alt + left button**, drag from the cursor | Move the window under the cursor (the window moves, not its contents) |
| **Alt + double-click** (two clicks within the system double-click time, no movement) | **Maximize** that window |
| **Alt + double-click** on an already maximized window | **Restore** it to its previous size and position |
| Hold **Alt + Ctrl**, drag **anywhere** on the window | Resize it (direction inferred from cursor position — **no need to aim at an edge**) |
| Hold **middle button**, drag in a direction, release | **Snap** the window: left/right → half screen; up/down → half screen; diagonal → quarter screen |

- The target is fixed at the moment the button goes down, so you can drag a window past
  the screen edge and pull it back — the scenario title-bar dragging cannot handle.
- ⚠️ **Alt + double-click on the content area, not the title bar.** Windows has a built-in
  "Alt + double-click title bar = close window" shortcut which fires first and closes the
  window. That is OS behaviour; no global hook can intercept it.
- **Alt + Ctrl resizing does not require hitting an edge.** Think of the window as a 3×3
  grid: grab a left/right column to resize horizontally, a top/bottom row to resize
  vertically, the exact centre to resize from the nearest corner. Any grab point works.

### How dragging is implemented (one paragraph)

Every application takes the same path: the engine computes the target rectangle from the
cursor delta, applies it with `SetWindowPos`, and brackets the sequence with
`WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE` so the target defers reflow until release. Throttling
and back-pressure keep the update rate below what the target can reflow.

An earlier second strategy (remote `SC_DRAGMOVE`, letting the target run its own native
drag loop) was **removed on 2026-10-21**: it only worked for a subset of apps
(explorer, mspaint) and silently did nothing for the rest, while `SetWindowPos` works
everywhere. See `AGENTS.md` §4.

- Drag threshold is 5 px; the double-click window follows the system
  (`GetDoubleClickTime`) by default. Both are configurable.
- **Middle-drag never interferes with a plain middle click**: snapping only happens once
  the drag exceeds the threshold (40 px by default). Release without dragging and the
  click is passed through untouched. Can be disabled in settings.

### Tray menu (left or right click the tray icon)

| Item | Behaviour |
| --- | --- |
| **Enabled / Disabled** | Toggles gestures immediately, **writes the config file and swaps the icon**: when disabled the icon fades and the tooltip says so; toggle back any time |
| **Settings…** | Opens the **graphical settings window** (trigger keys, thresholds, exclusion list, middle-drag snapping, UI language). Save applies **immediately, no restart** |
| **Usage…** | Shows the gesture reference and the config file path |
| **Config file…** | Opens `config.json` in the default editor (writes a template first if missing) |
| **Quit** | Removes the tray icon and exits (the mouse hook is uninstalled with the process) |

The tray icon is the **program icon** (two offset rounded squares) — the same image as the
exe, the taskbar and Alt+Tab, so it is obvious which program it belongs to. State is
expressed through **opacity**: full colour = gestures active, faded = disabled.

> Why opacity instead of a colour change: the program icon is multicolour (white fill,
> charcoal outline, mid-grey back layer), and no single tint can safely express "disabled"
> — recolouring would break the consistency with the exe icon that motivated using the
> program icon in the first place. Reduced opacity works for any palette, and
> "faded = inactive" reads more directly than "greyed out".

The icon is **embedded in the exe's PE resource section** (`assets/app.rc` + `build.rs`),
so the exe is a **single file** — no adjacent `.ico` or `.manifest` needed.

### Settings window

Open it from the tray menu (**Settings…**) or with `anydrag-win.exe --settings`. It
exposes: the drag/double-click trigger keys, the resize trigger keys, the drag threshold,
the double-click interval, middle-drag snapping and its threshold, and the list of
processes that should **ignore all gestures** (pick them from a window list with
**Pick processes…**, or type the exe name).

**Save applies immediately** — the config hot-reloads into the engine thread and the
trigger keys are pushed to the mouse hook; no restart.

**UI language**: a **中文 / English** segmented control in the top right switches the
interface **instantly** (no window rebuild, no loss of what you typed); Cancel rolls back.
First run follows the system language; afterwards your choice is remembered in
`config.json` (`lang`; empty = keep following the system). The **tray menu, tooltip and
the Usage dialog are bilingual too**.

**Dark theme** follows the system "app mode" and **repaints immediately** when it changes
(no restart). Colours and radii follow three verifiable principles: neutrals carry a very
slight blue cast (pure grey looks dirty), borders are translucent black/white (solid grey
borders look heavy on any background), and radii come in three tiers by role (4/8/12 px).
All values satisfy the Windows 11 type scale and WCAG 2.1 contrast requirements — the
latter enforced by unit tests.

**Keyboard accessible**: Tab / Shift+Tab move focus, Space toggles a checkbox, Enter
triggers Save, Esc cancels.

| Chinese · light | English · dark |
|---|---|
| ![Chinese light](docs/spec-assets/settings-after-zh-light.png) | ![English dark](docs/spec-assets/settings-after-en-dark.png) |

## Configuration

File: `%APPDATA%\anydrag-win\config.json` (the tray menu's **Config file…** takes you
there). **Changes apply immediately, no restart** — both saving from the settings window
and editing the file by hand go through hot reload. A missing or malformed file falls
back to defaults without an error.

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Master switch. When `false` the hook stays installed but no gesture acts |
| `modifiers` | array of VK codes | `[18]` (Alt) | All of these must be held. `18`=Alt, `17`=Ctrl, `16`=Shift, `91`=Win |
| `resize_modifiers` | array of VK codes | `[17, 18]` (Ctrl+Alt) | Resize trigger; all must be held simultaneously. Works from any point in the window (direction from the 3×3 grid / nearest corner). `[]` disables resizing |
| `drag_threshold_px` | px | `5` | Movement needed before a drag commits; below it the press stays a double-click candidate |
| `double_click_ms` | ms / `null` | `null` (system) | Double-click window; set explicitly to override the system value |
| `native_excluded` | array of process names (lowercase) | `[]` | Listed processes **ignore all gestures** (e.g. apps you don't want a global hook to touch). **Pick processes…** in the settings window lets you choose from currently visible windows |
| `middle_snap` | bool | `true` | Middle-drag snapping. `false` makes the middle button entirely hands-off |
| `middle_snap_threshold_px` | px | `40` | Middle-drag must exceed this before a snap direction is chosen; releasing without dragging is a plain middle click |
| `lang` | string | `""` | UI language: `""` follows the system, `"zh"` / `"en"` pins it |

Process name examples: `notepad.exe`, `explorer.exe`, `msedge.exe`, `code.exe`.

Minimal config (omitted fields take defaults):

```json
{
  "modifiers": [18],
  "native_excluded": ["game.exe"]
}
```

### When config changes take effect

There are two paths, and **neither needs a restart**:

1. **Save in the settings window** — goes through `config::replace()`, bumping the version;
   the save also posts an empty `WM_APP` to the engine thread to wake it out of its
   `GetMessageW` block, so it applies **without having to nudge the mouse first**.
2. **Edit the file by hand** — the engine compares the file mtime on every batch and
   reloads when it changes (added 2026-10-21). Before that fix, hand edits did nothing:
   the engine only trusted the version counter and kept using the in-memory snapshot from
   startup, so it looked like "I changed it, why is it still wrong?" until you restarted.

   With `ANYDRAG_DEBUG=1` you get `[config] reloaded (file_changed=true) mods=[...]`.

**Careful when changing trigger keys**: the engine self-heals by comparing the hook's
watch list against the config, and that comparison is **content-level**
(`hook::mods_match`). Swapping Alt for Ctrl — same key count — still refreshes the hook's
key table. Before the fix it compared lengths only, concluded "unchanged", and the first
gesture after the edit silently failed.

## Known limits (MVP scope)

- **Elevated windows are invisible to the tool**: windows owned by processes running as
  administrator (e.g. an elevated cmd/explorer) cannot have their process name read, so
  they are skipped (a security boundary, not a bug).
- **Secure Desktop is out of reach**: the hook does not run on UAC prompts and similar.
- **Full-screen games: add them to `native_excluded`.** A global mouse hook feeds events
  to the game process; some exclusive-fullscreen or anti-cheat setups may ignore or flag
  that. Excluding them is the safe choice.
- **Multiple monitors**: coordinates are physical pixels throughout (Per-Monitor v2 DPI
  aware) and dragging across monitors works; maximize decisions use the work area of the
  monitor the window is on.
- **Windows 11 packaged apps** (Notepad and friends): their host helper window hijacks hit
  testing. The tool works around it with a Z-order candidate scan (verified: it neither
  moves nor targets the wrong window).
- Config is **fully hot-reloadable**: settings-window saves and hand edits both apply
  immediately (hand edits were completed on 2026-10-21).
- **Single instance**: launching a second copy only installs a second hook, with no benefit.

## Development

> **Read [`AGENTS.md`](AGENTS.md) before changing this repo.** It holds the architecture
> invariants, the config hot-reload mechanics, designs that were removed (and why), and
> 26 pitfalls found the hard way — timer bombs, back-pressure latch deadlocks, ring
> overflow, async placement inconsistency, GDI's lack of alpha, `IsDialogMessageW` not
> completing the Tab cycle, and more.

```bash
cargo test                  # 92 unit tests (pure-logic modules, zero windows-sys)
cargo build --release       # release build (icon/manifest embedded by build.rs)
```

**Full test procedure in [`docs/testing.md`](docs/testing.md)** — environment setup, unit
tests, on-device gesture smoke tests (four gestures plus probe criteria), real-world
regression, and a result template you can hand to an agent on another machine.

Smoke driver: `target/release/probe_gesture.exe`
(`frame|alt-drag|alt-dbl|ctrl-resize|middle-snap|maximize|send-save|close`). It is for
automated acceptance, not for users; it aborts unless the target window is foreground, to
avoid disturbing other windows.

Diagnostics: `ANYDRAG_DEBUG=1` prints to stderr (still visible when launched from cmd under
the GUI subsystem). The heartbeat line carries `dropped=` and `state=`, the first things to
look at for ring overflow or a stuck state.

### Icons

- **Exe icon**: two offset rounded squares (the back one an outlined grey, the front one
  white with a charcoal outline) — "one window moved onto another". It is
  **generated by a script, not hand-tuned binary**: `python3 assets/icon-src/build_ico.py`
  reproducibly rebuilds `assets/anydrag-win.ico` (each of the 7 sizes rasterized
  independently, which is why small sizes stay crisp). Design notes in
  `assets/icon-src/README.md`.
- **Tray icon**: the **same program icon** (`src/icon.rs` loads it from the PE resource
  section at the tray size). State is opacity (full colour = enabled, 45% = disabled).
  The multi-size `.ico` lets Windows pick the matching frame instead of downscaling 32 px
  to 16 px (which blurs into grey edges).
- **README hero**: `assets/hero.png`, also script-generated (`assets/icon-src/hero.py`).
  Text is rendered with real fonts rather than generated by an image model (which turns
  text into garbage).
- **Visual style manifest**: embedded into the PE resource section (`assets/app.rc`).
  `build.rs` compiles it with the Windows SDK's `rc.exe` and passes the result to the
  linker via `cargo:rustc-link-arg` — **no Rust build dependency** (no
  winres/embed-resource). `build.rs` scans the SDK include directories itself and does
  **not** rely on the `INCLUDE` variable set by `vcvarsall.bat`; otherwise a plain
  `cargo build` fails to find `winres.h`, silently skips the resources, and still
  "succeeds" with an exe that has neither icon nor manifest. If `rc.exe` really cannot be
  found the build prints a warning and skips (non-blocking), but such an exe is **not fit
  for distribution**.

  Hence the **exe is a single distributable file** — no adjacent
  `anydrag-win.exe.manifest`. The manifest is embedded as `RT_MANIFEST` / ID=1 (process
  level) and **must contain `PerMonitorV2`**: without it the process is virtualized at
  96 DPI under 150% scaling and the settings window is clipped in half.

## License

MIT — see [`LICENSE`](LICENSE).

The interaction design references AnyDrag (macOS) and AltSnap (Windows), but the code is a
**clean-room independent implementation**; none of their source was copied.
