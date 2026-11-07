"""Draws the NSIS installer bitmaps from the AIO Merger brand mark.

    python scripts/make-installer-art.py

Writes src-tauri/installer/nsis-sidebar.bmp (164x314) and nsis-header.bmp (150x57).
Drawn at 4x and downsampled for clean anti-aliasing. Needs Pillow.
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "src-tauri" / "installer"
FONT = ROOT / "src" / "fonts" / "geist-latin-wght-normal.woff2"
SS = 4

INK, PAPER, MID = (12, 12, 13), (250, 250, 250), (113, 113, 122)
AMBER = CREAM = TEAL = CORAL = PAPER


def bezier(p0, p1, p2, p3, n=80):
    pts = []
    for i in range(n + 1):
        t = i / n
        a, b, c, d = (1 - t) ** 3, 3 * (1 - t) ** 2 * t, 3 * (1 - t) * t * t, t ** 3
        pts.append((a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]))
    return pts


def stroke(draw, pts, color, width):
    """Round-capped thick stroke, stamped as dense discs (no joint artifacts)."""
    r = width / 2
    dense = []
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        steps = max(1, int(((x1 - x0) ** 2 + (y1 - y0) ** 2) ** 0.5 / (r * 0.15)))
        dense += [(x0 + (x1 - x0) * k / steps, y0 + (y1 - y0) * k / steps) for k in range(steps)]
    dense.append(pts[-1])
    for x, y in dense:
        draw.ellipse((x - r, y - r, x + r, y + r), fill=color)


def mark(draw, ox, oy, s):
    """The logo on a 1024-unit grid, scaled by s and offset to (ox, oy)."""
    P = lambda x, y: (ox + x * s, oy + y * s)
    w = 66 * s
    stroke(draw, bezier(P(196, 318), P(420, 318), P(440, 512), P(604, 512)), AMBER, w)
    stroke(draw, bezier(P(196, 706), P(420, 706), P(440, 512), P(604, 512)), TEAL, w)
    stroke(draw, [P(196, 512), P(604, 512)], CREAM, w)
    draw.polygon([P(612, 336), P(856, 512), P(612, 688)], fill=CORAL)


def gradient(w, h, top, bottom):
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        t = y / (h - 1)
        c = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        for x in range(w):
            px[x, y] = c
    return img


def glow(img, cx, cy, r, color, alpha):
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    ImageDraw.Draw(layer).ellipse((cx - r, cy - r, cx + r, cy + r), fill=color + (alpha,))
    layer = layer.filter(ImageFilter.GaussianBlur(r * 0.6))
    img.paste(layer, (0, 0), layer)


def font(size):
    try:
        f = ImageFont.truetype(str(FONT), size)
        f.set_variation_by_axes([600])
        return f
    except Exception:
        return None


def sidebar():
    W, H = 164 * SS, 314 * SS
    img = Image.new("RGB", (W, H), INK)
    d = ImageDraw.Draw(img)
    mark(d, W * 0.14, H * 0.2, W * 0.62 / 1024)
    f = font(14 * SS)
    if f:
        d.text((18 * SS, H - 46 * SS), "AIO Merger", font=f, fill=PAPER)
    img.resize((164, 314), Image.LANCZOS).save(OUT / "nsis-sidebar.bmp")


def header():
    W, H = 150 * SS, 57 * SS
    img = Image.new("RGB", (W, H), INK)
    d = ImageDraw.Draw(img)
    s = H * 0.8 / 1024
    mark(d, W - 1024 * s * 0.9 - 8 * SS, H * 0.1, s)
    img.resize((150, 57), Image.LANCZOS).save(OUT / "nsis-header.bmp")


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    sidebar()
    header()
    print("wrote", OUT / "nsis-sidebar.bmp", "and", OUT / "nsis-header.bmp")
