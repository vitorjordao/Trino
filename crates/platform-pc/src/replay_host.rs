//! Recording and replaying input on the PC (issue #67): what
//! `TRINO_RECORD` and `TRINO_REPLAY` do in `apps/pc` and
//! `trino-runner-pc`, and the crash report's input tail.
//!
//! A host builds one [`InputSession`] ([`InputSession::from_env`]) and, each
//! frame, asks it for the input to feed ([`InputSession::input`]: the live
//! players, or the recording's), runs the game's update with it, then hands
//! the game back ([`InputSession::after`]: the frame is recorded with its
//! state hash, or the replay compares it). The session prints:
//!
//! - `TRINO_RECORD: <path> frames=<n> bytes=<b> hashes=<h>` when a
//!   recording is written (at exit, or when the host calls
//!   [`InputSession::finish`]);
//! - `TRINO_REPLAY: ok frames=<n> checked=<c> hash=<h> on pc`, or
//!   `TRINO_REPLAY: diverged at frame <f> (expected <e>, got <g>) on pc`,
//!   when a replay ends. `cargo xtask replay` reads these lines.
//!
//! `TRINO_RECORD_FROM=<frame>` (with `TRINO_RECORD`) plays live until that
//! frame, then records from a snapshot of the game (issue #117, `TREC` v2).
//!
//! Every frame also goes into a ring of the last minute ([`note_frame`],
//! [`recent_input`]), which the runner's crash report writes beside the
//! report as `crash-*.trec`. A game whose snapshots may leave the process
//! ([`Game::portable_state`]) is snapshotted at its start and every
//! [`TAIL_SNAPSHOT_EVERY`] frames
//! ([`note_game`]), and its hash noted every second, so the tail starts
//! from the oldest snapshot still in the ring and **replays to the crash**
//! (`TRINO_REPLAY=crash-….trec`) on the build that wrote it (issue #117).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use trino_core::replay::{HEADER_LEN, Header, Recorder, Session};
use trino_core::{Game, PlayersInput};

/// How many frames the crash tail keeps (a minute at 60 fps).
pub const TAIL_FRAMES: usize = 3600;
/// A snapshot of the game for the tail every this many frames (30 s).
pub const TAIL_SNAPSHOT_EVERY: u64 = 1800;
/// The tail's frames carry the game's hash this often (once a second).
const TAIL_HASH_EVERY: u64 = 60;

struct Tail {
    game: String,
    screen: (u16, u16),
    players: u8,
    /// Frames noted since the host started.
    total: u64,
    /// Each frame's input, `dt` and (every second) the hash after it.
    frames: VecDeque<(PlayersInput, f32, Option<u64>)>,
    /// (frames played before it, state, its hash), oldest first.
    snaps: VecDeque<(u64, Vec<u8>, u64)>,
}

fn tail_for<'a>(tail: &'a mut Option<Tail>, game: &str, screen: (u16, u16)) -> &'a mut Tail {
    tail.get_or_insert_with(|| Tail {
        game: game.to_string(),
        screen,
        players: 1,
        total: 0,
        frames: VecDeque::with_capacity(TAIL_FRAMES),
        snaps: VecDeque::new(),
    })
}

static TAIL: Mutex<Option<Tail>> = Mutex::new(None);

/// Note one frame's input for the crash tail.
pub fn note_frame(game: &str, screen: (u16, u16), players: &PlayersInput, dt: f32) {
    let Ok(mut tail) = TAIL.lock() else { return };
    let t = tail_for(&mut tail, game, screen);
    if t.frames.len() == TAIL_FRAMES {
        t.frames.pop_front();
    }
    t.players = t.players.max(players_in(players));
    t.frames.push_back((*players, dt, None));
    t.total += 1;
}

/// Note the game after a frame (or before the first): its hash every
/// second, a snapshot at the start and every [`TAIL_SNAPSHOT_EVERY`]
/// frames, the two newest kept (issue #117). A game without
/// `save_state` leaves the tail input-only.
pub fn note_game(name: &str, screen: (u16, u16), game: &dyn Game) {
    let Ok(mut tail) = TAIL.lock() else { return };
    let t = tail_for(&mut tail, name, screen);
    if t.total.is_multiple_of(TAIL_HASH_EVERY)
        && let Some(last) = t.frames.back_mut()
    {
        last.2 = game.state_hash();
    }
    if !t.total.is_multiple_of(TAIL_SNAPSHOT_EVERY)
        || t.snaps.back().is_some_and(|s| s.0 == t.total)
    {
        return;
    }
    let mut state = vec![0u8; game.state_size()];
    let Some(n) = game.save_state(&mut state) else {
        return;
    };
    state.truncate(n);
    if t.snaps.len() == 2 {
        t.snaps.pop_front();
    }
    t.snaps
        .push_back((t.total, state, game.state_hash().unwrap_or(0)));
}

/// The players a frame needs recorded: up to the highest connected one.
fn players_in(p: &PlayersInput) -> u8 {
    (8 - (p.connected & 0x0f).leading_zeros() as u8).max(1)
}

/// The last minute of input as a `TREC` (its `start_frame` is where the
/// tail starts in the session). From the oldest snapshot still inside the
/// ring when the game has one (v2: it replays to the end of the tail),
/// else the whole ring with no start state (v1: a diagnostic). `None`
/// before the first frame.
pub fn recent_input() -> Option<Vec<u8>> {
    let tail = TAIL.lock().ok()?;
    let t = tail.as_ref()?;
    let first = t.total - t.frames.len() as u64;
    let snap = t.snaps.iter().find(|s| s.0 >= first);
    let from = snap.map_or(first, |s| s.0);
    let mut header = Header::new(&t.game, t.screen, t.players, TAIL_HASH_EVERY as u16);
    header.start_frame = from as u32;
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| out.extend_from_slice(b);
    let mut rec = match snap {
        Some((_, state, hash)) => {
            header.start_hash = *hash;
            Recorder::begin_from(header, state, &mut sink)
        }
        None => {
            header.hash_every = 0;
            Recorder::begin(header, &mut sink)
        }
    };
    for (p, dt, hash) in t.frames.iter().skip((from - first) as usize) {
        rec.frame(&mut sink, p, *dt, if snap.is_some() { *hash } else { None });
    }
    let head = rec.finish_header();
    out[..HEADER_LEN].copy_from_slice(&head);
    Some(out)
}

/// One run's input: live, recorded or replayed (a
/// [`trino_core::replay::Session`] with the PC's file and lines).
pub struct InputSession {
    game: String,
    screen: (u16, u16),
    session: Session<'static>,
    /// The recording so far, and where it goes.
    out: Vec<u8>,
    path: Option<PathBuf>,
    done: bool,
    /// `TRINO_RECORD_FROM`: (frame, path, players, hash every) of a
    /// recording that starts from a snapshot once that frame is reached.
    from: Option<(u32, PathBuf, u8, u16)>,
    /// Frames played so far.
    seen: u32,
}

/// Why a session couldn't start.
pub type SessionError = String;

impl InputSession {
    /// Live input, nothing recorded (but the crash tail).
    pub fn live(game: &str, screen: (u16, u16)) -> InputSession {
        InputSession {
            game: game.to_string(),
            screen,
            session: Session::live(),
            out: Vec::new(),
            path: None,
            done: false,
            from: None,
            seen: 0,
        }
    }

    /// Play live until `frame` frames have run, then record into `path`
    /// from a snapshot of the game (issue #117; `TRINO_RECORD_FROM`).
    pub fn record_from(
        game: &str,
        screen: (u16, u16),
        frame: u32,
        path: PathBuf,
        players: u8,
        hash_every: u16,
    ) -> InputSession {
        let mut s = InputSession::live(game, screen);
        s.from = Some((frame, path, players, hash_every));
        s
    }

    /// Record into `path`, a hash every `hash_every` frames, `players`
    /// players per frame.
    pub fn record(
        game: &str,
        screen: (u16, u16),
        path: PathBuf,
        players: u8,
        hash_every: u16,
    ) -> InputSession {
        let header = Header::new(game, screen, players, hash_every);
        let mut out = Vec::new();
        let session = Session::record(header, &mut |b: &[u8]| out.extend_from_slice(b));
        InputSession {
            game: game.to_string(),
            screen,
            session,
            out,
            path: Some(path),
            done: false,
            from: None,
            seen: 0,
        }
    }

    /// Replay `bytes` (the recording stays alive for the process: a few
    /// KB).
    pub fn replay(
        game: &str,
        screen: (u16, u16),
        bytes: Vec<u8>,
        until: Option<u32>,
    ) -> Result<InputSession, SessionError> {
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let session = Session::replay(bytes, game, until).map_err(|e| e.message().to_string())?;
        Ok(InputSession {
            game: game.to_string(),
            screen,
            session,
            out: Vec::new(),
            path: None,
            done: false,
            from: None,
            seen: 0,
        })
    }

    /// `TRINO_REPLAY=<file>` (with `TRINO_REPLAY_UNTIL=<frames>`) replays,
    /// `TRINO_RECORD=<file>` records (`TRINO_RECORD_HASH_EVERY`, default
    /// 1: every frame; `TRINO_RECORD_PLAYERS`, default 4;
    /// `TRINO_RECORD_FROM=<frame>`: from a snapshot taken then), else live.
    pub fn from_env(game: &str, screen: (u16, u16)) -> Result<InputSession, SessionError> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        if let Some(path) = var("TRINO_REPLAY") {
            let bytes = std::fs::read(&path).map_err(|e| format!("TRINO_REPLAY {path}: {e}"))?;
            let until = var("TRINO_REPLAY_UNTIL").and_then(|v| v.parse().ok());
            return Self::replay(game, screen, bytes, until).map_err(|e| format!("{path}: {e}"));
        }
        if let Some(path) = var("TRINO_RECORD") {
            let every = var("TRINO_RECORD_HASH_EVERY")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let players = var("TRINO_RECORD_PLAYERS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(trino_core::MAX_PLAYERS as u8);
            if let Some(from) = var("TRINO_RECORD_FROM") {
                let from = from
                    .parse()
                    .map_err(|_| format!("TRINO_RECORD_FROM={from}: a frame number"))?;
                return Ok(Self::record_from(
                    game,
                    screen,
                    from,
                    path.into(),
                    players,
                    every,
                ));
            }
            return Ok(Self::record(game, screen, path.into(), players, every));
        }
        Ok(Self::live(game, screen))
    }

    pub fn is_replay(&self) -> bool {
        self.session.is_replay()
    }

    pub fn is_recording(&self) -> bool {
        self.session.is_recording()
    }

    /// A replay of a mid-game recording loads its snapshot into `game`
    /// (issue #117); call it before [`start`](Self::start). False (and a
    /// `TRINO_REPLAY:` line) when the game refuses it.
    pub fn restore(&mut self, game: &mut dyn Game) -> bool {
        let ok = self.session.restore(game);
        if !ok {
            println!(
                "TRINO_REPLAY: the recording's snapshot doesn't load into this build of {}",
                self.game
            );
        }
        ok
    }

    /// The game before its first frame: a recording stores its hash, a
    /// replay compares it (a different start is `diverged at frame 0`).
    /// The crash tail takes its first snapshot.
    pub fn start(&mut self, game: &dyn Game) {
        if let Some(head) = self.session.start(game) {
            self.out[..HEADER_LEN].copy_from_slice(&head);
        }
        note_game(&self.game, self.screen, game);
    }

    /// The input to feed this frame: `live` and `dt` (live, recording), or
    /// the recording's frame (replay; `None` once it ended).
    pub fn input(&mut self, live: &PlayersInput, dt: f32) -> Option<(PlayersInput, f32)> {
        self.session.input(live, dt)
    }

    /// After the game's update with `players` and `dt`: record the frame
    /// (with the game's hash when due) or check the replayed one.
    pub fn after(&mut self, game: &dyn Game, players: &PlayersInput, dt: f32) {
        note_frame(&self.game, self.screen, players, dt);
        note_game(&self.game, self.screen, game);
        self.seen += 1;
        if self.from.as_ref().is_some_and(|f| f.0 == self.seen)
            && let Some((frame, path, players, every)) = self.from.take()
        {
            // TRINO_RECORD_FROM: the snapshot, then every frame after it.
            let mut state = vec![0u8; game.state_size()];
            let saved = if game.portable_state() {
                game.save_state(&mut state)
            } else {
                None
            };
            match saved {
                Some(n) => {
                    state.truncate(n);
                    let mut header = Header::new(&self.game, self.screen, players, every);
                    header.start_frame = frame;
                    header.start_hash = game.state_hash().unwrap_or(0);
                    let out = &mut self.out;
                    self.session = Session::record_from(header, &state, &mut |b: &[u8]| {
                        out.extend_from_slice(b)
                    });
                    self.path = Some(path);
                    println!(
                        "TRINO_RECORD: recording from frame {frame} (snapshot {} bytes)",
                        state.len()
                    );
                }
                None => eprintln!(
                    "TRINO_RECORD_FROM: {}'s snapshots don't leave the process \
                     (Game::portable_state): nothing recorded",
                    self.game
                ),
            }
            return;
        }
        let out = &mut self.out;
        if let Some(d) = self
            .session
            .after(game, players, dt, &mut |b: &[u8]| out.extend_from_slice(b))
        {
            eprintln!(
                "TRINO_REPLAY: first divergence at frame {} (expected {:#018x}, got {:#018x})",
                d.frame, d.expected, d.got
            );
        }
    }

    /// Has a replay run out (the host then stops: a smoke-style exit)?
    pub fn ended(&self) -> bool {
        self.session.ended()
    }

    /// Frames recorded or replayed so far.
    pub fn frames(&self) -> u32 {
        self.session.frames()
    }

    /// Write the recording, or report the replay: the `TRINO_RECORD:` /
    /// `TRINO_REPLAY:` line (printed, and returned). Once per session.
    pub fn finish(&mut self) -> Option<String> {
        if self.done {
            return None;
        }
        let line = if let Some(head) = self.session.finish_header() {
            self.out[..HEADER_LEN].copy_from_slice(&head);
            let path = self.path.clone()?;
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::write(&path, &self.out) {
                Ok(()) => format!(
                    "TRINO_RECORD: {} frames={} bytes={} hashes={}",
                    path.display(),
                    self.session.frames(),
                    self.out.len(),
                    self.session.hashes()
                ),
                Err(e) => format!("TRINO_RECORD: {}: {e}", path.display()),
            }
        } else if self.session.is_replay() {
            let mut line = String::new();
            let _ = self.session.verdict("pc", &mut line);
            line
        } else {
            return None;
        };
        self.done = true;
        println!("{line}");
        Some(line)
    }
}

impl Drop for InputSession {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{Audio, Button, InputState, MusicId, Renderer, SoundId};

    struct Counter(u64);
    impl Game for Counter {
        fn update(&mut self, input: &InputState, _: &mut dyn Audio, dt: f32) {
            self.0 = self.0.wrapping_mul(7) ^ input.button_bits() as u64 ^ dt.to_bits() as u64;
        }
        fn render(&mut self, _: &mut dyn Renderer) {}
        fn state_hash(&self) -> Option<u64> {
            Some(self.0)
        }
        fn state_size(&self) -> usize {
            8
        }
        fn portable_state(&self) -> bool {
            true
        }
        fn save_state(&self, out: &mut [u8]) -> Option<usize> {
            out.get_mut(..8)?.copy_from_slice(&self.0.to_le_bytes());
            Some(8)
        }
        fn load_state(&mut self, b: &[u8]) -> bool {
            match b.try_into() {
                Ok(b) => {
                    self.0 = u64::from_le_bytes(b);
                    true
                }
                Err(_) => false,
            }
        }
    }

    /// A game without `save_state`: its tail stays input-only.
    struct Stateless(u64);
    impl Game for Stateless {
        fn update(&mut self, input: &InputState, _: &mut dyn Audio, _: f32) {
            self.0 += input.button_bits() as u64;
        }
        fn render(&mut self, _: &mut dyn Renderer) {}
    }

    struct Mute;
    impl Audio for Mute {
        fn play_sound(&mut self, _: SoundId) {}
        fn play_music(&mut self, _: MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    fn live(f: u32) -> PlayersInput {
        let mut s = InputState::IDLE;
        s.set(Button::A, f.is_multiple_of(3));
        PlayersInput::single(s)
    }

    #[test]
    fn a_session_records_and_replays_itself() {
        let _turn = TAIL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("trino-rec-{}", std::process::id()));
        let path = dir.join("run.trec");
        let mut g = Counter(1);
        let mut s = InputSession::record("counter", (320, 240), path.clone(), 1, 1);
        s.start(&g);
        for f in 0..50 {
            let dt = 0.016 + f as f32 * 1e-5;
            let (p, dt) = s.input(&live(f), dt).unwrap();
            g.update(&p.players[0], &mut Mute, dt);
            s.after(&g, &p, dt);
        }
        let line = s.finish().unwrap();
        assert!(
            line.contains("frames=50") && line.contains("hashes=50"),
            "{line}"
        );
        let bytes = std::fs::read(&path).unwrap();

        let mut g = Counter(1);
        let mut r = InputSession::replay("counter", (320, 240), bytes.clone(), None).unwrap();
        r.start(&g);
        while let Some((p, dt)) = r.input(&PlayersInput::NONE, 0.0) {
            g.update(&p.players[0], &mut Mute, dt);
            r.after(&g, &p, dt);
        }
        let line = r.finish().unwrap();
        assert!(
            line.starts_with("TRINO_REPLAY: ok frames=50 checked=50"),
            "{line}"
        );

        // Stopped early, and a run that starts elsewhere.
        let mut g = Counter(2);
        let mut r = InputSession::replay("counter", (320, 240), bytes, Some(10)).unwrap();
        r.start(&g);
        while let Some((p, dt)) = r.input(&PlayersInput::NONE, 0.0) {
            g.update(&p.players[0], &mut Mute, dt);
            r.after(&g, &p, dt);
        }
        assert_eq!(r.frames(), 10);
        let line = r.finish().unwrap();
        assert!(line.contains("diverged at frame 0"), "{line}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The tail is process-wide: the tests that fill it take turns.
    static TAIL_TEST: Mutex<()> = Mutex::new(());

    fn reset_tail() {
        *TAIL.lock().unwrap() = None;
    }

    #[test]
    fn the_tail_keeps_the_last_minute() {
        let _turn = TAIL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        reset_tail();
        let g = Stateless(0);
        note_game("tail-test", (320, 240), &g);
        for f in 0..(TAIL_FRAMES as u32 + 25) {
            note_frame("tail-test", (320, 240), &live(f), 1.0 / 60.0);
            note_game("tail-test", (320, 240), &g);
        }
        let bytes = recent_input().unwrap();
        assert_eq!(
            trino_core::replay::Header::parse(&bytes).unwrap().version,
            1
        );
        let mut r = trino_core::replay::Reader::new(&bytes).unwrap();
        assert_eq!(r.header().frames as usize, TAIL_FRAMES);
        assert!(r.header().start_frame >= 25);
        let mut n = 0;
        while let Some(f) = r.next_frame() {
            f.unwrap();
            n += 1;
        }
        assert_eq!(n, TAIL_FRAMES);
    }

    /// Issue #117: a game with snapshots gets a tail that starts from the
    /// oldest snapshot still in the ring and replays to the end of it, hash
    /// for hash.
    #[test]
    fn the_crash_tail_replays_from_its_snapshot() {
        let _turn = TAIL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        reset_tail();
        let total = TAIL_FRAMES as u32 + 700;
        let mut g = Counter(5);
        let mut s = InputSession::live("counter", (320, 240));
        s.start(&g);
        for f in 0..total {
            let (p, dt) = s.input(&live(f), 1.0 / 60.0 + f as f32 * 1e-6).unwrap();
            g.update(&p.players[0], &mut Mute, dt);
            s.after(&g, &p, dt);
        }
        let end = g.0;
        let bytes = recent_input().unwrap();
        let h = trino_core::replay::Header::parse(&bytes).unwrap();
        // Snapshots at 0, 1800, 3600 (frames played): the ring holds
        // 700..4300, so the tail starts at 1800 and runs 2500 frames.
        assert_eq!(
            (h.version, h.start_frame, h.frames),
            (2, 1800, total - 1800)
        );
        let mut fresh = Counter(0);
        let mut r = InputSession::replay("counter", (320, 240), bytes, None).unwrap();
        assert!(r.restore(&mut fresh));
        r.start(&fresh);
        while let Some((p, dt)) = r.input(&PlayersInput::NONE, 0.0) {
            fresh.update(&p.players[0], &mut Mute, dt);
            r.after(&fresh, &p, dt);
        }
        let line = r.finish().unwrap();
        assert!(line.starts_with("TRINO_REPLAY: ok frames=2500"), "{line}");
        assert_eq!(fresh.0, end);
    }

    /// `TRINO_RECORD_FROM`: live until the frame, then a v2 recording that
    /// replays to the same end.
    #[test]
    fn recording_from_a_frame_starts_with_a_snapshot() {
        let _turn = TAIL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("trino-from-{}", std::process::id()));
        let path = dir.join("from.trec");
        let mut g = Counter(3);
        let mut s = InputSession::record_from("counter", (320, 240), 40, path.clone(), 1, 1);
        s.start(&g);
        for f in 0..100 {
            let (p, dt) = s.input(&live(f), 1.0 / 60.0).unwrap();
            g.update(&p.players[0], &mut Mute, dt);
            s.after(&g, &p, dt);
        }
        let line = s.finish().unwrap();
        assert!(line.contains("frames=60"), "{line}");
        let bytes = std::fs::read(&path).unwrap();
        let h = trino_core::replay::Header::parse(&bytes).unwrap();
        assert_eq!((h.version, h.start_frame, h.frames), (2, 40, 60));
        let mut fresh = Counter(0);
        let mut r = InputSession::replay("counter", (320, 240), bytes, None).unwrap();
        assert!(r.restore(&mut fresh));
        r.start(&fresh);
        while let Some((p, dt)) = r.input(&PlayersInput::NONE, 0.0) {
            fresh.update(&p.players[0], &mut Mute, dt);
            r.after(&fresh, &p, dt);
        }
        assert!(r.finish().unwrap().contains("ok frames=60 checked=60"));
        assert_eq!(fresh.0, g.0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
