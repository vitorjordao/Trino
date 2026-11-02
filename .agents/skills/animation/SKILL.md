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
cargo xtask anim export <name> --out x.tang [--platform p]  # the baked blob, for a game that embeds it
cargo xtask anim export <name> --rust x.rs [--static NAME]  # its decoded tables as static Rust: no parser linked (#138)
# every anim command takes --dir <project> (another project's assets/, e.g. examples/solid-eleven)
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

- **Masks name joints** (glTF node names, exactly; empty = every joint), or
  a subtree: `"torso/*"` is the torso and every joint under it (issue
  #136; no other `*` form). `cargo xtask model inspect models/x` lists the
  joints. The bake resolves them against the model's joints and parents
  (`trino_asset_pipeline::joints`, TMDL order): without them a mask is a
  `check` error, never a silent all-joints mask.
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
  The renderer stores the offsets once per transition (`PoseCache`, #136),
  so it costs the destination's clips: cheaper than a crossfade where the
  tier keeps inertia.
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

## The pose cache (issue #136)

Each backend keeps a `render3d::PoseCache` across frames: a layer's
inertial offsets (`From − To` per joint) and an additive clip's first
frame, keyed by content (the mesh's clips, the layer, the frozen samples'
bits; the clip). It is filled on the serial draw path
(`PoseCache::prepare_growing`, at each draw entry, before shadows, the RSP
command or the PC's CPU workers) and read through `Mesh::with_pose_cache`
wherever a mesh is built with `with_poses`. A miss samples as before: the
pose is bit for bit the same. A new backend draw path: `with_poses(..)
.with_pose_cache(cache.entries())`, and prepare at its entry; clear the
cache in `upload_mesh`/unload (an address can be reused). A clip's
compressed track offsets are decoded once per clip in a pose
(`Located::of`). Never on the base N64 (its fold leaves no layer or
inertia to cache).
`prepare` checks its keys before locating anything: its first version
located every sample (decoding each clip's tracks) on every draw, hit or
miss, and the N64 Expansion Pak's layered draw went 1 568 → 1 930 µs (8
paddings) — the draw locates them again anyway. A cache in front of work
must cost only its lookup on a hit.

## Crowds: tables and sized instances (issue #138, ADR 0040)

Reading the `TANG` blob byte by byte cost a crowd member ~280 µs an update
and ~105 µs a pose on the N64 (Solid Eleven's player graph, 8 layouts), and
an `AnimGraphInstance` is ~800 B. For a crowd:

```rust
// Static tables (a console build: no parser, no decode at run time):
//   cargo xtask anim export player --dir examples/solid-eleven \
//       --rust examples/solid-eleven/src/render/player_graph.rs --static PLAYER_GRAPH
include!("player_graph.rs");                         // pub static PLAYER_GRAPH: GraphTables
type Crowd = GraphInstance<3, 0, 0>;                 // base + 2 layers, no inertia, no events
let mut players = [Crowd::new(&PLAYER_GRAPH); 22];  // ~220 B each (host 224)
p.update(&PLAYER_GRAPH, dt); let pose = p.pose(&PLAYER_GRAPH);
// Or at run time from a blob (live reload, tools): an Arena of bytes_for.
let t = GraphTables::decode_in(&g, &arena).unwrap(); // ~2 KB for the player graph
```

- **Every instance method takes `impl Into<GraphRef>`**: `&AnimGraph` (the
  blob) or `&GraphTables`. Both play the same arithmetic in the same order:
  digests, poses, events and root motion are identical
  (`trino_scene::anim::compare_tables`, the fuzz target, the `graph crowd`
  step's three variants on every target).
- **`GraphInstance<L, I, E>`**: `L` machines (base + `L - 1` layers; a graph
  with more plays the first ones), `I` inertia slots (an inertialized
  transition of a layer without one crossfades over the same time — see
  `target/evidence/graph138/no_inertia.png`), `E` = 0 (no events: nothing is
  fired, `events()` is empty) or 1. `AnimGraphInstance` is `<4, 4, 1>`.
  Digests of any size agree while no inertia runs.
- **Measured (ares, Expansion Pak, mean of 8 layouts), per crowd member:**
  blob 280 + 111 µs (update + pose) → tables 110 + 50 → `GraphInstance<3,
  0, 0>` over tables 86 + 46 (ADR 0040 §6, `N64_COST.us_per_graph_update`/
  `us_per_graph_pose`): 22 players ≈ 2.9 ms;
  `cargo xtask test n64` prints the three variants and fails when the crowd
  line passes twice `N64_COST`.
- **`sync: true` on a layer** (TANG v3): its looping states land, every
  update, at the fraction of a cycle the base's looping state is at (a carry
  layer in step with the walk). `GraphInstance::seek_layer(l, clock)` moves a
  layer's clock by hand (fires nothing).
- **`models: ["models/x_with_tray"]`** (TANG v3): one graph for rigs that
  differ by joints; each layer mask is resolved by joint name per model at
  the bake (a missing clip, another duration or a missing joint fails it);
  the game asks `g.for_model(model_id)` (and decodes that view's tables).
- The bake writes v2 (bytes unchanged) unless a graph uses `sync` or
  `models`.

## Layers read as data (Solid Eleven, Plan & Party, #136)

**Before #138 a graph instance per character didn't fit a crowd on the N64**
(~400 µs an update, 0.8 KB an instance: Solid Eleven's 22 players took the
football template from 30.4 to 39.9 ms). Solid Eleven still reads its kick
layers as data — its picking is the base — now from static tables
(`render::graph::PLAYER_GRAPH`). See the section above for running instances.

For a game that poses its own rig, read the layers as data:
`AnimGraph::layer_named("strike")` → `layer(l).mask` (rig joint bits) and
`layer(l).states` → `state_clip(s)` (a clip state's clip), once (a match, a
level), and lay the clip over the game's own picking with the game's own
weights. Bake the graph against a QA rig with the same clips and joints (a
PC-only model is enough), embed the blob (`anim export`, a test holding it to
its bake) and map the rig's joints to the game's (a test holding the names).
Solid Eleven: `render::graph::KickLayers`, `AnimState::pose_with` (a kick's
`strike` layer over the gait, its `arms` layer only while slow). Plan &
Party: a `carry` layer pushed with `Renderer::push_pose` over the walk, at the
walk's clock. Notes:

- A mask counts its model's joints: rigs that differ by a prop joint list the
  others in `models` (#138) and pick theirs with `for_model`.
- A layer that must stay in step with the base: `sync: true`, or
  `seek_layer` (#138).
- `GraphBlob` links the parser where it is called: a console build that never
  has the blob plays `anim export --rust` tables instead (#138).

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

The layer mask is picked on the 2D skeleton or **in the 3D preview**
(issue #136: "In the preview", or `TRINO_EDITOR_ANIM_SELECT=layer:upper,mask3d`):
dots where the renderer posed each joint, click toggles one, Shift+click its
subtree (`mask_toggle` keeps `"x/*"` entries as written).

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


`graph crowd` (`castle64::graph_crowd`, issue #138, last in `anim_steps`): 22
instances of Solid Eleven's player graph (embedded) through a scripted match,
three variants — `AnimGraphInstance` over the blob, over the tables,
`GraphInstance<3, 0, 0>` over the tables — whose hashes must agree and equal
`CROWD_HASH`; `TRINO_GRAPH_COST: scene=micro-graph variant=… update_us=…
pose_us=… instance_bytes=…` on every target (gated on the N64). The digest is
taken outside the timed spans (it was once inside and doubled `pose_us`).

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
- **Generic playback is a copy per source, per instance size and per
  crate on the N64** (#138's first cut: anim code 27 → 70 KB of text). The
  instance methods take `impl Into<GraphRef>` and call non-generic code over
  slices; the `GraphInstance<L, I, E>` wrappers stay thin. Keep it that way.
- **`optimize(size)` cost the graph crowd ~25%** (#138): the per-update path
  (advance, shape, transitions, pose) is compiled for speed; cold code
  (init, digest, transitions' setup, decode) stays `size`. The recursive
  condition evaluator cost ~400 cycles an edge: leaves under `And`/`Or` are
  now read in the loop.
- **Never keep a float-carrying `Option<ClipSample>` in a hot pose path**
  (#136): LLVM hoisted `f0 as f32 + k` of an absent blend above its test,
  and the unset `k` (a denormal) trapped the VR4300 — "Generic floating
  point exception" in `pose_world`, every padding, after an unrelated change
  moved the inlining. Use `ClipSample::NONE` and a flag (`Samples`).
