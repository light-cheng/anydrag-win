//! T3.4 — main 接线（GOAL 原文）+ 三线程拓扑：
//! main（DPI 感知 + 托盘泵）/ engine（ring 消费 + 状态机 + FFI 动作）/
//! hook（WH_MOUSE_LL，O(1) 回调塞 ring + PostThreadMessage）。

mod config;
mod engine;
mod engine_loop;
mod hook;
mod mods;
mod ring;
mod tray;
mod winmod;

fn main() {
    winmod::dpi_aware_v2(); // 必须是第一个 FFI 调用（GOAL 不变量 1）
    let cfg = std::sync::Arc::new(config::load());
    // 启动时把磁盘配置的 enabled 同步进原子镜像（config.rs 注记"main 启动时按
    // Config::load() 初始化"——缺了这一步，config.json 里 enabled=false 会被
    // 完全忽略，引擎恒按启用跑；2026-10-15 冒烟 Phase 3 实测抓出）。
    config::ENABLED.store(cfg.enabled, std::sync::atomic::Ordering::Release);
    let _engine_tid = engine_loop::run_in_thread(std::sync::Arc::clone(&cfg)); // 内部 spawn hook 线程并绑定 tid
    tray::install(&cfg); // main 线程跑托盘消息泵（阻塞至 Quit）
}
