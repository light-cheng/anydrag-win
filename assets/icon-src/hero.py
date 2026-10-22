#!/usr/bin/env python3
"""生成 README 头图 assets/hero.png。

设计意图：README 第一眼要回答三件事——叫什么、干什么、怎么用。
所以头图 = 图标 + 名称 + 一句定位 + 四个手势的极简图示（不靠文字解释手势，
因为手势正是这个工具唯一的交互面）。

文字全部由真实字体渲染（AI 生图会把中文画成乱码，绝不外包）。
配色沿用图标的炭黑/中灰单色，保持与 exe 图标、托盘图标一致。
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 1600, 560
BG = (255, 255, 255)
INK = (46, 51, 56)        # #2E3338 主文字
GREY = (154, 160, 166)    # #9AA0A6 次要文字
HAIR = (232, 234, 237)    # 分隔线

F_SF = "/System/Library/Fonts/SFNS.ttf"
F_BOLD = "/System/Library/Fonts/Supplemental/Arial Bold.ttf"


def font(path, size):
    return ImageFont.truetype(path, size)


def rounded(d, box, r, outline, width):
    d.rounded_rectangle(box, radius=r, outline=outline, width=width)


def draw_icon(d, x, y, s):
    """与 assets/anydrag-win.ico 同构的错位双方块（直接复用同一几何）。"""
    box = s * 0.60
    r = int(box * 0.16)
    bx, by = x + s * 0.085, y + s * 0.085
    fx, fy = x + s * 0.315, y + s * 0.315
    sw = max(2, int(s * 0.053))
    fw = max(2, int(s * 0.066))
    rounded(d, [bx, by, bx + box, by + box], r, GREY, sw)
    d.rounded_rectangle([fx, fy, fx + box, fy + box], radius=r,
                        fill=(255, 255, 255), outline=INK, width=fw)


def draw_gesture(d, x, y, kind):
    """四个手势的极简线稿（56x56 区域内）。"""
    s = 64
    lw = 2
    c = (x + s // 2, y + s // 2)
    if kind == "drag":          # 窗口 + 四向箭头
        rounded(d, [x + 4, y + 12, x + 40, y + 40], 5, INK, lw)
        ax = x + 42
        d.line([ax - 7, c[1], ax + 7, c[1]], fill=INK, width=lw)
        d.line([ax, c[1] - 7, ax, c[1] + 7], fill=INK, width=lw)
        d.polygon([(ax + 7, c[1]), (ax + 3, c[1] - 3), (ax + 3, c[1] + 3)], fill=INK)
        d.polygon([(ax - 7, c[1]), (ax - 3, c[1] - 3), (ax - 3, c[1] + 3)], fill=INK)
        d.polygon([(ax, c[1] - 7), (ax - 3, c[1] - 3), (ax + 3, c[1] - 3)], fill=INK)
        d.polygon([(ax, c[1] + 7), (ax - 3, c[1] + 3), (ax + 3, c[1] + 3)], fill=INK)
    elif kind == "double":      # 窗口 + 双击波纹
        rounded(d, [x + 8, y + 14, x + 44, y + 42], 5, INK, lw)
        for i, rr in enumerate((5, 9)):
            d.ellipse([x + 26 - rr, y + 28 - rr, x + 26 + rr, y + 28 + rr],
                      outline=GREY if i else INK, width=lw)
    elif kind == "resize":      # 窗口 + 对角缩放箭头
        rounded(d, [x + 6, y + 10, x + 34, y + 34], 5, INK, lw)
        d.line([x + 34, y + 34, x + 50, y + 50], fill=GREY, width=lw)
        d.polygon([(x + 50, y + 50), (x + 50, y + 42), (x + 42, y + 50)], fill=GREY)
    else:                       # snap：2x2 网格，一格高亮
        for i in range(2):
            for j in range(2):
                bx = x + 8 + i * 22
                by = y + 8 + j * 22
                if i == 0 and j == 0:
                    d.rounded_rectangle([bx, by, bx + 18, by + 18], radius=4, fill=INK)
                else:
                    rounded(d, [bx, by, bx + 18, by + 18], 4, GREY, lw)


def main():
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)

    # 图标
    draw_icon(d, 80, 96, 190)

    # 标题
    d.text((316, 104), "anydrag-win", font=font(F_BOLD, 68), fill=INK)

    # 定位句
    d.text((318, 196), "Hold Alt and drag anywhere to move a window.",
           font=font(F_SF, 30), fill=GREY)
    d.text((318, 238), "No title bar required. Windows tray utility.",
           font=font(F_SF, 30), fill=GREY)

    # 分隔线
    d.line([80, 348, W - 80, 348], fill=HAIR, width=2)

    # 四个手势
    items = [
        ("drag", "Alt + drag", "Move"),
        ("double", "Alt + double-click", "Maximize / restore"),
        ("resize", "Alt + Ctrl + drag", "Resize from anywhere"),
        ("snap", "Middle-drag", "Snap to edge"),
    ]
    colw = (W - 160) // 4
    for i, (kind, k, v) in enumerate(items):
        x = 80 + i * colw
        draw_gesture(d, x, 392, kind)
        d.text((x + 78, 400), k, font=font(F_SF, 25), fill=INK)
        d.text((x + 78, 434), v, font=font(F_SF, 23), fill=GREY)

    out = Path(__file__).resolve().parents[1] / "hero.png"
    img.save(out)
    print("wrote", out, img.size)


if __name__ == "__main__":
    main()
