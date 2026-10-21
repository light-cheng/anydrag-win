//! T3.4 — main 接线（GOAL 原文）+ 三线程拓扑：
//! main（DPI 感知 + 托盘泵）/ engine（ring 消费 + 状态机 + FFI 动作）/
//! hook（WH_MOUSE_LL，O(1) 回调塞 ring + PostThreadMessage）。

// **GUI 子系统**（2026-10-21 修复）：不加这行时 Rust 默认编成 CONSOLE 子系统
// （PE Subsystem=3），双击启动会弹出一个黑框终端窗口。这是个纯托盘程序，不该有它——
// 更严重的是：**关掉那个窗口 = 向本进程的控制台发 CTRL_CLOSE_EVENT = 程序直接被杀**，
// 用户很容易误以为"程序崩了"。
//
// 换成 WINDOWS 子系统后进程不再附带控制台，但 `eprintln!` 仍有出路（见 `attach_debug_console`）：
// 从 cmd/PowerShell 启动时继承父控制台，`ANYDRAG_DEBUG=1` 的诊断日志照常可见。
#![windows_subsystem = "windows"]

mod coalesce;
mod config;
mod engine;
mod engine_loop;
mod hook;
mod icon;
mod mods;
mod ring;
mod settings;
mod throttle;
mod tray;
mod winmod;

fn main() {
    winmod::dpi_aware_v2(); // 必须是第一个 FFI 调用（GOAL 不变量 1）
    // 先装入全局配置存放点：设置界面保存后靠它做**热重载**（见 config::STORE 注释）。
    config::init(config::load());
    let cfg = std::sync::Arc::new(config::snapshot());
    // 启动时把磁盘配置的 enabled 同步进原子镜像（config.rs 注记"main 启动时按
    // Config::load() 初始化"——缺了这一步，config.json 里 enabled=false 会被
    // 完全忽略，引擎恒按启用跑；2026-10-15 冒烟 Phase 3 实测抓出）。
    config::ENABLED.store(cfg.enabled, std::sync::atomic::Ordering::Release);
    // `--settings`：启动即打开设置窗口。窗口建在 main 线程，由随后的托盘消息泵分发消息；
    // 既方便用户直达，也让 GUI 可被自动化验证（FindWindow 按标题/类名找得到）。
    if std::env::args().any(|a| a == "--settings") {
        settings::open();
    }
    let _engine_tid = engine_loop::run_in_thread(std::sync::Arc::clone(&cfg)); // 内部 spawn hook 线程并绑定 tid
    tray::install(&cfg); // main 线程跑托盘消息泵（阻塞至 Quit）
}
