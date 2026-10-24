#!/usr/bin/env python3
"""Charts for issue #101's evidence (ADR 0031), drawn with Pillow only.

    python3 tools/evidence/precompile_charts.py target/evidence/precompile

Reads `timeline.json`, `threads.json` and `bench.json` (written by
`cargo test --release -p xtask --test precompile_shots -- --ignored` and the
bench, plus the build timings) from that folder and writes `timeline.png`,
`bench.png` and `threads.png` beside them.
"""
import json
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

SURFACE = "#fcfcfb"
INK = "#0b0b0b"
INK2 = "#52514e"
GRID = "#e4e3df"
WARM = "#2a78d6"  # series 1: after (warm-up / threads)
COLD = "#eb6834"  # series 2: before (lazy / 1 thread)
SCALE = 2  # draw at 2x for crisp text


def font(size, bold=False):
    names = ["DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"]
    for n in names:
        for d in ["/usr/share/fonts/truetype/dejavu/", ""]:
            try:
                return ImageFont.truetype(d + n, size * SCALE)
            except OSError:
                pass
    return ImageFont.load_default()


def s(v):
    return int(v * SCALE)


def timeline(data, out):
    w, h = 1200, 520
    img = Image.new("RGB", (s(w), s(h)), SURFACE)
    d = ImageDraw.Draw(img)
    left, right, top, bottom = 70, 30, 80, 70
    cold = [f["ms"] for f in data["cold"]]
    warm = [f["ms"] for f in data["warm"]]
    n = len(cold)
    ymax = max(max(cold), max(warm)) * 1.1
    ymax = max(10, (int(ymax / 10) + 1) * 10)

    def x(i):
        return left + (w - left - right) * i / (n - 1)

    def y(v):
        return h - bottom - (h - top - bottom) * v / ymax

    d.text((s(left), s(18)), "castle64 hub, native profile: time per frame while the lazy passes switch on",
           fill=INK, font=font(17, True))
    d.text((s(left), s(44)),
           f"{data['adapter']}, {data['size'][0]}×{data['size'][1]}, driver disk cache off. "
           f"Warm-up before frame 0: {data['warm_ms']:.0f} ms (a loading screen).",
           fill=INK2, font=font(12))
    step = 10 if ymax <= 60 else 20 if ymax <= 160 else 50
    v = 0
    while v <= ymax:
        d.line([(s(left), s(y(v))), (s(w - right), s(y(v)))], fill=GRID, width=s(1))
        d.text((s(left - 8), s(y(v))), f"{v}", fill=INK2, font=font(11), anchor="rm")
        v += step
    d.text((s(18), s((top + h - bottom) / 2)), "ms", fill=INK2, font=font(12), anchor="lm")
    # Events.
    for i, f in enumerate(data["cold"]):
        if f["event"]:
            d.line([(s(x(i)), s(top)), (s(x(i)), s(h - bottom))], fill="#c9c8c2", width=s(1))
            d.text((s(x(i) + 3), s(h - bottom + 8)), f["event"], fill=INK2, font=font(10))
    for series, color, label in [(cold, COLD, "lazy (before)"), (warm, WARM, "precompiled (after)")]:
        pts = [(s(x(i)), s(y(v))) for i, v in enumerate(series)]
        d.line(pts, fill=color, width=s(2), joint="curve")
    # Direct labels at the worst cold frame and the end of the warm line.
    wi = max(range(n), key=lambda i: cold[i])
    d.text((s(x(wi) + 8), s(y(cold[wi]) - 4)),
           f"lazy: {cold[wi]:.0f} ms (pipelines compiled in the frame)", fill=INK, font=font(11))
    pc = sum(f["pipes"] for f in data["cold"])
    pw = sum(f["pipes"] for f in data["warm"])
    # Legend.
    lx = w - right - 420
    for k, (color, label) in enumerate([(COLD, f"lazy (before): {pc} pipelines compiled inside frames"),
                                         (WARM, f"precompiled (after): {pw} inside frames")]):
        ly = 72 + k * 0  # one row
        ox = lx + k * 0
        d.line([(s(lx), s(top - 14 + k * 16)), (s(lx + 22), s(top - 14 + k * 16))], fill=color, width=s(3))
        d.text((s(lx + 28), s(top - 14 + k * 16)), label, fill=INK, font=font(11), anchor="lm")
    img.save(out)


def bars(panels, title, subtitle, out, width=1200):
    row = 34
    ph = [56 + row * len(items) for _, items, _ in panels]
    h = 90 + sum(ph) + 20
    img = Image.new("RGB", (s(width), s(h)), SURFACE)
    d = ImageDraw.Draw(img)
    d.text((s(30), s(18)), title, fill=INK, font=font(17, True))
    d.text((s(30), s(44)), subtitle, fill=INK2, font=font(12))
    top = 90
    label_w, value_w = 330, 140
    for (ptitle, items, unit), hgt in zip(panels, ph):
        d.text((s(30), s(top)), ptitle, fill=INK, font=font(13, True))
        vmax = max(v for _, v, _ in items) or 1
        x0, x1 = 30 + label_w, width - 30 - value_w
        for k, (label, v, color) in enumerate(items):
            yy = top + 30 + k * row
            d.text((s(30), s(yy + 9)), label, fill=INK2, font=font(12), anchor="lm")
            bw = max(2, (x1 - x0) * v / vmax)
            d.rounded_rectangle([s(x0), s(yy), s(x0 + bw), s(yy + 18)], radius=s(4), fill=color)
            shown = f"{v:,.1f}" if v < 10 else f"{v:,.0f}"
            d.text((s(x0 + bw + 8), s(yy + 9)), f"{shown} {unit}".replace(",", " "),
                   fill=INK, font=font(12), anchor="lm")
        top += hgt
    img.save(out)


def main():
    d = Path(sys.argv[1])
    tl = json.loads((d / "timeline.json").read_text())
    timeline(tl, d / "timeline.png")
    b = json.loads((d / "bench.json").read_text())
    bars([
        (f"Driver compile of {b['shaders']} heavy shaders ({b['wgsl_kb']} KB of WGSL)", [
            ("1 thread", b["compile_1"], COLD),
            (f"{b['threads']} threads (auto = all CPUs − 1)", b["compile_n"], WARM)], "ms"),
        ("The same shaders on the next launch", [
            ("no pipeline cache", b["cache_fill"], COLD),
            ("from the pipeline cache file", b["cache_next"], WARM)], "ms"),
        (f"Engine warm-up, precompile(EVERYTHING): {b['passes']} passes", [
            ("1 thread", b["engine_1"], COLD),
            (f"{b['engine_threads']} threads", b["engine_n"], WARM)], "ms"),
    ], "Shader compile time, before and after",
        f"{b['adapter']}; Mesa disk cache off. Orange: before. Blue: after.", d / "bench.png")
    t = json.loads((d / "threads.json").read_text())
    panels = [
        ("Asset bake of the engine's assets (PC)", [
            ("1 thread", t["bake_ms"]["1"], COLD),
            (f"{t['auto']} threads", t["bake_ms"]["auto"], WARM)], "ms"),
        (f"Decode of one streamed group ({t['decode']['assets']} assets, "
         f"{t['decode']['bundle_bytes'] // 1024} KB LZ4)", [
            ("1 thread", t["decode"]["1"], COLD),
            (f"{t['auto']} threads", t["decode"]["auto"], WARM)], "ms"),
    ]
    if "decode_big" in t:
        db = t["decode_big"]
        panels.append((f"Decode of a level-sized group ({db['assets']} textures, "
                       f"{db['bundle_bytes'] // (1024 * 1024)} MB)", [
            ("1 thread", db["1"], COLD),
            (f"{t['auto']} threads", db["auto"], WARM)], "ms"))
    for key, label in [("n64_shim", "N64 rebuild after a shader edit (shim + 4 RSP overlays, Docker)"), ("n3ds_picasso", "3DS rebuild after a shader edit (picasso + shim + cargo jobs)")]:
        if key in t:
            panels.append((label, [("1 job", t[key]["1"], COLD),
                                   (f"{t['auto']} jobs", t[key]["auto"], WARM)], "ms"))
    bars(panels, "The thread budget, before and after",
         f"{t['cpus']} logical CPUs; auto = {t['auto']}. Orange: 1 thread. Blue: the budget.", d / "threads.png")


if __name__ == "__main__":
    main()
