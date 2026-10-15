//! T2.3 spike 探针：实测"跨进程 PostMessage 0xF012 (SC_DRAGMOVE) 是否让目标应用
//! 自己跑原生 caption 拖动循环"。对 notepad / explorer / msedge 各跑一遍，
//! 输出"生效/失效"结果（供 docs/spike-scandrag.md 引用）。
//!
//! 判定逻辑（GOAL T2.3）：按住左键 → PostMessage SC_DRAGMOVE → 移动鼠标 +80px，
//! 若窗口 DWM 几何 x 位移 ≈ 80（≥40 阈值）= remote 0xF012 生效；不动 = 失效。
//! 决策规则：notepad 与 explorer 均生效 → 默认策略 native；否则 fallback。
//!
//! 用法：`probe_scandrag`（需交互式桌面会话；会短暂移动真实鼠标并开关 3 个窗口）。

use std::sync::Mutex;
use std::time::Duration;

use windows_sys::core::{PWSTR, PCWSTR};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, PostMessageW, SetForegroundWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    WM_CLOSE, WM_SYSCOMMAND,
};

// ---- 未文档化但数十年来稳定的 syscommand（GOAL §3：0xF012 由本 spike 背书）----
const SC_DRAGMOVE: u32 = 0xF012;
// 命令移动 80px；窗口 x 位移 ≥ 40 记为"生效"（容忍 compositor 舍入/节流）。
const DRAG_COMMAND_PX: i32 = 80;
const WORKS_THRESHOLD: i32 = 40;

/// 由 EnumWindows 回调收集 (hwnd, pid, title, class)。
static CANDIDATES: Mutex<Vec<(isize, u32, String, String)>> = Mutex::new(Vec::new());

unsafe extern "system" fn collect_proc(hwnd: HWND, _lparam: isize) -> i32 {
    if IsWindowVisible(hwnd) != 0 {
        let mut title = [0u16; 512];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr() as PWSTR, 512);
        let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);
        let mut class = [0u16; 64];
        let c = GetClassNameW(hwnd, class.as_mut_ptr() as PWSTR, 64);
        let class = String::from_utf16_lossy(&class[..c.max(0) as usize]);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        CANDIDATES
            .lock()
            .unwrap()
            .push((hwnd as isize, pid, title, class));
    }
    1 // 继续枚举
}

fn enum_windows() -> Vec<(isize, u32, String, String)> {
    unsafe {
        *CANDIDATES.lock().unwrap() = Vec::new();
        EnumWindows(Some(collect_proc), 0);
    }
    CANDIDATES.lock().unwrap().clone()
}

fn frame_bounds(hwnd: isize) -> Option<[i32; 4]> {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(
            hwnd as *mut _,
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

// ---- SendInput 鼠标原语（物理像素；Per-Monitor-V2 下与 DWM 几何同坐标系）----
fn mouse_move_abs(x: i32, y: i32) {
    // SendInput 绝对坐标：65535 归一化到虚拟屏幕（多屏合并坐标系）。
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    let nx = (x as u32) * 65535 / (vw as u32 - 1);
    let ny = (y as u32) * 65535 / (vh as u32 - 1);
    unsafe {
        let inputs = [INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx as i32,
                    dy: ny as i32,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }];
        SendInput(
            1,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn mouse_button(flags: u32) {
    unsafe {
        let inputs = [INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }];
        SendInput(
            1,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn post_dragmove(hwnd: isize) {
    unsafe {
        PostMessageW(hwnd as *mut _, WM_SYSCOMMAND, SC_DRAGMOVE as usize, 0);
    }
}

/// 0xF012 拖动测试，两个变体（都从窗口中心起，命令移动 +80px）：
/// - A：GOAL T2.3 原文序列（无按键：移动→PostMessage→移动+80→读几何），
///   结尾补一个 LDOWN+LUP 解除可能挂起的原生拖动循环（保护性收尾）；
/// - B：产品真实序列（按住左键：移动→LDOWN→PostMessage→移动+80→LUP），
///   引擎在 Down 时刻发 0xF012，此刻用户按键必为按下——生产保真。
/// 返回 (works, dxa, dxb)。
fn drag_test(hwnd: isize) -> Option<(bool, i32, i32)> {
    let f0 = frame_bounds(hwnd)?;
    let cx = (f0[0] + f0[2]) / 2;
    let cy = f0[1] + 40; // 略偏上（避开可能的底部内容区控件，仍在客户区内）
    unsafe {
        SetForegroundWindow(hwnd as *mut _);
    }
    std::thread::sleep(Duration::from_millis(150));

    // ---- 变体 A（GOAL 原文：无按键）----
    mouse_move_abs(cx, cy);
    std::thread::sleep(Duration::from_millis(60));
    post_dragmove(hwnd);
    std::thread::sleep(Duration::from_millis(50));
    mouse_move_abs(cx + DRAG_COMMAND_PX, cy);
    std::thread::sleep(Duration::from_millis(50));
    let fa = frame_bounds(hwnd)?;
    let dxa = fa[0] - f0[0];
    // 保护性收尾：若 A 变体的原生循环还挂着（无按键不会自然结束），点一下结束它。
    mouse_button(MOUSEEVENTF_LEFTDOWN);
    mouse_button(MOUSEEVENTF_LEFTUP);
    std::thread::sleep(Duration::from_millis(120));
    let _ = frame_bounds(hwnd); // 让循环有时间收尾

    // ---- 变体 B（产品保真：按住按键）----
    let fb0 = frame_bounds(hwnd)?;
    mouse_move_abs(cx, cy);
    std::thread::sleep(Duration::from_millis(60));
    mouse_button(MOUSEEVENTF_LEFTDOWN);
    post_dragmove(hwnd);
    std::thread::sleep(Duration::from_millis(180));
    mouse_move_abs(cx + DRAG_COMMAND_PX, cy);
    std::thread::sleep(Duration::from_millis(80));
    mouse_button(MOUSEEVENTF_LEFTUP);
    std::thread::sleep(Duration::from_millis(80));
    let fb1 = frame_bounds(hwnd)?;
    let dxb = fb1[0] - fb0[0];

    let works = dxa.abs() >= WORKS_THRESHOLD || dxb.abs() >= WORKS_THRESHOLD;
    Some((works, dxa, dxb))
}

/// 启动子进程（lpCommandLine 需要可写 wide 缓冲）。
fn spawn_wide(cmd: &str) -> Option<PROCESS_INFORMATION> {
    let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    let pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let mut pi = pi;
    unsafe {
        // 实测（本机）：lpStartupInfo 传 NULL 会得 ERROR_INVALID_PARAMETER(87)——
        // 必须有 cbSize 有效的 STARTUPINFOW。诊断脚本 scratch/probe_spawn_diag3.py 记录在案。
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let created = CreateProcessW(
            0 as PCWSTR,
            cmd_w.as_mut_ptr() as PWSTR,
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            0 as PCWSTR,
            &si,
            &mut pi,
        );
        if created == 0 {
            let err = GetLastError();
            eprintln!("spawn failed: {cmd} (GetLastError={err})");
            return None;
        }
    }
    let _ = &cmd_w; // 缓冲活到返回（CreateProcess 已复制命令行，这是双保险）
    Some(pi)
}

fn kill_pi(pi: &mut PROCESS_INFORMATION) {
    unsafe {
        TerminateProcess(pi.hProcess, 0);
        WaitForSingleObject(pi.hProcess, 2000);
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
}

/// 等目标窗口出现（按 pid 和/或标题/类名），返回 hwnd；超时 None。
fn wait_window(
    by_pid: Option<u32>,
    title_sub: &str,
    class_sub: &str,
    timeout_ms: u32,
) -> Option<isize> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms as u64);
    let (t, c) = (title_sub.to_lowercase(), class_sub.to_lowercase());
    loop {
        for (hwnd, pid, title, class) in enum_windows() {
            let ok_pid = by_pid.map(|p| p == pid).unwrap_or(true);
            let ok_title = t.is_empty() || title.to_lowercase().contains(&t);
            let ok_class = c.is_empty() || class.to_lowercase().contains(&c);
            if ok_pid && ok_title && ok_class && frame_bounds(hwnd).is_some() {
                return Some(hwnd);
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

struct AppProbe<'a> {
    spawn: &'a str, // 启动命令（None = 用已有窗口）
    by_pid: bool,   // true: 窗口必须属于新 pid（notepad 类独立进程）
    title_sub: &'a str,
    class_sub: &'a str,
}

fn run_probe(p: &AppProbe) -> String {
    // explorer/msedge：新窗口由既有实例承载，子进程快速退出是正常现象；
    // notepad：独立进程，按 pid 认窗口。
    let pi: Option<PROCESS_INFORMATION> = spawn_wide(p.spawn);
    let target_pid = pi.as_ref().map(|pi| pi.dwProcessId);
    let found = if p.by_pid {
        // 按 pid 认窗口：spawn 失败绝不退回"任意窗口"匹配（那会测到错误的目标）。
        match target_pid {
            Some(pid) if pid != 0 => {
                let hit = wait_window(Some(pid), p.title_sub, p.class_sub, 12_000);
                if hit.is_none() {
                    // 调试：pid 匹配失败时转标题/类名匹配（并 dump 候选）。
                    eprintln!(
                        "  debug: pid {pid} 未匹配到窗口，候选：{}",
                        enum_windows()
                            .into_iter()
                            .filter(|(_, pid2, _, _)| Some(*pid2) == Some(pid))
                            .count()
                    );
                    wait_window(None, "notepad", "", 6_000)
                } else {
                    hit
                }
            }
            _ => None,
        }
    } else {
        wait_window(None, p.title_sub, p.class_sub, 12_000)
    };
    let desc = match found.and_then(drag_test) {
        Some((works, dxa, dxb)) => format!(
            "{} (A无键 dxa={dxa} / B按键 dxb={dxb})",
            if works { "生效" } else { "失效" }
        ),
        None => "未找到窗口/拖测失败".to_string(),
    };
    // 清理：找到窗口就 WM_CLOSE；by_pid 的进程再终止（notepad 场景）。
    if let Some(hwnd) = found {
        unsafe {
            PostMessageW(hwnd as *mut _, WM_CLOSE, 0, 0);
        }
    }
    if p.by_pid {
        if let Some(mut pi) = pi {
            kill_pi(&mut pi);
        }
    }
    desc
}

fn main() {
    unsafe {
        // 探针自身也遵守"首个 FFI 调用 = DPI 感知"（GOAL 不变量 1）。
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let notepad = AppProbe {
        spawn: "notepad.exe",
        by_pid: true,
        title_sub: "",
        class_sub: "",
    };
    let explorer = AppProbe {
        spawn: "explorer.exe C:\\Windows",
        by_pid: false,
        title_sub: "Windows",
        class_sub: "CabinetWClass",
    };
    let edge = AppProbe {
        spawn: "msedge --window-size=800,600 about:blank",
        by_pid: false,
        title_sub: "about:blank",
        class_sub: "Chrome_WidgetWin",
    };

    let np_res = run_probe(&notepad);
    std::thread::sleep(Duration::from_millis(400));
    let ex_res = run_probe(&explorer);
    std::thread::sleep(Duration::from_millis(400));
    let ed_res = run_probe(&edge);
    std::thread::sleep(Duration::from_millis(400));

    let np_works = np_res.contains("生效");
    let ex_works = ex_res.contains("生效");
    let decision = if np_works && ex_works { "native" } else { "fallback" };

    println!("=== probe_scandrag results ===");
    println!("notepad : {np_res}");
    println!("explorer: {ex_res}");
    println!("msedge  : {ed_res}");
    println!("----------------------------------------");
    println!(
        "decision: notepad_works={np_works} explorer_works={ex_works} => default_strategy = \"{decision}\""
    );
    // 机器可读行（写入 docs/spike-scandrag.md 时引用）。
    println!(
        "RESULT_JSON={{\"notepad\":\"{np_res}\",\"explorer\":\"{ex_res}\",\"msedge\":\"{ed_res}\",\"decision\":\"{decision}\"}}"
    );
}
