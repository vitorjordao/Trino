# ADR 0020: Console hardware tiers — the Expansion Pak and the New 3DS

Status: accepted (October 2026, GitHub #54).

## Context

Each target console exists in two variants that run the same software:

| Console | Base | Expanded |
|---|---|---|
| Nintendo 64 | 4 MB RDRAM | **Expansion Pak**: 8 MB (same CPU, RSP, RDP) |
| Nintendo 3DS | old 3DS / 2DS: 2 ARM11 cores at 268 MHz, 64 MB for the app | **New 3DS / New 2DS XL**: 4 cores, 804 MHz with the speedup, an L2 cache |

Trino treated each console as one machine. Games could not use the RAM of an Expansion
Pak or the clock and cores of a New 3DS, and nothing in the engine said which one a game
was running on. Commercial games handled this per title (Majora's Mask required the Pak,
Perfect Dark locked its campaign without it; Xenoblade 3D required a New 3DS, others
ran faster). Issue #54 asks for the engine to make that a decision of data, detected at
boot, previewed on the PC and tested in CI on both variants of each console.

## Decisions

### 1. Two tiers, one `Caps` table each (`trino-core`)

- `HwTier { Base, Expanded }`. The N64 with the Pak and the New 3DS are `Expanded`;
  everything else is `Base`. The **PC and the Web are always `Base`**: their hardware
  spread is a different problem (resolution, GPU class), left to #55.
- `Caps` gains what a tier changes: `ram_bytes`, `cpu_cores`, `cpu_mhz`, `hw_tier`, and
  two advisory content scales, `crowd_percent` / `particle_percent`
  (`crowd_scale()`/`particle_scale()`, `crowd(n)`/`particles(n)`).
- `Caps::N64_EXPANDED` is `Caps::N64` with 8 MB. Nothing else: the Pak adds memory, not
  CPU time, so every per-frame budget stays the base console's (the ROM's emulated frame
  times with and without it are within noise, §7).
- `Caps::N3DS_NEW` is `Caps::N3DS` with 2 cores (the app gets cores 0 and 2), 804 MHz and
  crowds and particles ×2. The texture, triangle and light tables stay the PICA200's:
  the GPU is the same chip.
- `Caps::with_tier(tier)` maps a console's table to its other tier (keyed by the
  console's CPU clock, so it also works on the PC renderer's copy, whose picture size
  differs); `Caps::base()` is the base table.
- `Platform::hw_tier()`: a trait method with a default (`Base`), so no
  `GAME_API_VERSION` bump. The renderer's `caps()` is that tier's table. A game branches
  on it at update time; content picks its tier variants as data (§3).

**The design ceiling stays the base N64.** A tier may hold more content (RAM) or more of
it (cores, scales); it may never be the only machine a feature works on.

### 2. Detection at boot, printed first

- **N64:** the shim's `trino_memory_size()` returns libdragon's `get_memory_size()`;
  8 MB or more is the Pak (`platform-n64/src/hw.rs`). `TRINO_HW n64 ram=8M tier=expanded`.
- **3DS:** `trino_hw_probe(want_speedup)` asks `APT_CheckNew3DS`, and on a New 3DS turns
  `osSetSpeedupEnable(true)` on (804 MHz and the L2 cache). It returns the console, the
  speedup and the app's memory region in one word, decoded in `platform-3ds/src/hw.rs`.
  `TRINO_HW 3ds new=1 speedup=1 app_ram=96M tier=expanded`.
- **The speedup is on by default** and a project opts out in `trino.toml`
  (`[3ds] speedup = false`; `--speedup off` for one command). The bake writes a
  `no_speedup` file into the RomFS, which the boot checks. A New 3DS without the speedup
  is the **base tier**: a game that opted out for identical timing on every model also
  gets identical content.

### 3. Content: tier keys that fall back to their console

- `n64_expanded` and `n3ds_new` are keys wherever a platform is: scene and prefab
  variants, LOD starts (`At(d)`/`Never`), particle caps, manifest model `platforms`.
- **Resolution order:** `n64_expanded` → `n64` → the base entity (`n3ds_new` → `n3ds` →
  base), the same rule as the Web's fallback to the PC (`Target::fallback`). A project
  with no tier keys resolves exactly as before.
- Formats: scenes v5, prefabs v4, particle definitions v3 (migrations from every earlier
  version; nothing to convert, the new keys are optional). TSCN, TPFX and TMDL are
  unchanged: the tiers are resolved before the bake.
- A model named only for a tier (`platforms = ["n64_expanded"]`) is in the console's
  ROM / app and loaded on that tier only. Naming the console covers its tier.

### 4. One ROM, one app: tier twins in the index

The alternative — one ROM per tier — doubles every build, test and release artifact,
and a player would have to know which to pick. Instead the console bake resolves each
scene and particle effect for both tiers (`asset_pipeline::tier_twins`):

- identical bytes → one blob, one index line, no tier column (what every existing
  asset gets: their ROMs and apps are byte-for-byte what they were);
- different → `<id>.blob` and `<id>.x.blob`, with index lines ending in `\tbase` and
  `\texpanded`.

The runtime reads the optional 4th column and keeps the lines for its tier
(`trino_core::asset::loads_on`); a tier-only model gets the `expanded` column. Both
loaders run before the game sees an asset, so a game asks for `scenes/level` and gets its
hardware's.

### 5. The 3DS's second core: a job API (`trino_platform_3ds::worker`)

#53 (audio decoding) and #56 (AI) need a core besides the main thread. The shim starts
one worker thread: **core 2** on a New 3DS (all of it is the app's), else **core 1**
after `APT_SetAppCpuTimeLimit(30)` (the system keeps 70% of it). Jobs are
`extern "C" fn(u32)` plus their word, queued in a 32-slot ring (`LightLock`,
`LightSemaphore`, a one-shot `LightEvent` when the pending count reaches 0);
`submit` runs a job inline when there is no worker or the ring is full, so a game that
never checks `worker::core()` behaves the same everywhere. Jobs compute into memory the
main thread reads after `wait()`; the renderer, audio and platform stay on the main
thread. The N64 has no second CPU, and the PC's T&L already has its own pool.

### 6. Tools, PC simulation and the editor

- `cargo xtask test n64 --expansion-pak on|off|both` boots ares with
  `--setting Nintendo64/ExpansionPak=…`; `test 3ds --new3ds on|off|both` forces Azahar's
  `[System] is_new_3ds` for the session (the `--accurate-timing` machinery, generalized
  into `n3ds::Setting`). Unset, `test` boots on — both emulators' defaults, so every
  golden and perf ceiling measured before still applies; `run`/`watch` take on|off and
  otherwise leave the emulator's setting alone. `--device` refuses both switches (real
  hardware is the console it is).
- Every boot checks three lines against the switch: `TRINO_HW`; the engine self-test's
  `hw tier` step (`TRINO_STEP:hw tier <tier> … data=<tier>`: the probe scene's card, the
  tier sparks' cap and the tier-only model were all that tier's — the bake, the index
  column and the loader end to end); and on the 3DS `TRINO_STEP:worker core=2|1`.
  **CI boots both tiers of both consoles.**
- `scene budget --platform n64_expanded|n3ds_new` (and `scene export`) estimate a tier:
  its variants, its models' RAM against `ram_bytes`, its cores and scales. PC strict
  mode panics when the loaded models pass the tier's `ram_bytes` (the rest of a RAM
  budget — textures, audio, the heap's watermark — is #56's).
- The PC: `TRINO_SIM=n64x|3dsnew` (`SimProfile::parse_tier`) keeps the console's look
  and gives the renderer and the platform the tier's `Caps` and `hw_tier`. The PC's one
  bake serves every simulated tier (its data is the base tier's; the PC suite checks the
  `hw tier` step sees exactly that).
- The editor's Game view has "N64 + Expansion Pak" and "New 3DS"; Inspector › Platforms
  shows each tier's column collapsed under its console; the budget HUD sets the base and
  expanded estimates side by side with the RAM line.

### 7. Measured

N64, castle64's perf scenes in ares, one ROM, emulated µs per frame (with → without the
Pak; ceilings unchanged):

| Scene | With the Pak | Without |
|---|---|---|
| hub | 12 332 | 12 211 |
| green-hills | 7 449 | 7 690 |
| lava-land | 8 076 | 8 262 |
| sky-tower | 7 366 | 7 379 |
| throne | 18 573 | 18 438 |
| showcase-river | 25 000 | 25 470 |

Within the layout noise both ways (AGENTS.md › N64 timings move with the code layout):
the Pak adds RAM, not speed, which is why `Caps::N64_EXPANDED` changes nothing else.

3DS, Azahar as a New 3DS with `--accurate-timing`, main-thread CPU µs per frame
(`cpu_us`: the frame minus the GPU and vsync waits; every frame is vsync-paced at
16 718 µs):

| Scene | Speedup on (`expanded`) | Speedup off (`base`) |
|---|---|---|
| castle64 hub | 1 703 | 1 703 |
| castle64 green-hills | 1 311 | 1 311 |
| castle64 lava-land | 1 461 | 1 462 |
| castle64 sky-tower | 1 244 | 1 245 |
| castle64 throne | 2 262 | 2 262 |
| Solid Eleven match (300 frames) | 5 424 | 5 356 |

Equal both ways: Azahar runs the ARM11 at one clock whatever `osSetSpeedupEnable`
asks, so the speedup's gain is a real-hardware number (`--device`). Both runs log the
tier they booted (`TRINO_HW … speedup=1 … tier=expanded` / `speedup=0 … tier=base`) and
load that tier's data, and Solid Eleven's match hash is the same (`d51e91bf`).

What Azahar does model is the second core. The `worker` step's 8 jobs take
**6.0 ms on core 2** of a New 3DS and **19.9 ms on core 1** of an old 3DS: 3.3×,
the 30% of core 1 that `APT_SetAppCpuTimeLimit(30)` gives the app.

## Consequences

- One ROM and one `.3dsx` per game, as before; content opts into the tiers as data.
- A tier variant costs the console's ROM / RomFS a second copy of that asset only.
- CI's console jobs boot twice (ares and Azahar each run their self-test per tier).
- The Expansion Pak and New 3DS numbers come from emulators; ares does not time the RDP
  and Azahar ignores the speedup's clock (§7). Real hardware (`--device`) stays the
  reference (`docs/HARDWARE.md`).
- Azahar's log and config are per user: two `test 3ds` runs at once on one machine read
  each other's lines (AGENTS.md › Pitfalls).

## Alternatives rejected

- **A ROM / app per tier.** Doubles builds, artifacts and CI; players pick wrong.
- **Runtime-only scaling (`crowd_scale()` and nothing else).** Cannot add a model the
  base console has no RAM for, or swap a mesh for a richer one; the scales stay as the
  advisory half of the design.
- **Tiers as `SimProfile`s.** A profile is a look (resolution, filtering, color depth),
  and a tier changes none of it; the PC keeps the two apart (`profile` + `hw_tier`).
- **Speedup off by default.** The New 3DS's clock is the most common reason to target
  it; a project that wants identical timing opts out in one line.
