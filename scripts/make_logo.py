"""Generate the rounded Hako logo + an icon source from a source image.

Usage: python scripts/make_logo.py <source-image>

Emits:
  public/logo.png         256px rounded logo used in-app (sidebar, promo)
  <tmp>/hako-icon.png     1024px rounded source for `tauri icon`
"""

import os
import sys
import tempfile

from PIL import Image, ImageDraw

RADIUS_RATIO = 0.20  # ~20% corner radius — a soft rounded square, not a circle
SUPERSAMPLE = 4  # draw the mask larger, then downscale for smooth anti-aliased corners


def center_square(img: Image.Image) -> Image.Image:
    w, h = img.size
    side = min(w, h)
    left = (w - side) // 2
    top = (h - side) // 2
    return img.crop((left, top, left + side, top + side))


def rounded(square: Image.Image, size: int) -> Image.Image:
    base = square.resize((size, size), Image.LANCZOS).convert("RGBA")
    big = size * SUPERSAMPLE
    mask = Image.new("L", (big, big), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle(
        [0, 0, big - 1, big - 1], radius=int(big * RADIUS_RATIO), fill=255
    )
    base.putalpha(mask.resize((size, size), Image.LANCZOS))
    return base


def main() -> None:
    src = sys.argv[1] if len(sys.argv) > 1 else None
    if not src or not os.path.isfile(src):
        raise SystemExit(f"source image not found: {src!r}")

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    square = center_square(Image.open(src).convert("RGBA"))

    public_logo = os.path.join(root, "public", "logo.png")
    os.makedirs(os.path.dirname(public_logo), exist_ok=True)
    rounded(square, 256).save(public_logo)
    print("wrote", public_logo)

    icon_src = os.path.join(tempfile.gettempdir(), "hako-icon.png")
    rounded(square, 1024).save(icon_src)
    print("wrote", icon_src)


if __name__ == "__main__":
    main()
