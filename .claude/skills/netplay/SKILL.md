---
name: netplay
description: Local multiplayer, input recordings/replays and rollback netplay in Trino — `Platform::players` (seats, the N64's four ports, PC/Web pads, `Join`, per-player bindings), `TREC` recordings (`TRINO_RECORD`/`TRINO_REPLAY`, `?record`/`?replay=`, the consoles' debug channel, `cargo xtask replay dump|run|check`, the `replay` self-test gate, the crash report's input tail), `Game::state_hash`/`save_state`/`load_state`, `trino-net` (ggrs rollback over UDP, WebRTC and a simulated network, the lobby, `TRINO_NET`, `cargo xtask net`), the editor's Play here record/replay/scrubber and Net tab, and the determinism traps. Use for "two players", "split screen input", "record/replay a session", "a replay diverged", "desync", "online", "rollback", "netcode", "lobby", "lockstep".
---

# netplay: players, recordings, online play

ADR: `docs/adr/0036-players-replays-netplay.md`. Per platform: `docs/PLATFORMS.md` ›
Players, recordings and netplay.

## Local players

- A game reads every player from `platform.players()` (a `PlayersInput`: four
  `InputState`s and a `connected` mask). Player 0 is the `input` the update already gets,
  so a single-player game changes nothing.
- **Seats per target:**
  - **N64:** ports 0..3 are players 0..3. ares v148 plugs a pad into every port, so the
    `players` step expects `connected=1111`.
  - **3DS:** one player (`0001`).
  - **PC and Web:** every device on player 0 until the game calls
    `platform.set_player_setup(PlayerSetup::up_to(n))`, or `trino.toml [players] max`
    (the runner reads the baked `net/config`). From then on, the keyboard and the first
    pad are player 0 and every further pad takes the lowest seat without a pad.
- **An assignment screen** is `trino_core::Join`: `Join::begin(platform, max)` (it
  unseats keyboards and pads), then `join.update(platform)` each frame (A/Start joins,
  B leaves, a seated Start → `JoinEvent::Start`), and `join.draw(platform, r, font,
  area)`.
- **Rebinding per player:** `platform.rebind_player(p, button, device)` and
  `binding_label_for`. The PC's `<save>.input` starts `# trino-input 2`, and player
  1's lines read as before.
- **A host** (a new backend, a test platform) holds a `HostedPlayers`, returns it from
  `hosted_players(_mut)` and, each frame:
  1. hands the game's requests to its `Seats`
     (`while let Some(r) = hosted.take_request() { seats.apply(r) }`);
  2. polls its devices (`Input::poll_players`);
  3. `hosted.publish(&seats)`;
  4. passes `players.players[0]` as the update's input.

  A platform without `hosted_players` reports `PlayersInput::NONE` (nobody).

## Recordings and replays

- **Make a game replayable:** `fn state_hash(&self) -> Option<u64>` over everything the
  next update depends on, through `StateHasher`, field by field:
  - positions, velocities, timers, RNG, scores;
  - the previous frame's buttons, because edges are pressed against them;
  - overlays that pause the game.

  Leave render-only state out. **Never `#[derive(Hash)]`**: integer arrays hash as raw
  memory, big-endian on the N64.
- **Record and replay:**
  - **PC:** `TRINO_RECORD=run.trec` (`TRINO_RECORD_HASH_EVERY`, `TRINO_RECORD_PLAYERS`),
    `TRINO_REPLAY=run.trec` (`TRINO_REPLAY_UNTIL=N`, `TRINO_SCREENSHOT`). castle64's bot
    can play a recording: `TRINO_BOT=green`.
  - **Web:** `?record` (the recording is in `__trino.info.recording`, base64) and
    `?replay=<url>`.
  - **Consoles:** `cargo xtask replay run <file> --platform n64|3ds [--until-frame N]
    [--shot x.png]` puts `replay.trec` in the image and boots the emulator.
- **Read one:** `cargo xtask replay dump <file> --json` gives every frame's buttons,
  sticks, keys and hash. `replay run --platform pc` replays it headless in-process
  (castle64, the showcase, Solid Eleven); `--window` uses the real app.
  `replay check <file> --platforms pc,n64,3ds,web` runs every target.
- **A verdict line** reads `TRINO_REPLAY: diverged at frame N`: the state after N frames
  first differed, and 0 means another start state (a fresh game must equal the
  recording's `start_hash`). To find the cause:
  1. `replay run --platform pc --until-frame N-1 --shot a.png` and `--until-frame N`;
  2. diff the frames;
  3. read frame N's input in `dump`;
  4. rebuild with `--features trino-core/strict-determinism` (a NaN or a subnormal in a
     hashed float panics with its name).
- **Portable recordings** run the game against `replay::Neutral(platform)`: no save, the
  default accessibility and quality, no language. castle64's Select opens the Graphics
  page only where `quality_info()` is `Some`, and Solid Eleven loads its settings from
  the save. Those paths make a recording platform-specific unless it starts outside
  them.
- **The CI gate is the self-test, not a job:**
  - The `replay` step (`castle64::selftest::replay`) loads
    `assets/shared/replays/castle64_green.trec` from its asset group, plays it through
    `update_with` and checks all 731 hashes, on PC, ares (both tiers), Azahar (New and
    old) and the browsers.
  - Each target also re-records what it fed and hashed (`TRINO_REC:` lines through
    `LineSink`). `test n64|3ds|web` reassembles them (`xtask::replay::Rerecord`) and
    compares them byte for byte with the file; the copy goes to
    `target/n64/replay-n64.trec` etc.
  - **After a castle64 sim change:**
    1. `TRINO_BOT=green TRINO_RECORD=assets/shared/replays/castle64_green.trec
       TRINO_RECORD_HASH_EVERY=1 TRINO_RECORD_PLAYERS=1 cargo run -p trino-app-pc`;
    2. update `the_pc_recording_replays…`'s frame count if the run's length changed.
- **Solid Eleven:** `selftest::replay_two_players` plays
  `examples/solid-eleven/assets/shared/replays/two_players.trec` on the template's
  N64/3DS hosts and in the Web self-test. After a sim change, run `cargo test -p
  solid-eleven write_two_player_recording -- --ignored`
  (`the_two_player_recording_is_fresh_and_replays` fails on a stale file).
- **Crash reports:** the runner writes `crash-*.trec` (the last 3 600 frames) beside the
  report. It is a tail, so it doesn't replay from boot; read it with `dump`.

## Netplay (`trino-net`)

- **A game needs:**
  - two-or-more-player support through `platform.players()`;
  - `state_size` / `save_state` / `load_state`. A plain-data game (no_std, no alloc)
    copies itself: `unsafe { trino_core::snapshot::save_raw(self, out) }`;
  - `state_hash`, the checksum the peers compare every `desync_every` frames.
- **Prove it before two machines do:** `trino_net::SyncTest` (ggrs's sync test) rolls
  back N frames every frame and compares the hashes (`crates/net/tests/synctest.rs` on
  Solid Eleven). Leave a field out of the snapshot on purpose and it must fail. The
  clock was the mutation that proved it.
- **Run it:**
  - **PC:** `TRINO_NET=host:<port>` (serves its own room), `join:<ip:port>`,
    `lobby:<code>@<ip:port>` (with `cargo xtask net lobby`). Add `TRINO_NET_SIM=
    latency=80,jitter=10,loss=3`, and `TRINO_NET_MODE|ROLLBACK|DELAY|PLAYERS`.
  - **Web:** `?net=<code>&lobby=ws://<ip>:7401` (`?players=`, `?stun=`, `?net_delay=`).
  - **Both, scripted and checked:** `cargo xtask net local --script
    examples/solid-eleven/assets/shared/replays/two_players.trec --frames 900` and
    `cargo xtask net web`. Each instance plays its seat of the recording and both print
    `TRINO_NET_HASH:760:<hash>`. PC and wasm agree: `0xaeadc42f3bbe07f9` today.
- **What crosses the wire** is a `NetPad`: the 16 buttons, both sticks (1/127), the
  triggers (1/255), the pad flag. The local seat sees its own input quantized the same
  way. The keyboard, the pointer and the wheel don't cross.
- **Modes:** `rollback` (default: 8-frame window, 2 frames of input delay) or `delay`
  (lockstep, prediction window 0; no snapshots needed). Set in `trino.toml [net]`, `cargo
  xtask net config set net.mode=delay`, or the editor's Net tab.
- **Tests that drive sessions fast** (`crates/net/tests/soak.rs`) must sleep a
  millisecond when every peer stalls: ggrs re-sends on 200 ms wall-clock timers, so a
  loop faster than real time over a lossy `SimNet` otherwise never recovers. For the
  same reason a hang guard is wall-clock time without progress (20 s), never a loop
  count: lockstep under loss passed locally and timed out on CI's Linux and Windows
  runners with a tick budget.
- **Re-simulated frames are silent** (`session::run_requests` mutes every advance but a
  batch's last). A game that triggers effects from `update` gets them once.

## Editor

- **Play here** (Game view bar):
  - `Rec` restarts the scene and records; `Save (n)` writes
    `recordings/<scene>-<n>.trec`.
  - `Replay` ▸ a file plays it back with a scrubber (snapshots every 60 frames; a seek
    pauses) and the hash status.
  - The players combo gives 2–4 seats: P1 WASD + F G R T, P2 arrows + K L I O, P3/P4
    as a bot, an idle pad or a copy of P1.
- **The Net tab** (View ▸ Go to Net view; not in the default dock: the 960 px layout
  is full) edits `[players]` / `[net]` and starts `net local` or `net lobby`. It shows
  the command it runs.
- **Hooks:**
  - `TRINO_EDITOR_REPLAY=<file>` (+ `TRINO_EDITOR_REPLAY_FRAME=N`: paused there);
  - `TRINO_EDITOR_PLAY=record`;
  - `TRINO_EDITOR_PLAYERS=2`;
  - `TRINO_EDITOR_TAB=net`.

## What a rollback costs on the consoles

The football template's console self-tests log `TRINO_ROLLBACK:` through
`N64Host`/`N3dsHost::rollback_cost` (snapshot, load, one update, Solid Eleven's match):

| | Snapshot | Save | Load | Update | 8-frame rollback |
|---|---|---|---|---|---|
| N64 (ares) | 34.6 KB | 3.4 ms | 3.3 ms | 4.2 ms | 64.6 ms |
| 3DS (Azahar) | 34.6 KB | 0.13 ms | 0.13 ms | 1.37 ms | 12.1 ms |

Azahar doesn't time the 3DS's clocks: the old and the New 3DS read alike there. A 3DS
local-wireless session would be delay-based (one update a frame). The `uds` transport
is not written (BACKLOG P2 #20).
