"""Draw the app icon: two overlapping cards, one lifted off the other.

The shape has to survive being scaled to 16 pixels in a system tray, so it is
built from two large blocks with a wide gap between them rather than anything
detailed. Drawn at 4x and downsampled for antialiasing, which avoids needing an
image library beyond PIL's basics.
"""
import sys

try:
    from PIL import Image, ImageDraw
except ImportError:
    sys.exit("PIL_MISSING")

SIZE = 1024
SCALE = 4
S = SIZE * SCALE

# The app's accent blue, and a slightly deeper tone for the card behind.
BACKDROP_TOP = (66, 133, 244)
BACKDROP_BOTTOM = (26, 86, 219)
CARD_BACK = (255, 255, 255, 110)
CARD_FRONT = (255, 255, 255, 255)
MARK = (26, 86, 219)


def rounded(draw, box, radius, fill):
    draw.rounded_rectangle(box, radius=radius, fill=fill)


image = Image.new("RGBA", (S, S), (0, 0, 0, 0))

# Vertical gradient backdrop, clipped to a rounded square.
gradient = Image.new("RGBA", (S, S))
pixels = gradient.load()
for y in range(S):
    t = y / (S - 1)
    pixels_row = tuple(
        round(BACKDROP_TOP[i] + (BACKDROP_BOTTOM[i] - BACKDROP_TOP[i]) * t) for i in range(3)
    )
    for x in range(S):
        pixels[x, y] = pixels_row + (255,)

mask = Image.new("L", (S, S), 0)
rounded(ImageDraw.Draw(mask), (0, 0, S - 1, S - 1), int(S * 0.22), 255)
image.paste(gradient, (0, 0), mask)

overlay = Image.new("RGBA", (S, S), (0, 0, 0, 0))
draw = ImageDraw.Draw(overlay)

# The card behind, offset up and left: the "other copy".
card_radius = int(S * 0.06)
rounded(draw, (int(S * 0.20), int(S * 0.16), int(S * 0.66), int(S * 0.72)), card_radius, CARD_BACK)

# The card in front, the one you keep.
rounded(draw, (int(S * 0.34), int(S * 0.28), int(S * 0.80), int(S * 0.84)), card_radius, CARD_FRONT)

# A check on the front card, so the icon says "resolved", not just "two files".
tick = [
    (int(S * 0.435), int(S * 0.560)),
    (int(S * 0.530), int(S * 0.655)),
    (int(S * 0.700), int(S * 0.430)),
]
draw.line(tick, fill=MARK, width=int(S * 0.055), joint="curve")
for point in (tick[0], tick[-1]):
    r = int(S * 0.0275)
    draw.ellipse((point[0] - r, point[1] - r, point[0] + r, point[1] + r), fill=MARK)

image = Image.alpha_composite(image, overlay)
image = image.resize((SIZE, SIZE), Image.LANCZOS)

out = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
image.save(out)
print(f"wrote {out} ({SIZE}x{SIZE})")
