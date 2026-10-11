"""生成 Windows NSIS 安装器的侧栏与页眉品牌图（不透明 RGB PNG），输出到 assets/brand/swoop/installer/。
打包时 scripts/package-windows.ps1 把它们转成 24 位 BMP；安装器在运行时按对话框实际像素尺寸挑最接近的一档，
所以每档尺寸都要和 scripts/windows-installer/installer.nsi 里 SwoopPick* 宏的阈值一致。
用法：python3 scripts/make_installer_art.py（需要 Pillow 和 rsvg-convert）"""
import io
import os
import subprocess

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.join(os.path.dirname(__file__), "..")
BRAND = os.path.join(ROOT, "assets", "brand", "swoop")
OUT = os.path.join(BRAND, "installer")
FONT = os.path.join(ROOT, "assets", "fonts", "AlibabaPuHuiTi-3-55-Regular.subset.otf")

# 档位：MUI2 的侧栏是 109×193 对话框单位（默认字体 100% 缩放时 164×314 px），页眉 150×57 px。
SCALES = {"100": 1.0, "125": 1.25, "150": 1.5, "175": 1.75, "200": 2.0, "250": 2.5}
BLUE = (0x17, 0x6F, 0xFF)
PURPLE = (0x7C, 0x3C, 0xF4)


def px(base, scale):
    return int(base * scale + 0.5)


def render_svg(name, size):
    png = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), os.path.join(BRAND, name)],
        check=True, capture_output=True,
    ).stdout
    return Image.open(io.BytesIO(png)).convert("RGBA")


def gradient(w, h):
    """左上蓝 → 右下紫的对角渐变。"""
    img = Image.new("RGB", (w, h))
    span = w + h - 2 or 1
    img.putdata([
        tuple(int(a + (b - a) * (x + y) / span + 0.5) for a, b in zip(BLUE, PURPLE))
        for y in range(h) for x in range(w)
    ])
    return img


def sidebar(scale):
    w, h = px(164, scale), px(314, scale)
    img = gradient(w, h).convert("RGBA")
    # 右下角放一枚放大的白色标志做水印，只露出一部分。
    big = render_svg("swoop-white.svg", px(300, scale))
    alpha = big.getchannel("A").point(lambda a: a * 10 // 100)
    big.putalpha(alpha)
    img.alpha_composite(big, (w - px(170, scale), h - px(190, scale)))
    logo = render_svg("swoop-white.svg", px(84, scale))
    logo_top = px(84, scale)
    img.alpha_composite(logo, ((w - logo.width) // 2, logo_top))
    draw = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT, px(26, scale))
    text = "Swoop"
    left, top, right, bottom = draw.textbbox((0, 0), text, font=font)
    draw.text(((w - (right - left)) // 2 - left, logo_top + logo.height + px(10, scale) - top),
              text, font=font, fill=(255, 255, 255, 255))
    return img.convert("RGB")


def header(scale):
    w, h = px(150, scale), px(57, scale)
    img = Image.new("RGBA", (w, h), (255, 255, 255, 255))
    logo = render_svg("swoop.svg", px(44, scale))
    img.alpha_composite(logo, (w - logo.width - px(8, scale), (h - logo.height) // 2))
    return img.convert("RGB")


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, scale in SCALES.items():
        for kind, make in (("sidebar", sidebar), ("header", header)):
            img = make(scale)
            path = os.path.join(OUT, f"{kind}-{name}.png")
            img.save(path, optimize=True)
            print(f"{os.path.relpath(path, ROOT)} {img.width}x{img.height}")


if __name__ == "__main__":
    main()
