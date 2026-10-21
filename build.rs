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

    embed_icon(&manifest_dir);
}


/// 把 exe 文件图标内嵌进 PE 资源段。
///
/// 托盘图标仍是运行时自绘（src/icon.rs，不依赖资源文件）；但 exe 自身的图标
/// 必须内嵌进 PE 资源段，否则 Explorer / 任务栏 / Alt+Tab 只能显示通用默认图标。
///
/// 做法：调 Windows SDK 的 rc.exe 把 assets/app.rc 编成 .res，再用
/// cargo:rustc-link-arg 交给链接器 —— 不引入任何 Rust 构建依赖。
///
/// 找不到 rc.exe 时只发 warning、不失败：图标是外观优化，不该阻断构建。
fn embed_icon(manifest_dir: &PathBuf) {
    let rc_src = manifest_dir.join("assets").join("app.rc");
    let ico = manifest_dir.join("assets").join("anydrag-win.ico");
    println!("cargo:rerun-if-changed={}", rc_src.display());
    println!("cargo:rerun-if-changed={}", ico.display());
    if !rc_src.exists() || !ico.exists() {
        println!("cargo:warning=icon assets missing, skipping exe icon");
        return;
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    if out_dir.as_os_str().is_empty() {
        return;
    }
    let res = out_dir.join("anydrag-win.res");

    // rc.exe 一般在 Windows SDK 的 bin/<ver>/x64 下。优先 PATH，再扫 SDK 目录。
    let mut candidates: Vec<PathBuf> = vec![PathBuf::from("rc.exe")];
    if let Ok(kits) = std::env::var("WindowsSdkDir") {
        candidates.push(PathBuf::from(kits).join("bin").join("x64").join("rc.exe"));
    }
    let sdk_bin = PathBuf::from("C:\\Program Files (x86)\\Windows Kits\\10\\bin");
    if let Ok(rd) = std::fs::read_dir(&sdk_bin) {
        let mut versions: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        versions.sort();
        versions.reverse();
        for v in versions {
            candidates.push(v.join("x64").join("rc.exe"));
        }
    }

    let mut ok = false;
    for rc in &candidates {
        // /nologo 静默；/fo 指定输出；cwd 设成 assets（.rc 里用的是相对 ico 名）。
        let status = std::process::Command::new(rc)
            .current_dir(manifest_dir.join("assets"))
            .arg("/nologo")
            .arg("/fo")
            .arg(&res)
            .arg(&rc_src)
            .status();
        if let Ok(st) = status {
            if st.success() {
                ok = true;
                break;
            }
        }
    }
    if !ok {
        println!("cargo:warning=rc.exe not found or failed; exe icon not embedded");
        return;
    }
    // MSVC link.exe 直接接受 .res 作为链接器输入。
    println!("cargo:rustc-link-arg={}", res.display());
}
