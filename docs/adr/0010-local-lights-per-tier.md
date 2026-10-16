# ADR 0010: Local lights, one API with a quality tier per platform

Status: accepted (October 2026)

## Context

The engine lit a draw with one directional light, an ambient floor and fog
(`render3d::Light`). Games wanted lamps, lanterns, torches and stage spots: lights with a
position and a reach. The PC can shade those per pixel with highlights. The N64 cannot
afford even per-vertex point lights on most draws: its RSP overlay (ADR 0007) knows only
the global light, so a lit draw must take the CPU T&L.

The design ceiling says the engine exposes nothing the N64 can't do. Local lights *can*
run on the N64, per vertex on the CPU. The ceiling is about cost, so the API stays the
same everywhere and each platform declares what it evaluates.

## Decisions

### 1. One API on every target

- `trino_core::light`: `LocalLight { kind: Point | Spot { dir, inner, outer }, pos,
  color, intensity, radius }`. The falloff is `(1 - d²/r²)²`: smooth, and exactly zero
  past the radius, so a light provably never touches a draw outside it.
- `Renderer::set_local_lights(&[LocalLight])` and `Renderer::set_wetness(f32)`.
  **Both are cleared at `begin_frame`**, unlike `set_light` (lights usually move, and
  state that outlived frames once leaked fog between games). Draws made before the call
  keep the lights they had.
- The global light stays `Light`: that is the directional light (sun, moon, weather).
  The issue's `Light::Directional` is `Light`; `Point` and `Spot` are `LightKind`s.
- `ModelParams::gloss` (0..=255) and the material file's `gloss` (defaulted, not written
  when 0, so existing files keep their bytes).

### 2. The tier comes from `Caps`, never from the host

`Caps::light_tier` and `Caps::max_local_lights`:

| Caps | Tier | Cap | What it evaluates |
|---|---|---|---|
| `PC` (native profile) | `PerPixel` | 8 | diffuse, Blinn-Phong specular (`gloss`), wetness |
| `N3DS` (3DS, PC 3DS profile) | `PerVertex` | 4 | diffuse in the vertex colors |
| `N64` (N64, PC N64 profile) | `PerVertex` | 2 | diffuse in the vertex colors |

A target uses the first `max_local_lights` lights it is given. The PC's console profiles
follow their console's caps, so `TRINO_SIM=n64` renders the N64 tier and `n64_parity`
keeps holding.

### 3. Per vertex: a separate copy of the T&L

`TnlContext::with_local_lights` keeps the lights in view space (at most
`MAX_VERTEX_LIGHTS = 4`). `tessellate_with` sends a lit draw (not `Unlit`, and some light
reaching its bounding sphere) to `render3d/lit.rs`, a copy of `tessellate_joints` that
adds the local lights per vertex. Vertices out of every radius take the usual shading.

The copy is deliberate. The first version folded the lights into `tessellate_joints`
behind never-taken branches and zero-sized generics. That grew the static path's body
from 15.6 to 18.0 KB, past the VR4300's 16 KB instruction cache. Unlit and unlit-by-
local-light draws got 10–35 % slower in ares: `micro-tris` T&L went from 21.5 to
29.0 ms. With the copy, the static path is byte-for-byte the size it was (`0x3d04`), and
the micro scenes are back on the baseline.

### 4. On the N64, a lit draw leaves the RSP

`rsp_check` refuses a draw that a local light reaches (`RspReject::LocalLight`), and so
does a draw-memo hit, which re-tests the lights. No light set: every draw decides
exactly as before. Each lit draw pays the full CPU transform. In castle64's throne
room, the star (30 vertices) went from 0.14 ms on the RSP to 1.3 ms on the CPU.

So console games keep lights few and small. For floors they can draw a decal instead:
`ground_pool(light, floor_y)` gives the center, radius and color of the pool a light
throws on a horizontal floor. Drawn as opaque unlit discs, it stays on the RSP. A big
floor with vertices only at its corners can't show a per-vertex light at all.

The RSP overlay doesn't evaluate point lights. That would mean a per-vertex distance,
reciprocal square root and falloff in `rsp_trino.S` behind the oracle; it is a backlog
item.

### 5. Per pixel on the PC

`tessellate_surface` runs the same lit copy with each vertex's `SurfaceVert` as well:
world position, world normal and the unlit color. Its screen triangles are the ones
`tessellate_with` makes. The PC renderer's lit pipeline (`fs_tri_lit`) adds per pixel:

- the local lights' diffuse on the unlit color;
- Blinn-Phong highlights from the global light and the local lights, by `gloss`;
- wetness: surfaces facing up darken and turn glossy.

The vertex color already holds the ambient and the global diffuse, as on every target.
Each distinct lighting state of a frame (global light, local lights, wetness, eye) is
one *set* in a uniform buffer (8 per frame), and each lit vertex names its set. A draw
uses the lit pipeline only when it has something to add: local lights, wetness or gloss.
With none, the PC draws exactly what it drew before, which a test checks.

The fragment shader reads its set through a pointer into the uniform buffer. A `let`
copy of the set put its eight lights in private memory, read back from scratch for each
light and pixel: a 4K frame of an external project went from 8.6 to 19.1 ms on a Renoir
iGPU.

### 6. The 3DS

The 3DS evaluates lights per vertex in the core's T&L, like the N64 but with a cap of 4.
PICA200 fragment lighting would need the same normal plumbing as the PC's lit pipeline,
through the citro3d shim. It is a follow-up if a game needs it.

## Consequences

- Games set lights every frame, before the draws they light.
- No lights, no wetness, no gloss: every golden is unchanged on every target, and so are
  the T&L work counters.
- The N64's per-frame cost of a lit scene is visible in `cargo xtask test n64`.
  castle64's throne room, which has torches now, went from 9.3 to 16.0 ms, still under a
  60 fps frame. The ceiling was re-blessed on purpose.
- Not in scenes or the editor yet: a `light` component in scene files is a backlog item.
