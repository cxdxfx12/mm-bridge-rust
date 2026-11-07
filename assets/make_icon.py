"""生成 Rust 版应用图标（多尺寸 .ico + 预览 png）。

设计与 design/ui-mockup.html 的品牌主色 #2F6BED 一致：
圆角方形蓝色渐变底 + 一组白色双向箭头（钉钉 → 微信 的「转发桥」）。
全部在 4 倍画布上绘制后 LANCZOS 下采样，保证 16px 依然清晰。

用法：python rust/assets/make_icon.py
"""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
S = 256
SS = 4  # 超采样倍率
W = S * SS

TOP = (92, 151, 255)     # #5C97FF
BOTTOM = (35, 82, 199)   # #2352C7
SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]


def px(v: float) -> int:
    return int(round(v * SS))


def gradient_base() -> Image.Image:
    """垂直渐变 + 左上高光，先按 S 尺寸算再放大，省内存。"""
    small = Image.new("RGB", (S, S), TOP)
    p = small.load()
    for y in range(S):
        f = y / (S - 1)
        # 稍微带一点横向明暗，避免纯线性渐变的塑料感
        for x in range(S):
            g = min(1.0, max(0.0, f * 0.86 + (x / S) * 0.14))
            p[x, y] = (
                int(TOP[0] + (BOTTOM[0] - TOP[0]) * g),
                int(TOP[1] + (BOTTOM[1] - TOP[1]) * g),
                int(TOP[2] + (BOTTOM[2] - TOP[2]) * g),
            )
    # 左上柔光
    glow = Image.new("L", (S, S), 0)
    gd = ImageDraw.Draw(glow)
    gd.ellipse((-70, -130, 230, 120), fill=70)
    glow = glow.filter(ImageFilter.GaussianBlur(38))
    white = Image.new("RGB", (S, S), (255, 255, 255))
    small = Image.composite(white, small, glow.point(lambda a: int(a * 0.34)))
    return small.resize((W, W), Image.LANCZOS)


def round_line(d: ImageDraw.ImageDraw, pts, width: int, fill) -> None:
    d.line(pts, fill=fill, width=width, joint="curve")
    r = width / 2
    for x, y in (pts[0], pts[-1]):
        d.ellipse((x - r, y - r, x + r, y + r), fill=fill)


def arrow(d: ImageDraw.ImageDraw, y: float, direction: int, fill) -> None:
    """direction=1 向右、-1 向左；中心线 y 用 256 坐标系。"""
    cy = px(y)
    x_left, x_right = px(66), px(190)
    width = px(19)
    head = px(34)
    if direction == 1:
        tip = x_right
        tail = x_left
        off = head
    else:
        tip = x_left
        tail = x_right
        off = -head
    round_line(d, [(tail, cy), (tip, cy)], width, fill)
    round_line(d, [(tip - off, cy - head), (tip, cy)], width, fill)
    round_line(d, [(tip - off, cy + head), (tip, cy)], width, fill)


def build() -> Image.Image:
    base = gradient_base().convert("RGBA")

    # 圆角遮罩（角半径 56 / 256）
    mask = Image.new("L", (W, W), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (px(6), px(6), px(250), px(250)), radius=px(56), fill=255
    )

    # 箭头阴影先画在半透明层上，制造一点浮起感
    shadow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    arrow(sd, 101 + 3, 1, (10, 40, 110, 90))
    arrow(sd, 157 + 3, -1, (10, 40, 110, 90))
    shadow = shadow.filter(ImageFilter.GaussianBlur(px(6)))

    arrows = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    ad = ImageDraw.Draw(arrows)
    arrow(ad, 101, 1, (255, 255, 255, 255))
    arrow(ad, 157, -1, (255, 255, 255, 240))

    out = Image.alpha_composite(base, shadow)
    out = Image.alpha_composite(out, arrows)
    # 顶部一道极细的高光边，让图标在深色任务栏上不糊成一团
    hi = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    hd = ImageDraw.Draw(hi)
    hd.rounded_rectangle(
        (px(8), px(8), px(248), px(248)), radius=px(54),
        outline=(255, 255, 255, 46), width=px(2),
    )
    out = Image.alpha_composite(out, hi)

    out.putalpha(Image.composite(out.getchannel("A"), Image.new("L", (W, W), 0), mask))
    return out.resize((S, S), Image.LANCZOS)


def main() -> None:
    master = build()
    ico = HERE / "app.ico"
    pngs = [master.resize((n, n), Image.LANCZOS) for n, _ in SIZES]
    # PIL 用第一张作为基准，sizes 参数会逐尺寸重采样；这里直接按尺寸列表保存
    master.save(ico, format="ICO", sizes=SIZES)
    master.resize((64, 64), Image.LANCZOS).save(HERE / "app_64.png")
    master.resize((256, 256), Image.LANCZOS).save(HERE / "app_256.png")
    # 运行时窗口图标：egui::IconData 要裸 RGBA，避免为此引入 PNG 解码依赖。
    (HERE / "app_32.rgba").write_bytes(master.resize((32, 32), Image.LANCZOS).convert("RGBA").tobytes())
    print(f"已生成 {ico} ({ico.stat().st_size} 字节)，尺寸：{[n for n, _ in SIZES]}")
    print(f"预览：{HERE / 'app_256.png'}")


if __name__ == "__main__":
    main()
