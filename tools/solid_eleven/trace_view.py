#!/usr/bin/env python3
"""Top-down contact sheet of a Solid Eleven match trace (debug aid).

    SE_TRACE=target/se_trace.csv cargo test -p solid-eleven trace_match -- --ignored
    python3 tools/solid_eleven/trace_view.py target/se_trace.csv out.png [start_tick] [count] [step]

Each panel is one sampled tick: the pitch, side 0 red, side 1 blue (keepers
darker), the ball white with its owner ringed, the phase and events.
"""
import csv, sys
from PIL import Image, ImageDraw

path, out = sys.argv[1], sys.argv[2]
start = int(sys.argv[3]) if len(sys.argv) > 3 else 0
count = int(sys.argv[4]) if len(sys.argv) > 4 else 24
step = int(sys.argv[5]) if len(sys.argv) > 5 else 1

rows = []
with open(path) as f:
    for r in csv.reader(f):
        if int(r[0]) >= start:
            rows.append(r)
rows = rows[::step][:count]

W, H, S = 330, 230, 3.0  # panel size, px per meter
cols = 4
sheet = Image.new("RGB", (cols * W, ((len(rows) + cols - 1) // cols) * H), (20, 20, 20))
for n, r in enumerate(rows):
    img = Image.new("RGB", (W, H), (40, 120, 50))
    d = ImageDraw.Draw(img)
    cx, cy = W / 2, H / 2 + 6
    P = lambda x, y: (cx + x * S, cy + y * S)
    d.rectangle([P(-52.5, -34), P(52.5, 34)], outline=(230, 230, 230))
    d.line([P(0, -34), P(0, 34)], fill=(230, 230, 230))
    d.ellipse([P(-9.15, -9.15), P(9.15, 9.15)], outline=(230, 230, 230))
    def rect(x0, y0, x1, y1, **kw):
        (a, b), (c, e) = P(x0, y0), P(x1, y1)
        d.rectangle([min(a, c), min(b, e), max(a, c), max(b, e)], **kw)
    for sx in (-1, 1):
        rect(sx * 52.5, -20.16, sx * 36, 20.16, outline=(230, 230, 230))
        rect(sx * 52.5, -3.66, sx * 54.5, 3.66, outline=(255, 255, 255))
    tick, phase, s0, s1 = r[0], r[1], r[2], r[3]
    bx, bz, by, owner = float(r[4]), float(r[5]), float(r[6]), int(r[7])
    for i in range(22):
        x, y, a = float(r[8 + i * 3]), float(r[9 + i * 3]), r[10 + i * 3]
        col = (220, 40, 40) if i < 11 else (40, 80, 230)
        if i in (0, 11):
            col = tuple(c // 2 for c in col)
        X, Y = P(x, y)
        d.ellipse([X - 3, Y - 3, X + 3, Y + 3], fill=col)
        if a != 'f':
            d.text((X + 3, Y - 8), a, fill=(255, 255, 0))
        if i == owner:
            d.ellipse([X - 5, Y - 5, X + 5, Y + 5], outline=(255, 255, 255))
    X, Y = P(bx, bz)
    rr = 2 + by * 0.6
    d.ellipse([X - rr, Y - rr, X + rr, Y + rr], fill=(255, 255, 255))
    d.text((4, 2), f"t{tick} {s0}-{s1} {phase[:28]}", fill=(255, 255, 255))
    ev = r[8 + 66] if len(r) > 74 else ""
    if ev:
        d.text((4, H - 12), ev[:60], fill=(255, 230, 120))
    sheet.paste(img, ((n % cols) * W, (n // cols) * H))
sheet.save(out)
print(out, len(rows), "panels")
