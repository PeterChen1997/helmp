"""从设计稿图标（白底方块）生成 macOS 正式图标与菜单栏模板图。

用法：python3 -I script/make-icons.py design/assets/helmp-icon-b-fullbleed.png resources/
产物：AppIcon.icns、AppIcon-1024.png、AppIcon-64.png、menubar-template.png（18px）、menubar-template@2x.png（36px）
"""
import math
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

src, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)

# 1. 源图是满版方图（背景铺满四边，没有圆角和白边），这里统一按 macOS 图标网格裁切：
#    1024 画布、824 方块，连续曲率圆角（超椭圆 n=5，接近 Apple 的 squircle）。
#    不再从白底图里抠方块：那样圆角形状对不上，深色背景下会漏出一圈白边。
SIZE, BODY = 1024, 824
tile = Image.open(src).convert("RGBA").resize((BODY, BODY), Image.LANCZOS)

def squircle_mask(size: int, n: float = 5.0, oversample: int = 4) -> Image.Image:
    big = size * oversample
    r = big / 2
    points = []
    steps = 2048
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s_ = math.cos(t), math.sin(t)
        x = r + r * math.copysign(abs(c) ** (2 / n), c)
        y = r + r * math.copysign(abs(s_) ** (2 / n), s_)
        points.append((x, y))
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).polygon(points, fill=255)
    return mask.resize((size, size), Image.LANCZOS)

mask = squircle_mask(BODY)
tile.putalpha(mask)

canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
offset = (SIZE - BODY) // 2
shadow_mask = Image.new("L", (SIZE, SIZE), 0)
shadow_mask.paste(mask, (offset, offset + 10))
shadow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
shadow.putalpha(shadow_mask.point(lambda v: int(v * 0.30)).filter(ImageFilter.GaussianBlur(16)))
canvas = Image.alpha_composite(canvas, shadow)
canvas.alpha_composite(tile, (offset, offset))
canvas.save(out / "AppIcon-1024.png")
canvas.resize((64, 64), Image.LANCZOS).save(out / "AppIcon-64.png")

# 3. iconset → icns
with tempfile.TemporaryDirectory() as tmp:
    iconset = Path(tmp) / "AppIcon.iconset"
    iconset.mkdir()
    for base in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = base * scale
            name = f"icon_{base}x{base}{'@2x' if scale == 2 else ''}.png"
            canvas.resize((px, px), Image.LANCZOS).save(iconset / name)
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out / "AppIcon.icns")], check=True)

# 4. 菜单栏模板图：H + 右上星芒，纯黑 + alpha，系统会按明暗模式着色。
def glyph(px: int) -> Image.Image:
    k = 8  # 超采样
    s = px * k / 18
    im = Image.new("RGBA", (px * k, px * k), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    w = int(2.2 * s)
    def line(x1, y1, x2, y2):
        d.line((x1 * s, y1 * s, x2 * s, y2 * s), fill=(0, 0, 0, 255), width=w)
        for x, y in ((x1, y1), (x2, y2)):
            d.ellipse((x * s - w / 2, y * s - w / 2, x * s + w / 2, y * s + w / 2), fill=(0, 0, 0, 255))
    line(4, 3.5, 4, 14.5)
    line(11.5, 7.5, 11.5, 14.5)
    line(4, 9, 11.5, 9)
    cx, cy, r, i = 14.2, 3.6, 2.6, 0.75
    pts = [(cx, cy - r), (cx + i, cy - i), (cx + r, cy), (cx + i, cy + i), (cx, cy + r), (cx - i, cy + i), (cx - r, cy), (cx - i, cy - i)]
    d.polygon([(x * s, y * s) for x, y in pts], fill=(0, 0, 0, 255))
    return im.resize((px, px), Image.LANCZOS)

glyph(18).save(out / "menubar-template.png")
glyph(36).save(out / "menubar-template@2x.png")
print("ok")
