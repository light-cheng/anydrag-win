//! 构建脚本：把 comctl32 v6 视觉样式清单复制到产物目录。
//!
//! 清单文件在包根目录 `anydrag-win.exe.manifest`（Windows 外部清单命名规则：
//! `<exe 名>.exe.manifest`，与 exe 同目录即被系统加载）。为什么用外部清单而不是
//! 内嵌资源：项目原则是"不内嵌资源文件"（托盘图标也是运行时自绘），外部清单
//! 零构建依赖、零工具链要求（不需要 rc.exe / windres）。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=anydrag-win.exe.manifest");

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("anydrag-win.exe.manifest");
    if !src.exists() {
        return;
    }
    // 产物目录：CARGO_TARGET_DIR 优先，否则 <包目录>/target/<profile>。
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "release".to_string());
    let target = match std::env::var("CARGO_TARGET_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => manifest_dir.join("target"),
    };
    let dst = target.join(&profile).join("anydrag-win.exe.manifest");
    if let Err(e) = std::fs::copy(&src, &dst) {
        println!("cargo:warning=failed to copy manifest to {dst:?}: {e}");
    }
}
