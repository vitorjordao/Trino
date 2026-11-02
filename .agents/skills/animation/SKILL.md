---
name: animation
description: Animate Trino characters without hand-written clip logic — animation graphs (state machines, 1D and 2D blend spaces, layers with joint masks — override and additive —, inertialized transitions) as `assets/anim/<name>.anim.ron`, folded per platform by `Caps::anim_layers`, clip events and root motion in the manifest, keyframe compression (`anim_compression`, TMDL `ANMC`) for console RAM, and tiered foot IK (`ik`, `FootIk`, `Caps::anim_ik`) — checked with `cargo xtask anim check`, run headless with `anim simulate`, measured with `model inspect --anim --compressed`, edited in the Animation tab's Graph mode. Use for "state machine", "blend walk/run", "animation graph", "footstep sound/event", "root motion", "foot sliding", "compress animations / save RAM", "feet on slopes / IK", or porting a game's own clip-picking code to a graph.
---

# animation: graphs, events, root motion, compression, foot IK

Everything is text an agent writes and the editor edits; everything runs in
`trino_core::anim` (`no_std`, fixed size, deterministic), so a graph plays the
same poses on PC, N64, 3DS and the Web. The output is an `AnimRef` every
backend already draws: no backend code for any of it (ADR 0034).

## The files

| What | Where | Baked to |
|---|---|---|
| clip events, root motion, compression, IK chains | `[models.<name>]` in `assets/manifest.toml` | TMDL `AEVT`, `ROOT`, `ANMC`, `IKCH` |
| a graph | `assets/anim/<name>.anim.ron` | blob `anim/<name>` (`TANG`), every platform (rides `ui::bake`) |
| a scene entity playing a graph | `anim_graph: Some((graph: "anim/x", state: "s", params: {"speed": 0.8}))` on the entity | not in TSCN or exported code (a console game drives its graph itself) |

```toml
[models.strider]
file = "models/showcase/strider.glb"
anim_fps = 20
once = ["hop"]
root_motion = ["walk", "run"]          # the root's ground motion leaves the clip for a track
anim_compression = true                # or { rotation = 0.5, translation = 0.002, scale = 0.001 }
ik = { pelvis = "hips", left = ["thigh_l", "shin_l", "foot_l"], right = ["thigh_r", "shin_r", "foot_r"] }
events.walk = [{ at = 0.0, name = "foot_l" }, { at = 0.5, name = "foot_r" }]
events.hop = [{ at = 0.4, name = "sfx:jump" }]   # sfx:x carries sounds/x
```

Write `events.<clip>` as dotted keys on the model's table: the editor's text
edits (`manifest::edit::set_model_events`) keep every other line, and a
`[models.x.events]` sub-table would end the model's table for them.

```ron
AnimGraph(
    model: "models/strider",
    params: { "speed": Float(0.0), "grounded": Bool(true), "hop": Trigger },
    states: {                                   // written order kept; the first is the start
        "locomotion": Blend1D(param: "speed", points: [(0.0, "idle"), (0.8, "walk"), (2.0, "run")]),
        "hop": Clip(clip: "hop", root_motion: false, events: [(0.1, "crouch")]),
    },
    transitions: [                              // first true one wins; from: "*" or Any = any state
        (from: "locomotion", to: "hop", when: And([Trigger("hop"), Param("grounded")]), blend: 0.08),
        (from: "hop", to: "locomotion", when: Finished, blend: 0.2),
    ],
)
```

Conditions: `Always`, `Trigger("t")`, `Finished`, `Param("b")`, `Not(..)`,
`And([..])`, `Or([..])`, `Greater("f", 1.0)`, `Less("f", 0.5)`, `After(seconds)`.
States: `Clip(clip:, loop:, speed:, speed_param:, root_motion:, events:)`,
`Blend1D(param:, points:, wrap:, speed:, speed_param:, root_motion:)`.
Optional fields are written bare (`start: "idle"`, not `Some("idle")`); unknown
fields are errors.

## The loop (no GUI needed)

```
cargo xtask anim check [name] [--platform n64] [--json]   # names, kinds, limits, events past the end, unreachable, dead ends
cargo xtask anim simulate strider --script s.ron --out trace.json [--json]   # states, transitions, events, root motion, pose samples
cargo xtask anim new <name> --model models/x              # one state per clip
cargo xtask model inspect models/strider --anim --compressed [--json]   # stride, slide (constant vs carried), events, ANMC bytes and errors
TRINO_SCENE=scenes/striders.scene.ron cargo run -p trino-app-pc         # entities play their graphs, carried by root motion
```

A script: `Script(dt: 0.0166, duration: 5.0, steps: [(at: 0.5, set: {"speed": 0.8}), (at: 2.5, trigger: ["hop"])])`.

## Game code

```rust
let mut blob = GraphBlob::new("strider");             // in the game struct
let (graph, changed) = blob.get(platform);            // in update_with (live reload)
let g = graph.unwrap();
let speed = g.param_named("speed").unwrap();          // cache the indices
inst.set_float(speed, v);  inst.set_trigger(hop);
inst.update(&g, dt);
for e in inst.events().iter() { if let Some(s) = e.sound { audio.play_sound(s) } }
let (pos, yaw) = inst.root_motion().apply(pos, yaw, 1.0 / WORLD_SCALE);
let ik = FootIk::default().pose(caps, &matrix, inst.feet(&g).unwrap(), |p| ground_y(p));
params.anim = Some(inst.anim(&g).with_ik(ik));
```

- **A trigger lives one update**: set it right before `update`; a transition
  that reads it consumes it, the update clears the rest.
- **A clip state's clock is the game's own seconds** (unwrapped, not clamped):
  an instance updated with the same `dt` as an old `self.time += dt` poses
  bit-identically (the showcase robots, `xtask/tests/anim_showcase.rs`). A
  blend space's clock is cycles (both clips at the same fraction).
- **Several entities on one graph**: one instance each, `AnimGraphInstance::new_in`
  for a start state of its own.
- Events: a blend fires only its heavier clip's (walk and run don't both
  step); a transition fires only the state it goes to.
- An `AnimRef` mixes at most three clips (`AnimRef::blend` + `AnimRef::over`);
  a pose beyond that (layers, a 2D space fading into another, inertia) is an
  `AnimPose` through the renderer's pose table (below), folded per tier.

## Layers, 2D blend spaces, inertialized transitions (issue #112, ADR 0039)

```ron
AnimGraph(
    version: 2,                                  // written by to_ron when needed
    model: "models/strider",
    params: { "speed": Float(0.0), "aim_x": Float(0.0), "aim_y": Float(0.0), "aiming": Float(0.0), "fire": Trigger },
    states: { "locomotion": Blend1D(param: "speed", points: [(0.0, "idle"), (0.8, "walk"), (2.0, "run")]) },
    transitions: [(from: "locomotion", to: "hop", when: Trigger("hop"), blend: 0.15, mode: Inertial)],
    layers: [
        (name: "upper", mask: ["torso", "head", "arm_l", "arm_r"], weight: "aiming",   // or 0.6
         states: { "aim": Blend2D(x: "aim_x", y: "aim_y", points: [(0.0, 0.0, "aim"), (-1.0, 0.0, "aim_left"), ..]) },
         transitions: [..]),
        (name: "breath", mode: Additive, weight: 0.6, states: { "breathe": Clip(clip: "breathe") }),
    ],
)
```

- **Masks name joints** (glTF node names, exactly; empty = every joint):
  `cargo xtask model inspect models/x` lists them. The bake resolves them
  against the model (`trino_asset_pipeline::joint_names`): without the
  model's joints a mask is a `check` error, never a silent all-joints mask.
- **Layers are machines** of their own (states, transitions, start, layout),
  evaluated bottom to top after the base; a trigger a lower machine consumed
  is gone for the layers above. Only the base moves the root and gives
  `feet()`; every layer fires its events.
- **Override** lerps the masked joints toward the layer's pose by its weight;
  **Additive** adds each clip's motion from **its first frame** (author a
  breath or a recoil as full poses). Two override layers on the same joints:
  a `check` warning.
- **`Blend2D`**: Delaunay triangles baked from the points (no duplicates, not
  all on a line — `check` errors), barycentric weights, the nearest edge
  outside the hull, phase-synced like `Blend1D`.
- **`mode: Inertial`**: the destination plays alone from frame one plus the
  offset from where the source was, decaying over `blend` (`blend` > 0).
- **Game code:** `let anim = renderer.push_pose(&inst.pose(&g).with_ik(ik));`
  after `begin_frame`, then `ModelParams { anim: Some(anim), .. }`. The
  renderer folds it to its `Caps::anim_layers` (base N64 1 layer / 3 clips,
  Expansion Pak and 3DS 2 / 6, New 3DS / PC / Web 4 / 12) and keeps it until
  the next `begin_frame`. `inst.anim(&g)` still works: the base N64's fold.
- **The fold** (`AnimPose::fold`, the same on every target): layers past the
  count, then inertia → a crossfade from the frozen source, then the lightest
  samples. `cargo xtask anim check <g> --platform n64` prints it per tier;
  the editor's N64 look shows it (and its notes).
- **Foot slopes**: `FootIk::slope(caps, &m, feet, |p| ground_normal(p))` →
  `pose.with_slope(..)`; feet turn onto the ground at most 30°; layered poses
  only (the base N64 drops it).
- **Graphs ride their model's groups on the consoles**: a grouped model's
  graph is not in boot RAM; read it with `platform.group_blob(id)` once the
  group is loaded (`GraphBlob` is the boot path).
- `cargo xtask anim simulate` traces each layer (`layer` on segments and
  transitions, `layers` and the pose digest on samples, `inertial` marks).

## Root motion

`root_motion = ["walk"]` moves the root joint's ground translation and turn
(about +Y, from frame 0) into a `ROOT` track; the clip plays in place. The
graph sums the track over each update (`root_motion()`, in the character's
frame at the update's start). The reference: the strider's walk surges (the
hips speed up mid-step); played at a constant speed its planted foot slides
3 cm a frame, carried by its track 0.0 (`animation_metrics.rs::
root_motion_removes_the_foot_slide_of_a_surging_walk`). Build a root-motion
reference with a non-constant root speed, or the evidence proves nothing: a
constant-speed walk doesn't slide either way.

## Compression (`ANMC`)

Per joint and channel: a constant, or the keys Douglas-Peucker keeps,
quantized (rotations 4 × i16, translations/scales u16 over the track's
range). The bake decodes every frame with the runtime's code and adds keys
until every frame is within the bounds; a clip it can't help stays `ANIM`.
Measured: strider 42 304 → 5 672 bytes of clips (13 %), robot 20 928 → 1 930,
windmill 3 696 → 184 (all three compressed: the base N64's boot assets fell
22.5 KB; the robot's moved four pixels of the N64 `showcase_river` golden).

## Foot IK

`ik` bakes the chains; `IkPose { left, right }` (ground heights under each
foot, model millimetres) rides `AnimRef::ik`; `Mesh::pose` solves two bones
per leg out of line (`anim::ik::solve`), the pelvis dropping for the lower
foot, the feet keeping their clip orientation. `IkPose::NONE` is the clip's
pose bit for bit. `Caps::anim_ik` is on for PC/Web/3DS, off for the N64
(`FootIk { force: true, .. }` for one character, ~0.5 ms a draw in ares:
`perf::N64_COST.us_per_ik_draw`; 33–41 µs in Azahar). The graph's `feet()` gives the ankles for
the ground casts without a mesh.

## Editor

Animation tab › **Graph** (`TRINO_EDITOR_ANIM_GRAPH=strider:speed=1.4`,
`TRINO_EDITOR_ANIM_SELECT=state:locomotion`): canvas (drag states: `layout`),
parameters driving a live instance, the selected state/transition (conditions
as RON), the check's issues. **Clips** mode: the events lane, Root motion
(extract at bake, carry the preview), Foot IK (`TRINO_EDITOR_ANIM_VIEW=ik=<slope>:<step>[:force]`;
the N64 look has IK off). Inspector › Animation graph on a model entity.

## Self-test steps

`anim layers` (`castle64::selftest::anim_layers`, inside `anim_steps` with the
strider's group): `anim/strider_layers` plays a script whose whole-pose hash
and folds to two tiers are pinned (`ANIM_LAYERS_HASH`), the renderer's joints
for the busiest pose per tier (`ANIM_LAYERS_JOINTS`), layered vs folded
frames, the hardware T&L (RSP, PC GPU) against the CPU via
`GroupHost::set_hardware_tnl`, `TRINO_LAYER_COST:`.
`xtask/tests/asset_groups.rs` panics with the measured values when they move.


`anim graph` (pose hash pinned in `castle64::selftest::ANIM_GRAPH_HASH`;
`xtask/tests/engine_checks.rs::the_anim_graph_hash_is_pinned` prints a moved
one) and `foot ik` (tier, forced, frame read-back, `TRINO_IK_COST:`), on PC,
ares, Azahar and the browsers.

## Traps

- **A layered pose's slot lives one frame**: `push_pose` after `begin_frame`,
  never cache the returned `AnimRef` across frames (a stale slot draws the
  folded clips, or another pose).
- **A backend's new draw path must build its mesh with `with_poses`**, or it
  draws the folded `AnimRef` silently (`Renderer::pose_joints` tells).
- **Code is RAM on the base N64:** #112's first build was +33 KB of text and the
  base console ran out at the micro scenes. `optimize(size)` on every new
  console function (closures don't inherit it: make them `fn`s), and look for
  duplicated monomorphizations (`mips64-elf-nm -S --size-sort -C` against
  main's ELF): one `Ui` per size was 60 KB (now `UiCore`).

- **The base N64 has little heap to spare** (the self-test gates 128 KB free): a
  test-only model goes in its own group, loaded where nothing big allocates
  after (`anim_steps` runs last on every host), and one draw a frame in timing
  loops (a batch of 16 rigid models grew the renderer's batch buffers 24 KB for
  good). Check `cargo xtask test n64 --expansion-pak off`.

- A `Some(..)`-free optional RON field needs both `deserialize_with = "bare"`
  and `serialize_with = "bare_ser"`, or the editor's save writes `Some(..)`
  and the next load fails (the round trip test sets every optional field).
- RON hands a bare identifier (`Any`) to `deserialize_any` as a unit:
  `visit_unit`.
- A self-test counting triangles must add the hardware paths
  (`tnl.rsp_tris`, `gpu_tris`, `pc_gpu_tris`): an RSP-drawn model has
  `stats().tris == 0` on the N64.
- Validate every float a blob feeds to `clamp` or a division at parse: a
  corrupt duration made `f32::clamp` panic (`min > max`) in the fuzz test.
