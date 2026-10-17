#!/usr/bin/env python3
"""Solid Eleven's player skeleton and animation clips: the one source of truth.

Pure Python (no Blender): `solid_eleven_models.py` builds the rig and its
actions from these tables inside Blender, and this file writes the same
poses as Rust constants for the game:

    python3 tools/blender/solid_eleven_anim.py --rust examples/solid-eleven/src/render/clips_gen.rs
    python3 tools/blender/solid_eleven_anim.py --check examples/solid-eleven/src/render/clips_gen.rs

(`cargo xtask blender solid-eleven` runs both.) The game poses rigid parts
with these quaternions on every target (one code path), and
`xtask/tests/solid_eleven_clips.rs` holds the generated file to this table.

Conventions (Trino space: x = the player's left, y = up, z = forward):
- each joint's rotation is Euler degrees (rx, ry, rz) applied X, then Y,
  then Z (R = Rz·Ry·Rx, like `Mat34::from_rotation_scale_translation`),
  in the parent's frame, at the joint's pivot;
- rx > 0 tips a part's +y end toward +z: a torso leans forward, but a
  hanging leg or arm swings **back**; rz > 0 tips +y toward -x;
- `root` is (dx, clearance, dz): the pelvis moves dx, dz meters, and the
  generator raises or lowers it so the body's lowest point sits
  `clearance` meters over the turf (0 = a foot, a hip or a shoulder on the
  ground), separately for the PC's body (knees bend) and the consoles'
  (straight legs) — feet never sink or float.
"""
import math, sys

# name, parent, pivot (meters, relative to the parent's pivot)
JOINTS = [
    ("pelvis", None, (0.0, 0.95, 0.0)),
    ("torso", "pelvis", (0.0, 0.02, 0.0)),
    # The neck's base, at the collar: the neck nods about it inside the shirt.
    ("head", "torso", (0.0, 0.44, 0.0)),
    # Inside the shoulder, so the sleeve's cap turns within the shirt.
    ("arm_l", "torso", (0.235, 0.415, 0.0)),
    ("arm_r", "torso", (-0.235, 0.415, 0.0)),
    ("forearm_l", "arm_l", (0.0, -0.29, 0.0)),
    ("forearm_r", "arm_r", (0.0, -0.29, 0.0)),
    ("thigh_l", "pelvis", (0.10, -0.03, 0.0)),
    ("thigh_r", "pelvis", (-0.10, -0.03, 0.0)),
    ("shin_l", "thigh_l", (0.0, -0.43, 0.0)),
    ("shin_r", "thigh_r", (0.0, -0.43, 0.0)),
]
NAMES = [j[0] for j in JOINTS]
MIRROR = {"arm_l": "arm_r", "arm_r": "arm_l", "forearm_l": "forearm_r", "forearm_r": "forearm_l",
          "thigh_l": "thigh_r", "thigh_r": "thigh_l", "shin_l": "shin_r", "shin_r": "shin_l"}


# ------------------------------------------------------------------ body
#
# The player's parts: every part is rigid and follows one joint (an
# N64-era limb model: a vertex follows exactly one bone). Clothing follows
# the bone it covers — the sleeve rides the upper arm, the shorts' legs ride
# the thighs — so skin never cuts through cloth; each joint's child reaches
# into its parent so no gap opens. `solid_eleven_models.py` builds the
# meshes from these solids (plus flat details: eyes, hems, numbers) and the
# game's fit test (`examples/solid-eleven/src/render/fit.rs`) poses them
# through every clip, mirror and crossfade and checks the joints.
#
# A solid is a frustum between two axis-aligned rectangles in the joint's
# frame: y0 (w0 x d0) to y1 (w1 x d1), centered at (cx, cz); `skip` lists
# faces never built because another part always covers them (the fit test
# checks that), `color` the base color class. Tiers: "pc" (11 joints, the
# PC's native profile), "near" / "far" (consoles; far players drop their
# arms; their legs are the kit legs, shorts leg and sock in one mesh).

def taper(y0, y1, w0, d0, w1, d1, cx=0.0, cz=0.0, skip="", color="base"):
    return {"y0": y0, "y1": y1, "w0": w0, "d0": d0, "w1": w1, "d1": d1, "cx": cx, "cz": cz,
            "skip": skip.split(), "color": color}


def box(y0, y1, w, d, cx=0.0, cz=0.0, skip="", color="base"):
    return taper(y0, y1, w, d, w, d, cx, cz, skip, color)


# part -> (joint (left side; "_x" parts are drawn on both sides), tint, tiers, triangle budget, solids)
BODY = {
    # --- the PC's native profile
    # The skull alone: the hair is a part of its own (`HAIR`), a shell
    # around it, so skin never cuts through hair.
    "head_hd": ("head", "skin", "pc", 80, [
        box(-0.06, 0.15, 0.11, 0.11, skip="-y +y"),
        taper(0.13, 0.25, 0.17, 0.19, 0.21, 0.235, skip="+y"),
        taper(0.25, 0.34, 0.21, 0.235, 0.215, 0.24, skip="-y +y"),
        taper(0.34, 0.40, 0.215, 0.24, 0.15, 0.175, skip="-y"),
    ]),
    "torso_hd": ("torso", "shirt", "pc", 70, [
        taper(-0.06, 0.07, 0.27, 0.16, 0.30, 0.19, skip="-y +y"),
        taper(0.07, 0.30, 0.30, 0.19, 0.35, 0.22, skip="-y +y"),
        # The shoulders flare over the arms' pivots (the sleeves' caps turn
        # inside them).
        taper(0.30, 0.46, 0.35, 0.22, 0.54, 0.19, skip="-y"),
    ]),
    # The hem low and wide enough that the shorts legs (on the thighs) stay
    # inside its sides: only their bottoms show, and a raised thigh lifts its
    # own, never a flap out to the side.
    "shorts_hd": ("pelvis", "shorts", "pc", 50, [
        taper(-0.21, 0.07, 0.35, 0.22, 0.32, 0.21),
    ]),
    "sleeve_x": ("arm_l", "shirt", "pc", 24, [
        taper(-0.15, 0.04, 0.13, 0.13, 0.12, 0.12),
    ]),
    "upper_arm_x": ("arm_l", "skin", "pc", 14, [
        taper(-0.32, -0.1, 0.085, 0.085, 0.1, 0.1, skip="+y"),
    ]),
    "forearm_x": ("forearm_l", "skin", "pc", 24, [
        taper(-0.27, 0.02, 0.072, 0.072, 0.085, 0.085),
        box(-0.35, -0.27, 0.085, 0.095),
    ]),
    "shorts_leg_hd_x": ("thigh_l", "shorts", "pc", 24, [
        # Narrower at the top, where the shorts taper in to the waist.
        taper(-0.24, 0.04, 0.145, 0.18, 0.12, 0.17),
    ]),
    "thigh_x": ("thigh_l", "skin", "pc", 26, [
        taper(-0.40, -0.18, 0.115, 0.12, 0.13, 0.14, skip="+y"),
        taper(-0.49, -0.40, 0.105, 0.11, 0.115, 0.12, skip="+y"),
    ]),
    "shin_x": ("shin_l", "socks", "pc", 40, [
        taper(-0.08, 0.02, 0.105, 0.115, 0.09, 0.095, skip="+y"),
        taper(-0.38, -0.08, 0.09, 0.095, 0.105, 0.115, skip="+y"),
        box(-0.47, -0.38, 0.11, 0.26, cz=0.04, color="boot"),
    ]),
    # --- consoles
    # One wedge from the neck (inside the collar) to the crown; hair on
    # top and at the back, two eye quads.
    "head": ("head", "skin", "near far", 14, [
        taper(-0.06, 0.39, 0.12, 0.13, 0.22, 0.24, skip="-y"),
    ]),
    "torso": ("torso", "shirt", "near far", 18, [
        taper(-0.06, 0.30, 0.28, 0.16, 0.34, 0.22, skip="-y +y"),
        taper(0.30, 0.46, 0.34, 0.22, 0.56, 0.22, skip="-y"),
    ]),
    "shorts": ("pelvis", "shorts", "near far", 18, [
        taper(-0.24, 0.07, 0.37, 0.22, 0.32, 0.21),
    ]),
    "arm_x": ("arm_l", "skin", "near", 12, [
        taper(-0.62, 0.0, 0.07, 0.075, 0.085, 0.085),
    ]),
    "shorts_leg_x": ("thigh_l", "shorts", "near far", 12, [
        taper(-0.28, 0.02, 0.15, 0.17, 0.13, 0.16),
    ]),
    "leg_x": ("thigh_l", "socks", "near far", 20, [
        # Its top, under the shorts leg, narrower than the shorts leg there.
        taper(-0.82, -0.05, 0.10, 0.11, 0.12, 0.15, skip="+y -y"),
        box(-0.91, -0.82, 0.11, 0.26, cz=0.04, color="boot"),
    ]),
}


# ------------------------------------------------------------------ hair
#
# The PC's hair styles: parts on the head joint, tinted with the hair's
# color, drawn over the skull above. Each is built around the skull, never
# through it: a shell is the skull's own solids above a hairline, every
# side pushed out by a margin (the sides are parallel, so the gap holds all
# the way up and over the top); back and side locks stand off the skull's
# widest section. The fit test (`render/fit.rs`) checks that no outer face
# of a style comes within 2 mm of the skull, and poses every style through
# every clip against the rest of the body (long hair against the collar).

SKULL = BODY["head_hd"][4][1:]


def _span(so, y):
    """Width and depth of solid `so` at height y (linear, extrapolated)."""
    f = (y - so["y0"]) / (so["y1"] - so["y0"])
    return so["w0"] + (so["w1"] - so["w0"]) * f, so["d0"] + (so["d1"] - so["d0"]) * f


def skull_extent(y0, y1):
    """The skull's widest half-width and deepest half-depth over y0..y1."""
    hw = hd = 0.0
    for so in SKULL:
        for y in (max(y0, so["y0"]), min(y1, so["y1"])):
            if so["y0"] <= y <= so["y1"]:
                w, d = _span(so, y)
                hw, hd = max(hw, w / 2), max(hd, d / 2)
    return hw, hd


def shell(hairline, m):
    """The skull's solids above `hairline`, each side `m` out, the top `m`
    over the crown. Internal faces between the layers are left out."""
    out = []
    layers = [so for so in SKULL if so["y1"] > hairline]
    for i, so in enumerate(layers):
        y0 = max(so["y0"], hairline)
        y1 = so["y1"] + (m if i == len(layers) - 1 else 0.0)
        w0, d0 = _span(so, y0)
        w1, d1 = _span(so, y1)
        skip = ([] if i == 0 else ["-y"]) + ([] if i == len(layers) - 1 else ["+y"])
        out.append(taper(y0, y1, w0 + 2 * m, d0 + 2 * m, w1 + 2 * m, d1 + 2 * m, skip=" ".join(skip)))
    return out


def back_lock(y0, y1, t, m=0.003, w=0.2):
    """Hair down the back of the head, `t` thick, `m` off the skull (its
    outer face about flush with the shell's back)."""
    _, hd = skull_extent(y0, y1)
    return box(y0, y1, w, t, cz=-(hd + m + t / 2))


def side_locks(y0, y1, t, z0, z1, m=0.003):
    """Hair down both sides of the head (the temples, where the ears would
    be), `t` thick, `m` off the skull."""
    hw, _ = skull_extent(y0, y1)
    x = hw + m + t / 2
    return [box(y0, y1, t, z1 - z0, cx=s * x, cz=(z0 + z1) / 2) for s in (-1, 1)]


def _curly():
    """A rounded mass, fuller at the back and the sides than over the
    brow (no hat brim): a band out of the hairline, the crown, a narrower
    top; a thick lock low at the back, fuller sides."""
    hw, hd = skull_extent(0.30, 0.30)
    back = -0.014  # the mass sits back on the head
    band = taper(0.30, 0.35, 2 * hw + 0.03, 2 * hd + 0.03 - 2 * back,
                 2 * hw + 0.07, 2 * hd + 0.05 - 2 * back, cz=back, skip="+y")
    crown = taper(0.35, 0.44, band["w1"], band["d1"], band["w1"] - 0.02, band["d1"] - 0.02,
                  cz=back, skip="-y +y")
    top = taper(0.44, 0.475, crown["w1"], crown["d1"], crown["w1"] - 0.1, crown["d1"] - 0.1,
                cz=back, skip="-y")
    return [band, crown, top, back_lock(0.2, 0.32, 0.04, w=0.24)] + side_locks(0.22, 0.32, 0.012, -0.13, -0.01)


def _mohawk():
    """A stubble strip over the crown, the sides shaved, and a ridge on
    top. The strip's sides lie inside the skull (left out)."""
    strip = [dict(so) for so in shell(0.335, 0.004)]
    for so in strip:
        so["w0"] = so["w1"] = 0.07
        so["skip"] = so["skip"] + ["-x", "+x"]
    return strip + [taper(0.404, 0.47, 0.05, 0.17, 0.04, 0.22)]


# style -> (triangle budget, solids); the order is the game's `HairStyle`
# (after them, `Bald`: no hair part).
HAIR = {
    "crop": (60, shell(0.325, 0.01) + [back_lock(0.19, 0.33, 0.008)]
             + side_locks(0.24, 0.33, 0.006, -0.12, -0.03)),
    "buzz": (40, shell(0.335, 0.005) + [back_lock(0.21, 0.34, 0.004)]),
    "curly": (80, _curly()),
    "long": (70, shell(0.325, 0.012) + [back_lock(0.11, 0.33, 0.02)]
             + side_locks(0.16, 0.33, 0.012, -0.12, 0.04)),
    "mohawk": (40, _mohawk()),
}


def body_parts():
    """(model name, joint, tint, tiers, solids) per drawn part: a "_x" part
    once (one mesh, drawn on both sides, its joint named for the left)."""
    out = []
    for name, (joint, tint, tiers, budget, solids) in BODY.items():
        out.append((name[:-2] if name.endswith("_x") else name, joint, tint, tiers.split(), solids))
    return out


def hair_parts():
    """(model name, joint, tint, tiers, solids) per hair style (PC only)."""
    return [("hair_" + name, "head", "hair", ["pc"], solids) for name, (budget, solids) in HAIR.items()]


def solid_parts(tier):
    """The body's parts on `tier` and, on the PC, every hair style: what
    can touch the turf."""
    return [p for p in body_parts() + hair_parts() if tier in p[3]]


def mirror_pose(pose):
    """Left-right mirror of a pose dict: swap sides, negate ry and rz."""
    out = {}
    for j, (rx, ry, rz) in pose.items():
        out[MIRROR.get(j, j)] = (rx, -ry, -rz)
    return out


def run_keys(stride, knee, arm, elbow, lean, bob, lift):
    """A run/jog/sprint cycle: contact, passing, opposite contact, passing.
    `stride` thigh swing (deg), `knee` back-leg knee bend, `arm` arm swing,
    `elbow` bend, `lean` torso lean, `bob` pelvis rise, `lift` swing-leg knee
    lift at passing."""
    contact = {
        "torso": (lean, 4, 0), "head": (-lean * 0.6, -3, 0),
        "thigh_l": (-stride, 0, 0), "shin_l": (12, 0, 0),
        "thigh_r": (stride * 0.8, 0, 0), "shin_r": (knee, 0, 0),
        "arm_l": (arm, 0, 6), "forearm_l": (-elbow * 0.7, 0, 0),
        "arm_r": (-arm, 0, -6), "forearm_r": (-elbow, 0, 0),
    }
    passing = {
        "torso": (lean, 0, 0), "head": (-lean * 0.6, 0, 0),
        "thigh_l": (0, 0, 0), "shin_l": (8, 0, 0),
        "thigh_r": (-lift, 0, 0), "shin_r": (knee + 20, 0, 0),
        "arm_l": (0, 0, 6), "forearm_l": (-elbow, 0, 0),
        "arm_r": (0, 0, -6), "forearm_r": (-elbow, 0, 0),
    }
    # The support leg straightens at passing, which lifts the pelvis by
    # itself; `bob` is the flight a fast stride adds there.
    return [
        (0.0, contact, (0, 0, 0)),
        (0.25, passing, (0, bob, 0)),
        (0.5, mirror_pose(contact), (0, 0, 0)),
        (0.75, mirror_pose(passing), (0, bob, 0)),
    ]


IDLE = {"torso": (2, 0, 0), "arm_l": (0, 0, 8), "arm_r": (0, 0, -8), "forearm_l": (-12, 0, 0),
        "forearm_r": (-12, 0, 0), "shin_l": (4, 0, 0), "shin_r": (4, 0, 0), "thigh_l": (-2, 0, 3),
        "thigh_r": (-2, 0, -3)}

# clip name -> (seconds, loop, keys [(t 0..1, {joint: (rx, ry, rz)}, root (dx, dy, dz))])
CLIPS = {
    "idle": (1.6, True, [
        (0.0, IDLE, (0, 0, 0)),
        (0.5, {**IDLE, "torso": (4, 0, 0), "arm_l": (0, 0, 10), "arm_r": (0, 0, -10), "thigh_l": (-5, 0, 3),
               "thigh_r": (-5, 0, -3), "shin_l": (10, 0, 0), "shin_r": (10, 0, 0)}, (0, 0, 0)),
    ]),
    "walk": (1.0, True, run_keys(stride=22, knee=25, arm=18, elbow=15, lean=2, bob=0.0, lift=12)),
    "jog": (0.70, True, run_keys(stride=34, knee=55, arm=30, elbow=55, lean=8, bob=0.03, lift=40)),
    "sprint": (0.52, True, run_keys(stride=48, knee=80, arm=48, elbow=80, lean=18, bob=0.05, lift=62)),
    "dribble": (0.62, True, run_keys(stride=30, knee=50, arm=26, elbow=50, lean=14, bob=0.02, lift=34)),
    # Kicks: plant the left foot, swing the right leg through the ball.
    "pass": (0.42, False, [
        (0.0, {"torso": (6, 0, 0), "thigh_r": (25, 0, 0), "shin_r": (50, 0, 0), "arm_l": (-10, 0, 25),
               "arm_r": (15, 0, -20), "forearm_l": (-20, 0, 0), "forearm_r": (-20, 0, 0)}, (0, 0, 0)),
        (0.45, {"torso": (4, -10, 0), "thigh_r": (-35, 15, 0), "shin_r": (5, 0, 0), "thigh_l": (5, 0, 0),
                "arm_l": (-20, 0, 35), "arm_r": (25, 0, -25), "forearm_l": (-25, 0, 0)}, (0, 0, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    "lob": (0.5, False, [
        (0.0, {"torso": (-4, 0, 0), "thigh_r": (40, 0, 0), "shin_r": (70, 0, 0), "arm_l": (-20, 0, 40),
               "arm_r": (20, 0, -35), "forearm_l": (-30, 0, 0)}, (0, 0, 0)),
        (0.45, {"torso": (-12, 0, 0), "thigh_r": (-60, 0, 0), "shin_r": (10, 0, 0), "arm_l": (-30, 0, 50),
                "arm_r": (30, 0, -45)}, (0, 0, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    "shot": (0.55, False, [
        (0.0, {"torso": (8, 0, 0), "thigh_r": (55, 0, 0), "shin_r": (95, 0, 0), "thigh_l": (-10, 0, 0),
               "arm_l": (-35, 0, 45), "arm_r": (30, 0, -40), "forearm_l": (-30, 0, 0)}, (0, 0, 0)),
        (0.35, {"torso": (14, -8, 0), "thigh_r": (-75, 10, 0), "shin_r": (8, 0, 0), "thigh_l": (8, 0, 0),
                "shin_l": (20, 0, 0), "arm_l": (-45, 0, 55), "arm_r": (40, 0, -50)}, (0, 0, 0.05)),
        (0.65, {"torso": (10, 0, 0), "thigh_r": (-40, 0, 0), "shin_r": (30, 0, 0), "arm_l": (-20, 0, 30),
                "arm_r": (20, 0, -30)}, (0, 0, 0.1)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # A full-power strike: a longer backswing, the body over the ball, the
    # kicking leg through high, the arms wide. (Grounded: the standing foot
    # stays planted, so no hop.)
    "power_shot": (0.65, False, [
        (0.0, {"torso": (4, 6, 0), "thigh_r": (70, 0, 0), "shin_r": (110, 0, 0), "thigh_l": (-14, 0, 0),
               "shin_l": (14, 0, 0), "arm_l": (-45, 0, 55), "arm_r": (40, 0, -50), "forearm_l": (-30, 0, 0),
               "forearm_r": (-20, 0, 0)}, (0, 0, 0)),
        (0.3, {"torso": (14, -6, 0), "thigh_r": (-90, 12, 0), "shin_r": (6, 0, 0), "thigh_l": (10, 0, 0),
               "shin_l": (24, 0, 0), "arm_l": (-55, 0, 70), "arm_r": (50, 0, -65), "forearm_l": (-20, 0, 0)},
         (0, 0, 0.08)),
        (0.55, {"torso": (12, -4, 0), "thigh_r": (-70, 6, 0), "shin_r": (20, 0, 0), "thigh_l": (4, 0, 0),
                "shin_l": (10, 0, 0), "arm_l": (-35, 0, 50), "arm_r": (30, 0, -45)}, (0, 0, 0.18)),
        (1.0, IDLE, (0, 0, 0.1)),
    ]),
    "tackle": (0.4, False, [
        (0.0, {"torso": (15, 0, 0), "thigh_l": (-45, 0, 8), "shin_l": (15, 0, 0), "thigh_r": (25, 0, 0),
               "shin_r": (40, 0, 0), "arm_l": (-20, 0, 25), "arm_r": (25, 0, -30)}, (0, 0, 0)),
        (0.5, {"torso": (15, 0, 0), "thigh_l": (-60, 0, 10), "shin_l": (5, 0, 0), "thigh_r": (30, 0, 0),
               "shin_r": (60, 0, 0), "arm_l": (-30, 0, 35), "arm_r": (30, 0, -35)}, (0, 0, 0.15)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # Down onto the turf, the leading leg out along it, the other splayed
    # out to the side (straight on the consoles, its knee bent flat on the
    # PC): the pelvis rolls back onto the left hip, the hips on the grass.
    "slide": (0.6, False, [
        (0.0, {"pelvis": (-25, 0, -8), "torso": (12, 0, 4), "thigh_l": (-50, 0, 0), "shin_l": (10, 0, 0),
               "thigh_r": (-5, 0, 0), "shin_r": (75, 0, 0), "arm_l": (-10, 0, 45), "arm_r": (10, 0, -45),
               "forearm_l": (-20, 0, 0), "forearm_r": (-20, 0, 0)}, (0, 0, 0)),
        (0.25, {"pelvis": (-35, 0, -14), "torso": (-8, 0, 6), "head": (14, 0, 0), "thigh_l": (-49, 0, 2), "shin_l": (0, 0, 0), "thigh_r": (-12, 0, -60), "shin_r": (20, 0, 0), "arm_l": (-25, 0, 65), "arm_r": (25, 0, -70), "forearm_l": (-15, 0, 0), "forearm_r": (-15, 0, 0)}, (0, 0, 0)),
        (1.0, {"pelvis": (-35, 0, -14), "torso": (-8, 0, 6), "head": (14, 0, 0), "thigh_l": (-49, 0, 2), "shin_l": (0, 0, 0), "thigh_r": (-12, 0, -60), "shin_r": (20, 0, 0), "arm_l": (-25, 0, 65), "arm_r": (25, 0, -70), "forearm_l": (-15, 0, 0), "forearm_r": (-15, 0, 0)}, (0, 0, 0)),
    ]),
    # Knocked onto the back, a beat on the turf, then up through a sit.
    "down": (1.2, False, [
        (0.0, {"pelvis": (-22, 0, 0), "torso": (-12, 0, 0), "thigh_l": (-25, 0, 0), "shin_l": (20, 0, 0),
               "thigh_r": (-5, 0, 0), "shin_r": (10, 0, 0), "arm_l": (-50, 0, 40), "arm_r": (-50, 0, -40),
               "forearm_l": (-30, 0, 0), "forearm_r": (-30, 0, 0)}, (0, 0, -0.2)),
        (0.15, {"pelvis": (-86, 0, 0), "torso": (-2, 0, 0), "head": (12, 0, 0), "thigh_l": (2, 0, 4),
                "thigh_r": (-25, 0, -3), "shin_r": (45, 0, 0), "arm_l": (-8, 0, 55), "arm_r": (-8, 0, -65),
                "forearm_l": (-10, 0, 0), "forearm_r": (-10, 0, 0)}, (0, 0, -0.45)),
        (0.7, {"pelvis": (-86, 0, 0), "torso": (-2, 0, 0), "head": (12, 0, 0), "thigh_l": (2, 0, 4),
               "thigh_r": (-25, 0, -3), "shin_r": (45, 0, 0), "arm_l": (-8, 0, 55), "arm_r": (-8, 0, -65),
               "forearm_l": (-10, 0, 0), "forearm_r": (-10, 0, 0)}, (0, 0, -0.45)),
        (1.0, {"pelvis": (-12, 0, 0), "torso": (18, 0, 0), "thigh_l": (-55, 0, 0), "shin_l": (95, 0, 0),
               "thigh_r": (-15, 0, 0), "shin_r": (70, 0, 0), "arm_l": (-35, 0, 20), "arm_r": (-35, 0, -20),
               "forearm_l": (-20, 0, 0), "forearm_r": (-20, 0, 0)}, (0, 0, -0.1)),
    ]),
    # Up from the turf after a dive or a slide: rolling up off the side,
    # a knee, a crouch, standing.
    "get_up": (0.8, False, [
        (0.0, {"pelvis": (0, 0, -70), "torso": (0, 0, -4), "head": (0, 0, -6), "thigh_l": (-5, 0, 4),
               "thigh_r": (-16, 0, -4), "shin_r": (35, 0, 0), "arm_l": (-12, 0, 120), "arm_r": (-55, 0, 95),
               "forearm_l": (-5, 0, 0), "forearm_r": (-5, 0, 0)}, (0, 0, 0)),
        (0.35, {"pelvis": (0, 0, -10), "torso": (18, 0, 6), "head": (-12, 0, 0), "thigh_l": (-70, 0, 8),
                "shin_l": (95, 0, 0), "thigh_r": (8, 0, -4), "shin_r": (100, 0, 0), "arm_l": (-35, 0, 30),
                "arm_r": (-60, 0, 30), "forearm_l": (-30, 0, 0), "forearm_r": (-30, 0, 0)}, (0, 0, 0)),
        (0.7, {"torso": (18, 0, 0), "head": (-8, 0, 0), "thigh_l": (-38, 0, 4), "shin_l": (65, 0, 0),
               "thigh_r": (-30, 0, -4), "shin_r": (60, 0, 0), "arm_l": (-15, 0, 18), "arm_r": (-15, 0, -18),
               "forearm_l": (-20, 0, 0), "forearm_r": (-20, 0, 0)}, (0, 0, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    "header": (0.45, False, [
        (0.0, {"torso": (-10, 0, 0), "head": (-15, 0, 0), "arm_l": (-40, 0, 45), "arm_r": (-40, 0, -45),
               "thigh_l": (-10, 0, 0), "shin_l": (40, 0, 0), "thigh_r": (-10, 0, 0), "shin_r": (40, 0, 0)},
         (0, 0, 0)),
        (0.4, {"torso": (15, 0, 0), "head": (25, 0, 0), "arm_l": (-60, 0, 60), "arm_r": (-60, 0, -60),
               "thigh_l": (-5, 0, 0), "shin_l": (20, 0, 0), "thigh_r": (5, 0, 0), "shin_r": (30, 0, 0)},
         (0, 0.35, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # Both hands over the head, then forward.
    "throw": (0.6, False, [
        (0.0, {"torso": (-15, 0, 0), "arm_l": (-170, 0, 10), "arm_r": (-170, 0, -10), "forearm_l": (-70, 0, 0),
               "forearm_r": (-70, 0, 0), "thigh_l": (-15, 0, 0)}, (0, 0, -0.05)),
        (0.4, {"torso": (12, 0, 0), "arm_l": (-120, 0, 8), "arm_r": (-120, 0, -8), "forearm_l": (-10, 0, 0),
               "forearm_r": (-10, 0, 0), "thigh_l": (-15, 0, 0)}, (0, 0, 0.05)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # Arms up, fists pumping, little hops (the title screen's huddle).
    "celebrate": (0.9, True, [
        (0.0, {"torso": (-6, 0, 0), "head": (-15, 0, 0), "arm_l": (-165, 0, -18), "arm_r": (-165, 0, 18),
               "forearm_l": (-40, 0, 0), "forearm_r": (-40, 0, 0)}, (0, 0, 0)),
        (0.5, {"torso": (-10, 0, 0), "head": (-20, 0, 0), "arm_l": (-172, 0, -12), "arm_r": (-172, 0, 12),
               "forearm_l": (-5, 0, 0), "forearm_r": (-5, 0, 0), "thigh_l": (-25, 0, 0), "shin_l": (50, 0, 0),
               "thigh_r": (-25, 0, 0), "shin_r": (50, 0, 0)}, (0, 0.18, 0)),
    ]),
    # The captain lifts the trophy over his head, both hands on it, with
    # little hops (the game draws the cup between his hands).
    "trophy_lift": (1.1, True, [
        (0.0, {"torso": (-8, 0, 0), "head": (-18, 0, 0), "arm_l": (-166, 0, 9), "arm_r": (-166, 0, -9),
               "forearm_l": (-18, 0, 0), "forearm_r": (-18, 0, 0), "thigh_l": (-4, 0, 3),
               "thigh_r": (-4, 0, -3), "shin_l": (8, 0, 0), "shin_r": (8, 0, 0)}, (0, 0, 0)),
        (0.5, {"torso": (-12, 0, 0), "head": (-24, 0, 0), "arm_l": (-174, 0, 7), "arm_r": (-174, 0, -7),
               "forearm_l": (-8, 0, 0), "forearm_r": (-8, 0, 0), "thigh_l": (-18, 0, 3),
               "thigh_r": (-18, 0, -3), "shin_l": (36, 0, 0), "shin_r": (36, 0, 0)}, (0, 0.1, 0)),
    ]),
    # Keeper: ready crouch, dive (to his left; the game mirrors it), hold.
    "gk_ready": (1.2, True, [
        (0.0, {"torso": (22, 0, 0), "head": (-18, 0, 0), "thigh_l": (-35, 0, 12), "shin_l": (60, 0, 0),
               "thigh_r": (-35, 0, -12), "shin_r": (60, 0, 0), "arm_l": (-35, 0, 35), "arm_r": (-35, 0, -35),
               "forearm_l": (-30, 0, 0), "forearm_r": (-30, 0, 0)}, (0, 0, 0)),
        (0.5, {"torso": (24, 0, 0), "head": (-18, 0, 0), "thigh_l": (-38, 0, 12), "shin_l": (64, 0, 0),
               "thigh_r": (-38, 0, -12), "shin_r": (64, 0, 0), "arm_l": (-40, 0, 38), "arm_r": (-40, 0, -38),
               "forearm_l": (-30, 0, 0), "forearm_r": (-30, 0, 0)}, (0, 0, 0)),
    ]),
    # Push off, fly stretched along the line of the dive, land on the side:
    # the whole body rolls (pelvis included) toward his left.
    "gk_dive": (0.6, False, [
        (0.0, {"pelvis": (0, 0, -15), "torso": (12, 0, -8), "thigh_l": (-30, 0, 10), "shin_l": (55, 0, 0),
               "thigh_r": (-12, 0, -6), "shin_r": (20, 0, 0), "arm_l": (-60, 0, 60), "arm_r": (-60, 0, 5),
               "forearm_l": (-20, 0, 0), "forearm_r": (-20, 0, 0)}, (0, 0, 0)),
        (0.3, {"pelvis": (0, 0, -68), "torso": (0, 0, -8), "head": (0, 0, -6), "thigh_l": (-5, 0, 6),
               "thigh_r": (-14, 0, -8), "shin_r": (30, 0, 0), "arm_l": (-12, 0, 165), "arm_r": (-55, 0, 125),
               "forearm_l": (-5, 0, 0), "forearm_r": (-5, 0, 0)}, (0.6, 0.25, 0)),
        (0.7, {"pelvis": (0, 0, -84), "torso": (0, 0, -4), "head": (0, 0, -6), "thigh_l": (-5, 0, 4),
               "thigh_r": (-16, 0, -4), "shin_r": (35, 0, 0), "arm_l": (-12, 0, 168), "arm_r": (-55, 0, 130),
               "forearm_l": (-5, 0, 0), "forearm_r": (-5, 0, 0)}, (1.0, 0, 0)),
        (1.0, {"pelvis": (0, 0, -84), "torso": (0, 0, -4), "head": (0, 0, -6), "thigh_l": (-5, 0, 4),
               "thigh_r": (-16, 0, -4), "shin_r": (35, 0, 0), "arm_l": (-12, 0, 168), "arm_r": (-55, 0, 130),
               "forearm_l": (-5, 0, 0), "forearm_r": (-5, 0, 0)}, (1.1, 0, 0)),
    ]),
    "gk_hold": (1.2, True, [
        (0.0, {"torso": (8, 0, 0), "arm_l": (-70, 0, -4), "arm_r": (-70, 0, 4), "forearm_l": (-50, 0, 0),
               "forearm_r": (-50, 0, 0)}, (0, 0, 0)),
        (0.5, {"torso": (10, 0, 0), "arm_l": (-72, 0, -4), "arm_r": (-72, 0, 4), "forearm_l": (-52, 0, 0),
               "forearm_r": (-52, 0, 0)}, (0, 0, 0)),
    ]),
    # Officials: the referee's arm points the restart, a card goes up; the
    # linesman raises his flag (the flag is drawn at his hand).
    "ref_point": (0.8, False, [
        (0.0, IDLE, (0, 0, 0)),
        (0.4, {**IDLE, "arm_l": (-95, 0, 25), "forearm_l": (0, 0, 0)}, (0, 0, 0)),
        (1.0, {**IDLE, "arm_l": (-95, 0, 25), "forearm_l": (0, 0, 0)}, (0, 0, 0)),
    ]),
    "ref_card": (0.8, False, [
        (0.0, IDLE, (0, 0, 0)),
        (0.4, {**IDLE, "torso": (-4, 0, 0), "arm_r": (-178, 0, -4), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
        (1.0, {**IDLE, "torso": (-4, 0, 0), "arm_r": (-178, 0, -4), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
    ]),
    "flag_up": (0.6, False, [
        (0.0, IDLE, (0, 0, 0)),
        (0.5, {**IDLE, "arm_r": (-175, 0, -6), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
        (1.0, {**IDLE, "arm_r": (-175, 0, -6), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
    ]),
}

# Clips that keep a foot (or a hip, a back) on the turf every frame: the
# game snaps the body down onto it. The others only stop at the turf
# (runs have a flight phase, hops and dives leave it).
GROUNDED = {"idle", "walk", "pass", "lob", "shot", "power_shot", "tackle", "slide", "down", "get_up", "throw", "gk_ready",
            "gk_hold", "ref_point", "ref_card", "flag_up"}


def quat_from_euler(rx, ry, rz):
    """(x, y, z, w) of R = Rz·Ry·Rx, degrees."""
    hx, hy, hz = (math.radians(a) * 0.5 for a in (rx, ry, rz))
    cx, sx = math.cos(hx), math.sin(hx)
    cy, sy = math.cos(hy), math.sin(hy)
    cz, sz = math.cos(hz), math.sin(hz)
    # q = qz * qy * qx
    w = cz * cy * cx + sz * sy * sx
    x = cz * cy * sx - sz * sy * cx
    y = cz * sy * cx + sz * cy * sx
    z = sz * cy * cx - cz * sy * sx
    return (x, y, z, w)


def _qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz)


def _qrot(q, v):
    x, y, z, w = q
    # v + 2w(q x v) + 2 q x (q x v)
    cx = y * v[2] - z * v[1]
    cy = z * v[0] - x * v[2]
    cz = x * v[1] - y * v[0]
    ccx = y * cz - z * cy
    ccy = z * cx - x * cz
    ccz = x * cy - y * cx
    return (v[0] + 2 * (w * cx + ccx), v[1] + 2 * (w * cy + ccy), v[2] + 2 * (w * cz + ccz))


def _corners(so):
    out = []
    for y, w, d in ((so["y0"], so["w0"], so["d0"]), (so["y1"], so["w1"], so["d1"])):
        for sx in (-1, 1):
            for sz in (-1, 1):
                out.append((so["cx"] + sx * w / 2, y, so["cz"] + sz * d / 2))
    return out


def lowest(pose, root, tier):
    """The body's lowest point (m over the turf) for `pose` with the pelvis
    offset `root`, on `tier`'s parts ("pc" or "near")."""
    frames = {}
    for name, parent, piv in JOINTS:
        q = quat_from_euler(*pose.get(name, (0, 0, 0)))
        t = piv if parent is not None else (piv[0] + root[0], piv[1] + root[1], piv[2] + root[2])
        if parent is None:
            frames[name] = (q, t)
        else:
            pq, pt = frames[parent]
            o = _qrot(pq, t)
            frames[name] = (_qmul(pq, q), (pt[0] + o[0], pt[1] + o[1], pt[2] + o[2]))
    low = 1e9
    for name, joint, tint, tiers, solids in solid_parts(tier):
        sides = [joint] + ([joint[:-2] + "_r"] if joint.endswith("_l") else [])
        for j in sides:
            q, t = frames[j]
            for so in solids:
                for c in _corners(so):
                    low = min(low, t[1] + _qrot(q, c)[1])
    return low


def grounded_root(pose, root, tier):
    """The pelvis offset putting the lowest point `root[1]` over the turf."""
    low = lowest(pose, (root[0], 0.0, root[2]), tier)
    return (root[0], root[1] - low, root[2])


def _nlerp(a, b, f):
    if sum(x * y for x, y in zip(a, b)) < 0:
        b = tuple(-x for x in b)
    q = tuple(x + (y - x) * f for x, y in zip(a, b))
    n = math.sqrt(sum(x * x for x in q))
    return tuple(x / n for x in q)


def frames_of(quats, root):
    """World (rotation, translation) per joint for joint quaternions."""
    frames = {}
    for (name, parent, piv), q in zip(JOINTS, quats):
        if parent is None:
            frames[name] = (q, (piv[0] + root[0], piv[1] + root[1], piv[2] + root[2]))
        else:
            pq, pt = frames[parent]
            o = _qrot(pq, piv)
            frames[name] = (_qmul(pq, q), (pt[0] + o[0], pt[1] + o[1], pt[2] + o[2]))
    return frames


def tier_points(tier):
    """(joint, corner) of every solid of `tier`'s parts, both sides."""
    pts = []
    for name, joint, tint, tiers, solids in solid_parts(tier):
        sides = [joint] + ([joint[:-2] + "_r"] if joint.endswith("_l") else [])
        for j in sides:
            for so in solids:
                for c in _corners(so):
                    pts.append((j, c))
    return pts


def contacts(tier, steps=48):
    """The corners that are ever the body's lowest (within 3 cm) through
    any clip, sampled like the game: the points the game plants."""
    pts = tier_points(tier)
    near = set()
    for name, (secs, loop, keys) in CLIPS.items():
        ks = [(t, key_quats(p), grounded_root(p, r, tier)) for t, p, r in keys]
        for k in range(steps + 1):
            u = k / steps
            if loop:
                u = u % 1.0
            i = max(i for i, kk in enumerate(ks) if kk[0] <= u + 1e-9) if u >= ks[0][0] else len(ks) - 1
            a = ks[i]
            if i + 1 < len(ks):
                b, span = ks[i + 1], ks[i + 1][0] - a[0]
            elif loop:
                b, span = ks[0], 1.0 - a[0] + ks[0][0]
            else:
                b, span = a, 1.0
            d = (u - a[0]) % 1.0 if loop else u - a[0]
            f = min(max(d / span, 0.0), 1.0) if span > 0 else 0.0
            f = f * f * (3 - 2 * f)
            qs = [_nlerp(x, y, f) for x, y in zip(a[1], b[1])]
            root = tuple(x + (y - x) * f for x, y in zip(a[2], b[2]))
            fr = frames_of(qs, root)
            ys = [fr[j][1][1] + _qrot(fr[j][0], c)[1] for j, c in pts]
            low = min(ys)
            for (j, c), y in zip(pts, ys):
                if y <= low + 0.03:
                    near.add((j, c))
                    # The game mirrors clips (a dive to the right).
                    near.add((MIRROR.get(j, j), (-c[0], c[1], c[2])))
    return sorted(near, key=lambda jc: (NAMES.index(jc[0]), jc[1]))


def key_quats(pose):
    return [quat_from_euler(*pose.get(name, (0, 0, 0))) for name in NAMES]


def rust_source():
    out = []
    w = out.append
    w("// @generated by tools/blender/solid_eleven_anim.py — do not edit.")
    w("// `cargo xtask blender solid-eleven` regenerates it (Blender not needed for")
    w("// this file: `python3 tools/blender/solid_eleven_anim.py --rust <path>`).")
    w("")
    w("use super::anim::{Clip, Key, Part, Solid, Tint, tier};")
    w("use trino_core::ModelId;")
    w("")
    w(f"/// Joints, in order: {', '.join(NAMES)}.")
    w(f"pub const JOINTS: usize = {len(NAMES)};")
    w("")
    w("/// Each joint's parent (`usize::MAX` = the root) and pivot from it (m).")
    w("pub const SKELETON: [(usize, [f32; 3]); JOINTS] = [")
    for name, parent, piv in JOINTS:
        p = "usize::MAX" if parent is None else str(NAMES.index(parent))
        w(f"    ({p}, [{piv[0]:.4f}, {piv[1]:.4f}, {piv[2]:.4f}]), // {name}")
    w("];")
    w("")
    for ci, (name, (secs, loop, keys)) in enumerate(CLIPS.items()):
        w(f"const K_{name.upper()}: [Key; {len(keys)}] = [")
        for t, pose, root in keys:
            qs = ", ".join("[{:.5f}, {:.5f}, {:.5f}, {:.5f}]".format(*q) for q in key_quats(pose))
            pc = grounded_root(pose, root, "pc")
            lo = grounded_root(pose, root, "near")[1]
            w(f"    Key {{ t: {t:.4f}, root: [{pc[0]:.4f}, {pc[1]:.4f}, {pc[2]:.4f}], lo: {lo:.4f}, q: [{qs}] }},")
        w("];")
    w("")
    bits = {"-x": 1, "+x": 2, "-y": 4, "+y": 8, "-z": 16, "+z": 32}
    tiers = {"pc": "tier::FULL", "near": "tier::NEAR", "far": "tier::FAR"}
    parts = []
    for name, joint, tint, tl, solids in body_parts():
        sides = [joint]
        if joint.endswith("_l"):
            sides.append(joint[:-2] + "_r")
        for j in sides:
            parts.append((name, j, tint, tl, solids))
    def emit(name, j, tint, tl, solids):
        w("    Part {")
        w(f"        model: ModelId::from_path(\"models/se_{name}\"),")
        w(f"        joint: {NAMES.index(j)}, // {j}")
        w(f"        tint: Tint::{tint.capitalize()},")
        w(f"        tiers: {' | '.join(tiers[t] for t in tl)},")
        w("        solids: &[")
        for so in solids:
            sk = sum(bits[f] for f in so["skip"])
            w("            Solid {{ y0: {:.4f}, y1: {:.4f}, w0: {:.4f}, d0: {:.4f}, w1: {:.4f}, d1: {:.4f}, cx: {:.4f}, cz: {:.4f}, skip: {} }},".format(
                so["y0"], so["y1"], so["w0"], so["d0"], so["w1"], so["d1"], so["cx"], so["cz"], sk))
        w("        ],")
        w("    },")

    w("/// The body's parts (`BODY` in the clip source), both sides of each limb.")
    w(f"pub const PARTS: [Part; {len(parts)}] = [")
    for part in parts:
        emit(*part)
    w("];")
    w("")
    hair = hair_parts()
    w("/// The PC's hair styles (`HAIR` in the clip source), in `HairStyle` order:")
    w(f"/// {', '.join(n[5:] for n, *_ in hair)}.")
    w(f"pub const HAIR: [Part; {len(hair)}] = [")
    for name, joint, tint, tl, solids in hair:
        emit(name, joint, tint, tl, solids)
    w("];")
    w("")
    for tier, const in (("pc", "CONTACTS_FULL"), ("near", "CONTACTS_LO")):
        cs = contacts(tier)
        w(f"/// The points of the {tier} body that touch the turf in some clip")
        w("/// (joint, point in its frame): the game plants the lowest of them.")
        w(f"pub const {const}: [(usize, [f32; 3]); {len(cs)}] = [")
        for j, c in cs:
            w(f"    ({NAMES.index(j)}, [{c[0]:.4f}, {c[1]:.4f}, {c[2]:.4f}]), // {j}")
        w("];")
        w("")
    w("/// Every clip, indexed by `ClipName as usize`.")
    w(f"pub const CLIPS: [Clip; {len(CLIPS)}] = [")
    for name, (secs, loop, keys) in CLIPS.items():
        g = "true" if name in GROUNDED else "false"
        w(f"    Clip {{ name: \"{name}\", seconds: {secs:.4f}, looped: {'true' if loop else 'false'}, grounded: {g}, keys: &K_{name.upper()} }},")
    w("];")
    w("")
    w("/// Clip names as an enum (same order as [`CLIPS`]).")
    w("#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]")
    w("pub enum ClipName {")
    for name in CLIPS:
        w(f"    {''.join(p.capitalize() for p in name.split('_'))},")
    w("}")
    w("")
    w("impl ClipName {")
    w("    /// Every clip, in [`CLIPS`] order.")
    w(f"    pub const ALL: [ClipName; {len(CLIPS)}] = [")
    for name in CLIPS:
        w(f"        ClipName::{''.join(p.capitalize() for p in name.split('_'))},")
    w("    ];")
    w("}")
    return "\n".join(out) + "\n"


def main(argv):
    if len(argv) >= 2 and argv[0] in ("--rust", "--check"):
        text = rust_source()
        if argv[0] == "--rust":
            # UTF-8 and LF on every OS (Windows defaults to cp1252 and CRLF).
            with open(argv[1], "w", encoding="utf-8", newline="\n") as f:
                f.write(text)
            print(f"wrote {argv[1]}: {len(CLIPS)} clips, {len(NAMES)} joints")
            return 0
        cur = ""
        if argv[1] != "-":
            with open(argv[1], encoding="utf-8", newline="") as f:
                cur = f.read()
        if cur != text:
            print(f"{argv[1]} is stale: run python3 tools/blender/solid_eleven_anim.py --rust {argv[1]}")
            return 1
        print("clips up to date")
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
