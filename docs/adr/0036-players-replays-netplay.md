# ADR 0036 — Local players, input recordings and replays, rollback netplay

- Status: accepted (2026-10-25)
- Issue: GitHub #67
- Supersedes: nothing. `Game::update_with`'s `input` stays player 1's; everything here
  adds to the contract.

## Context

Trino's sim is deterministic: castle64's bot and Solid Eleven's scripted match play the
same on the PC, the N64 and the 3DS. Three things were missing on top of that:

- **Local multiplayer.** `InputState` described one controller. Solid Eleven took two
  `humans` sides but nothing gave it a second pad. The N64 has four ports; the PC and the
  Web have many pads.
- **Recordings.** A bug report couldn't attach a run, a failing session couldn't be
  replayed, and long goldens needed their own bots.
- **Online play.** None.

## Decision

### 1. Players (`trino_core::players`)

- **`PlayersInput`** holds four `InputState`s and a `connected` bit mask.
  `Platform::players()` returns it, and player 0 is the `input` every host passes to
  `update_with`. So the hot-reload ABI is unchanged (`GAME_API_VERSION` only moved for
  the state exports, §2).
- **Devices and seats.** A host reports each controller as an `InputDevice` (keyboard,
  gamepad or console port), and **`Seats`** folds them into players:
  - **N64:** port k is player k. `trino_joypad_poll` returns the mask of ports holding a
    controller, so an empty port reads disconnected.
  - **3DS:** one player.
  - **PC and Web, `PlayerSetup::max` 1 (the default):** every device is player 0, merged
    exactly as before (`InputState::merge` reproduces `PcInput::merge_pad`).
  - **PC and Web, `max` above 1:** the keyboard and the first pad sit on player 0, and
    every further pad takes the lowest seat no pad holds. A pad that leaves keeps its
    seat.
- **`HostedPlayers`** is the platform's copy. Hosts publish into it before the update,
  and a game's requests (`set_player_setup`, `assign_player`, `clear_players`) apply to
  the copy at once, then reach the input's `Seats` after the frame.
- **`Join`** is the assignment screen. A device's A or Start takes the lowest free seat,
  B leaves it, and Start begins the game. It draws the seats with each pad family's glyph
  (ADR 0028).
- **Rebinding per player.** `Platform::rebind_player` and `binding_label_for` (`rebind`
  is player 0's). The PC keeps four `Bindings`, and each device plays with its seat's.
  `<save>.input` starts with `# trino-input 2`. Player 1's lines are unchanged and the
  others are prefixed `P2 `…`P4 `, so older readers skip them and older files still
  parse.
- **The `players` self-test step.**
  - Each target's controllers read idle.
  - The seat rules run on synthetic devices.
  - The harnesses hold the mask: `1111` in ares (ares v148 plugs a pad into every port,
    so "port absent" is covered by host tests on the decoder, `pad::poll_ports`) and
    `0001` in Azahar.
  - The ROM logs `fcr31=` and the 3DS app `fpscr=` beside the step that follows.

### 2. Recordings and replays (`trino_core::replay`, `TREC` v1)

- **The format.**
  - **Header:** game id (`asset_id` of its name), screen, players per frame, hash
    interval, start frame (0 from boot), frame count, start state hash.
  - **Per frame:** a head byte, then only what changed against the previous frame, as raw
    bits: `dt`, the connected mask, and each changed player's buttons, sticks, triggers,
    pointer, wheel, keys and pad flag. Then the state hash when due.
  - **Size:** an idle frame is one byte. castle64's 731-frame bot run is 15 KB with a
    hash on every frame.
  - **No quantization.** A recording feeds the game exactly what the host fed it,
    including the PC's wall-clock `dt` bits.
- **`Game::state_hash`** (default `None`) hashes what the next update depends on. Use
  `StateHasher` (FNV-1a 64 over little-endian fixed-width values). It does not implement
  `core::hash::Hasher`, because `#[derive(Hash)]` hashes integer slices as raw memory in
  the CPU's byte order, and the N64 is big-endian.
- **`Session`** is the host-side state machine (live, record or replay). The PC's
  `InputSession`, the Web's `WebSession` and the consoles' `ChannelSession` wrap it:
  - **PC:** `TRINO_RECORD=<file>` / `TRINO_REPLAY=<file>` (+ `TRINO_REPLAY_UNTIL`,
    `TRINO_SCREENSHOT`).
  - **Web:** `?record` / `?replay=<url>`.
  - **Consoles:** `replay.trec` / `record` files in the DFS or RomFS (`cargo xtask replay
    run --platform n64|3ds`). A recording streams as `TRINO_REC:<hex>` lines over
    ISViewer or the 3DS debug channel (`LineSink`).
  - Each one prints `TRINO_REPLAY: ok frames=… checked=… hash=… on <platform>` or
    `TRINO_REPLAY: diverged at frame N (expected …, got …) on <platform>`. Frame N means
    the state after N frames: 0 is a different start.
- **`Neutral`** is a platform wrapper that forwards the clock, log, blobs, groups and
  players, and nothing the machine keeps (save, accessibility, quality, language). A
  portable recording and a netplay session run against it, so an SD card's old save
  can't change the sim.
- **The cross-platform gate** is a self-test step, with no new CI job:
  - **castle64:** `assets/shared/replays/castle64_green.trec`, recorded on the PC by the
    real app (`TRINO_BOT=green TRINO_RECORD=…`, variable `dt`), loaded from its own asset
    group by the `replay` step on PC, ares (both tiers), Azahar (New and old) and every
    browser run.
  - **Every target re-records** what it fed and hashed through its debug channel. `test
    n64|3ds|web` reassembles the lines and holds them byte for byte to the PC's file, so
    the console's recorder and hashes are checked too.
  - **Solid Eleven:** the template's console self-tests and the Web self-test replay
    `two_players.trec` (menus to a "P1 V P2" quick match, then 20 s of both pads, 2.8 KB)
    through `Neutral`.
- **Crash reports** write the last 3 600 frames (`replay_host::recent_input`) beside the
  report as `crash-*.trec`. This is a tail, so it doesn't replay from boot; it documents
  what the player did (#100).
- **Tools:**
  - `cargo xtask replay dump [--json]`;
  - `replay run --platform pc|n64|3ds|web [--until-frame N] [--shot x.png] [--window]`
    (the PC headless in-process, or the real window);
  - `replay check <files> [--platforms …]`.
- **Editor (Play here):**
  - Rec restarts the scene and records to `recordings/<scene>-<n>.trec`.
  - Replay plays one back with a scrubber (snapshots every 60 frames, seek = the nearest
    snapshot then re-simulation) and the hash status.
  - The split-input simulator gives 2–4 seats: P1 WASD + F G R T, P2 arrows + K L I O,
    P3/P4 as bots, idle pads or a copy of P1.
  - Hooks: `TRINO_EDITOR_REPLAY`, `TRINO_EDITOR_REPLAY_FRAME`, `TRINO_EDITOR_PLAY=record`,
    `TRINO_EDITOR_PLAYERS`.

### 3. Netplay (`trino-net`, ggrs 0.13)

- **The game-facing API is the existing contract:** `Platform::players`,
  `Game::save_state` / `load_state` / `state_size` (snapshots never leave the machine,
  so a plain-data game copies itself: `snapshot::save_raw`) and `state_hash` (the
  checksum the peers compare). A game that takes two local players plays online as is.
- **The whole session runs under rollback from the first frame.** Remote players sit in
  their seats, so the menus are shared, as on a couch. Each machine's local devices are
  its seat. The game runs on `Neutral` (§2), and every seat's input crosses as a
  `NetPad`:
  - 9 bytes: the console buttons, both sticks at 1/127, the triggers at 1/255;
  - no keyboard, pointer or wheel;
  - the local seat sees its own input after the same quantization.
- **`NetSession`** carries out ggrs's requests:
  - snapshots, checksummed with `state_hash`;
  - rollbacks;
  - advances with every seat's input published to the platform at a fixed `dt`;
  - re-simulated frames play no sound.
- **Modes and settings.** `NetConfig` sets the mode: `rollback`, or `delay` (lockstep
  with an input delay, ggrs's prediction window 0, for a sim too heavy to simulate
  again). It also sets the rollback window, input delay, players, desync interval and
  disconnect timeout. Sources: `trino.toml [net]` and `[players] max`, baked as the
  `net/config` blob (PC and Web), `TRINO_NET_*`, or `?net_*`.
- **Transports:**
  - **`SimNet`:** in-process latency, jitter and loss on a caller-advanced clock.
  - **`UdpSocket`:** ggrs datagrams tagged `0xA7`, lobby text lines on the same port.
  - **`Conditioned`:** a worse network on the wall clock, for two local instances.
  - **`RtcSocket` (wasm):** unordered, unreliable WebRTC data channels. The lower seat
    offers.
- **The lobby.** `lobby::Rooms` is transport-free, served by `cargo xtask net lobby`
  (UDP for the PC, a WebSocket for the Web's signalling). Peers meet by room code. A PC
  host can serve its own room (`TRINO_NET=host:<port>`, `join:<addr>`); otherwise
  `lobby:<code>@<addr>` on the PC, or `?net=<code>&lobby=ws://…` on the Web.
- **Desyncs.** Peers exchange state hashes every `desync_every` frames, and the first
  mismatch reports `Desynced { frame }`.
- **Determinism guard rails** (`trino_core::determinism`, feature `strict-determinism`):
  - a NaN or a subnormal reaching a state hash or `checked()` panics with its name;
  - the N64 runs with flush-to-zero (`FCR31.FS`, measured `0x01800e04`) and traps on
    invalid operations, and the 3DS runs ARM's RunFast (`FPSCR` FZ+DN, `0x83000013`),
    while the PC keeps gradual underflow;
  - a fixed-point sim (Solid Eleven, `physics::Fx`) never meets either.

### 4. The 3DS local-wireless evaluation (stretch)

- **Measured.** The football template's console self-tests log `TRINO_ROLLBACK:` (the
  hosts' `rollback_cost`): the snapshot size and the mean save, load and update times on
  the old and the New 3DS and the N64. That is the budget a session would spend; the
  numbers are in the PR and `docs/PLATFORMS.md`.
- **Not done.** The `uds` service (libctru) transport and delay-based lockstep between
  two 3DS units:
  - CI can't run two units, and Azahar's multiplayer needs a room server and two
    instances;
  - nothing here was run on hardware.
  - The core is ready for it: `NetConfig`'s delay mode, `NetPad`, `Neutral`, the replay
    hash checks. A follow-up issue holds the plan.
- **N64:** local players and replays only (no network hardware).
- **The base N64's memory.** This work adds about 45 KB of code to every N64 image, and
  on the 4 MB console that comes out of the heap. The self-test then found no block for
  its micro scenes' triangle batch. The fix recovered more than the code took:
  - the draw memo has 768 slots without the Expansion Pak (1 024 with it, −78 KB; the
    gated scenes draw ≤ 70 a frame and their frames didn't move);
  - the batch is sized once and exactly (`N64Renderer::reserve_batch`);
  - the N64 bakes a 4×4 stand-in for the 3DS format photos it never draws (−12 KB).

  The base tier's heap peak now leaves 213 KB against the 128 KB gate (main: 159 KB).
  `TRINO_HEAP … top=` logs the top free block (#110).

## Consequences

- **Test coverage:**
  - every target replays the PC's recording and re-records it byte for byte;
  - Solid Eleven passes ggrs's sync test (a 7-frame rollback on every frame of the
    two-player script) and a mutation (the clock left out of the snapshot) fails it;
  - the soak runs 10 000 frames for each latency 0/50/100/200 ms × loss 0/5/10 % with
    every frame's hash compared and the final state held to a one-machine run;
  - two PC instances (`net local`) and two browser pages (`net web`) end on the same
    hash at frame 760: `0xaeadc42f3bbe07f9`, the same on the PC and in wasm.
- **Limits:**
  - **Netplay drops the keyboard and the pointer.** Games that need them online must
    map them to buttons.
  - **Platform state the sim reads breaks portability.** Graphics options that pause the
    game, a save, the language: record from a point where none apply, or use `Neutral`.
  - **The online camera and HUD follow player 1 on every machine**, because the game
    doesn't know which seat is local. A `Platform::local_seats` is a follow-up.
- **ggrs times retries on the wall clock** (200 ms). Fast simulated tests let a little
  real time pass when every peer waits.
