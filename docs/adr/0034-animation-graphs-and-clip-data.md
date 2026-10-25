# ADR 0034: Animation 2.0 — graphs, clip events, root motion, compressed clips, foot IK

Status: accepted (October 2026, GitHub #65). Implemented in `trino-core` (`anim`:
`clip`, `graph`, `anmc`, `ik`; `AnimRef::over`/`ik`; `Mesh::pose_world`; TMDL
`ANMC`/`IKCH`/`ROOT`/`AEVT`; `Caps::anim_ik`; `perf::N64_COST.us_per_ik_draw`),
`trino-asset-pipeline` (manifest `events`, `root_motion`, `anim_compression`, `ik`;
`anim_compress`, `anim_graph`, the graphs riding `ui::bake`; clip metrics carried by the
root), `trino-scene` (`anim`: the RON model, `check`, `to_tang`, `simulate`; the
`anim_graph` entity component), the editor (Animation tab › Graph mode, the events lane,
root motion and foot IK previews, Inspector › Animation graph), xtask (`anim`, `model
inspect --compressed`), the showcase (its robots), castle64's self-test (`anim graph`,
`foot ik`) and the generated reference model `models/strider`.

## Context

Every character's animation logic was game code: castle64 poses rigid parts by hand,
Solid Eleven picks clips from Python-generated tables, the showcase plays one clip at its
own clock. The engine had clips, a two-clip crossfade (`AnimRef::blend_to`) and an
Animation tab that *measures* clips (stride, foot slide). Missing: state machines and
blend spaces as data, events on the timeline, root motion, foot IK, and RAM-cheap clips
for the N64.

## Decisions

### 1. A graph's output is an `AnimRef`

The backends already pose an `AnimRef` (`Mesh::pose`, called by the CPU T&L, the N64's
RSP skeleton path, the 3DS and the PC GPU T&L). So the graph runs in `trino_core::anim`
and hands them an `AnimRef`; no backend changed.

- `AnimRef` gains **`over: AnimBlend`** — a third clip over the first two
  (`lerp(lerp(clip, blend), over)`), what a blend space fading into a clip needs. It is
  not an `Option`: its floats are always initialized, so no N64 path reads a denormal
  payload (the `RspFog` pitfall). Weight 0 is off. A blend space fading into another
  blend space (four clips) drops the lightest and renormalizes.
- `AnimRef` gains **`ik: IkPose`** (§6). `IkPose::NONE` is the old pose bit for bit.
- `ModelParams` grows 20 bytes (an `EntityStore<128>` 2.6 KB): accepted; animated
  draws are never memoized, so the draw memo is unaffected.

### 2. Clip metadata lives with the model; games read it from the graph

The manifest's `[models.*]` gets `events.<clip>`, `root_motion`, `anim_compression` and
`ik` (§4–6). The model bake writes them as TMDL sections (`AEVT`, `ROOT`, `ANMC`,
`IKCH`): `Mesh` keeps only what the T&L needs (`ANMC`, `IKCH`; ROOT and AEVT are read by
`anim::ClipMeta` for tools), so no hot struct grew past a flag and the chains.

Games never parse a mesh: the **graph blob** (`TANG`) copies what playback needs from the
model's bake — each clip's duration, loop flag, events, root track and the IK feet per
frame — so `AnimGraphInstance::update` reads one blob.

### 3. `TANG` v1, the graph runtime

`assets/anim/<name>.anim.ron` → blob `anim/<name>` on every platform, baked against that
platform's model and riding `asset_pipeline::ui::bake` (so the PC/Web packs, the N64 and
3DS images, the memory plan and live reload all carry it with no five-place edit). The
format is in `crates/core/src/anim/graph.rs`: a table of parameters, clips, states and
transitions with offsets into a data area (points, event/root/feet tracks, conditions as
a prefix code). `AnimGraph::parse` validates every index, offset, condition and float
once (a fuzz test flips every byte), and the accessors read without panic paths (code is
RAM on the N64).

Semantics, chosen to be predictable and deterministic:

- The **first** transition whose condition holds fires, in the file's order; `from:
  Any` skips the state it leads to. Conditions: `Always`, `Trigger`, `Finished`, `Param`,
  `Not`, `And`, `Or`, `Greater`, `Less`, `After`.
- A **trigger lives one update**: the transition that reads it consumes it, the update
  clears what nothing consumed.
- A **clip state's clock is seconds, unwrapped and unclamped** — the same additions a
  game's own `self.time += dt` makes — so a graph reproduces an existing game's poses bit
  for bit (§8). A **blend space's** clock is cycles of its effective duration, both clips
  at the same fraction.
- **Events** fire on `(from, to]` of the playhead (an event at 0 fires on entry and every
  wrap, never twice; a hitch fires every crossing in order); a blend fires only its
  heavier clip's; a transition fires only its destination's. `sfx:x` carries `sounds/x`.
- **Root motion** sums each contributing clip's track delta, weighted.
- `digest()` quantizes the playback state for cross-target comparison.

### 4. Root motion is taken out at bake

`root_motion = ["walk"]` moves the root joint's ground translation (x, z) and turn
(about +Y), relative to frame 0, out of the clip into a `ROOT` track; the clip plays in
place. Doing it at bake keeps `Mesh::pose` and the N64 hot path unchanged. A delta across
wraps composes whole cycles; the 2D compose uses exact `(0, 1)` for no turn (the core's
approximate trig would shorten a straight walk).

### 5. `ANMC`: error-bounded keyframe tracks

Per joint and channel: a constant, or the keys Douglas-Peucker keeps (75 % of the
bound), quantized — rotations 4 × i16, translations and scales u16 over the track's
range. The bake decodes **every frame with the runtime's own code** and adds keys where a
frame misses until all pass; a clip body over 64 KB or no smaller than `ANIM` stays
`ANIM`. `Mesh::pose` computes each clip's joint offsets once per pose, then each sample is
a binary search and a lerp. Opt-in per model (`anim_compression = true` or bounds).

### 6. Foot IK, tiered

The manifest's `ik` names the legs (hip → knee → foot) and the pelvis; the bake resolves
names to joints and checks the chain descends (`IKCH`). `IkPose { left, right }` is the
ground under each foot relative to the clip's floor, in model millimetres (4 bytes).
`Mesh::pose_world` solves it after sampling, out of line (`anim::ik::solve`): the pelvis
drops for the lower foot, each leg bends by the law of cosines in the plane it already
bends in, each foot keeps its clip orientation. `FootIk::pose` turns ground heights (a
game's raycasts at the graph's `feet()`) into the pose, by **`Caps::anim_ik`**: on for
the PC, the Web and the 3DS, off for the N64 unless `force` (cost in
`N64_COST.us_per_ik_draw`). The tier is decided at update time, so the editor's N64 look
and the PC's N64 profile show what the console shows.

### 7. Authoring: text first, the editor writes the same files

- `trino_scene::anim`: the RON model (ordered maps, bare optional fields read *and*
  written bare, unknown fields denied, `from: "*"` or `Any`), `check` (names, kinds,
  limits, blend points, events past the end, unreachable states, one-shot dead ends,
  missing sounds), `to_tang`, `simulate` (a script of parameter changes → a trace).
- `cargo xtask anim list|check|simulate|new`, `model inspect --anim --compressed`.
- The editor's Animation tab: **Graph** mode (canvas with draggable states written to the
  file's `layout`, parameters driving a live instance, state/transition inspectors with
  conditions as RON, the check's issues, outside edits reloaded), the **events lane** and
  **root motion** in Clips mode (manifest edits through `manifest::edit`), the **foot IK**
  preview on a slope or a step with the tier of the previewed look.
- Scene entities: `anim_graph: Some((graph:, state:, params:, root_motion:))`; the PC
  scene player plays them carried by root motion, the editor's views pose them in place
  (`asset_pipeline::anim_graph::GraphPlayback`). Not in TSCN or the exported code yet (a
  console game drives its graph itself; TSCN can carry it at its next bump).

### 8. References

- `models/strider` (generated by `cargo xtask gen-assets`): a biped whose walk and run
  move the hips with a **surge** while each planted foot stays put (legs solved per key
  by the same law of cosines), so root motion has something to prove; idle and a hop;
  IK legs; compressed clips. `assets/anim/strider.anim.ron`: a blend space by speed and
  a hop.
- The showcase's two robots play `assets/anim/robot.anim.ron` (clip states started in
  their scene clip's state) and draw the same poses as before, every frame
  (`anim_showcase.rs`); the N64 showcase golden stayed pixel-exact.

## Measurements

- **Determinism:** the `anim graph` step's pose hash is `0x5dd683cb` on the PC, in ares
  (VR4300) and in Azahar (ARM11): the graph's f32 math gives the same quantized poses on
  every target.
- **Root motion** (`model inspect models/strider --anim`): the surging walk's planted foot
  slides 2.9 cm a frame at a constant speed, 0.0 carried by its track (raw clips; 0.2 cm
  with the compressed ones); the run 4.3 → 0.0.
- **Compression** (0.5° / 2 mm bounds): strider clips 42 304 → 5 672 bytes (13 %), its
  TMDL 53 912 → 17 296; robot clips 20 928 → 1 930 (9 %), windmill 3 696 → 184 (5 %);
  every frame within the bounds, posed vertices within 1 cm of the raw clip's.
- **Foot IK on the N64** (`TRINO_IK_COST:`, 16 strider draws one a frame with both legs
  solved, less 16 without, per draw): 525–650 µs over four runs, both tiers
  (`N64_COST.us_per_ik_draw` = 550; batched draws measured 125–300 µs, the RSP
  overlapping). On the 3DS (Azahar) 33–41 µs. Hence off on the N64 by default.
- **N64 frame cost:** the `test n64` scene table moves within code-layout noise against
  the main branch (hub 12 046 → 12 280 µs, showcase-river 25 779 → 26 105, the others
  ±1 %; one build each, not a claim).
- **N64 RAM** (the base console's self-test, no Expansion Pak): the animation code adds
  ~18 KB of text after its trims (panic-free blob readers, `optimize(size)` on its big
  cold functions); the showcase's N64 build stopped linking the PC's scene table (40 KB of
  dead rodata); the strider lives in its own asset group (`[groups.strider]`) loaded by the
  animation steps, which every host runs last (in the middle of the run its load and unload
  fragmented the heap past the perf scenes' 45 KB copies); the robot's and the windmill's
  compressed clips cut boot assets 570 784 → 548 280 bytes. The run's heap peak is
  2 194 944 of 2 326 176 bytes (131 KB free; the gate is 128 KB).

## Deviations from the issue

- **The reference migration is the showcase's robots, not castle64's player.** castle64's
  player is five code-posed rigid parts with no clips: migrating it means new art and
  re-blessed goldens on every target. The robots already had clips, and moving them to a
  graph with every golden unchanged is the stronger proof that the graph replaces game
  code without changing what it draws. The strider is the reference for blend spaces,
  events, root motion and IK.
- **No layers, no 2D blend spaces.** A pose is an `AnimRef` of at most three clips; a
  layered upper body or a triangle-blended 2D space needs more, and a backend change. 1D
  blend spaces take a `wrap` period for an angle.
- **`anim_graph` is not in TSCN or `scene export`** (§7).
- **IK aligns the foot's height, not its slope:** the foot keeps the clip's orientation.

## Consequences

- Character logic is data an agent writes and checks headless; the editor shows it live.
- A new per-clip datum goes in the model bake and is copied into `TANG`; the runtime never
  needs the mesh.
- A future TSCN bump can carry `anim_graph`; a future backend change could lift the
  three-clip limit (layers).
