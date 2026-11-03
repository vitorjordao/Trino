---
name: netplay
description: Local multiplayer, input recordings/replays and netplay in Trino — `Platform::players` and `local_seats` (seats, the N64's four ports, PC/Web pads, `Join`, per-player bindings, whose seat this machine plays online), `TREC` recordings (`TRINO_RECORD`/`TRINO_REPLAY`, `TRINO_RECORD_FROM`, v2 snapshots, `?record`/`?replay=`, the consoles' debug channel, `cargo xtask replay dump|run|cut|check`, `replay run --device` on real consoles, the `replay` self-test gate, the replayable crash tail), `Game::state_hash`/`save_state`/`load_state`/`portable_state`, `trino-net` (ggrs rollback over UDP, WebRTC and a simulated network; spectators; 2–4 peers; the lobby with its fingerprint check, NAT hole punching and relay; `NetInput` keys/pointer; `TRINO_NET`, `cargo xtask net local|web|3ds|lobby`), the consoles' delay lockstep (`trino_core::lockstep`) over the 3DS's local wireless (`uds`, two Azahar instances), the editor's Play here record/replay/scrubber and Net tab, and the determinism traps. Use for "two players", "split screen input", "record/replay a session", "a replay diverged", "desync", "online", "rollback", "netcode", "lobby", "NAT", "relay", "spectator", "lockstep", "local wireless", "uds", "crash replay".
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
- **Whose seat is this machine's** (issue #117): `platform.local_seats()`, a bit per
  seat, never empty. Locally every connected seat; online only this machine's
  (`HostedPlayers::publish_net`); a spectator the seat it follows. A shared camera or
  HUD follows `trino_core::first_seat(mask)`. **Presentation only:** keep it out of
  `state_hash` and out of anything the sim reads, or the peers desync by design.
  Solid Eleven's HUD guides read `Session::viewer` (left out of its hash).

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
    `update_with` and checks all 761 hashes, on PC, ares (both tiers), Azahar (New and
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
- **Recordings from a snapshot (`TREC` v2, issue #117):** the game's `save_state`
  follows the header, and `Session::restore` / `Replay::restore` load it before `start`.
  Make one with `TRINO_RECORD_FROM=<frame>` (with `TRINO_RECORD`) or `cargo xtask replay
  cut <file> --from N --out <cut>`. A recording without a snapshot is still v1, byte for
  byte (the console re-recordings compare against the fixtures).
- **A snapshot leaves its process only for a game with `Game::portable_state()`**
  (default `false`): raw snapshots (`save_raw`) hold `&'static` borrows of blobs and of
  the binary, and ASLR moves both between runs. castle64 opts in. Its `load_state` keeps
  its own `HostLocal` fields (blob borrows, captions, services, the player's settings,
  the screen, the save policy), so a replay never writes progress. Solid Eleven doesn't
  opt in (its UI and kick tables hold static borrows): its tails stay v1 and `replay cut`
  refuses it. A new game opting in lists its process-local fields the same way and gets
  a test like castle64's `a_snapshot_brings_the_sim_and_keeps_the_hosts_state`.
- **Crash reports:** the runner writes `crash-*.trec` beside the report. For a portable
  game the tail starts from the oldest snapshot still in its last-minute ring (snapshots
  at the start and every 30 s, a hash every second), so `TRINO_REPLAY=crash-….trec`
  replays to the crash on that build. Otherwise it is input only (a v1 tail: `dump` it).
- **On real consoles:** `cargo xtask replay run <file> --platform n64|3ds --device
  sc64|everdrive|network [--ip IP] [--no-build]` builds the image with `replay.trec` and
  deploys it. The loader's log is followed under `script` (a pty) until the
  `TRINO_REPLAY` verdict. Never pipe a loader: its unflushed stdio arrives in 4 KB
  blocks. Snapshot recordings are refused: they are the PC's.

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
- **What crosses the wire** is a `NetInput` (32 bytes): a `NetPad` (the 16 buttons, both
  sticks at 1/127, the triggers at 1/255, the pad flag), then the keyboard, the pointer,
  its buttons and the wheel, but only when the sender opts in (`[net] keys = true`,
  `pointer = true`, `TRINO_NET_KEYS/POINTER`, `?net_keys/?net_pointer`).
  - Receivers decode whatever arrives, so a mismatch there can't desync.
  - The pointer crosses as a fraction of the sender's screen and arrives in `[net]
    pointer_space` units (default 320×240) on every peer: a game reads it in those units
    online.
  - The local seat sees its own input quantized the same way. `NetPad::from_bytes`
    reads a hostile −128 axis as −127 (a fuzz find).
- **Spectators and 2–4 peers:** `NetSession::with_spectators` on seat 0,
  `NetSession::spectate` on the watcher (confirmed frames only: no rollbacks; `follow`
  picks its view). Lobby `WATCH <code> <players>` seats them after the players, before
  the room fills. A late spectator isn't supported (ggrs fixes them at build time).
  - PC: `TRINO_NET=watch:<code>@<lobby>` or `watch:<host>` (`TRINO_NET_FOLLOW=2`).
  - Web: `?net_watch&net_follow=2`.
  - `cargo xtask net local --watch`.
- **NAT traversal (`udp::meet`):** peers are seats (`PeerAddr::Room`).
  - Direct once both hear each other: `0xA9` datagrams carry an "I hear you" flag, and
    hellos go every 100 ms to the public and LAN candidates.
  - Through the lobby's relay otherwise (`0xAA`).
  - `TRINO_NET_ROUTE=auto|direct|relay`. The `TRINO_NET:` line ends with `routes=[2:direct]`.
  - Only symmetric against port-restricted or symmetric NATs needs the relay
    (`natsim::NatKind::punches_with`, proved by `crates/net/tests/nat.rs` over a
    simulated Internet).
  - `cargo xtask net local --lobby --route relay --watch` runs the real binary through
    the real relay. `docker/lobby` packages the server; nothing public runs it.
- **The settings fingerprint:** `NetConfig::fingerprint()` (mode, window, delay,
  players, fps, desync interval, pointer space) rides on every `JOIN`, and the lobby
  refuses a mismatch with both values. ggrs checks none of it and would just desync.
- **Modes:** `rollback` (default: 8-frame window, 2 frames of input delay) or `delay`
  (lockstep, prediction window 0; no snapshots needed). Set in `trino.toml [net]`, `cargo
  xtask net config set net.mode=delay`, or the editor's Net tab.
- **Tests that drive sessions fast** (`crates/net/tests/soak.rs`) must sleep a
  millisecond when every peer stalls: ggrs re-sends on 200 ms wall-clock timers, so a
  loop faster than real time over a lossy `SimNet` otherwise never recovers. For the
  same reason a hang guard is wall-clock time without progress (20 s), never a loop
  count: lockstep under loss passed locally and timed out on CI's Linux and Windows
  runners with a tick budget.
- **Re-simulated frames are silent.** `session::run_requests` follows ggrs's frame
  numbers (`SaveGameState`/`LoadGameState` carry them; `FrameClock`) and plays audio
  only for a frame past the newest one already played. A game that triggers effects
  from `update` gets them once. Never key "new frame" on a batch's last advance: ggrs
  sends rollbacks with no new frame (`Load 1498, Advance, Save 1499, Advance, Save 1500`),
  and that rule played frame 1500's sound twice (`three_players_stay_in_step` failed
  about 1 run in 10 on `main`, PR #127).

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

Azahar doesn't time the 3DS's clocks: the old and the New 3DS read alike there, so the
3DS plays local wireless in delay-based lockstep (one update a frame).

## Consoles: lockstep over local wireless (issue #117)

- **`trino_core::lockstep::Lockstep`** is `no_std`, does no I/O and allocates nothing.
  The 3DS can't use `trino-net`/ggrs (`build-std=core,alloc`). Per host frame:
  1. `receive` what arrived;
  2. `poll(now)` (silent peers leave);
  3. `add_local(input)`;
  4. `advance()` → `Some(players)`: play that frame and `note_hash`;
  5. send `packet(to)` to every peer.

  Packets repeat every unacknowledged input, so losses cost nothing. A differing hash →
  `LockState::Desynced`. Solid Eleven in lockstep reaches ggrs's rollback hash
  (`crates/net/tests/lockstep.rs`).
- **3DS `uds`:** `trino_platform_3ds::uds` over the shim's `trino_uds_*`.
  - A RomFS `/uds` file (`host 2 delay=4 hash_at=600`, or `join 2`) makes
    `N3dsHost::run_named` meet and play the whole run in lockstep. `/uds_script` (a
    `.trec`) plays this console's seat of a recording.
  - Stage them with `TRINO_UDS="host 2 …" TRINO_UDS_SCRIPT=<file> cargo xtask
    build|run 3ds`.
  - On hardware: two units, `run 3ds --device network` each (not run yet).
- **Two Azahar instances:** `cargo xtask net 3ds [--docker <image>]` (Linux) builds both
  apps and runs `tools/uds-test/two-azahar.sh` under Xvfb + openbox + xdotool.
  - The script creates an unlisted room on A.
  - It renames B's console (a room refuses a second "AZAHAR", the name every fresh
    Azahar has) and regenerates its MAC.
  - It direct-connects B, then checks that both print the same `TRINO_NET_HASH`.
  - castle64 at frame 600: `0x94fa2260aa5f5154`, equal to the PC's lockstep
    (`replay_gate.rs::castle64_in_lockstep_ends_where_the_two_azahar_consoles_did`).
  - Keyboard shortcuts reach Azahar only with a window manager running: without one,
    use clicks.
- **The `lockstep` self-test step** (PC, Azahar, the browsers; not the N64, which has no
  network and a thin heap) ends on `castle64::selftest::LOCKSTEP_HASH`. `test 3ds|web`
  hold the line to it. A change to `Lockstep`'s rules or to the step's mixing re-pins
  that hash, on purpose.
