#!/usr/bin/env python3
"""Draw src-tauri/app-icon.png: a slanted white "JU" on the brand red.

The letters are strokes and arcs drawn here, not a typeface, so the icon
carries no font licence. After changing it, derive the other sizes with
`cargo tauri icon src-tauri/app-icon.png` (run in src-tauri).
"""
import math
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024
SCALE = 4
RED = (233, 69, 96)
WHITE = (255, 255, 255)
STROKE = 36
SLANT = 0.2


def main(out: Path) -> None:
    big = SIZE * SCALE
    mask = Image.new("L", (big, big), 0)
    d = ImageDraw.Draw(mask)

    def s(v: float) -> float:
        return v * SCALE

    def dot(x: float, y: float) -> None:
        r = STROKE / 2
        d.ellipse([s(x - r), s(y - r), s(x + r), s(y + r)], fill=255)

    def line(x1: float, y1: float, x2: float, y2: float) -> None:
        n = max(1, round(math.hypot(x2 - x1, y2 - y1)))
        for i in range(n + 1):
            dot(x1 + (x2 - x1) * i / n, y1 + (y2 - y1) * i / n)

    def arc(cx: float, cy: float, r: float, a0: float, a1: float) -> None:
        n = max(1, round(math.radians(abs(a1 - a0)) * r))
        for i in range(n + 1):
            a = math.radians(a0 + (a1 - a0) * i / n)
            dot(cx + r * math.cos(a), cy + r * math.sin(a))

    top, mid = 390, 620
    # J: a stem with a hook to the left
    jx = 420
    line(jx, top, jx, mid)
    arc(jx - 58, mid, 58, 0, 172)
    # U: two stems joined by a half circle
    ux1, ux2 = 500, 620
    line(ux1, top, ux1, mid)
    line(ux2, top, ux2, mid)
    arc((ux1 + ux2) / 2, mid, (ux2 - ux1) / 2, 0, 180)

    # slant to the right going up, about the vertical middle
    cy = (top + mid + 58) / 2
    mask = mask.transform(
        mask.size, Image.AFFINE, (1, SLANT, -SLANT * s(cy), 0, 1, 0), resample=Image.BICUBIC
    )
    # centre horizontally
    xs = [x for x in range(0, big, 8) if mask.crop((x, 0, x + 8, big)).getbbox()]
    shift = round(big / 2 - (xs[0] + xs[-1] + 8) / 2)
    mask = mask.transform(mask.size, Image.AFFINE, (1, 0, -shift, 0, 1, 0))

    mask = mask.resize((SIZE, SIZE), Image.LANCZOS)
    img = Image.new("RGB", (SIZE, SIZE), RED)
    img.paste(Image.new("RGB", (SIZE, SIZE), WHITE), (0, 0), mask)
    img.save(out)


if __name__ == "__main__":
    main(Path(sys.argv[1]) if len(sys.argv) > 1 else Path("src-tauri/app-icon.png"))
