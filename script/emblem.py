"""从生成的图标原图里找出白色标志（H + 星芒）：连通的白色区域，只取画面中部的大块，排除背景高光。

供 make-icons.py 使用：返回标志的 alpha 蒙版（原图尺寸）与外框。
"""

from collections import deque

from PIL import Image, ImageFilter


def emblem_mask(
    im: Image.Image, scale: int = 4
) -> tuple[Image.Image, tuple[int, int, int, int], tuple[int, int, int, int]]:
    """返回（标志蒙版，标志外框，H 本体外框）。H 本体是最大的那块连通区域。"""
    rgb = im.convert("RGB")
    w, h = rgb.size
    small = rgb.resize((w // scale, h // scale), Image.BILINEAR)
    sw, sh = small.size
    px = small.load()

    def is_white(x: int, y: int) -> bool:
        r, g, b = px[x, y]
        return min(r, g, b) > 205 and max(r, g, b) - min(r, g, b) < 50

    seen = [[False] * sw for _ in range(sh)]
    keep: list[list[tuple[int, int]]] = []
    for y0 in range(sh):
        for x0 in range(sw):
            if seen[y0][x0] or not is_white(x0, y0):
                continue
            comp, queue = [], deque([(x0, y0)])
            seen[y0][x0] = True
            while queue:
                x, y = queue.popleft()
                comp.append((x, y))
                for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                    if 0 <= nx < sw and 0 <= ny < sh and not seen[ny][nx] and is_white(nx, ny):
                        seen[ny][nx] = True
                        queue.append((nx, ny))
            cx = sum(p[0] for p in comp) / len(comp) / sw
            cy = sum(p[1] for p in comp) / len(comp) / sh
            # 标志在画面中部且有一定面积；背景高光贴边或很细碎。
            if len(comp) > (sw * sh) * 0.002 and 0.2 < cx < 0.8 and 0.15 < cy < 0.85:
                keep.append(comp)
    if not keep:
        raise SystemExit("没找到白色标志")
    mask_small = Image.new("L", (sw, sh), 0)
    mp = mask_small.load()
    for comp in keep:
        for x, y in comp:
            mp[x, y] = 255
    bbox_small = mask_small.getbbox()
    bbox = tuple(v * scale for v in bbox_small)
    # 只在这些连通区域（放大并略微外扩）里取精细蒙版，排除外框内背景高光的零星白点。
    region = mask_small.resize((w, h), Image.NEAREST).filter(ImageFilter.MaxFilter(scale * 2 + 1))
    largest = max(keep, key=len)
    xs = [p[0] for p in largest]
    ys = [p[1] for p in largest]
    h_bbox = (min(xs) * scale, min(ys) * scale, (max(xs) + 1) * scale, (max(ys) + 1) * scale)
    # 回到原图尺寸取精细蒙版：外框内的白色像素。
    mask = Image.new("L", (w, h), 0)
    full = rgb.load()
    m = mask.load()
    region_px = region.load()
    pad = scale * 2
    for y in range(max(0, bbox[1] - pad), min(h, bbox[3] + pad)):
        for x in range(max(0, bbox[0] - pad), min(w, bbox[2] + pad)):
            r, g, b = full[x, y]
            lum = min(r, g, b)
            if region_px[x, y] and lum > 150 and max(r, g, b) - min(r, g, b) < 90:
                m[x, y] = 255 if lum > 205 else int((lum - 150) / 55 * 255)
    return mask, mask.getbbox(), h_bbox
