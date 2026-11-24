#!/usr/bin/env python3
"""Draws target/evidence/platform/rumble.csv (platform_evidence.rs) as a PNG."""
import csv, sys
from PIL import Image, ImageDraw, ImageFont

src, events_src, dst = sys.argv[1], sys.argv[2], sys.argv[3]
rows = list(csv.DictReader(open(src)))
events = list(csv.DictReader(open(events_src)))

def font(size):
    for f in ("/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
              "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(f, size)
        except OSError:
            pass
    return ImageFont.load_default()

F, FS, FT = font(16), font(13), font(20)
FRAMES = 240
X0, PW = 150, FRAMES * 4
PH, GAP = 90, 34
settings = ["100", "50", "0"]
H = 70 + len(settings) * (PH + GAP) + 60 + 40
W = X0 + PW + 30
img = Image.new("RGB", (W, H), (250, 250, 247))
d = ImageDraw.Draw(img)
d.text((20, 4), "Platform::rumble through HostedRumble, scaled by AccessSettings::rumble (60 fps, 4 s)", font=FT, fill=(30, 30, 30))
LOW, HIGH = (217, 95, 2), (31, 120, 180)

def panel(y, setting):
    d.rectangle([X0, y, X0 + PW, y + PH], outline=(200, 200, 200), fill=(255, 255, 255))
    d.text((20, y + PH / 2 - 18), f"setting {setting} %", font=F, fill=(30, 30, 30))
    for frac in (0.5, 1.0):
        yy = y + PH - frac * (PH - 6)
        d.line([X0, yy, X0 + PW, yy], fill=(235, 235, 235))
        d.text((X0 - 34, yy - 8), f"{frac:.1f}", font=FS, fill=(120, 120, 120))
    data = [r for r in rows if r["setting"] == setting]
    for key, col, dx in (("low", LOW, 0), ("high", HIGH, 2)):
        pts = [(X0 + int(r["frame"]) * 4 + dx, y + PH - float(r[key]) * (PH - 6)) for r in data]
        d.line(pts, fill=col, width=2)
    return data

y = 70
for s in settings:
    data = panel(y, s)
    if s == "100":
        n64_data = data
    y += PH + GAP
# N64 duty-cycle strip (setting 100 %).
d.text((20, y + 6), "N64 Rumble Pak", font=F, fill=(30, 30, 30))
d.text((20, y + 26), "(on/off per frame)", font=FS, fill=(90, 90, 90))
for r in n64_data:
    x = X0 + int(r["frame"]) * 4
    col = (40, 40, 40) if r["n64"] == "1" else (230, 230, 230)
    d.rectangle([x, y + 8, x + 3, y + 40], fill=col)
y += 52
for k, e in enumerate(events):
    x = X0 + int(float(e["second"]) * 60) * 4
    ty = 40 if k % 2 == 0 else 24
    d.line([x, ty + 14, x, y - 10], fill=(160, 160, 160), width=1)
    d.text((x + 3, ty), f'{e["name"]} ({e["low"]}, {e["high"]}, {e["ms"]} ms)', font=FS, fill=(70, 70, 70))
for sec in range(5):
    x = X0 + sec * 240
    d.text((x - 4, y), f"{sec} s", font=FS, fill=(90, 90, 90))
d.rectangle([X0, y + 22, X0 + 14, y + 30], fill=LOW)
d.text((X0 + 20, y + 17), "low (heavy) motor", font=FS, fill=(30, 30, 30))
d.rectangle([X0 + 200, y + 22, X0 + 214, y + 30], fill=HIGH)
d.text((X0 + 220, y + 17), "high (light) motor", font=FS, fill=(30, 30, 30))
img.save(dst)
print(dst)
