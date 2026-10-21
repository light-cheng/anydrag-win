//! 构建脚本：把 exe 图标与 comctl32 v6 视觉样式清单**内嵌进 PE 资源段**。
//!
//! 两者都由 `assets/app.rc` 描述，经 Windows SDK 的 `rc.exe` 编成 `.res`，
//! 再用 `cargo:rustc-link-arg` 交给链接器 —— **不引入任何 Rust 构建依赖**
//! （不用 winres / embed-resource crate）。
//!
//! 为什么 manifest 也从"外部同名文件"改成内嵌（2026-10-21）：外部清单要求
//! `anydrag-win.exe.manifest` 与 exe 同目录且同名，拷贝/打包时极易漏掉，一漏
//! 界面就退回 Windows 95 灰控件样式，且**没有任何报错**。内嵌进资源段后
//! **单文件即可分发**，不存在漏拷问题。
//!
//! 托盘图标仍是运行时自绘（`src/icon.rs`），与此无关。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=anydrag-win.exe.manifest");

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    embed_resources(&manifest_dir);
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
fn embed_resources(manifest_dir: &PathBuf) {
    let rc_src = manifest_dir.join("assets").join("app.rc");
    let ico = manifest_dir.join("assets").join("anydrag-win.ico");
    println!("cargo:rerun-if-changed={}", rc_src.display());
    println!("cargo:rerun-if-changed={}", ico.display());
    let manifest = manifest_dir.join("anydrag-win.exe.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if !rc_src.exists() || !ico.exists() {
        println!("cargo:warning=resource assets missing, skipping embedding");
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
        println!("cargo:warning=rc.exe not found or failed; icon/manifest not embedded");
        return;
    }
    // MSVC link.exe 直接接受 .res 作为链接器输入。
    println!("cargo:rustc-link-arg={}", res.display());
}
