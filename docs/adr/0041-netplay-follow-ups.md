# ADR 0041 — Netplay follow-ups: local seats, spectators, NAT traversal, console lockstep, snapshot recordings

- Status: accepted (2026-11-03)
- Issue: GitHub #117 (the follow-ups of #67)
- Amends: ADR 0036 (players, recordings, rollback netplay). Nothing there is
  withdrawn; every recording and session from before plays as it did.

## Context

ADR 0036 left these open:

- An online game couldn't tell which seat this machine plays, so the camera and HUD
  followed player 1 everywhere.
- Sessions were player against player. There were no spectators, and nothing tested
  more than three peers.
- The lobby only introduced peers. Two PCs behind different NATs had no fallback.
- `NetPad` carried the pad and nothing else.
- The 3DS had only a measured rollback budget, and no transport.
- A crash report's input tail couldn't replay, because it had no state to start from.
- Replays booted only in the emulators.

## Decision

### 1. `Platform::local_seats` (presentation only)

- `HostedPlayers` keeps a mask of the seats this machine plays. `Platform::local_seats()`
  is a default method, so there is no `GAME_API_VERSION` bump. It is never empty: seat 0
  when nothing is known.
  - Local hosts publish every connected seat (`publish`, `publish_players`).
  - A network session publishes its own seat (`HostedPlayers::publish_net`, from
    `NetSession`, `Lockstep` and the 3DS's `run_uds`).
  - A spectator publishes the seat it follows.
- `trino_core::first_seat(mask)` is the seat a shared view follows.
- **Rule:** peers differ here by design, so a game reads it into state it neither hashes
  nor feeds to its sim. Solid Eleven's HUD guides follow `Session::viewer`, which
  `state_hash` leaves out.

### 2. Spectators and four peers

- **Spectators.**
  - `NetSession::with_spectators(cfg, seat, peers, spectators, socket)` registers them on
    seat 0 (ggrs `PlayerType::Spectator`).
  - `NetSession::spectate(cfg, host, socket)` plays confirmed frames only, so it never
    rolls back. It catches up 2 frames at a time when more than 8 behind, and
    `follow(mask)` picks its view.
  - The lobby's `WATCH <code> <players>` seats spectators after the players. They must
    join before the room fills: ggrs adds spectators only when a session is built. A late
    joiner would need a snapshot (§6) sent over the network, which is future work.
- **Four peers:** a 4-player soak and a 4-player-plus-spectator NAT session are now tested.
- **Hosts:**
  - PC: `TRINO_NET=watch:<code>@<lobby>` or `watch:<host>`, with `TRINO_NET_FOLLOW`.
  - Web: `?net_watch` and `?net_follow`. The lower seat still offers, and a spectator
    links only to seat 0. `cargo xtask net web --watch` runs a spectator page beside
    the two players, and all three reach the same hash.

### 3. NAT traversal and the relay

- After `udp::meet`, peers are addressed by seat (`PeerAddr::Room`), and the socket
  routes each one itself.
- **Direct** once both sides hear each other. Room datagrams (`0xA9`) carry an "I hear
  you" flag. Hellos go every 100 ms to the public address the lobby saw and to the LAN
  address the peer reported (`JOIN … lan=`). This is UDP hole punching. It also connects
  two machines behind one NAT that doesn't hairpin.
- **Through the lobby** otherwise (`0xAA`). The lobby forwards a datagram to the member
  at the receiving seat, and only when the sender's address is the member at the sending
  seat.
  - Hellos continue, so a relayed pair turns direct once a path opens.
  - A direct pair falls back to the relay after 1.5 s of silence.
- `TRINO_NET_ROUTE=auto|direct|relay` forces either route. A host serving its own room
  (`host:<port>`) relays for its guests.
- **The settings fingerprint.** `NetConfig::fingerprint()` covers the mode, window,
  delay, players, fps, desync interval and pointer space, and goes with every `JOIN`.
  The lobby refuses a mismatch, naming both fingerprints, so it fails at the door instead
  of desyncing later. ggrs itself checks none of this.
- **Proof without a second network** (`trino_net::natsim`):
  - a simulated Internet with RFC 4787 mapping and filtering (open, full cone,
    address-restricted, port-restricted, symmetric), hairpinning on or off, and the
    lobby's own `serve_udp`;
  - the 15 NAT pairs go direct exactly where theory says (`NatKind::punches_with`): only
    symmetric against port-restricted or symmetric needs the relay;
  - the real binary also plays through the real lobby with the relay forced.
- **Deployment:** `docker/lobby` packages `cargo xtask net lobby`. It was built and run
  locally: two PC instances met in its room with the relay forced and both reached
  `0xaeadc42f3bbe07f9` at frame 760. Nothing public is run by the project.

### 4. Keyboard, pointer and wheel online

- `NetInput` is 32 bytes: a `NetPad`, then the 128-bit `KeySet`, the pointer as a `u16`
  fraction of the sender's screen, its screen and buttons, and the wheel in eighths of a
  notch.
- A sender shares the extras only with `[net] keys` / `pointer`. Receivers decode
  whatever arrives, so a mismatch in these two keys can't desync. ggrs's delta encoding
  sends the 23 unused bytes of a pad-only game as almost nothing.
- The pointer arrives in `[net] pointer_space` (default 320×240) on every peer, whatever
  each window's size. A game that plays the pointer online reads it in those units.
- Fuzzing the new decoder found a pre-existing flaw. A hostile `-128` axis decoded to
  −1.0079, outside the −1..1 a game expects. `NetPad::from_bytes` now reads it as −127.

### 5. Delay-based lockstep for the consoles (`trino_core::lockstep`)

- `Lockstep` is `no_std`, does no I/O and allocates nothing:
  - the 3DS builds with `build-std=core,alloc`, so `trino-net` and ggrs are out of reach;
  - each local input plays `delay` frames later;
  - a frame plays only once every seat's input for it is held.
- **Packets** (`L`, v1):
  - each repeats every input the receiver hasn't acknowledged, up to 24, so a lost
    packet costs nothing but the next one;
  - each acknowledges the receiver's inputs and carries the sender's newest state hash;
  - the first hash that differs is `LockState::Desynced { frame }`;
  - a peer silent past `timeout_ms` leaves, and its seat reads disconnected and idle.
- **3DS transport (`trino_platform_3ds::uds`):** libctru's `uds` (NWM) through seven
  shim calls, `trino_uds_*`, each with at most three arguments.
  - The host creates the network. Clients scan and join, and their seat follows the join
    order (seat = node − 1).
  - `N3dsHost::run_named` runs the whole session in lockstep when the RomFS has `/uds`
    (`host 2 delay=4 hash_at=600`), with an optional `/uds_script` recording that
    console's seat plays.
  - `TRINO_UDS=… cargo xtask build|run 3ds` stages these files, and `cargo xtask net 3ds`
    runs two Azahar instances in one room (`tools/uds-test/two-azahar.sh`).
- **The `lockstep` self-test step** runs on the PC, Azahar and the browsers, and is
  skipped on the N64 (no network, and its heap is thin):
  - two in-process peers over a link that loses one packet in five and delays the rest;
  - they end on `0x031055f051d3adfe` on every CPU.
- **N64:** no network hardware. It could run this lockstep, but no step or code
  references it in the ROM. ADR 0036's measured rollback cost stays the note.

### 6. Recordings from a snapshot (`TREC` v2)

- `Recorder::begin_from(header, state, sink)` puts the game's `save_state` after the
  header: `FLAG_SNAPSHOT`, version 2, then a `u32` length and the bytes. The header's
  `start_frame` and `start_hash` say where it was taken.
- `Replay::restore` / `Session::restore` load the snapshot before `start`. The start hash
  then holds the loaded state, so another build's snapshot reads `diverged at frame 0`.
- A recording without a snapshot is still written as v1, byte for byte. The committed
  fixtures and the consoles' re-recordings don't move.
- **Who makes them:**
  - the PC's crash tail snapshots the game at its start and every 30 s, keeps the two
    newest, and notes a hash every second, so `crash-*.trec` starts at the oldest
    snapshot still in the ring and replays to the crash on that build;
  - `TRINO_RECORD_FROM=<frame>`;
  - `cargo xtask replay cut <file> --from N --out <cut>`.
- castle64 now has snapshots too (`save_raw`, 1 348 bytes).
- A snapshot is a game's own bytes, so it replays on the build and target that wrote it.
  `replay run --device` refuses one for a console.

### 7. Replays on real hardware

- `cargo xtask replay run <file> --platform n64|3ds --device sc64|everdrive|network
  [--ip]` builds the image with `replay.trec` and deploys it through the `hardware`
  skill's loaders.
- It follows the console's log under a pseudo-terminal (`script`) until the
  `TRINO_REPLAY` verdict, then stops the loader. A pty, because a pipe delivers the
  loaders' unflushed output in 4 KB blocks (an AGENTS.md pitfall).
- Tested with stub loaders. Windows has no pty: there, read the line yourself.

### 8. Fuzzing

The `trec` target existed since #68. It now also checks v2 snapshot bounds and has a v2
seed. `netpad` covers `NetInput`, and `net` covers the relay datagrams and `serve_udp`.

## Consequences

- **Local seats:** every soak, the NAT matrix and the real-binary run check that each
  machine sees only its own seat while the hashes agree. Screenshots of two windows show
  each HUD on its own player.
- **Rollback, lockstep and every target agree:**
  - Solid Eleven in lockstep with 15 % loss reaches ggrs's rollback hash at frame 760
    (`0xaeadc42f3bbe07f9`);
  - two emulated 3DS consoles over `uds` and the PC's lockstep reach
    `0x94fa2260aa5f5154` at frame 600 of castle64.
- **Snapshot recordings:** a castle64 cut at frame 400 and a Solid Eleven cut at frame
  300 replay to their full runs' final hashes.

### Limits and what's left

#117 closed with PR #142. The hardware runs below moved to #123, and the Internet
session, mid-match spectators and Solid Eleven's portable snapshot moved to #143.

- **Two physical 3DS units:** not run. The code path is the one Azahar's NWM emulation
  exercised.
- **A real Internet session between two machines:** not measured. The NAT simulator,
  loopback UDP with simulated latency and loss, and the real lobby relay stand in.
  `cargo xtask net lobby` plus `TRINO_NET=lobby:…` on two machines is the measurement
  to run.
- **Flashcart replays:** not run on a physical cart (stub loaders only).
- **Spectators joining mid-match:** not supported (see §2).
- **Azahar rooms:** they need distinct console usernames. A fresh Azahar is "AZAHAR"
  everywhere, so the script renames the second console through its settings.
