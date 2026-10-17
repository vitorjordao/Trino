#!/usr/bin/env python3
"""Solid Eleven's small effect sprites (reproducible):

    python3 tools/solid_eleven/make_sprites.py

- se_rain.png: a rain streak (4 x 32, white, alpha fading to the top);
- se_glow.png: a floodlight's glow (32 x 32, a soft radial spot).

Both are white masks tinted at draw time; the PC's native profile draws
them (rain, night glows), the consoles never do.
"""
import math, os
from PIL import Image

OUT = os.path.join(os.path.dirname(__file__), "..", "..", "examples", "solid-eleven", "assets", "shared", "sprites")
os.makedirs(OUT, exist_ok=True)

rain = Image.new("RGBA", (4, 32), (0, 0, 0, 0))
for y in range(32):
    a = int(200 * (y / 31) ** 1.5)
    for x in (1, 2):
        rain.putpixel((x, y), (255, 255, 255, a))
rain.save(os.path.join(OUT, "se_rain.png"))

glow = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
for y in range(32):
    for x in range(32):
        d = math.hypot(x - 15.5, y - 15.5) / 16.0
        a = max(0.0, 1.0 - d) ** 2
        if a > 0:
            glow.putpixel((x, y), (255, 255, 255, int(255 * a)))
glow.save(os.path.join(OUT, "se_glow.png"))
print("wrote", OUT)
