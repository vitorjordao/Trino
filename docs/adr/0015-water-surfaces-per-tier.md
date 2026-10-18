# ADR 0015: Water surfaces, one material with a tier per platform

Status: accepted (October 2026, GitHub #49)

## Context

Games want water that moves: a swell, ripples where things fall in, two layers of
texture flowing, the sky reflected at grazing angles, a glint of sun, foam on the shore.
On a modern GPU that is a vertex and a fragment shader. The consoles Trino targets have
no fragment shaders: the N64's RDP has a 2-cycle color combiner and 4 KB of TMEM, and
the 3DS's PICA200 has a programmable vertex unit but fixed-function TexEnv stages.

Issue #49 asked for a "tiered surface program": one declarative material, drawn by each
platform as well as its hardware allows, with the showcase reformulated around a river
that the hero, a robot and falling objects disturb.

## Decisions

### 1. The material: `ModelParams::water: Option<Water>`

- `trino_core::water::Water { flow_speed, detail_flow, wave_amplitude, wave_frequency,
  ripple_responsiveness }`, in material files as `water: Some((...))` (`WaterDef`, every
  field defaulted, ranges checked by `scene check`). The material version stays 1: the
  field is optional and skipped when absent.
- The water's second flowing layer is the material's `detail` texture. On water it is
  blended half and half with the base, not multiplied. The N64 tile and TMEM rules of
  ADR 0011 still apply: two intensity layers, no palette (32×32 I8 + I4 = 1.5 KB).
- Translucency comes from the mesh's vertex alpha and the tint's alpha. Fresnel makes
  the surface more opaque at grazing angles.
- TSCN v4 adds a 28-byte water block per node (flag bit 6). v3 blobs are refused.
- `ModelParams` grew by 32 bytes (108 on a 64-bit host).
  - Its `EntityStore` slots grow with it, so the showcase game is now ~25 KB.
  - The N64 self-test used to build that game on its 64 KB stack (58.7 KB peak).
    It now copies `ShowcaseGame::blank()` from a static into a box and resets it in
    place (`reset_from_scene`, `check_baked_in`).
  - The stack peak dropped to 25 KB.

### 2. Frame state: `Renderer::set_water(time, &[Ripple])`

- The clock drives the swell and the flow. The ripples are rings in world space
  (`Ripple { pos, age, strength }`), spreading at 1.6 m/s, 0.55 m apart, fading over
  2.2 s.
- Games keep a `RippleBuffer<4>` (`spawn`, `update`).
- `set_water` is cleared at `begin_frame`, like local lights and AO. It has a default
  body, so `GAME_API_VERSION` stays.
- `Caps::max_ripples = 4` on every target: the 3DS program holds four in its uniforms.
  Strict mode panics on more live ripples.
- `Caps::water_tier` names the tier (§4).

### 3. Geometry: displaced in the core, on every target

- The swell is two directional sines: along the flow, and 70° off it at 1.7× the
  frequency and 45 % of the height. Each crest moves at the deep-water speed
  `sqrt(g k / 2π)`.
- The ripples are `e² cos(2π (d − r) / L)` with `e = max(0, 1 − ((d − r)/W)²)`, a
  compact band around the ring's radius. A vertex outside every band pays one squared
  distance per ripple.
- Normals come from the analytic gradient.
- The sine is `water::sin_turns`: one `fract`, a parabola and a quartic term. It does
  no `fmodf`, which is software on the N64.
- The CPU path is a `WATER` instantiation of `render3d/lit.rs`'s copy of the T&L, kept
  out of `tessellate_joints`, whose N64 I-cache budget is untouched.
- Water never takes the 3DS's static-mesh GPU path (`GpuReject::Water`) or the draw
  memo. It never took the RSP either until #69 gave it an overlay of its own (ADR 0018).
- **Deviation from the issue:** there is no `vs_water` WGSL vertex shader. Every PC draw
  arrives already projected by the core (ADR 0003). Displacing on the GPU would need a
  second projection path, and the PC's console profiles must keep drawing exactly what
  the consoles draw (`n64_parity`). The PC adds its detail per pixel instead (§4).

### 4. The tiers

| Tier (`Caps::water_tier`) | Target | Surface |
|---|---|---|
| `PerPixel` | PC native | `fs_water`: the swell's and the ripples' normals evaluated **per pixel**, two short capillary waves, Schlick Fresnel toward the sky, a Blinn-Phong glint, bright ripple crests, and soft foam where the water meets the ground. The foam reads the scene's depth, copied before the first water segment. |
| `VertexProgram` | 3DS | `water.v.pica` displaces, lights and applies Fresnel, glint and fog per vertex on the PICA200, with the four ripples in uniform lanes. TexEnv stage 0 interpolates the two layers 50/50 (a constant); stage 1 multiplies by the vertex color. Draws that `gpu_water` refuses (crossing the near plane, local lights, skinned) take the CPU path with the same TexEnv. |
| `PerVertex` | N64, and the PC's N64 and 3DS profiles | The core displaces and shades per vertex: lighting, Fresnel (alpha too), glint, fog. The RDP's 2-cycle combiner computes `((TEX1 − TEX0) · ENV_A + TEX0) · SHADE` with `ENV_A = 0.5`, alpha from the shade. TILE1 is the detail tile (ADR 0011), translated by the layer's own flow. |

- **Water is drawn last in its batch, blended, with the depth test on and depth writes
  off.**
  - **N64:** a water bucket flushed after the batch's CPU triangles *and* RSP commands.
    `trino_3d_water` sets `RDPQ_BLENDER_MULTIPLY` and `rdpq_mode_zbuf(true, false)`.
    3D was never blended on the N64 before.
  - **3DS:** a water bucket. GPU water draws go first, then the CPU's water triangles,
    using `GPU_WRITE_COLOR`.
  - **PC:** a `TriKind::Water` run sorted last, on the water pipelines (depth write off).
- **The detail layer's offset:**
  - The core scrolls the base UVs. The detail layer reads `uv · scale + offset`, where
    `offset = frac(detail_flow · t − base · scale)` (`Water::detail_offset`).
  - On the N64 the offset is the TILE1 origin, `(1 − offset) · size`, because the RDP
    subtracts it.
  - On the 3DS it is a uniform on texcoord1 (`tri3d.v.pica` and `water.v.pica`).
  - On the PC it lives in the water sets.

### 5. Ripples on the per-vertex tiers: rings, not displacement alone

A river ribbon of 16 × 4 quads has vertices 0.7–2 m apart, and a 0.55 m ripple can't
show in them. So each ripple that reaches a water draw also gets a ring: 10 quads at
its radius, light, fading with it.

- `render3d::tessellate_ripples` emits the rings. `tessellate_with` adds them to every
  water draw, and the 3DS GPU path calls it for the rings alone.
- The rings sort after the surface (depth key 0).
- The per-pixel tier draws its rings in the shader instead.

### 6. Depth splits: fog only where it changes

`DEPTH_SPLIT` bisects long edges of fogged, opaque, untextured draws because the fog
factor is per vertex. **An edge entirely nearer than the fog's start, or past its end,
has the same factor at both ends, so bisecting it changes nothing.** Such edges are not
split now (`fog_split_band`).

- The rule is exact. No castle64 work counter or PC golden moved.
- The showcase's carved terrain went from 864 to 738 splits.

### 7. Clipping is symmetric

`clip_plane` interpolates from the inside vertex. Each neighbor of a clipped shared
edge walked it in its own direction and got a slightly different point, which left
single-pixel cracks on the N64 (seen as three sky pixels in the river terrain). No PC
golden and no castle64 golden changed.

### 8. Showcase river

The course is one `showcase::river` table: 17 rings with a half width each. From it
come:

- the carved terrain;
- the water ribbon (128 triangles, UVs along the flow, deep opaque blue in the middle,
  translucent turquoise at the banks);
- the floor the game walks on (`floor_height`: terrain, stepping stones, the bridge).

`cargo xtask gen-assets` writes the meshes, and a test checks that every terrain vertex
lies where the game's floor says.

The gameplay:

- the hero wades in, slower;
- a robot patrols across the ford;
- a loose crate can be shoved off the bank: it falls, floats downstream and respawns.

Whatever enters the water starts a ripple, a burst of droplets (`fx_drop`) and a
positional splash (`sounds/water_splash`). Wading keeps rippling. Two positional
`sounds/river_flow` loops murmur along the course.

## Measurements

- **N64, ares** (`showcase-river` perf scene: the scene camera, the hero wading, two
  ripples), per frame:
  - the river: **12.5 ms** of CPU T&L (129 vertices, 168 triangles counting 2 rings);
  - the carved terrain: 20.6 ms;
  - the whole showcase frame: 284 ms. All 46 draws are on the CPU because of the
    colored light and the fog.

  The issue's **≤ 1.2 ms river budget is not met.** The CPU T&L costs ~25 µs per
  source triangle on this ROM (the micro scenes), so 128 triangles alone are ~3 ms.
  Only the RSP can meet it (follow-up below).
- **3DS, Azahar:** the self-test's water sheet drawn by `water.v.pica` against the CPU
  path: 0 of 960 samples differ by more than 24, mean 0.08. The showcase river under
  its scene camera stays on the CPU path: part of it lies behind the camera.
- **PC:** `crates/platform-pc/tests/water.rs` and the goldens `water_native`,
  `water_n64`, `water_n3ds` and `showcase_river`.
- **After #69 (ADR 0018 §5):** the water and the rest of the showcase on the N64 RSP:
  `micro-river` 19.6 → 10.5 ms, `showcase-river` 286.6 → 75.2 ms (mean over 8 paddings).

## Consequences

- No water set: every target draws what it drew before.
  - castle64's goldens and work counters are unchanged.
  - The showcase's N64 golden changed: the river replaced the pond, and the stepping
    stones are 24-triangle slabs.
  - The showcase's 3DS bottom golden changed (its metrics).
- **Follow-ups:**
  - **An RSP water command.**
    - It would displace and shade on the overlay and feed textured triangles to
      `RDPQ_Triangle_Send_Async` (S/T s10.5, W and 1/W per vertex).
    - It needs fog and colored light too, or the showcase can't use it.
    - It is the way to the 1.2 ms river.
  - **Fragment lighting on the 3DS:** `C3D_LightEnv` with a Fresnel LUT, for a
    per-pixel glint.
  - **Splitting long water surfaces into pieces,** so the 3DS GPU takes the visible
    ones. The rings then need deduplication.
