"""从同一份 SVG 标志生成全部图标，保证菜单栏图标与 App 图标完全一致。

用法：python3 -I script/make-icons.py resources/
产物：
- helmp-mark.svg                    标志源文件（H + 右上星芒）
- AppIcon.icns / AppIcon-1024.png / AppIcon-64.png
- menubar-template.png（18px）/ menubar-template@2x.png（36px）：纯黑 + alpha，系统按明暗着色
"""

import io
import math
import subprocess
import sys
import tempfile
from pathlib import Path

import cairosvg
from PIL import Image, ImageDraw, ImageFilter

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

# ---------- 标志几何（100 × 100 坐标系） ----------
# H：两根圆头竖条 + 横梁；星芒：四角星，四边内凹，贴在右竖条右上方。
STEM_W = 17
LEFT_X, RIGHT_X = 18, 55
TOP, BOTTOM = 24, 82
BAR_Y, BAR_H = 46, 14
STAR_CX, STAR_CY, STAR_R = 83, 19, 14
STAR_PINCH = 0.22  # 内凹程度：控制点离中心的比例，越小越尖


def star_path(cx: float, cy: float, r: float, k: float) -> str:
    c = r * k
    return (
        f"M{cx},{cy - r} "
        f"Q{cx + c},{cy - c} {cx + r},{cy} "
        f"Q{cx + c},{cy + c} {cx},{cy + r} "
        f"Q{cx - c},{cy + c} {cx - r},{cy} "
        f"Q{cx - c},{cy - c} {cx},{cy - r} Z"
    )


def mark_shapes(fill: str) -> str:
    rx = STEM_W / 2
    return (
        f'<rect x="{LEFT_X}" y="{TOP}" width="{STEM_W}" height="{BOTTOM - TOP}" rx="{rx}" fill="{fill}"/>'
        f'<rect x="{RIGHT_X}" y="{TOP}" width="{STEM_W}" height="{BOTTOM - TOP}" rx="{rx}" fill="{fill}"/>'
        f'<rect x="{LEFT_X + rx}" y="{BAR_Y}" width="{RIGHT_X - LEFT_X}" height="{BAR_H}" fill="{fill}"/>'
        f'<path d="{star_path(STAR_CX, STAR_CY, STAR_R, STAR_PINCH)}" fill="{fill}"/>'
    )


# 标志外接框约 x 18–97、y 5–82；取正方形视窗并留一点边，菜单栏图标居中饱满。
MARK_VIEWBOX = "14 1 86 86"


def mark_svg(fill: str = "#000000") -> str:
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{MARK_VIEWBOX}">{mark_shapes(fill)}</svg>'


def render(svg: str, px: int) -> Image.Image:
    data = cairosvg.svg2png(bytestring=svg.encode(), output_width=px, output_height=px)
    return Image.open(io.BytesIO(data)).convert("RGBA")


(out / "helmp-mark.svg").write_text(mark_svg("#FFFFFF"), encoding="utf-8")

# ---------- 菜单栏模板图 ----------
for px, name in ((18, "menubar-template.png"), (36, "menubar-template@2x.png")):
    render(mark_svg("#000000"), px).save(out / name)

# ---------- App 图标 ----------
SIZE, BODY = 1024, 824

BACKGROUND = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {BODY} {BODY}">
  <defs>
    <linearGradient id="base" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#2AA8FF"/>
      <stop offset="0.55" stop-color="#2F6BF5"/>
      <stop offset="1" stop-color="#6A45EE"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.22" cy="0.12" r="0.75">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.42"/>
      <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="wave" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.16"/>
      <stop offset="1" stop-color="#FFFFFF" stop-opacity="0.02"/>
    </linearGradient>
  </defs>
  <rect width="{BODY}" height="{BODY}" fill="url(#base)"/>
  <rect width="{BODY}" height="{BODY}" fill="url(#glow)"/>
  <path d="M0 {BODY * 0.70} C {BODY * 0.30} {BODY * 0.58}, {BODY * 0.62} {BODY * 0.86}, {BODY} {BODY * 0.66} L {BODY} {BODY} L 0 {BODY} Z" fill="url(#wave)"/>
  <path d="M0 {BODY * 0.84} C {BODY * 0.36} {BODY * 0.74}, {BODY * 0.66} {BODY * 0.98}, {BODY} {BODY * 0.82} L {BODY} {BODY} L 0 {BODY} Z" fill="url(#wave)"/>
</svg>"""

body = render(BACKGROUND, BODY)

# 白色标志：轻微自上而下的冷白渐变，加柔和投影体现层次。
MARK_FILL = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="{MARK_VIEWBOX}">
  <defs>
    <!-- 用整个标志的坐标系，不按单个形状：否则横梁和竖条各算一套渐变，交界处会出现色带。 -->
    <linearGradient id="m" gradientUnits="userSpaceOnUse" x1="0" y1="{TOP}" x2="0" y2="{BOTTOM}">
      <stop offset="0" stop-color="#FFFFFF"/>
      <stop offset="1" stop-color="#E4EAFF"/>
    </linearGradient>
  </defs>
  {mark_shapes("url(#m)")}
</svg>"""
MARK_PX = int(BODY * 0.62)
mark = render(MARK_FILL, MARK_PX)
mark_origin = ((BODY - MARK_PX) // 2 + int(BODY * 0.01), (BODY - MARK_PX) // 2 + int(BODY * 0.02))

shadow = Image.new("RGBA", (BODY, BODY), (0, 0, 0, 0))
shadow_alpha = Image.new("L", (BODY, BODY), 0)
shadow_alpha.paste(mark.getchannel("A"), (mark_origin[0], mark_origin[1] + int(BODY * 0.025)))
shadow = Image.new("RGBA", (BODY, BODY), (24, 18, 120, 0))
shadow.putalpha(shadow_alpha.point(lambda v: int(v * 0.32)).filter(ImageFilter.GaussianBlur(BODY * 0.03)))
body = Image.alpha_composite(body, shadow)
body.alpha_composite(mark, mark_origin)


# macOS 图标网格：1024 画布、824 方块，连续曲率圆角（超椭圆 n=5，接近 Apple 的 squircle）。
def squircle_mask(size: int, n: float = 5.0, oversample: int = 4) -> Image.Image:
    big = size * oversample
    r = big / 2
    points = []
    for i in range(2048):
        t = 2 * math.pi * i / 2048
        c, s = math.cos(t), math.sin(t)
        points.append((r + r * math.copysign(abs(c) ** (2 / n), c), r + r * math.copysign(abs(s) ** (2 / n), s)))
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).polygon(points, fill=255)
    return mask.resize((size, size), Image.LANCZOS)


mask = squircle_mask(BODY)
body.putalpha(mask)

canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
offset = (SIZE - BODY) // 2
drop = Image.new("L", (SIZE, SIZE), 0)
drop.paste(mask, (offset, offset + 10))
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

print("ok")
