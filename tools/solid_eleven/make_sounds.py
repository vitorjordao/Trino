#!/usr/bin/env python3
"""Solid Eleven's sound masters, synthesized (reproducible, no samples):

    python3 tools/solid_eleven/make_sounds.py

Writes examples/solid-eleven/assets/shared/{sounds,music}/*.wav (mono, 16-bit,
22 050 Hz: small for the N64's ROM and the 3DS's RomFS).
"""
import math, os, random, struct, wave

RATE = 22050
OUT = os.path.join(os.path.dirname(__file__), "..", "..", "examples", "solid-eleven", "assets", "shared")


def write(rel, samples):
    path = os.path.join(OUT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", max(-32767, min(32767, int(s * 32767)))) for s in samples))
    print(path, len(samples))


def env(n, attack, release):
    a, r = int(attack * RATE), int(release * RATE)
    return [min(1.0, i / max(1, a), (n - i) / max(1, r)) for i in range(n)]


def whistle():
    # A referee's pea whistle: ~2.8 kHz with a fast warble, three blasts
    # would be too long — one sharp blast.
    n = int(0.42 * RATE)
    e = env(n, 0.01, 0.06)
    out = []
    ph = 0.0
    for i in range(n):
        f = 2850 + 140 * math.sin(2 * math.pi * 32 * i / RATE)
        ph += 2 * math.pi * f / RATE
        out.append(0.45 * e[i] * (math.sin(ph) + 0.25 * math.sin(2 * ph)))
    return out


def thump(dur, f0, f1, noise, vol, seed):
    rng = random.Random(seed)
    n = int(dur * RATE)
    out = []
    ph = 0.0
    for i in range(n):
        t = i / n
        f = f0 + (f1 - f0) * t
        ph += 2 * math.pi * f / RATE
        e = (1 - t) ** 3
        out.append(vol * e * (math.sin(ph) * (1 - noise) + noise * rng.uniform(-1, 1)))
    return out


def crowd(dur, seed, swell=0.0, vol=0.35):
    """Many voices: low-passed noise with slow swells."""
    rng = random.Random(seed)
    n = int(dur * RATE)
    out = []
    lp = 0.0
    lp2 = 0.0
    for i in range(n):
        t = i / RATE
        s = rng.uniform(-1, 1)
        lp += 0.08 * (s - lp)
        lp2 += 0.3 * (lp - lp2)
        level = 0.75 + 0.25 * math.sin(2 * math.pi * 0.23 * t) * math.sin(2 * math.pi * 0.071 * t + 1)
        level += swell * min(1.0, t / 0.4) * max(0.0, 1 - (t - 0.4) / (dur - 0.4))
        out.append(vol * level * lp2 * 3.0)
    return out


def loop_crossfade(x, fade):
    """Make a seamless loop: fade the tail into the head."""
    f = int(fade * RATE)
    head, body = x[:f], x[f:]
    for i in range(f):
        k = i / f
        body[len(body) - f + i] = body[len(body) - f + i] * (1 - k) + head[i] * k
    return body


def theme():
    """A short upbeat loop: bass, chords and a lead in C major, 128 bpm."""
    bpm = 128
    beat = 60 / bpm
    bars = 8
    n = int(bars * 4 * beat * RATE)
    out = [0.0] * n
    chords = [(48, 52, 55), (45, 48, 52), (41, 45, 48), (43, 47, 50)] * 2
    lead = [72, 76, 79, 76, 74, 72, 74, 76, 69, 72, 76, 72, 71, 74, 79, 74,
            72, 76, 79, 81, 79, 76, 74, 72, 77, 76, 74, 72, 71, 72, 74, 71]

    def hz(m):
        return 440.0 * 2 ** ((m - 69) / 12)

    def add(start, dur, f, vol, shape):
        s0 = int(start * RATE)
        ln = int(dur * RATE)
        ph = 0.0
        for i in range(ln):
            if s0 + i >= n:
                break
            ph += f / RATE
            e = min(1.0, i / 200) * (1 - i / ln) ** 1.5
            v = (2 * (ph % 1) - 1) if shape == "saw" else (1.0 if (ph % 1) < 0.5 else -1.0)
            out[s0 + i] += vol * e * v

    for b, ch in enumerate(chords):
        t0 = b * 4 * beat
        for k in range(8):
            add(t0 + k * beat / 2, beat / 2 * 0.9, hz(ch[0] - 12), 0.18, "sq")
        for m in ch:
            for k in range(4):
                add(t0 + k * beat, beat * 0.5, hz(m), 0.05, "saw")
    for k, m in enumerate(lead):
        add(k * beat, beat * 0.85, hz(m), 0.09, "sq")
    rng = random.Random(9)
    for k in range(bars * 4):
        add(k * beat, 0.06, 60.0, 0.25, "sq")
        if k % 2 == 1:
            s0 = int((k * beat) * RATE)
            for i in range(int(0.05 * RATE)):
                if s0 + i < n:
                    out[s0 + i] += 0.12 * rng.uniform(-1, 1) * (1 - i / (0.05 * RATE))
    return out


def main():
    write("sounds/se_whistle.wav", whistle())
    write("sounds/se_kick.wav", thump(0.16, 160, 60, 0.35, 0.8, 1))
    write("sounds/se_tap.wav", thump(0.09, 220, 120, 0.5, 0.5, 2))
    write("sounds/se_post.wav", [0.5 * math.sin(2 * math.pi * 880 * i / RATE) * (1 - i / (0.35 * RATE)) ** 2
                                 + 0.3 * math.sin(2 * math.pi * 1320 * i / RATE) * (1 - i / (0.35 * RATE)) ** 3
                                 for i in range(int(0.35 * RATE))])
    write("sounds/se_cheer.wav", crowd(2.6, 3, swell=2.2, vol=0.45))
    write("sounds/se_ooh.wav", [s * (1 - i / (1.2 * RATE)) for i, s in enumerate(crowd(1.2, 4, swell=1.2, vol=0.4))])
    write("sounds/se_click.wav", thump(0.05, 1800, 1200, 0.1, 0.35, 5))
    write("sounds/se_back.wav", thump(0.08, 700, 300, 0.1, 0.35, 6))
    write("music/se_crowd.wav", loop_crossfade(crowd(6.5, 7), 0.5))
    write("music/se_theme.wav", theme())


main()
