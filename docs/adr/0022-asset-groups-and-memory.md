# ADR 0022: Asset groups, bundles, compression and a memory budget

Status: accepted (October 2026, GitHub #56). Every phase of the issue's roadmap is
implemented: the RAM report, strict mode and the heap watermark (§6), groups, `TBND`
bundles and compression in every bake (§1–§3), loading in time slices on every backend
(§4), arenas and load/unload soaks (§7), the CLI, the editor and the docs (§8), and
portal cells that name groups, the base of scene streaming (§5).

## Context

Every asset of a project loaded at boot and stayed for the whole run:

- **N64:** `xtask/src/n64.rs` packed the DFS with plain `mkdfs`; the ELF was compressed
  (`n64elfcompress -c 1`) but the assets were not. Everything in `index.tsv` was read
  into the 4 MB heap at boot, so a game grew until it failed at an arbitrary
  allocation, with nothing saying which asset or how far over.
- **3DS:** the same per-file `index.tsv` from the RomFS, loose files, no compression.
  The worker core of #54 (ADR 0020 §5) existed but nothing in the runtime used it.
- **PC / Web:** `index.toml` and the Web's pack were all-or-nothing as well; the Web
  downloaded every asset of a game before its first frame.
- **Measurement:** no tool said what a project's assets take of a console's RAM. The
  frame report counted draws and triangles, not memory; nothing failed a build that
  would not fit the N64's 4 MB.

## Decisions

### 1. Groups in the manifest; `boot` is today's behaviour

An asset names the groups it is in (`groups = ["level1", "level2"]`); without the key it
is in `boot`. Groups are declared as `[groups.<name>]` (lowercase letters, digits, `_`,
`-`), with an optional per-platform `compression`. `boot` loads at boot and stays;
every other group loads when the game asks and frees its memory when it says so.
Handles never change: `models/boar` is the same id whatever group holds it. An asset
in `boot` and in another group is resident from boot and stays out of the other
bundle. A project without groups bakes byte for byte as before: the regression gate
is every existing self-test and golden, unchanged.

### 2. `TBND`: one bundle per group and platform

`trino_core::bundle`: a 24-byte header (`"TBND"`, version, entry count, group id,
payload offset and size), 24-byte entries (kind, codec, flags, tier, id, offset,
stored bytes, raw bytes, FNV-1a hash), a string area and a payload area. An entry is
**inline** (its bytes in the payload, `Stored` or `Lz4`) or **external** (a file beside
the bundle: the N64's DFS files and sounds that stream). Entries with the same raw
bytes share one payload (the bake dedups by hash). `Bundle::parse` needs only the
table to start, and checks every offset against the sizes it declares: a corrupt
bundle is an error, never an out-of-bounds read. The index names each bundle on one
line (`<group id> \t group \t groups/<name>.tbnd`); the Web pack (TPAK v3) names groups
with a `Group` entry and keeps each bundle beside the pack (`trino.level1.tbnd`).

### 3. Compression per platform, measured on the pinned toolchains

| Platform | Codec | Default (non-boot) | Decoded by |
|---|---|---|---|
| N64 | libdragon `mkasset -c 1/2/3` (LZ4, aPLib, Shrinkler) | level 1 | `asset_load` / `sprite_load` while reading |
| 3DS | LZ4 blocks in the bundle | `lz4` | the worker core (ADR 0020 §5) |
| PC / Web | LZ4 blocks in the bundle | `lz4` | the main thread, a time slice per frame |

- The pinned libdragon decodes all three levels in `asset_load` and streams 1 and 2 in
  `asset_fopen`; levels 2 and 3 need `asset_init_compression`, which the index asks for
  with a `compression` line before any asset.
- `mksprite` already compresses its output at level 1: the bake passes `--compress` at
  the sprite's group level and `mkasset` leaves `.sprite` files alone (compressing
  them twice makes `sprite_load` read garbage). `wav64` files stream from the
  cartridge and are left as written.
- The encoder for LZ4 is the engine's (`trino_asset_pipeline::lz4`), the decoder
  `trino_core::lz4` (`no_std`, bounds-checked, round-tripped against the encoder).
- **Boot compresses on the consoles only:** `[groups.boot] compression = { n64 = 1,
  3ds = "lz4" }` compresses the N64's boot files in place (the same `mkasset` path) and
  packs the 3DS's textures, meshes, fonts and blobs into `groups/boot.tbnd`, decoded on
  the worker at boot (`<id> \t boot \t groups/boot.tbnd`); sounds stay loose. The PC's
  and the Web's boot files stay as they were: there is no ROM to fit.

castle64 (the engine's assets) and Solid Eleven (the football template) ship with boot
compressed:

| Project | Target | Before | After | Saved |
|---|---|---|---|---|
| castle64 | N64 ROM / DFS | 1 589 248 / 474 365 B | 1 441 792 / 326 296 B | 9.3 % / 31.2 % |
| castle64 | 3DS `.3dsx` / RomFS | 1 299 988 / 494 863 B | 1 141 400 / 342 837 B | 12.2 % / 30.7 % |
| Solid Eleven | N64 ROM / DFS | 1 638 400 / 522 988 B | 1 523 712 / 411 759 B | 7.0 % / 21.3 % |
| Solid Eleven | 3DS `.3dsx` / RomFS | 1 474 356 / 531 202 B | 1 343 416 / 411 921 B | 8.9 % / 22.5 % |

The ROM and the app shrink by about what their filesystem does: their code was already
compressed, so the percentage is smaller. The audio (VADPCM, DSP-ADPCM, ADR 0021) was
already compressed and stays as it was. What boot takes in RAM does not change (it is
the decoded size), nor does the heap's peak: castle64 boots with 534 112 B of assets
and peaks at 2 144 584 B in ares with the Expansion Pak, as before, and Azahar logs the
same boot heap within a few hundred bytes on both models.

### 4. Loading never blocks a frame

`trino_core::Platform` gains `load_group` (queues, then reports `Loading { progress }`,
`Ready` or `Failed`), `unload_group`, `group_state`, `group_blob` (a blob of any loaded
group, borrowed until it unloads) and `memory` (`MemoryReport`: heap used/peak/total,
resident bytes by category). `trino_core::group::GroupTable` is the bookkeeping every
backend shares: requests, refcounts (an asset two loaded groups hold is uploaded once
and freed with the last of them), progress. Drawing an asset whose group is not loaded
draws nothing and counts `FrameStats::not_loaded`.

Each frame, before the game's update, the host pumps the loader for
`Caps::load_budget_us` (4 ms N64, 6 ms 3DS, 8 ms PC and Web) of main-thread time, so a
loading screen animates:

- **N64** (`N64Assets::pump`): one entry per step through `asset_load`/`sprite_load`;
  the locator strings come from a `FrameArena` reset per entry (§7), not the heap.
- **3DS** (`N3dsAssets::pump`): the main thread reads an entry's stored bytes and hands
  them to the worker core, up to four ahead, uploading results in order as they come
  back and never waiting on the worker; the budget counts only main-thread time (or a
  job that ran inline when the worker's ring was full). GPU memory is freed two frames
  after its last draw (the PICA200 may still read it).
- **PC** (`trino_runner_pc::GroupLoader`) and **Web** (`platform-web/src/groups.rs`):
  the same loop; the Web fetches a bundle once, lazily, when the game first asks (#47),
  and keeps the bytes so a level loaded again needs no network.

### 5. Scenes name the groups they need; cells stream theirs

Scene version 6 adds `requires` (default `["boot"]`) and `groups` on a `cell(…)`
component; versions 1–5 migrate. Codegen emits `REQUIRES` and `CELL_GROUPS` constants
only when a scene uses them, so earlier scenes generate the same code.
`trino_core::CellGroups<G>` is the base of BACKLOG P2 #11 (scene streaming): it keeps the
groups of the cells in view and of their portal neighbours loaded (a room starts
loading a doorway before it shows), and unloads a group once no such cell has named
it for `linger` frames, so a camera standing in a doorway does not reload a room every
frame. Fixed size (at most 64 groups), no allocation; `Resolved::cell_streamer` builds
one from a resolved scene. Verified on the PC backend's real groups
(`xtask/tests/cell_streaming.rs`).

### 6. A memory budget, enforced

`trino_asset_pipeline::memory` measures what each asset takes once loaded on each
console tier (textures as the platform stores them, audio's resident bytes, the rest
their baked bytes) and splits the RAM into the engine's reserve (2.2 MB on the N64,
6.25 MB on the 3DS, measured from castle64's `TRINO_MEM` boot line in ares and Azahar,
plus a twentieth), the game's own state (`[memory] game = "256K"`) and the rest for
assets. One plan feeds four consumers:

- **strict mode** (`TRINO_STRICT=1` on a console profile): the PC runner checks every
  load against the console plan and panics naming the groups, the overrun and the
  biggest assets;
- **`cargo xtask assets report`**: per tier, each group's assets, RAM and baked size,
  and every scenario (boot, boot + each group, each scene's groups) against the budget;
- **`cargo xtask assets check`**: fails when a scene draws an asset none of its groups
  loads, names an undeclared group, or holds more than a tier's budget at once;
- **the editor's Memory tab** (§8).

On the consoles the self-tests log `TRINO_MEM` (RAM, heap, the engine's share and
boot's) and `TRINO_HEAP` lines; the N64 self-test fails when the heap's peak leaves less
than 128 KB of libdragon's heap free (`check_heap`, like `check_stack`). The frame
report's `TRINO_FRAME` line ends in `heap=used/peak` on every backend.

### 7. Arenas and soaks

`trino_core::mem`: `FrameArena<N>` (storage in the value, reset at a frame's start),
`Arena` (over memory the caller owns) and `LevelArena` (reset when its group unloads).
Allocations borrow the arena, so the borrow checker proves nothing outlives a reset;
overflow returns `None` and `high_water` tells a test how close it came. The self-test
step `asset groups` loads both test groups behind a loading bar, draws from them,
unloads them and then soaks load/unload cycles (1 000 frames on the N64, 600 on the
3DS, 10 000 on the PC); the heap must come back within 8 KB of its baseline on ares (both
Expansion Pak settings) and Azahar (both models), within 64 KB on the PC and the Web,
where wgpu's resource trackers grow in steps whether anything loads or not. The Web runs
its soak over animation frames (wgpu's GL fences need event-loop turns), in all three
browsers. Measured drift: under 3 KB on the N64, under 300 B on the 3DS.

### 8. Tools

- **CLI:** `cargo xtask assets report [--platform …] [--tier …] [--json]`,
  `cargo xtask assets check [--json]` and `cargo xtask assets group <asset> <group>…`
  (edits the manifest in place, declaring new groups; `boot` alone removes the key).
  Exit codes follow strict mode, so CI and agents fail on what would not fit.
- **Editor:** the Assets panel's group chips (filter; drop a tile on a chip to move it;
  a context-menu submenu to toggle groups; "new group"), the scene settings' `Requires`
  picker with the scene's problems and per-tier budget, a cell's `Groups` picker in the
  inspector, and a Memory tab with each tier's RAM split, the open scene's bar and every
  group's bytes. Every edit writes the same manifest lines `assets group` writes, and an
  agent's edit to the manifest shows at once.

## Consequences

- ROM space drops by a fifth to a third of the DFS/RomFS on both shipped projects; boot
  costs the decode (level 1 / LZ4: fast; the 3DS's runs on the worker).
- A game must call `load_group` before drawing a grouped asset, and the frame stats and
  `assets check` say when it did not. Games that never name a group are unchanged.
- Live reload on the PC re-uploads changed `boot` assets; a grouped asset's new bytes
  are in its rebaked bundle and show the next time its group loads (the editor's play
  mode loads every asset, so it reloads them all).
- Follow-ups: per-cell streaming in castle64's interior; a RAM line in the Game view's
  budget overlay; a worker for PC/Web decoding; zstd if a project needs smaller bundles
  than LZ4 gives.

## Addendum (2026-10-29, GitHub #110): the base N64's fragmentation

A group's load and unload in the middle of the base N64's self-test left 137 KB free and
no block for a 45 KB RSP mesh copy, and one big frame kept the batch buffers' growth for
good. What changed:

- **The RSP copies' own region** (`N64Renderer::reserve_rsp_arena`, or `trino.toml`
  `[n64] rsp_arena_kb` through `rom:/rsp_arena` and `N64Host::boot`): one uncached
  block taken at boot, before any mesh registers, carved by `trino_core::mem::Blocks`
  (best fit, neighbors merge, ≤ 96 free ranges; a free past that gives its block up and
  counts it). A copy that doesn't fit takes the heap as before; frees still wait in the
  renderer's grave. Opt-in: an unused arena is heap nothing else gets, and only the
  game's self-test knows its peak (`TRINO_HEAP … rsp=`). The engine's ROM reserves
  376 KB (its copies peak at ~374 KB alive; the anim steps' group spills to the heap).
- **A copy is staged one segment at a time**: `rsp::bake` built the whole copy in a heap
  `Vec` beside its uncached block, so a 45 KB copy needed two 45 KB blocks at once.
- **Batch buffers shrink by themselves**: `N64Renderer::trim` (`mem::TrimWindow`) keeps
  each buffer group's peak over 120 frames; `trim_to` shrinks one that holds more than
  twice that and 16 KB beyond (`mem::shrink_to`), in place on newlib.
- `TRINO_HEAP` lines add `rsp=live/peak arena=used/size` (and `batch=` before the micro
  scenes).
