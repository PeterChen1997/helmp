"""从生图工具生成的图标原图制作全部图标，菜单栏图标直接取自原图里的白色标志，保证一致。

用法：python3 -I script/make-icons.py design/assets/helmp-icon-v3.png resources/
产物：AppIcon.icns / AppIcon-1024.png / AppIcon-64.png、menubar-template.png（18px）/ @2x（36px）

原图要求：满版方图（背景铺满四边，没有圆角和白边），中间一个白色标志（H + 星芒）。
"""

import math
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

sys.path.insert(0, str(Path(__file__).parent))
from emblem import emblem_mask  # noqa: E402

src, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)

art = Image.open(src).convert("RGBA")
W, H = art.size
mask, (ex0, ey0, ex1, ey1), (hx0, hy0, hx1, hy1) = emblem_mask(art)

# ---------- App 图标：以标志为中心裁正方形，标志落在正中 ----------
cx, cy = (ex0 + ex1) / 2, (ey0 + ey1) / 2
half = int(min(cx, W - cx, cy, H - cy))
window = art.crop((int(cx - half), int(cy - half), int(cx + half), int(cy + half)))

SIZE, BODY = 1024, 824
body = window.resize((BODY, BODY), Image.LANCZOS)


# macOS 图标网格：1024 画布、824 方块，连续曲率圆角（超椭圆 n=5，接近 Apple 的 squircle）。
def squircle_mask(size: int, n: float = 5.0, oversample: int = 4) -> Image.Image:
    big = size * oversample
    r = big / 2
    points = []
    for i in range(2048):
        t = 2 * math.pi * i / 2048
        c, s = math.cos(t), math.sin(t)
        points.append((r + r * math.copysign(abs(c) ** (2 / n), c), r + r * math.copysign(abs(s) ** (2 / n), s)))
    m = Image.new("L", (big, big), 0)
    ImageDraw.Draw(m).polygon(points, fill=255)
    return m.resize((size, size), Image.LANCZOS)


squircle = squircle_mask(BODY)
body.putalpha(squircle)
canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
offset = (SIZE - BODY) // 2
drop = Image.new("L", (SIZE, SIZE), 0)
drop.paste(squircle, (offset, offset + 10))
drop_layer = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
drop_layer.putalpha(drop.point(lambda v: int(v * 0.30)).filter(ImageFilter.GaussianBlur(16)))
canvas = Image.alpha_composite(canvas, drop_layer)
canvas.alpha_composite(body, (offset, offset))
canvas.save(out / "AppIcon-1024.png")
canvas.resize((64, 64), Image.LANCZOS).save(out / "AppIcon-64.png")

with tempfile.TemporaryDirectory() as tmp:
    iconset = Path(tmp) / "AppIcon.iconset"
    iconset.mkdir()
    for base in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = base * scale
            name = f"icon_{base}x{base}{'@2x' if scale == 2 else ''}.png"
            canvas.resize((px, px), Image.LANCZOS).save(iconset / name)
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out / "AppIcon.icns")], check=True)

# ---------- 菜单栏模板图：取原图里的白色标志 ----------
# 系统图标按主体居中：H 本体垂直居中（星芒占上方留白），整体水平居中；H 高约 13/18。
SUPER = 8
CANVAS = 18 * SUPER
scale = (13 * SUPER) / (hy1 - hy0)
glyph = mask.crop((ex0, ey0, ex1, ey1))
gw, gh = int((ex1 - ex0) * scale), int((ey1 - ey0) * scale)
glyph = glyph.resize((gw, gh), Image.LANCZOS)
left = (CANVAS - gw) // 2
top = int(CANVAS / 2 - ((hy0 + hy1) / 2 - ey0) * scale)
top = max(0, min(top, CANVAS - gh))  # 星芒不出界
alpha = Image.new("L", (CANVAS, CANVAS), 0)
alpha.paste(glyph, (left, top))
template = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
template.putalpha(alpha)
template.resize((36, 36), Image.LANCZOS).save(out / "menubar-template@2x.png")
template.resize((18, 18), Image.LANCZOS).save(out / "menubar-template.png")

print("ok", f"标志中心偏移 ({cx - W / 2:+.0f}, {cy - H / 2:+.0f}) 已按标志重新居中")
