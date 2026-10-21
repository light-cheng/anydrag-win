//! 托盘图标：**运行时绘制**，不内嵌资源文件、不需要 `.rc` 或资源编译器
//! （"不内嵌资源文件"原则继续成立）。
//!
//! 图形语义：**四向移动箭头**——本工具做的就是"拖窗口"，箭头最直白，
//! 而且比 `IDI_APPLICATION`（一个通用窗口图标，跟谁都撞脸）有辨识度。
//! 配色即状态：**启用 = 亮蓝**，**禁用 = 灰**，托盘上扫一眼就知道当前生效与否
//! （比原先借用 `IDI_QUESTION` 这种系统图标更清楚）。
//!
//! 实现：把形状解析式光栅化进 32 位预乘 BGRA 的 DIB（3×3 超采样抗锯齿），
//! 再 `CreateIconIndirect` 成 HICON。32px 交给系统缩到托盘实际尺寸（通常 16px），
//! 因为有抗锯齿边缘，缩小后仍然清晰。

use std::ffi::c_void;

use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

/// 画布边长（像素）。
const SIZE: i32 = 32;
/// 每像素每轴的超采样份数（3×3 = 9 个采样点算覆盖率）。
const SS: i32 = 3;

// 箭头几何（以画布中心为原点的局部坐标，单位像素）。
const L: f32 = 8.5; // 箭杆半长
const T: f32 = 2.6; // 箭杆半粗
const HL: f32 = 6.0; // 箭头三角的长
const HW: f32 = 6.0; // 箭头三角的半宽

/// 生成托盘图标句柄；失败返回 0（调用方回退到系统默认图标）。
/// `enabled` 只影响配色：true = 亮蓝（生效中），false = 灰（已禁用）。
pub fn tray_icon(enabled: bool) -> isize {
    let (r, g, b) = if enabled {
        (45u32, 127, 249) // #2D7FF9
    } else {
        (150, 156, 166) // #969CA6
    };
    unsafe {
        let hdc: HDC = GetDC(0 as _);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = SIZE;
        bi.bmiHeader.biHeight = -SIZE; // 负高度 = 自上而下，内存顺序与图像一致
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(hdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        ReleaseDC(0 as _, hdc);
        if color.is_null() || bits.is_null() {
            return 0;
        }
        let px = bits as *mut u8;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let a = coverage(x, y);
                let i = ((y * SIZE + x) * 4) as usize;
                // CreateIconIndirect 期望**预乘** alpha 的 BGRA。
                *px.add(i) = (b * a as u32 / 255) as u8;
                *px.add(i + 1) = (g * a as u32 / 255) as u8;
                *px.add(i + 2) = (r * a as u32 / 255) as u8;
                *px.add(i + 3) = a;
            }
        }
        // 掩码位图：32 位带 alpha 的图标实际由 alpha 决定透明，但 API 要求提供，
        // 全 0 表示"不额外抠掉任何像素"。
        let stride = ((SIZE as usize + 31) / 32) * 4;
        let zeros = vec![0u8; stride * SIZE as usize];
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, zeros.as_ptr() as *const c_void);
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

/// 像素 (x,y) 的覆盖率 → 0..=255 的 alpha（3×3 超采样）。
fn coverage(x: i32, y: i32) -> u8 {
    let mut hit = 0u32;
    for sy in 0..SS {
        for sx in 0..SS {
            let fx = x as f32 + (sx as f32 + 0.5) / SS as f32 - SIZE as f32 / 2.0;
            let fy = y as f32 + (sy as f32 + 0.5) / SS as f32 - SIZE as f32 / 2.0;
            if inside(fx, fy) {
                hit += 1;
            }
        }
    }
    (hit * 255 / (SS * SS) as u32) as u8
}

/// 点是否落在"四向箭头"内部（十字箭杆 + 四个三角箭头）。
fn inside(x: f32, y: f32) -> bool {
    // 十字箭杆（两个矩形）
    if x.abs() <= L && y.abs() <= T {
        return true;
    }
    if y.abs() <= L && x.abs() <= T {
        return true;
    }
    // 四个箭头三角
    tri(x, y, (L + HL, 0.0), (L, -HW), (L, HW))
        || tri(x, y, (-(L + HL), 0.0), (-L, HW), (-L, -HW))
        || tri(x, y, (0.0, -(L + HL)), (HW, -L), (-HW, -L))
        || tri(x, y, (0.0, L + HL), (-HW, L), (HW, L))
}

/// 点 (x,y) 是否在三角形 (v1,v2,v3) 内（同号法，含边界）。
fn tri(x: f32, y: f32, v1: (f32, f32), v2: (f32, f32), v3: (f32, f32)) -> bool {
    let s = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    let p = (x, y);
    let d1 = s(p, v1, v2);
    let d2 = s(p, v2, v3);
    let d3 = s(p, v3, v1);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
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

    /// 纯逻辑守卫（不碰 FFI）：形状与覆盖率。
    /// 跑 `cargo test icon_ -- --nocapture` 会打印 ASCII 图形，可肉眼确认形状。
    #[test]
    fn icon_shape_and_coverage() {
        let mut art = String::from("\n");
        for y in 0..SIZE {
            for x in 0..SIZE {
                let a = coverage(x, y);
                art.push(if a > 200 {
                    '#'
                } else if a > 40 {
                    '+'
                } else {
                    '.'
                });
            }
            art.push('\n');
        }
        println!("{art}");

        // 中心实心、四角透明
        assert_eq!(coverage(SIZE / 2, SIZE / 2), 255, "center must be solid");
        assert_eq!(coverage(0, 0), 0, "corner must be transparent");
        // 四向都要够到接近边缘（说明是"四向箭头"而不是单箭头/十字残条）
        assert!(coverage(SIZE / 2, 4) > 200, "up arm reaches near the top");
        assert!(
            coverage(SIZE / 2, SIZE - 5) > 200,
            "down arm reaches near the bottom"
        );
        assert!(coverage(4, SIZE / 2) > 200, "left arm reaches near the left");
        assert!(
            coverage(SIZE - 5, SIZE / 2) > 200,
            "right arm reaches near the right"
        );
        // 水平/垂直镜像对称（形状以画布中心为对称点）
        for y in 0..SIZE {
            for x in 0..SIZE {
                assert_eq!(
                    coverage(x, y),
                    coverage(SIZE - 1 - x, y),
                    "horizontal symmetry at ({x},{y})"
                );
                assert_eq!(
                    coverage(x, y),
                    coverage(x, SIZE - 1 - y),
                    "vertical symmetry at ({x},{y})"
                );
            }
        }
    }
}

/// 兜底：绘制失败时退回系统默认图标（保持"至少有个图标"）。
/// 注意返回的是 `LR_SHARED` 共享句柄，**不可**用 `destroy()` 销毁。
pub fn fallback() -> isize {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::LoadImageW(
            std::ptr::null_mut(),
            windows_sys::Win32::UI::WindowsAndMessaging::IDI_APPLICATION,
            windows_sys::Win32::UI::WindowsAndMessaging::IMAGE_ICON,
            0,
            0,
            windows_sys::Win32::UI::WindowsAndMessaging::LR_DEFAULTSIZE
                | windows_sys::Win32::UI::WindowsAndMessaging::LR_SHARED,
        ) as isize
    }
}
