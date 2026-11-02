# ADR 0039: Animation layers — joint masks, additive layers, 2D blend spaces, inertialized transitions, tiered per platform

Status: accepted (October 2026, GitHub #112, the follow-up of #65 / ADR 0034). Implemented in
`trino-core` (`anim::pose`: `AnimPose`, `PoseLayer`, `PoseSample`, `JointMask`, `LayerMode`,
`SampleRole`, `AnimLayers`, `FoldReport`, `FootSlope`, `inertial_decay`; `AnimRef::pose`;
`Renderer::{push_pose, pose_joints}`; `Mesh::{with_poses, layered, joint_parent}` and the
layered `pose_world`; `anim::graph` `TANG` v2; `anim::ik::align_feet`, `FootIk::slope`;
`Caps::anim_layers`; `ui::UiCore`), every backend (`trino-render-wgpu` for the PC and the Web,
`trino-platform-n64`, `trino-platform-3ds`: a pose table cleared at `begin_frame`),
`trino-scene` (`anim`: `LayerDef`, `Blend2D`, `TransitionMode`, `check_with`, `to_tang_with`,
`triangulate`, `fold_report`, per-layer `simulate`), `trino-asset-pipeline` (`joint_names`,
`anim_graph::model_joints`, graphs riding their model's groups through `UiBlob::follows`),
xtask (`anim check` fold notes per tier, `model inspect` joint names), the editor (Graph mode:
layers, the mask on the skeleton, the 2D blend space, the transition mode, the layered preview
and its fold notes), castle64's self-test (`anim layers`) and the strider's new upper-body clips
with `assets/anim/strider_layers.anim.ron`.

## Context

#65 made character logic data, but a pose was an `AnimRef` of at most three clips for the whole
body: no upper body shooting over the legs' locomotion, no additive breath or recoil, no 2D
blend space (direction × speed, aim yaw × pitch), a small pop when a blend space faded into
another (the lightest of four clips dropped), no inertialized transitions, feet that only knew
the ground's height. Solid Eleven and Plan & Party hand-code exactly these. The N64 is the
ceiling: its base console has #65's pose and nothing to spare.

## Decisions

### 1. The pose travels in a per-frame table the renderer owns

`AnimPose` holds up to 12 weighted clip samples (`MAX_POSE_SAMPLES`) in up to 4 layers
(`MAX_POSE_LAYERS`, the base included), each layer with a `JointMask(u32)` (one bit per joint;
`MAX_JOINTS` is 32), a mode (`Override` / `Additive`), a weight and an inertialization decay;
plus the `IkPose` and a `FootSlope`. 268 bytes (host): far too big for `ModelParams`.

- `Renderer::push_pose(&AnimPose) -> AnimRef` (a default method) folds the pose to the
  backend's `Caps::anim_layers`, appends it to a table **cleared at `begin_frame`**
  (append-only within a frame: the GPU and N64 batches pose at flush time, so a slot must stay
  valid until `end_frame`) and returns an `AnimRef` whose new `pose: u16` is the slot (0 =
  none) and whose three clips are the pose folded to the base N64's tier.
- The backend attaches its table where it builds a mesh to draw (`Mesh::with_poses`): the
  slice rides the mesh's `Skeleton`, and `Mesh::pose_world` reads the slot. So the CPU T&L,
  `rsp_plan`, the N64's RSP skeleton path (`draw_rigid`), the 3DS and the PC GPU T&L (its
  batch keys compare the `AnimRef`, slot included) pose the same joints with no other change:
  `tessellate_joints::<false, false>` is byte-identical (0x3f7c).
- A pose that fits an `AnimRef` (one layer, ≤ 3 clips, no offsets, flat feet) gets no slot and
  draws on #65's path bit for bit; a backend without a table (or a stale slot) draws the folded
  clips: a defined pose, never garbage.
- `ModelParams` grows 4 bytes (measured against main: `AnimRef` 40 → 44, `ModelParams` 132 →
  136; an `EntityStore<128>` +512 B). Animated
  draws are still never memoized (`memo.rs::only_plain_draws_are_memoized`).
- `Renderer::pose_joints(model, &AnimRef, &mut [Mat34; 32])` returns the joints the backend
  poses (its table included): the self-test hashes them across targets.

Rejected: growing `AnimRef` to N clips (every `ModelParams` pays it); a `draw_model_posed` with
game-posed joints (games never hold the mesh, and every backend needs a new draw path).

### 2. Folding is at run time, by tier, the same everywhere

`Caps::anim_layers` (`AnimLayers { layers, clips }`): the base N64 **1 layer / 3 clips** (#65's
pose), the Expansion Pak and the 3DS **2 / 6**, the New 3DS, the PC and the Web **4 / 12**
(`with_tier` carries it). `AnimPose::fold` is one deterministic rule: layers past the count go;
over the clip budget (or on the base N64's tier, which draws through an `AnimRef`) an
inertialized transition becomes a crossfade from its frozen source; then the lightest samples
go (their weight times their layer's), each layer renormalized; an emptied top layer is dropped;
the base tier drops the slopes.

**Deviation from the issue ("the bake folds"):** one ROM serves both N64 tiers and one `.3dsx`
both 3DS models, so a bake-time fold would need tier twins of every graph and would make the
base N64's blob differ from the PC's (no common hash). Folding at run time, like `anim_ik`, lets
the editor's N64 look and the PC's N64 profile show the console's pose, and `cargo xtask anim
check --platform n64` reports what each tier folds (`fold_report`: layers dropped, inertia turned
into crossfades, "up to N clips fold to M"), so the fallback is written down, never silent.

### 3. `TANG` v2

v1's reserved header bytes become `u16 layers, u32 layers offset, u16 base states, u16 base
transitions`: a v1 blob is a v2 blob with no layers, read by the same `parse`, and a graph with
no v2 feature still bakes to v1 byte for byte (`v1_graphs_bake_the_bytes_they_always_did`, main's
strider blob as the fixture `crates/core/tests/fixtures/strider_v1.tang`; `ANIM_GRAPH_HASH` is
unchanged on every target). Layers' states and transitions follow the base's in the same tables;
a 28-byte layer record names its mask, mode, weight (a constant times a float parameter clamped
to 0..1), its ranges and start. Bit 15 of a transition's `to` marks it inertialized. State kind 2
is `Blend2D`: x and y parameters, 12-byte points, baked triangles (3 × u8). `parse` validates
every range, index and float (a fuzz test flips every byte of a v2 blob).

Runtime: each layer is its own machine (clocks, crossfade, `Finished`, its transitions in its
range, evaluated after the base's; a trigger a lower machine consumed is gone). Only the base
moves the root and gives the feet; every layer fires its events (scaled by its weight).
`AnimGraphInstance::pose()` is the whole pose; `anim()` stays #65's code exactly for a graph
without layers or inertia, and folds `pose()` otherwise.

### 4. 2D blend spaces

`Blend2D(x:, y:, points: [(x, y, clip)..])`: the bake triangulates the points (Delaunay: every
triangle whose circumcircle holds no other point; triangles crossing an earlier one, from
cocircular points like a square's corners, are dropped; duplicates and collinear sets are
`check` errors). At run time the barycentric weights of the triangle holding the value, else of
the nearest point on the triangles' edges: weights sum to 1 and are continuous across edges
(tested both ways at x = ±1e-4). Clips are phase-synced like `Blend1D` (the cycle is the
weighted durations). With a crossfade, a 2D space into another holds 6 clips: kept whole where
the tier has them (no more dropped lightest clip on the PC), folded elsewhere.

### 5. Inertialized transitions

`(.., blend: 0.15, mode: Inertial)`: the destination plays alone from its first frame; when it
fires, the instance snapshots the layer's pose (its three heaviest clips, `From`) and the
destination's first pose (`To`), and the pose adds `(From − To) × decay` joint by joint
(translation and scale added, rotation `nlerp(identity, From·To⁻¹, decay)` premultiplied), with
`decay = 1 − smootherstep(t / blend)`. The source's clock stops: no events, no root motion from
it.

**Deviation:** the graph has no mesh and the renderer keeps nothing between frames, so the
offsets are not computed once and stored: the frozen `From`/`To` clips are sampled every frame
(up to six samples more). It is still the inertialized look — the destination at full weight from
frame one — and it bounds a blend space into a blend space at the destination's clips plus six.
A tier without room plays it as a crossfade from the frozen source.

### 6. Additive layers and foot slopes

An additive sample adds its clip's motion relative to **the clip's first frame** (`Pose ·
first⁻¹` for rotations, differences for translation and scale), by the layer's weight: a breath
or a recoil authored as full poses works. Foot alignment (#65's IK set only the height):
`FootIk::slope(caps, matrix, feet, normal_at)` gives a `FootSlope` (each normal's x, z in 1/127,
model space) for `AnimPose::with_slope`; after the IK, `anim::ik::align_feet` turns each foot
about its ankle onto the normal, at most 30°. It rides the layered pose, not `IkPose` (which is
in every `AnimRef`); the base N64's fold drops it (its IK is off by default anyway).

### 7. Graphs ride their model's groups on the consoles

`UiBlob::follows`: a graph is routed with its model's asset groups by the N64 and 3DS stagers
and the memory plan (the PC and the Web keep it resident). The strider's two graphs (3 153 and
3 628 bytes) now load with `[groups.strider]` instead of sitting in boot RAM; games read them with
`Platform::group_blob` after loading the group (`GraphBlob` keeps the boot path). The bake writes
feet and root tracks only for clips the base plays (layers' clips need neither).

### 8. Found on the way: one copy of the UI code per `Ui` size

Measuring the base N64's RAM: every `Ui<N>` size monomorphized the whole UI (layout solver,
draw, navigation): the self-test's `Ui<16>` and the N64 app's `Ui<128>` were 110 KB and 60 KB of
code, and code is RAM there. `Ui<N>` is now a newtype over `UiCore<[SlotData; N]>` (a slot's
data — slot, rect, the two memo cells — in one array, the struct's last field) that derefs to
`UiCore<[SlotData]>`, where every method lives: one copy for every size, the public API the same
(`slots()` returns a `Slots` view that indexes and iterates like the slice did). Every UI golden,
the access goldens and Solid Eleven's 1 716-state menu crawl are unchanged. The ROM's text fell
68 KB: the N64 build is **38 KB smaller than main** with all of #112 in it. A game with several
sizes (Solid Eleven: 8, 16, 80) saves more.

## Measurements

- **Determinism:** the `anim layers` script's whole-pose hash `0xf037ddb5` and its folds to the
  base N64's tier `0x8802b891` and to two layers of six `0x46e58fc2` (computed by every target
  for every tier) are equal on the PC, in ares (both tiers) and in Azahar (New and old); the
  joints the renderer poses for the busiest pose, per tier: `0x09bff162` (1/3), `0xc98e71dd`
  (2/6), `0xd107cb25` (all). The RSP skeleton path and the CPU T&L draw the layered strider with 0
  of 96 probes apart (ares); the PC GPU T&L within the oracle (`layered_draws`).
- **Base N64 RAM** (no Expansion Pak, `test n64 --expansion-pak off`): the heap's peak with the
  strider group loaded leaves 174 648 B free (main: 143 336; the gate 131 072). Text 1 862 836 →
  1 824 588 B (#112's code +30 KB after `optimize(size)` on its console paths, the UI −68 KB);
  stack peak 53 128 → 52 848 B.
- **Cost:** a layered draw of the busiest pose over its fold, one a frame (`TRINO_LAYER_COST:`):
  N64 Expansion Pak 1 370–1 607 µs (two layouts; 2 layers, 6 clips against 3), Azahar 254 µs
  (New 3DS, the whole pose) and 158 µs (old 3DS, 2 / 6; Azahar's timing is not the
  hardware's), the PC 3–12 µs. Hence `N64_COST.us_per_layer_sample = 45` (an extra clip sample
  per joint) and the base N64 at #65's pose.
- **Scenes:** the `test n64` table's work stages (T&L, sort, submit, game) are within noise of
  main; only the frame-start wait moves with the vblank phase; every gate holds.

## Deviations from the issue

- **Folding at run time, not at bake (§2)**, with the report `anim check` prints per tier.
- **Inertialization samples frozen clips (§5)** instead of storing per-joint offsets.
  *Resolved by #136 (§ Follow-ups 1): the renderer stores them once.*
- **The mask picker clicks joints on a 2D skeleton** (the rest pose seen from the front) beside
  the per-joint checkboxes, not in the 3D preview. *#136 adds the picker in the preview.*
- **Masks name joints; no subtree syntax** (`"torso"` is the torso only). *#136: `"torso/*"`.*
- **The reference is the strider shooting while it runs**, not Solid Eleven: the strider is the
  #65 reference with IK chains and a group of its own, and its clips cost the base N64 3.3 KB.
  *#136: Solid Eleven's kicks are graph layers, read as data (§ Follow-ups 5).*

## Follow-ups (issue #136, November 2026)

1. **Inertial offsets stored once — a renderer-side cache keyed by content.** `render3d::PoseCache`
   (`render3d/pose_cache.rs`) keeps, across frames, what a layered pose computes the same way
   every frame: a layer's inertial offsets (`From − To` per joint: translation and scale
   differences, the rotation `From·To⁻¹`) and an additive clip's first frame. The key is the
   content — the mesh's clip data, the layer and the exact bits of its frozen `From`/`To`
   samples (they never change during a transition: §5), or the clip — never an instance, so
   nothing in `AnimRef`/`ModelParams` grows, games add no call, and two characters in one
   transition share an entry. Each backend owns one (`PoseCache<Vec<PoseCacheEntry>>`, 8
   entries of 1.3 KB, allocated by the first layered pose that needs it: never on the base N64,
   whose fold removes layers and inertia), fills it on its serial draw path before any mesh
   reads it (`prepare_growing`, the draw entry points: shadows, the RSP path, the PC's CPU
   workers and GPU batches then read it through `Mesh::with_pose_cache`), keeps it through
   `begin_frame` (entries unused for two frames are replaced; a full cache skips, never
   thrashes) and clears it when a mesh is uploaded or freed. A miss — no cache, full, a path
   that never prepared — samples as before with the same expressions, so the pose is **bit for
   bit** the same either way (`the_pose_cache_stores_offsets_once_and_poses_bit_for_bit`, the
   fuzz target, the pinned `ANIM_LAYERS_JOINTS` on every target). With the offsets stored, an
   inertialized transition costs the destination's clips plus a per-joint add: cheaper than a
   crossfade, as #112 intended. On the Expansion Pak and the 3DS (2 layers / 6 clips) a busy
   pose's inertia still folds into a crossfade first (§2): the cache pays where the tier keeps
   inertia (New 3DS, PC, Web).
2. **Cheaper layered sampling.** A clip's compressed track offsets (`anmc::joint_offsets`, a walk
   over every track) depend on the clip, not the time: `Located::of` decodes them once per clip
   in a pose (a `To` sample is usually a `Pose` clip, a folded `From` the clip it froze, an
   additive reference its own clip). The additive reference frame is the cache's second kind.
   Measured: the Expansion Pak's layered draw (8 paddings, ares) 1 536 µs mean (1 377–1 769)
   against main's 1 568 (1 419–1 695): within noise there (its fold leaves no inertia or
   additive layer to cache); the New 3DS 254 → 233 µs, the old 3DS 158 → 131 µs (Azahar).
   A first `prepare` that located every sample before looking its keys up cost 1 930 µs:
   a cache in front of work must cost only its lookup on a hit.
3. **Mask subtrees:** `"torso/*"` is a joint and every joint under it, resolved at check and
   bake against the model's joints and parents (`trino_scene::anim::Joint`,
   `trino_asset_pipeline::joints`: TMDL order, parents first — a test holds it to the baked
   skeleton's). Masks still bake to a `u32` (no `TANG` change: `strider_layers` now writes
   `["torso/*"]` and bakes the same bytes). Any other `*` is a check error naming the form.
4. **The mask picker in the 3D preview:** the Animation tab's Graph mode draws a dot on each
   joint where the renderer posed it (`Renderer::pose_joints`, the previewed look's tier),
   lit when the selected layer's mask has it; a click (not an orbit) toggles the nearest within
   12 px, Shift+click its subtree. `trino_scene::anim::mask_toggle` keeps the entries as
   written (a joint turned off inside `"x/*"` splits it into the joints it held). The 2D
   skeleton stays as the other view.
5. **Games on layers — read as data.** A graph instance is too heavy for a crowd on the N64: one
   `AnimGraphInstance::update` of a three-machine graph costs ~400 µs there (the runtime decodes
   the `TANG` blob byte by byte, ~1 000 reads an update) and `pose()` ~110 µs; Solid Eleven's 22
   players took the football template's match from 30.4 to 39.9 ms (gate 33.3) and its stack past
   the 60 KB gate (an instance is 0.8–0.9 KB). So both reference games read their layers **as
   data** and lay them over their own clip picking:
   - **Solid Eleven:** `assets/anim/player.anim.ron` (baked against the PC-only QA rig
     `models/se_player_rig`, embedded as `src/render/player.tang` through the new `cargo xtask
     anim export`, a test holding it to its bake) has a `strike` layer (hips, legs, torso, head)
     and an `arms` layer, a state per kick. `render::graph::KickLayers` reads their masks (rig
     joints mapped to the game's) and kicks once a match (`AnimGraph::layer_named`,
     `state_clip`, both new); `pick_clip` keeps the gait under a kick and `AnimState::pose_with`
     lays the kick over it — the arms only while slow, so a shot at a sprint keeps the run's arm
     swing. Everything else (gait, falls, get-ups) is main's picking, unchanged.
   - **Plan & Party** (private repo, its own branch): carrying is a `carry` layer (`"torso/*"`)
     over the walk, read the same way and pushed with `Renderer::push_pose`.
   A faster runtime (states, transitions and conditions decoded once into tables) is the
   follow-up that would let a crowd run real instances.
6. **Found on the way:** `pose_world` read clip samples out of `Option` payloads; after #136's
   changes moved the inlining, LLVM hoisted `f0 as f32 + k` of an absent blend above its test
   and an unset `k` (a denormal) trapped the VR4300 in castle64's game ("Generic floating point
   exception" in ares, every padding). Hot pose paths now keep `ClipSample::NONE` and flags
   (`pose_cache::Samples`), never `Option<ClipSample>`.
7. **Not done here:** the cost on real hardware (#123) — ares and Azahar only.

## Consequences

- An upper body over locomotion, additive layers and 2D spaces are text an agent writes, the
  editor edits and every target plays alike; the base N64 plays a documented fold.
- A new backend attaches its pose table where it builds a mesh to draw (`with_poses`) and clears
  it at `begin_frame`; a stereo replay keeps it (both eyes draw the same slots).
- Inertial offsets stored once (cheaper) need state across frames on the renderer side: a later
  change if the cost matters.
