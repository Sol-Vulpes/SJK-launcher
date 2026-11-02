"""The JoF Launcher's art: the Jedi of Freedom clan's J o F logo in its green, drawn
from the logo's shapes (two polygons and a ring, the same ones SJK used for its JoF
emblem) so it stays sharp at any size.

    python scripts/jof_art.py assets/jof

writes `jof-emblem-512.png` (the logo over the launcher's horizon) and
`launcher-icon.png` (the window and tray icon: the logo in a green ring on the
JoF Launcher's near-black green). Needs Pillow.
"""

import os
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter

J = [(38, 9), (38, 66), (10, 94), (10, 109), (50, 69), (50, 9)]
F = [(58, 9), (58, 69), (69, 80), (69, 56), (84, 41), (84, 26), (69, 41), (69, 33),
     (87, 15), (87, 0), (69, 18), (69, 9)]
RING = ((54, 90), 16.5, 8.0)  # centre, outer and inner radius
# The logo's box: x 10..87, y 0..109 (the ring reaches 106.5).
LEFT, TOP, WIDTH, HEIGHT = 10, 0, 77, 109

SPACE = (0x04, 0x0B, 0x07)
SKY = (0x0E, 0x2E, 0x1A)
GREEN_LIGHT = (0x8C, 0xFF, 0x6E)
GREEN = (0x1E, 0xC8, 0x2A)
GREEN_DEEP = (0x06, 0x6E, 0x10)
OUTLINE = (0x02, 0x24, 0x06)
SUPERSAMPLE = 4


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def vertical(size, stops):
    """A square shaded top to bottom through `stops` [(t, colour)]."""
    image = Image.new("RGBA", (size, size))
    draw = ImageDraw.Draw(image)
    for y in range(size):
        t = y / (size - 1)
        for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
            if t0 <= t <= t1:
                draw.line([(0, y), (size, y)], fill=mix(c0, c1, (t - t0) / (t1 - t0)) + (255,))
                break
    return image


def logo_mask(size, box):
    """The logo's shape, white on black, fitted into `box` (left, top, side) of a
    `size` square."""
    left, top, side = box
    scale = side / max(WIDTH, HEIGHT)
    ox = left + (side - WIDTH * scale) / 2
    oy = top + (side - HEIGHT * scale) / 2

    def at(point):
        return (ox + (point[0] - LEFT) * scale, oy + (point[1] - TOP) * scale)

    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    draw.polygon([at(p) for p in J], fill=255)
    draw.polygon([at(p) for p in F], fill=255)
    (cx, cy), outer, inner = RING
    x, y = at((cx, cy))
    draw.ellipse([x - outer * scale, y - outer * scale, x + outer * scale, y + outer * scale], fill=255)
    draw.ellipse([x - inner * scale, y - inner * scale, x + inner * scale, y + inner * scale], fill=0)
    return mask


def logo(size, box, out):
    """Paint the logo into `out`: a dark outline, a green body lit from the top, a
    thin light edge along its upper sides, as the clan's own logo is."""
    shape = logo_mask(size, box)
    side = box[2]
    outline = shape.filter(ImageFilter.MaxFilter(odd(side * 0.035)))
    out.paste(Image.new("RGBA", (size, size), OUTLINE + (255,)), mask=outline)
    side = round(side)
    body = Image.new("RGBA", (size, size))
    shade = vertical(side, [(0, GREEN_LIGHT), (0.45, GREEN), (1, GREEN_DEEP)]).resize((size, side))
    body.paste(shade, (0, round(box[1])))
    out.paste(body, mask=shape)
    # The bevel: the shape minus itself moved down-right, kept as a soft highlight.
    shift = max(1, round(side * 0.012))
    moved = ImageChops.offset(shape, shift, shift)
    edge = ImageChops.subtract(shape, moved).filter(ImageFilter.GaussianBlur(side * 0.004))
    out.paste(Image.new("RGBA", (size, size), (0xD8, 0xFF, 0xC8, 255)), mask=edge.point(lambda v: v * 200 // 255))
    return out


def odd(value):
    value = max(3, round(value))
    return value if value % 2 else value + 1


def circle(size, radius):
    mask = Image.new("L", (size, size), 0)
    centre, r = size / 2, radius * size
    ImageDraw.Draw(mask).ellipse([centre - r, centre - r, centre + r, centre + r], fill=255)
    return mask


def emblem(size):
    big = size * SUPERSAMPLE
    out = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    # A soft green glow behind it, as the SJK emblem floats over the horizon.
    glow = logo_mask(big, (big * 0.1, big * 0.06, big * 0.8)).filter(ImageFilter.GaussianBlur(big * 0.04))
    out.paste(Image.new("RGBA", (big, big), GREEN + (255,)), mask=glow.point(lambda v: v * 110 // 255))
    logo(big, (big * 0.1, big * 0.06, big * 0.8), out)
    return out.resize((size, size), Image.LANCZOS)


def icon(size):
    big = size * SUPERSAMPLE
    out = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    out.paste(vertical(big, [(0, SKY), (1, SPACE)]), mask=circle(big, 0.47))
    ring = circle(big, 0.47)
    ring.paste(0, mask=circle(big, 0.425))
    out.paste(vertical(big, [(0, GREEN_LIGHT), (1, GREEN_DEEP)]), mask=ring)
    logo(big, (big * 0.2, big * 0.17, big * 0.6), out)
    return out.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    folder = sys.argv[1]
    os.makedirs(folder, exist_ok=True)
    emblem(512).save(os.path.join(folder, "jof-emblem-512.png"), optimize=True)
    icon(256).save(os.path.join(folder, "launcher-icon.png"), optimize=True)
