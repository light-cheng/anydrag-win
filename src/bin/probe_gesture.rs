//! 临时 T3.4 冒烟驱动（GOAL T3.4 手动冒烟项 2/3/4/5 的自动化替代；跑完即删）。
//!
//! 模拟"用户"：在目标窗口客户区做 Alt+拖 / Alt+双击（SendInput），
//! DWM 几何前后差判定窗口是否跟手。被测对象是**运行中的 anydrag-win.exe**。
//! 子命令：
//!   frame <title-substr>     打印 DWM 几何 + 主屏工作区
//!   alt-drag <title-substr>  Alt 按住 + 客户区中心左键拖 +80px（分段 4 步）
//!   alt-dbl <title-substr>   Alt 双击（两次点按无位移，120ms 间隔）

use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetSystemMetrics, GetWindowPlacement, GetWindowTextW, IsWindowVisible,
    PostMessageW, SetForegroundWindow, SetWindowPlacement, SetWindowPos, WindowFromPoint, GA_ROOT,
    HWND_NOTOPMOST, HWND_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_MAXIMIZE, WINDOWPLACEMENT, WM_CLOSE, WM_COMMAND,
};

const VK_MENU: u16 = 0x12;
/// resize 触发键（默认配置 = Alt+Ctrl 组合，见 config.rs DEFAULT_RESIZE_MODS）。
const VK_CONTROL: u16 = 0x11;
const DRAG_PX: i32 = 80;
/// resize 探针拖拽量（px）：右移 +80 ⇒ 宽度 +80，PASS 判据 ≥ 40（半量容差）。
const RZ_DRAG_PX: i32 = 80;

fn frame_of(hwnd: HWND) -> Option<[i32; 4]> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        ) == 0
        {
            Some([r.left, r.top, r.right, r.bottom])
        } else {
            None
        }
    }
}

fn work_area() -> [i32; 4] {
    unsafe {
        let mon = MonitorFromWindow(0 as HWND, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        [
            mi.rcWork.left,
            mi.rcWork.top,
            mi.rcWork.right,
            mi.rcWork.bottom,
        ]
    }
}

/// 四坐标均在 ±2px 内（与 winmod::is_maximized_simplified 同容差：Win11 DWM
/// 最大化帧有 1px 取整偏差，严格 == 不可用）。
fn within2(a: &[i32; 4], b: &[i32; 4]) -> bool {
    [0, 1, 2, 3].iter().all(|&i| (a[i] - b[i]).abs() <= 2)
}

/// 标题包含（忽略大小写）的顶层窗口（取最后一个命中，覆盖多个同名窗口）。
fn find_window(sub: &str) -> Option<HWND> {
    use std::cell::RefCell;
    // 泄漏到堆上拿到 &'static RefCell，回调 lparam 直接指向它（类型不混淆）。
    let state: &'static RefCell<(String, isize)> =
        Box::leak(Box::new(RefCell::new((sub.to_ascii_lowercase(), 0))));
    unsafe extern "system" fn cb(hwnd: HWND, lparam: isize) -> i32 {
        let st = lparam as *const RefCell<(String, isize)>;
        // 过滤：Win11 打包应用（如 notepad）名下有不可见的宿主顶层窗（1px 帧）。
        // 只认"可见 + DWM 帧 > 100px"的真实窗口，否则测到隐形宿主窗（dx 恒 0）。
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let f = frame_of(hwnd);
        let Some(fb) = f else { return 1 };
        if (fb[2] - fb[0]) < 100 || (fb[3] - fb[1]) < 100 {
            return 1; // 帧过小 = 宿主/消息窗
        }
        let mut t = [0u16; 512];
        let n = GetWindowTextW(hwnd, t.as_mut_ptr(), 512);
        if n > 0 {
            let title: String = String::from_utf16_lossy(&t[..n as usize]);
            let want = (*st).borrow().0.clone();
            if title.to_ascii_lowercase().contains(&want) {
                (*st).borrow_mut().1 = hwnd as isize;
            }
        }
        1 // TRUE：继续枚举
    }
    unsafe {
        let _ = EnumWindows(Some(cb), state as *const _ as isize);
    }
    let v = state.borrow().1;
    (v != 0).then(|| v as HWND)
}

/// 把目标窗抬到 Z 序最前再测，并**确认它确实在最前**。返回 false = 被别的窗口遮挡。
///
/// **为什么必须有这道守卫**：引擎的命中测试解析的是"光标下最顶层的可拖窗口"，而不是
/// 本驱动以为的那个窗口。2026-10-16 实测教训——中键分屏冒烟时，光标下的顶层窗口是
/// 全屏的远程桌面客户端 `GameViewer.exe`，结果**把用户的远程桌面窗口吸附成了半屏**。
/// 宁可中止，不可误伤：任何手势驱动前都先确认目标窗真的在最上面。
fn raise_target(hwnd: HWND) -> bool {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        SetForegroundWindow(hwnd);
    }
    std::thread::sleep(Duration::from_millis(250));
    // 用窗口中心做代理点校验：该点下的根窗口必须是目标窗本身。
    let Some(f) = frame_of(hwnd) else { return false };
    let (cx, cy) = ((f[0] + f[2]) / 2, (f[1] + f[3]) / 2);
    unsafe {
        let under = WindowFromPoint(POINT { x: cx, y: cy });
        if under.is_null() {
            return false;
        }
        GetAncestor(under, GA_ROOT) == GetAncestor(hwnd, GA_ROOT)
    }
}

/// 测完恢复非置顶（不改动用户窗口的置顶状态）；窗口已关（close 子命令）时静默失败。
fn restore_z(hwnd: HWND) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

fn mouse_move_abs(x: i32, y: i32) {
    unsafe {
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        let nx = (x as u32) * 65535 / (vw as u32 - 1);
        let ny = (y as u32) * 65535 / (vh as u32 - 1);
        mouse_input(nx as i32, ny as i32, MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE, 0);
    }
}

fn mouse_button(down: bool) {
    let flags = if down {
        MOUSEEVENTF_LEFTDOWN
    } else {
        MOUSEEVENTF_LEFTUP
    };
    unsafe {
        mouse_input(0, 0, flags, 0);
    }
}

/// 中键按下/抬起（中键方向分屏冒烟用）。
fn mouse_middle(down: bool) {
    let flags = if down {
        MOUSEEVENTF_MIDDLEDOWN
    } else {
        MOUSEEVENTF_MIDDLEUP
    };
    unsafe {
        mouse_input(0, 0, flags, 0);
    }
}

unsafe fn mouse_input(dx: i32, dy: i32, flags: u32, vk: u16) {
    let inp = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: windows_sys::Win32::UI::Input::KeyboardAndMouse::MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    SendInput(1, &inp as *const INPUT as *const _, std::mem::size_of::<INPUT>() as i32);
    let _ = vk;
}

fn key_event(vk: u16, up: bool) {
    unsafe {
        let inp = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: windows_sys::Win32::UI::Input::KeyboardAndMouse::KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(1, &inp as *const INPUT as *const _, std::mem::size_of::<INPUT>() as i32);
    }
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let sub = args.get(2).map(|s| s.as_str()).unwrap_or("");
    let Some(hwnd) = find_window(sub) else {
        eprintln!("window not found: {sub}");
        std::process::exit(2);
    };
    // 先抬到最前并**校验确实在最前**：否则引擎的命中测试会解析到遮挡窗，
    // 手势就会作用到别的窗口上（见 raise_target 的安全说明）。
    if !raise_target(hwnd) {
        eprintln!(
            "中止：目标窗 '{sub}' 被其他窗口遮挡（该点下的顶层窗口不是它）。\n\
             为避免误伤其他窗口，本驱动不会继续。\n\
             本机常见原因：全屏的远程桌面客户端窗口盖住了整个桌面。"
        );
        std::process::exit(3);
    }
    let f0 = frame_of(hwnd).expect("dwm frame");
    match cmd {
        "frame" => {
            let wa = work_area();
            println!(
                "hwnd=0x{:x} frame=[{}, {}, {}, {}] work_area=[{}, {}, {}, {}]",
                hwnd as isize, f0[0], f0[1], f0[2], f0[3], wa[0], wa[1], wa[2], wa[3]
            );
        }
        "alt-drag" => {
            let cx = (f0[0] + f0[2]) / 2;
            // 必须落在**明确无歧义的客户区**：+40 可能仍压在标题栏上，那样 PASS 会来自
            // OS 自带的标题栏拖动而非本工具（假阳性）。+80 稳在客户区。
            let cy = f0[1] + 80;
            key_event(VK_MENU, false);
            mouse_move_abs(cx, cy);
            std::thread::sleep(Duration::from_millis(80));
            mouse_button(true); // 引擎：Down(mods=Alt) → Armed
            std::thread::sleep(Duration::from_millis(60));
            for i in 1..=4 {
                mouse_move_abs(cx + DRAG_PX * i / 4, cy); // 每 Move → Fallback SetPos（引擎合并）
                std::thread::sleep(Duration::from_millis(50));
            }
            mouse_button(false);
            key_event(VK_MENU, true);
            std::thread::sleep(Duration::from_millis(300));
            let f1 = frame_of(hwnd).expect("dwm frame");
            let dx = f1[0] - f0[0];
            let dy = f1[1] - f0[1];
            println!("alt-drag: dx={dx} dy={dy} (cmd +{DRAG_PX}px)");
            println!("ALT_DRAG={}", if dx.abs() >= 40 { "PASS" } else { "FAIL" });
        }
        "alt-dbl" => {
            // 客户端区中心点击（**避开标题栏**：Alt+双击标题栏 是 OS 自带"关闭窗
            // 口"快捷键，会先于引擎手势关掉窗口——2026-10-15 实测，notepad 因此
            // 被 OS 关闭、引擎 SC_MAXIMIZE 落空）。
            let cx = (f0[0] + f0[2]) / 2;
            let cy = (f0[1] + f0[3]) / 2;
            for _ in 0..2 {
                key_event(VK_MENU, false);
                mouse_move_abs(cx, cy);
                std::thread::sleep(Duration::from_millis(60));
                mouse_button(true);
                std::thread::sleep(Duration::from_millis(40));
                mouse_button(false);
                key_event(VK_MENU, true);
                std::thread::sleep(Duration::from_millis(120)); // < GetDoubleClickTime
            }
            std::thread::sleep(Duration::from_millis(500));
            let f1 = frame_of(hwnd).expect("dwm frame after dbl#1");
            let wa = work_area();
            let maximized = within2(&f1, &wa);
            // 第二次双击（最大化窗中心仍是客户端点）→ 引擎应发 Restore。
            let cx2 = (f1[0] + f1[2]) / 2;
            let cy2 = (f1[1] + f1[3]) / 2;
            for _ in 0..2 {
                key_event(VK_MENU, false);
                mouse_move_abs(cx2, cy2);
                std::thread::sleep(Duration::from_millis(60));
                mouse_button(true);
                std::thread::sleep(Duration::from_millis(40));
                mouse_button(false);
                key_event(VK_MENU, true);
                std::thread::sleep(Duration::from_millis(120));
            }
            std::thread::sleep(Duration::from_millis(500));
            let f2 = frame_of(hwnd).expect("dwm frame after dbl#2");
            let restored = within2(&f2, &f0);
            println!(
                "alt-dbl: before=[{f0:?}] after_dbl1=[{f1:?}] after_dbl2=[{f2:?}] work_area=[{wa:?}]"
            );
            println!(
                "ALT_DBL={} (maximized={maximized}, restored={restored})",
                if maximized && restored { "PASS" } else { "FAIL" }
            );
        }
        "ctrl-resize" => {
            // 语义验证：**在窗口内部抓取（不贴边）也必须能缩放**——这正是本工具存在的
            // 理由（2026-10-16 用户指出旧版"贴边才触发"等于把原生缩放重做一遍且更难瞄）。
            // 抓点取"横向 3/4 处、纵向居中"：落在九宫格右列中格 ⇒ RZ_RIGHT，
            // 且距右边缘还有 1/4 窗宽（约 200px），明确不是"瞄边缘"。
            let w0 = f0[2] - f0[0];
            let rx = f0[0] + w0 * 3 / 4;
            let ry = (f0[1] + f0[3]) / 2;
            key_event(VK_MENU, false);
            key_event(VK_CONTROL, false);
            mouse_move_abs(rx, ry);
            std::thread::sleep(Duration::from_millis(80));
            mouse_button(true); // 引擎：Down(mods=Alt|Ctrl, 贴右边) → ArmedResize
            std::thread::sleep(Duration::from_millis(60));
            for i in 1..=4 {
                mouse_move_abs(rx + RZ_DRAG_PX * i / 4, ry); // 每 Move → SetRect
                std::thread::sleep(Duration::from_millis(50));
            }
            mouse_button(false);
            key_event(VK_CONTROL, true);
            key_event(VK_MENU, true);
            std::thread::sleep(Duration::from_millis(300));
            let f1 = frame_of(hwnd).expect("dwm frame");
            let dw = (f1[2] - f1[0]) - (f0[2] - f0[0]);
            println!("ctrl-resize: w0={} w1={} (cmd +{RZ_DRAG_PX}px)", f0[2] - f0[0], f1[2] - f1[0]);
            println!("CTRL_RESIZE={}", if dw >= 40 { "PASS" } else { "FAIL" });
        }
        "middle-snap" => {
            // 中键从窗口中心**向左**拖 200px 后松开 → 期望吸附到工作区左半屏。
            // 判据：窗口左缘贴近工作区左缘，且宽度约为工作区的一半（容差 20px，
            // 因为 SetWindowPlacement 写的是外框矩形，DWM 可见框内缩约 7px）。
            let cx = (f0[0] + f0[2]) / 2;
            let cy = (f0[1] + f0[3]) / 2;
            mouse_move_abs(cx, cy);
            std::thread::sleep(Duration::from_millis(80));
            mouse_middle(true);
            std::thread::sleep(Duration::from_millis(60));
            for i in 1..=4 {
                mouse_move_abs(cx - 200 * i / 4, cy);
                std::thread::sleep(Duration::from_millis(50));
            }
            mouse_middle(false);
            std::thread::sleep(Duration::from_millis(600));
            let f1 = frame_of(hwnd).expect("dwm frame");
            let wa = work_area();
            let half_w = (wa[2] - wa[0]) / 2;
            let left_ok = (f1[0] - wa[0]).abs() <= 20;
            let width_ok = ((f1[2] - f1[0]) - half_w).abs() <= 20;
            println!(
                "middle-snap: after=[{f1:?}] work_area=[{wa:?}] expect_left_half_w={half_w}"
            );
            println!(
                "MIDDLE_SNAP={} (left_ok={left_ok}, width_ok={width_ok})",
                if left_ok && width_ok { "PASS" } else { "FAIL" }
            );
        }
        "maximize" => {
            // 把窗口置为最大化。用途：冒烟测试可能改动了窗口几何（例如中键分屏把测试目标
            // 换成别的窗口），用它一键复位；也方便手动恢复。
            unsafe {
                let mut wndpl: WINDOWPLACEMENT = std::mem::zeroed();
                wndpl.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                if GetWindowPlacement(hwnd, &mut wndpl) != 0 {
                    wndpl.showCmd = SW_MAXIMIZE as u32;
                    SetWindowPlacement(hwnd, &wndpl);
                }
            }
            std::thread::sleep(Duration::from_millis(500));
            let f1 = frame_of(hwnd).expect("dwm frame");
            let wa = work_area();
            println!("maximize: hwnd=0x{:x} after=[{f1:?}] work_area=[{wa:?}]", hwnd as isize);
            println!("MAXIMIZE=OK");
        }
        "send-save" => {
            // 给设置窗口发一条 WM_COMMAND(ID_SAVE=1)，等价于点"保存"按钮。
            // 用于自动化验证「设置界面保存 → config::replace 热重载 + 写盘」这条链路。
            unsafe {
                PostMessageW(hwnd, WM_COMMAND, 1, 0);
            }
            std::thread::sleep(Duration::from_millis(500));
            println!("send-save: posted WM_COMMAND(ID_SAVE) to 0x{:x}", hwnd as isize);
            println!("SEND_SAVE=OK");
        }
        "resize-bench" => {
            // 缩放延迟基准：以约 200Hz 注入 40 步 × 5px（共 200px）的拖动，随后轮询窗口
            // 几何直到稳定，报出「最后一次注入鼠标移动之后，窗口还在继续变化多久」
            // —— 这就是用户能感觉到的"追不上光标"的延迟量。
            // 与 alt-drag 对照：同样的注入率下移动几乎瞬时收敛，缩放会暴露目标应用重排的代价。
            let w0 = f0[2] - f0[0];
            let rx = f0[0] + w0 * 3 / 4;
            let ry = (f0[1] + f0[3]) / 2;
            key_event(VK_MENU, false);
            key_event(VK_CONTROL, false);
            mouse_move_abs(rx, ry);
            std::thread::sleep(Duration::from_millis(80));
            mouse_button(true);
            std::thread::sleep(Duration::from_millis(60));
            let t0 = std::time::Instant::now();
            for i in 1..=40 {
                mouse_move_abs(rx + 5 * i, ry);
                std::thread::sleep(Duration::from_millis(5));
            }
            let inject_ms = t0.elapsed().as_millis();
            // 轮询直到几何连续 120ms 不变
            let mut last_w = i32::MIN;
            let mut last_change = std::time::Instant::now();
            loop {
                let w = frame_of(hwnd).map(|f| f[2] - f[0]).unwrap_or(-1);
                if w != last_w {
                    last_w = w;
                    last_change = std::time::Instant::now();
                }
                if last_change.elapsed().as_millis() >= 120 {
                    break;
                }
                if t0.elapsed().as_millis() > 8000 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let settle_ms = last_change.duration_since(t0).as_millis();
            mouse_button(false);
            key_event(VK_CONTROL, true);
            key_event(VK_MENU, true);
            println!(
                "resize-bench: w0={w0} w_final={last_w} expected≈{} 注入用时={inject_ms}ms 追上用时={settle_ms}ms",
                w0 + 200
            );
            println!("RESIZE_BENCH_DONE");
        }
        "resize-storm" => {
            // 持续高频缩放压测（2026-10-21 事故 5 的复现器）。
            // 用户报障是"一直高频缩放才会卡死"，而 resize-bench 每轮只注入 200ms、
            // 中间静默 300ms，节奏太松，触发不了。这里改成：
            //   - 按住 Alt+Ctrl 不松手，连续注入 rounds 次来回摆动；
            //   - 每次摆动 = 40 步的扫动，步间不留 sleep（尽可能快）；
            //   - 全程只按下/抬起一次鼠标（一个超长手势），最接近"一直快速缩放"。
            // 判据：风暴结束后窗口必须仍能被缩放。
            let rounds: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);
            let w0 = f0[2] - f0[0];
            let rx = f0[0] + w0 * 3 / 4;
            let ry = (f0[1] + f0[3]) / 2;
            key_event(VK_MENU, false);
            key_event(VK_CONTROL, false);
            mouse_move_abs(rx, ry);
            std::thread::sleep(Duration::from_millis(80));
            mouse_button(true);
            std::thread::sleep(Duration::from_millis(40));
            let t0 = std::time::Instant::now();
            let mut injected = 0u32;
            for r in 0..rounds {
                let dir: i32 = if r % 2 == 0 { 1 } else { -1 };
                for i in 1..=40 {
                    mouse_move_abs(rx + dir * (120 * i / 40), ry);
                    injected += 1;
                }
            }
            let inject_ms = t0.elapsed().as_millis();
            mouse_button(false);
            key_event(VK_CONTROL, true);
            key_event(VK_MENU, true);
            std::thread::sleep(Duration::from_millis(600));
            let f1 = frame_of(hwnd).expect("dwm frame after storm");
            let rate = injected as f64 * 1000.0 / (inject_ms.max(1) as f64);
            println!(
                "resize-storm: rounds={rounds} injected={injected} inject_ms={inject_ms} rate={rate:.0}/s"
            );
            println!("resize-storm: w0={w0} w_after={}", f1[2] - f1[0]);
            // 存活判定：风暴之后再做一个干净的缩放，必须仍然生效。
            let f_pre = frame_of(hwnd).expect("dwm frame");
            let w_pre = f_pre[2] - f_pre[0];
            let rx2 = f_pre[0] + w_pre * 3 / 4;
            let ry2 = (f_pre[1] + f_pre[3]) / 2;
            key_event(VK_MENU, false);
            key_event(VK_CONTROL, false);
            mouse_move_abs(rx2, ry2);
            std::thread::sleep(Duration::from_millis(80));
            mouse_button(true);
            std::thread::sleep(Duration::from_millis(60));
            for i in 1..=4 {
                mouse_move_abs(rx2 + RZ_DRAG_PX * i / 4, ry2);
                std::thread::sleep(Duration::from_millis(50));
            }
            mouse_button(false);
            key_event(VK_CONTROL, true);
            key_event(VK_MENU, true);
            std::thread::sleep(Duration::from_millis(400));
            let f2 = frame_of(hwnd).expect("dwm frame after verify");
            let dw = (f2[2] - f2[0]) - w_pre;
            println!(
                "resize-storm: verify w_pre={w_pre} w_post={} dw={dw}",
                f2[2] - f2[0]
            );
            println!(
                "RESIZE_STORM={}",
                if dw.abs() >= 40 { "SURVIVED" } else { "DEAD" }
            );
        }
        "native-resize" => {
            // **对照实验**：用户抱怨"还不如直接用鼠标去边缘缩放"，那就真的去边缘拖。
            // 光标放到窗口右边框（DWM 帧外那圈 ~7px 不可见边框里）按下左键拖动——
            // 这是**完全原生**的缩放路径（应用自己的 modal loop 驱动），
            // 注入序列与 resize-bench 完全相同，量同样的"追上用时"，直接 A/B。
            let w0 = f0[2] - f0[0];
            let bx = f0[2] + 3; // 右边框：DWM 帧右侧再往外 3px
            let by = (f0[1] + f0[3]) / 2;
            mouse_move_abs(bx, by);
            std::thread::sleep(Duration::from_millis(120));
            mouse_button(true);
            std::thread::sleep(Duration::from_millis(120));
            let t0 = std::time::Instant::now();
            for i in 1..=40 {
                mouse_move_abs(bx + 5 * i, by);
                std::thread::sleep(Duration::from_millis(5));
            }
            let inject_ms = t0.elapsed().as_millis();
            let mut last_w = i32::MIN;
            let mut last_change = std::time::Instant::now();
            loop {
                let w = frame_of(hwnd).map(|f| f[2] - f[0]).unwrap_or(-1);
                if w != last_w {
                    last_w = w;
                    last_change = std::time::Instant::now();
                }
                if last_change.elapsed().as_millis() >= 120 || t0.elapsed().as_millis() > 8000 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let settle_ms = last_change.duration_since(t0).as_millis();
            mouse_button(false);
            println!(
                "native-resize: w0={w0} w_final={last_w} expected≈{} 注入用时={inject_ms}ms 追上用时={settle_ms}ms",
                w0 + 200
            );
            println!("NATIVE_RESIZE_DONE");
        }
        "close" => {
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            std::thread::sleep(Duration::from_millis(300));
            let still = frame_of(hwnd).is_some();
            println!("close: hwnd=0x{:x} still_present={still}", hwnd as isize);
            println!("CLOSE=OK");
        }
        _ => {
            eprintln!("unknown cmd {cmd}");
            std::process::exit(1);
        }
    }
    restore_z(hwnd);
}
