#!/usr/bin/env python3
"""Compose the README's comparison images from frames already in the repo.

    python3 tools/media/readme_composites.py

Reads golden frames (tests/golden/...) and docs/media shots, writes
docs/media/readme/*.png. Console frames are upscaled with nearest-neighbor so
their pixels stay pixels. Re-run it after re-blessing a golden it uses.
Needs Pillow.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "docs/media/readme"
FONT = ROOT / "crates/editor/assets/fonts/Inter-SemiBold.ttf"
FONT_REG = ROOT / "crates/editor/assets/fonts/Inter-Regular.ttf"

BG = (13, 17, 23)
CARD = (22, 27, 34)
TEXT = (240, 246, 252)
MUTED = (139, 148, 158)
ACCENT = {
    "pc": (88, 166, 255),
    "n64": (63, 185, 80),
    "3ds": (248, 81, 73),
    "web": (210, 153, 34),
}

GAP = 24
LABEL_H = 64


def load(rel):
    return Image.open(ROOT / rel).convert("RGB")


def fit_h(img, h, pixel):
    """Scale to height h: nearest for console frames (whole multiples when
    possible), Lanczos for PC frames."""
    w = round(img.width * h / img.height)
    return img.resize((w, h), Image.NEAREST if pixel else Image.LANCZOS)


def font(size, bold=True):
    return ImageFont.truetype(str(FONT if bold else FONT_REG), size)


def panel(img, title, sub, key):
    """A frame with a label bar under it: a colored dot, a title, a subtitle."""
    w, h = img.size
    card = Image.new("RGB", (w, h + LABEL_H), CARD)
    card.paste(img, (0, 0))
    d = ImageDraw.Draw(card)
    d.rectangle([0, h, w, h + 4], fill=ACCENT[key])
    y = h + 4 + (LABEL_H - 4) // 2
    d.ellipse([18, y - 7, 32, y + 7], fill=ACCENT[key])
    ft, fs = font(26), font(22, bold=False)
    d.text((44, y), title, font=ft, fill=TEXT, anchor="lm")
    tw = d.textlength(title, font=ft)
    d.text((44 + tw + 14, y), sub, font=fs, fill=MUTED, anchor="lm")
    return card


def row(cards, pad=GAP):
    w = sum(c.width for c in cards) + pad * (len(cards) + 1)
    h = max(c.height for c in cards) + pad * 2
    out = Image.new("RGB", (w, h), BG)
    x = pad
    for c in cards:
        out.paste(c, (x, pad))
        x += c.width + pad
    return out


def column(rows, pad=0):
    w = max(r.width for r in rows)
    h = sum(r.height for r in rows) + pad * (len(rows) - 1)
    out = Image.new("RGB", (w, h), BG)
    y = 0
    for r in rows:
        out.paste(r, ((w - r.width) // 2, y))
        y += r.height + pad
    return out


def heading(text, sub, width):
    h = 84
    out = Image.new("RGB", (width, h), BG)
    d = ImageDraw.Draw(out)
    d.text((GAP, h // 2 + 6), text, font=font(34), fill=TEXT, anchor="lm")
    tw = d.textlength(text, font=font(34))
    d.text((GAP + tw + 18, h // 2 + 8), sub, font=font(24, bold=False), fill=MUTED, anchor="lm")
    return out


def dual(top, bottom, scale):
    """A 3DS frame: the 400x240 top screen over the 320x240 touch screen."""
    t = top.resize((top.width * scale, top.height * scale), Image.NEAREST)
    b = bottom.resize((bottom.width * scale, bottom.height * scale), Image.NEAREST)
    g = 8 * scale
    out = Image.new("RGB", (t.width, t.height + g + b.height), CARD)
    out.paste(t, (0, 0))
    out.paste(b, ((t.width - b.width) // 2, t.height + g))
    return out


def save(img, name):
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / name
    img.save(path, optimize=True)
    print(f"{path.relative_to(ROOT)}  {img.width}x{img.height}  {path.stat().st_size // 1024} KB")


def one_scene_three_machines():
    h = 480
    pc = fit_h(load("tests/golden/pc/showcase_river.png"), h, False)
    n64 = fit_h(load("tests/golden/n64/showcase_river.png"), h, True)
    n3ds = fit_h(load("tests/golden/3ds/showcase_river.png"), h, True)
    save(
        row([
            panel(pc, "PC & Web", "wgpu · per-pixel water", "pc"),
            panel(n64, "Nintendo 64", "ares framebuffer", "n64"),
            panel(n3ds, "Nintendo 3DS", "Azahar framebuffer", "3ds"),
        ]),
        "one-scene-three-machines.png",
    )


def tiers():
    h = 360
    rows = []
    for key, title, subs in [
        ("lights", "Local lights", ("per pixel, wet floor", "per vertex, 2 lights", "per vertex, 4 lights")),
        ("water", "Water", ("per pixel, foam + Fresnel", "RDP 2-cycle blend", "PICA200 vertex program")),
        ("shadows", "Shadows", ("shadow map + PCF", "blob + planar", "planar, stencil-merged")),
    ]:
        native = "lights_native_wet" if key == "lights" else f"{key}_native"
        cards = [
            panel(fit_h(load(f"tests/golden/pc/{native}.png"), h, False), "PC", subs[0], "pc"),
            panel(fit_h(load(f"tests/golden/pc/{key}_n64.png"), h, True), "N64", subs[1], "n64"),
            panel(fit_h(load(f"tests/golden/pc/{key}_n3ds.png"), h, True), "3DS", subs[2], "3ds"),
        ]
        r = row(cards)
        rows.append(column([heading(title, "", r.width), r]))
    save(column(rows), "tiers.png")


def solid_eleven():
    h = 480
    pc = fit_h(load("docs/media/solid-eleven/closeup_pc.png"), h, False)
    n64 = fit_h(load("docs/media/solid-eleven/match_n64.png"), h, True)
    n3ds = fit_h(load("docs/media/solid-eleven/match_3ds.png"), h, True)
    save(
        row([
            panel(pc, "PC", "4x MSAA · SSAO · shadow map", "pc"),
            panel(n64, "N64", "~27 ms in ares", "n64"),
            panel(n3ds, "3DS", "every draw on the PICA200", "3ds"),
        ]),
        "solid-eleven-three-machines.png",
    )


def two_screens():
    c64 = dual(load("tests/golden/3ds/castle64_hub.png"), load("tests/golden/3ds/castle64_hub_bottom.png"), 2)
    se = dual(load("docs/media/solid-eleven/match_3ds.png"), load("docs/media/solid-eleven/match_3ds_bottom.png"), 2)
    save(
        row([
            panel(c64, "castle64", "map, counters, touch buttons", "3ds"),
            panel(se, "Solid Eleven", "radar, formation, substitutions", "3ds"),
        ]),
        "3ds-two-screens.png",
    )


if __name__ == "__main__":
    one_scene_three_machines()
    tiers()
    solid_eleven()
    two_screens()
