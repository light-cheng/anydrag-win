#!/usr/bin/env python3
"""生成 anydrag-win 的 .ico：两个错位圆角方块（概念见 README 头图）。

设计（方向来自用户选定 + 一轮 AI 探索后收敛，最终由本脚本精确绘制）：

    后层  灰色细描边、空心圆角方，位于左上
    前层  白色填充 + 炭黑描边的圆角方，位于右下，压住后层约三分之一
    两者之间留一圈**透明**间隙，让"两个窗口"的层次不含糊

为什么用代码绘制而不是直接缩小 AI 出图：
1. AI 出图在 16px 下必然糊成色块（实测 v4/v5 全部方案）。托盘图标 99% 时间
   就是 16px，而 16px 只有 256 个像素——两层圆角方 + 间隙根本放不下，
   必须**逐尺寸单独光栅化并单独调笔画粗细**，缩小一张大图做不到这一点。
2. AI 图带不可控的背景（黑边/云絮/渐变），而 .ico 必须是干净 alpha。
3. 几何要精确：前层压后层的比例、间隙宽度、圆角半径必须严格一致。

因此：每个尺寸独立渲染（8x 超采样 → 降采样），笔画宽度按尺寸取整到整数像素，
保证小尺寸下描边不会碎成灰边。
"""
import struct
import sys
from pathlib import Path
from PIL import Image, ImageDraw

# 中性深灰单色（用户选定）。两个灰阶用来区分前后层，白填充让前层在任何
# 背景上都可见——纯深灰实心在深色任务栏上会消失（v4 的 16px 实测）。
CHARCOAL = (46, 51, 56, 255)      # #2E3338 前层描边
GREY = (154, 160, 166, 255)       # #9AA0A6 后层描边
WHITE = (255, 255, 255, 255)

# 几何（画布比例）。前层比后层低、右移，压住约 1/3。
BOX = 0.60          # 单个方块的边长
BACK_XY = (0.085, 0.085)
FRONT_XY = (0.315, 0.315)
RADIUS = 0.16       # 圆角半径（占方块边长）

# 每尺寸的笔画宽度（以最终像素为单位）与间隙宽度。
# 小尺寸必须加粗笔画、收窄间隙，否则 16px 下两层会糊成一团。
SPEC = {
    16: dict(back=1.0, front=1.2, gap=1.0),
    24: dict(back=1.4, front=1.6, gap=1.2),
    32: dict(back=1.8, front=2.2, gap=1.6),
    48: dict(back=2.6, front=3.2, gap=2.4),
    64: dict(back=3.4, front=4.2, gap=3.2),
    128: dict(back=6.8, front=8.4, gap=6.4),
    256: dict(back=13.6, front=16.8, gap=12.8),
}

SS = 8  # 超采样倍率


def render(size: int) -> Image.Image:
    """按目标尺寸独立渲染（不是缩放大图）。"""
    sp = SPEC[size]
    W = size * SS
    img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    box = BOX * W
    r = RADIUS * box
    bx, by = BACK_XY[0] * W, BACK_XY[1] * W
    fx, fy = FRONT_XY[0] * W, FRONT_XY[1] * W

    def rounded(x, y, w, stroke, color):
        d.rounded_rectangle([x, y, x + w, y + w], radius=r, outline=color, width=max(1, int(round(stroke * SS))))

    # 后层：空心灰描边
    rounded(bx, by, box, sp["back"], GREY)

    # 前层：白色填充 + 炭黑描边。
    #
    # **不要额外"挖间隙"**（v1 试过，已删）：橡皮擦会连带切断后层的轮廓，
    # 留下一道宽度不均的断口，看着像渲染 bug。前层本身就是**实心**的，
    # 它自然遮住后层的重叠段——"前层压着后层"的层次由遮挡关系表达，
    # 不需要人为造缝。这是双窗口图标的标准做法。
    d.rounded_rectangle([fx, fy, fx + box, fy + box], radius=r, fill=WHITE, outline=CHARCOAL,
                        width=max(1, int(round(sp["front"] * SS))))

    return img.resize((size, size), Image.LANCZOS)


def write_ico(path: str, sizes) -> None:
    """手写 ICO 容器：每个尺寸一个 PNG 载荷（Vista+ 支持全部尺寸用 PNG）。"""
    blobs = []
    for s in sizes:
        im = render(s)
        tmp = f"/tmp/_ico_{s}.png"
        im.save(tmp, format="PNG")
        with open(tmp, "rb") as f:
            blobs.append((s, f.read()))

    n = len(blobs)
    header = struct.pack("<HHH", 0, 1, n)
    offset = 6 + 16 * n
    entries = b""
    data = b""
    for s, blob in blobs:
        # 宽高：256 记作 0
        dim = 0 if s >= 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
        data += blob

    with open(path, "wb") as f:
        f.write(header + entries + data)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else str(Path(__file__).resolve().parents[1] / "anydrag-win.ico")
    sizes = [16, 24, 32, 48, 64, 128, 256]
    write_ico(out, sizes)
    print(f"wrote {out} sizes={sizes}")

    # 逐尺寸预览（最近邻放大，看真实像素）
    for s in (16, 24, 32, 48, 256):
        render(s).resize((s * 8, s * 8), Image.NEAREST).save(f"/tmp/check_{s}.png")
    print("previews written to /tmp/check_*.png")
