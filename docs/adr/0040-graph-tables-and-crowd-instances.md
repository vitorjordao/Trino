# ADR 0040: Animation graphs for crowds — decoded tables, sized instances, synced layers, one graph per rig family

Status: accepted (November 2026, GitHub #138, the follow-up of #136 / ADR 0039). Implemented in
`trino-core` (`anim::tables`: `GraphTables`, `GraphSource`, `GraphRef`, `ClipRec`, `Point`,
`Edge`, `CondNode`, `GraphTables::{decode_in, bytes_for, sizes}`; `anim::instance`:
`GraphInstance<L, I, E>`, `AnimGraphInstance = GraphInstance<4, 4, 1>`,
`GraphInstance::{seek_layer, layer_clock}`; `anim::graph`: `TANG` v3, `GraphLayer::sync`,
`AnimGraph::{for_model, bind_count, bind_model, version, clip_duration, clip_looping}`;
`EventTrack::from_records`, `RootTrack::{EMPTY, from_parts}`, `FeetTrack::{EMPTY,
from_parts}`; `perf::N64_COST.{us_per_graph_update, us_per_graph_pose}`,
`GRAPH_BLOB_FACTOR`, `CostModel::graph_us`), `trino-scene` (`anim`: `LayerDef::sync`,
`AnimGraphDef::models`, `BoundModel`, `to_tang_bound`, `check_binds`, `compare_tables`,
`tables_to_rust`, `runtime_bytes`), `trino-asset-pipeline` (binds resolved per model at the
bake), xtask (`anim export --rust`, `anim check`'s run-time sizes, `test n64`'s graph crowd
report and gate), castle64's self-test (`graph crowd`), Solid Eleven (`PLAYER_GRAPH`) and the
fuzz target `tang`.

## Context

#136 measured Solid Eleven's 22 players each driven by an `AnimGraphInstance` on the football
template in ares: an update of a three-machine graph ~400 µs, a pose ~110 µs, an instance
0.8–0.9 KB; the match went 30.4 → 39.9 ms (gate 33.3) and past the stack gate. The runtime
decoded the `TANG` blob byte by byte on every update (state, transition, condition and clip
records, ~1 000 bounds-checked reads, all `optimize(size)`). Plan & Party's carry port added
three more needs: a layer kept in step with the base's clock, one graph for rigs that differ
only by prop joints, and no blob parser in a console build that never has the blob.

## Decisions

### 1. A graph decodes once into native tables

`GraphTables<'a>` holds the graph as plain records: parameters, clips (`ClipRec`: id,
duration, looping, event / root / feet tracks as views), states (`GraphState`, whose
`points_at`/`tris_at` index the point and triangle tables), points, 2D triangles,
transitions (`Edge`: from, to, inertial, blend, the condition's first node), layers
(`GraphLayer`) and the conditions flattened in prefix order (`CondNode`: op, parameter,
children, `end` past its subtree, the value, and whether its subtree reads a trigger outside a
`Not`), plus the trigger bit mask.

- **Two ways to get them.** `GraphTables::decode_in(&graph, &arena)` at run time, in exactly
  `bytes_for` bytes of a `mem::Arena` (Solid Eleven's player graph: 2 156 B on the N64, 2 072
  on the host); `cargo xtask anim export <name> --rust` writes them as a `pub static`
  (`trino_scene::anim::tables_to_rust`). A build that plays only exported tables never calls
  `AnimGraph::parse` (#136's third request). Solid Eleven's kick layers are read from
  `PLAYER_GRAPH`, held to its bake by `solid_eleven_layers`.
- **No fixed-capacity buffer.** A first cut held the tables in a `GraphTablesBuf` sized by the
  format's limits: ~18 KB on the N64, too big for any console stack. The arena takes what the
  graph needs.
- **No float behind an `Option`.** A clip without root motion or feet keeps an empty track
  (`RootTrack::EMPTY`, frames 0) and `GraphClip`'s `Option`s are built from it on access: an
  uninitialized `None` payload is a possible denormal, which traps the VR4300 (ADR 0039's
  trap).

### 2. One playback reads either source, with the same arithmetic

`GraphSource` is what playback reads (states, clips, points, edges, conditions, layers);
`AnimGraph` implements it over the bytes, `GraphTables` over the records. The instance's
methods take `impl Into<GraphRef>`, an enum of the two: the update is **one copy of code**,
not generic. Generic playback (the first cut) was a monomorphized copy per source, per
instance size and per crate: the anim code went from 27 to 70 KB of N64 text, the
base-console heap's enemy (#112's lesson). With `GraphRef` it is ~41 KB including the
decoder, the static export's readers and the self-test's three variants.

Both sources run the same f32 operations in the same order, so blob and tables give identical
digests, poses, events, root motion and feet: `trino_scene::anim::compare_tables` drives both
(and a crowd instance) through long pseudo-random scripts for every graph the repo bakes on
every platform; the fuzz target does the same on mutated blobs; the self-test's three
variants must hash alike on every target.

### 3. Conditions short-circuit without changing what they consume

The blob's evaluator walks every child of an `And`/`Or`. The tables' evaluator stops an `And`
at its first false child (a false `And` consumes nothing), and an `Or` that already holds
evaluates only children that read a trigger outside a `Not` (each one that is on is consumed,
and the base's consumption is visible to the layers evaluated after it in the same update).
Leaves under an `And`/`Or` are read in its loop, not by a recursive call: the recursive
evaluator cost ~400 cycles an edge in ares, 12 edges a third of a crowd member's update.

### 4. Instances are sized by the game

`GraphInstance<L, I, E>`: `L` machines (the base and `L - 1` layers; a graph with more plays
the first ones), `I` inertia slots (layer `l` has one when `l < I`), `E` = 0 or 1 event
buffer. `AnimGraphInstance` is `<4, 4, 1>` and keeps every earlier behaviour (808 B on the
host, 800 on the N64, was 936 / ~800). A crowd member of a two-layer graph without inertia
or events is `GraphInstance<3, 0, 0>`: 224 B (220 on the N64).

- An inertialized transition of a layer without a slot **crossfades** over the same time
  (`no_inertia.png`); without an event buffer nothing is fired (`events()` is empty).
- The update runs over slices (`Play`, `View`): the wrappers are thin and generic only in
  their sizes.
- Digests of instances of any size agree while no inertia runs (missing slots hash as
  "none").
- `LayerPlay` keeps the fading-from state beside a flag, and `Inertia` an `on` flag, instead
  of `Option`s of floats (§1's trap).

### 5. Layers in step, one graph per rig family (`TANG` v3)

- **`sync: true`** on a layer: every update, its looping state lands at the fraction of a
  cycle the base's looping state is at (forward, its events firing over what it crossed).
  `GraphInstance::seek_layer(l, clock)` sets a layer's clock by hand (fires nothing).
- **`models: ["models/x_with_tray"]`**: the bake resolves each layer mask by joint name
  against each listed model (which must have every clip the graph plays, as long, and every
  joint a mask names) and writes them after the layer records. `AnimGraph::for_model(id)` is
  the graph with that model's masks (its tables decode with them). **A graph without
  layers serves its models too** (a fix after the first cut, which baked such a graph to v1
  and dropped the list, so `for_model` found only the graph's own model): its binds are the
  model ids alone, written at the layers offset.
- **v3 only when used:** the layer record's reserved byte becomes flags (bit 0 sync), and the
  binds follow the layer records (`u16 count, u16 0`, per model `u32 id` + a mask per layer;
  with no layers, at the layers offset and the ids alone).
  A graph that uses neither bakes the v2 (or v1) bytes it always did; v1 and v2 blobs parse
  and play as before.

### 6. Measured

`castle64::graph_crowd` (the `graph crowd` self-test step, last of `anim_steps`): 22
instances of Solid Eleven's player graph through a scripted match (speeds every 12 frames, a
kick per player every 40, the layers fading), 120 updates; update and pose timed apart, the
hash outside the timed spans. ares, Expansion Pak, µs per instance, mean (min–max) over 8
`TRINO_N64_TEXT_PAD` layouts:

| | update | pose | instance |
|---|---|---|---|
| `main` (blob, `AnimGraphInstance`) | 283.0 (273–328) | 105.5 (91–122) | 800 B |
| this ADR, blob (`AnimGraphInstance`) | 280.0 (269–339) | 111.0 (95–126) | 800 B |
| tables, `AnimGraphInstance` | 109.5 (107–118) | 49.7 (42–66) | 800 B |
| tables, `GraphInstance<3, 0, 0>` | 85.6 (83–87) | 45.6 (36–57) | 220 B |

Per crowd member: 388.5 → 131.2 µs (3.0×). 22 players: 8.5 → 2.9 ms a frame, which fits the
~2.9 ms #136 left the football template. A blob reader pays what it did on `main`. Azahar
(timing not cycle-exact), µs per instance: New 3DS blob 34.5 + 12.8 → crowd 13.2 + 7.5, old
3DS the same within 1%. The N64 ROM's text grew 14.3 KB (1 847 828 → 1 862 148 B); the
self-test's stack peak 53 440 → 52 776 B. The hash is `CROWD_HASH` (0x50a5e8bb) on `main`
and on this ADR's code, on the PC, in ares and in Azahar.

`N64_COST.us_per_graph_update` / `us_per_graph_pose` hold the crowd line; `cargo xtask test
n64` prints the three variants and fails when the crowd's update or pose passes twice those.

## Consequences

- A crowd of graph instances fits the N64 at a cost that `N64_COST.graph_us` estimates;
  Solid Eleven still reads its kick layers as data (its own clip picking is the base).
- The anim code grew ~14 KB of N64 text over `main` (the decoder, the tables' readers, the
  hot path compiled for speed, the self-test's variants); the base console's heap was
  checked (`test n64 --expansion-pak both`).
- `GraphSource` is public: tools and games read either source the same way (`layer_named`,
  `state_clip`, `param_named`).

## Not done

- Per-state candidate lists for transitions (the any-state edges must keep their order
  interleaved with the state's own; the flat loop over native edges was cheap enough).
- A runtime joint-name lookup: TMDL carries no joint names, so masks per model are resolved
  at the bake.
