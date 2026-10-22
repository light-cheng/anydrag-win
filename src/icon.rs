//! 托盘图标：**直接用程序图标**（`assets/anydrag-win.ico` 内嵌进 PE 资源段，
//! 由 `build.rs` + `assets/app.rc` 生成），不再自绘。
//!
//! 为什么改成用程序图标（2026-10-23 用户要求）：此前托盘是运行时自绘的**四向箭头**
//! 单色剪影，与 exe 图标（两个错位圆角方块）**形状和配色都不同**。用户实测后的原话是
//! "托盘图标能不能也用程序图标？这个原来没改啊，好丑"。这不是审美口味问题——同一款
//! 软件在任务栏和托盘里长得像两个东西，本身就是可识别性缺陷。
//!
//! 顺带删掉了整套箭头光栅化代码（`coverage` / `inside` / `tri`）：留着就是
//! "同一语义两个实现"，而 AGENTS.md 坑 #18 正是这条教训 —— 生效的那个必须是被测试的
//! 那个，不能并存。
//!
//! **状态指示从"换配色"改成"淡化"**：自绘箭头是单色剪影，配色即状态（亮蓝/灰）。
//! 程序图标是**多色**的（白填充 + 炭黑描边 + 中灰后层），没有一种颜色能安全地表达
//! "禁用"——换色会破坏它与 exe 图标的一致性，而一致性正是本次改动的目的。
//! 所以禁用态改为**整体降低不透明度**（见 `fade`）：对任何配色都成立，且
//! "变淡 = 不生效"比"变灰 = 不生效"更直观。tooltip 同时变成 `anydrag-win（已禁用）`。

use std::ffi::c_void;

use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HDC,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetIconInfo, GetSystemMetrics, LoadImageW, ICONINFO, IMAGE_ICON,
    LR_DEFAULTCOLOR, SM_CXSMICON, SM_CYSMICON,
};

/// `assets/app.rc` 里 `1 ICON "anydrag-win.ico"` 的资源 ID。
///
/// **不能改成 32512 / `IDI_ICON1`**（AGENTS.md 坑 #21）：`IDI_ICON1` 不是 SDK 宏，
/// rc.exe 会把未定义标识符当作**字符串名**，资源就变成命名项，`LoadImageW` 传数字
/// ID 恒失败 -> 托盘退回系统通用图标，且毫无报错。
const ICON_ID: usize = 1;

/// 禁用态的 alpha 系数（百分比）：45% 不透明度。
///
/// 取值依据：低于 ~30% 在深色任务栏上就基本看不见了；高于 ~60% 与启用态难以区分。
/// 45% 在两种主题下都能一眼看出"淡了"，同时仍认得出是那个双方块图标。
const DISABLED_ALPHA_PCT: u32 = 45;

/// 生成托盘图标句柄；失败返回 0（调用方回退到系统默认图标）。
/// `enabled` 只影响不透明度：true = 原色（生效中），false = 淡化（已禁用）。
pub fn tray_icon(enabled: bool) -> isize {
    let (cx, cy) = small_icon_size();
    let base = load_embedded(cx, cy);
    if base == 0 {
        return 0;
    }
    if enabled {
        return base;
    }
    match fade(base, cx, cy) {
        // 淡化失败时**返回原色图标**而不是 0：形状一致性优先于状态指示。
        // 返回 0 会让调用方退回系统通用图标，那等于为了一个次要提示丢掉主要辨识度。
        0 => base,
        dimmed => {
            destroy(base);
            dimmed
        }
    }
}

/// 托盘/小图标的标准边长（DPI 感知：150% 下是 24）。
/// 显式按这个尺寸取，让 Windows 从 `.ico` 里挑**对应那一帧**，
/// 而不是拿 32px 帧再缩 —— 后者在 16px 下会把描边糊成灰边。
fn small_icon_size() -> (i32, i32) {
    unsafe {
        let cx = GetSystemMetrics(SM_CXSMICON).max(16);
        let cy = GetSystemMetrics(SM_CYSMICON).max(16);
        (cx, cy)
    }
}

/// 从**本进程**的 PE 资源段加载程序图标。
///
/// 注意 `hInstance` 必须传本模块句柄（资源在自己的 exe 里）。这与 AGENTS.md 坑 #2
/// 的场景相反：那条讲的是取**系统**预定义图标（`IDI_APPLICATION` 之类）时必须传
/// NULL，否则会去自己的 exe 里找、找不到就返回 NULL。两条规则不冲突，方向相反。
fn load_embedded(cx: i32, cy: i32) -> isize {
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let h = LoadImageW(
            inst,
            ICON_ID as usize as *const u16,
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        ) as isize;
        if h == 0 {
            // 取不到就**说出来**。静默退回系统图标正是坑 #21 的形态：
            // 图标句柄为 NULL 时 NIM_ADD 依然成功，用户只看到"图标好丑"，
            // 日志里一个字都没有，排查全靠猜。
            eprintln!(
                "anydrag-win: LoadImageW(ICON_ID={ICON_ID}) 失败（err={}），托盘将退回系统默认图标；\
                 检查 assets/app.rc 里图标资源的 ID 是否为数字 1、以及 build.bat 是否带 rc.exe 资源编译",
                std::io::Error::last_os_error()
            );
        }
        h
    }
}

/// 把图标整体淡化（禁用态），返回新 HICON；失败返回 0。
///
/// 走 `GetIconInfo` + `GetDIBits` 而不是 `DrawIconEx` 到 DIB：GDI 绘制**不保留
/// alpha 通道**（画进 32bpp DIB 后 alpha 普遍是 0），那样淡化出来是全透明的空图标。
/// `GetDIBits` 直接读位图原始像素，alpha 是真实值。
fn fade(hicon: isize, cx: i32, cy: i32) -> isize {
    unsafe {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon as _, &mut ii) == 0 {
            return 0;
        }
        let mut buf = vec![0u8; (cx * cy * 4) as usize];
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = cx;
        bi.bmiHeader.biHeight = -cy; // 负高度 = 自上而下
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;

        let hdc: HDC = GetDC(0 as _);
        let got = windows_sys::Win32::Graphics::Gdi::GetDIBits(
            hdc,
            ii.hbmColor,
            0,
            cy as u32,
            buf.as_mut_ptr() as *mut c_void,
            &mut bi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(0 as _, hdc);
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor as _);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask as _);
        }
        if got == 0 {
            return 0;
        }

        // 有些图标帧的 hbmColor 不带 alpha（alpha 全 0，靠掩码位图表达透明）。
        // 直接淡化会把整张图乘成全透明 = 托盘上什么都看不见。此时按"全不透明"处理。
        let alpha_missing = buf.chunks_exact(4).all(|p| p[3] == 0);
        for p in buf.chunks_exact_mut(4) {
            let a = if alpha_missing { 255 } else { p[3] };
            let (b, g, r, a) = fade_px(p[0], p[1], p[2], a, DISABLED_ALPHA_PCT);
            p[0] = b;
            p[1] = g;
            p[2] = r;
            p[3] = a;
        }
        icon_from_bgra(&buf, cx, cy)
    }
}

/// 单个像素的淡化：四个通道**同比例**缩放。
///
/// 为什么不只乘 alpha：`CreateIconIndirect` 吃的是**预乘 alpha** 的 BGRA
/// （颜色已乘过 alpha）。只降 alpha 而不降颜色，会让颜色与 alpha 不再满足预乘关系，
/// 渲染出来是"边缘发亮"的错色。四个通道一起乘，预乘关系恒等保持。
///
/// 纯函数，可单测（不碰 FFI）。
fn fade_px(b: u8, g: u8, r: u8, a: u8, pct: u32) -> (u8, u8, u8, u8) {
    let k = pct.min(100) as u32;
    let f = |v: u8| ((v as u32 * k) / 100) as u8;
    (f(b), f(g), f(r), f(a))
}

/// 把 BGRA 像素缓冲做成 HICON（预乘 alpha，自上而下）。
fn icon_from_bgra(px: &[u8], cx: i32, cy: i32) -> isize {
    unsafe {
        let hdc: HDC = GetDC(0 as _);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = cx;
        bi.bmiHeader.biHeight = -cy;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(0 as _, hdc);
        if color.is_null() || bits.is_null() {
            return 0;
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());

        // 掩码位图：32 位带 alpha 的图标实际由 alpha 决定透明，但 API 要求提供，
        // 全 0 表示"不额外抠掉任何像素"。
        let stride = ((cx as usize + 31) / 32) * 4;
        let zeros = vec![0u8; stride * cy as usize];
        let mask = CreateBitmap(cx, cy, 1, 1, zeros.as_ptr() as *const c_void);
        let ii = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let hicon = CreateIconIndirect(&ii);
        DeleteObject(color as _);
        if !mask.is_null() {
            DeleteObject(mask as _);
        }
        hicon as isize
    }
}

/// 显式释放图标句柄（切换启用态时先销毁旧的，避免每次切换泄漏一个 HICON）。
pub fn destroy(hicon: isize) {
    if hicon != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(hicon as *mut c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯逻辑守卫（不碰 FFI）：淡化必须**四通道同比例**，否则破坏预乘 alpha。
    ///
    /// 这条替代了原先的 `icon_shape_and_coverage`（那条钉的是自绘箭头几何，
    /// 箭头代码已随本次改动删除 —— 留着未使用的形状代码就是坑 #18 的"两个实现"）。
    #[test]
    fn fade_scales_all_channels_equally() {
        // 全不透明像素：45% → 每个通道都按同一比例缩小，预乘关系保持。
        assert_eq!(fade_px(255, 255, 255, 255, 45), (114, 114, 114, 114));
        // 预乘语义：颜色与 alpha 同步缩放后，color/alpha 比值不变。
        let (b, _, _, a) = fade_px(200, 100, 50, 255, 45);
        assert_eq!((b, a), (90, 114));
        assert!(
            (b as f32 / a as f32 - 200.0 / 255.0).abs() < 0.02,
            "premultiplied ratio must survive fading"
        );
        // 全透明像素必须保持全透明（不能淡出一个可见的黑块）。
        assert_eq!(fade_px(0, 0, 0, 0, 45), (0, 0, 0, 0));
        // 边界：0% 全透明，100% 原样。
        assert_eq!(fade_px(120, 130, 140, 255, 0), (0, 0, 0, 0));
        assert_eq!(fade_px(120, 130, 140, 255, 100), (120, 130, 140, 255));
        // 越界百分比不得 panic 或溢出（钳到 100）。
        assert_eq!(fade_px(120, 130, 140, 255, 250), (120, 130, 140, 255));
    }
}

/// 兜底：绘制失败时退回系统默认图标（保持"至少有个图标"）。
/// 注意返回的是 `LR_SHARED` 共享句柄，**不可**用 `destroy()` 销毁。
pub fn fallback() -> isize {
    unsafe {
        LoadImageW(
            std::ptr::null_mut(),
            windows_sys::Win32::UI::WindowsAndMessaging::IDI_APPLICATION,
            IMAGE_ICON,
            0,
            0,
            windows_sys::Win32::UI::WindowsAndMessaging::LR_DEFAULTSIZE
                | windows_sys::Win32::UI::WindowsAndMessaging::LR_SHARED,
        ) as isize
    }
}
