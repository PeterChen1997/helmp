"""从设计稿图标（白底方块）生成 macOS 正式图标与菜单栏模板图。

用法：python3 -I script/make-icons.py design/assets/helmp-icon-b.png resources/
产物：AppIcon.icns、menubar-template.png（18px）、menubar-template@2x.png（36px）
"""
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

src, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)

# 1. 找到方块边界：与纯白差异明显的像素。
img = Image.open(src).convert("RGBA")
diff = ImageChops.difference(img.convert("RGB"), Image.new("RGB", img.size, (255, 255, 255)))
bbox = diff.convert("L").point(lambda v: 255 if v > 24 else 0).getbbox()
tile = img.crop(bbox)

# 2. 按 macOS 图标网格：1024 画布、824 方块、圆角约 22.4%，内缩 2px 去掉白边。
SIZE, BODY = 1024, 824
tile = tile.resize((BODY + 8, BODY + 8), Image.LANCZOS).crop((4, 4, BODY + 4, BODY + 4))
mask = Image.new("L", (BODY * 4, BODY * 4), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, BODY * 4 - 1, BODY * 4 - 1), radius=int(BODY * 4 * 0.224), fill=255)
mask = mask.resize((BODY, BODY), Image.LANCZOS)
tile.putalpha(mask)

canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
offset = (SIZE - BODY) // 2
shadow = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
shadow_mask = Image.new("L", (SIZE, SIZE), 0)
shadow_mask.paste(mask, (offset, offset + 12))
shadow.putalpha(shadow_mask.point(lambda v: v * 0.28).filter(ImageFilter.GaussianBlur(18)))
canvas = Image.alpha_composite(canvas, shadow)
canvas.paste(tile, (offset, offset), tile)
canvas.save(out / "AppIcon-1024.png")

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
