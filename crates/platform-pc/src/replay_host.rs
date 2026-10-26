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
//! Every frame also goes into a ring of the last minute ([`note_frame`],
//! [`recent_input`]), which the runner's crash report writes beside the
//! report as `crash-*.trec` (a diagnostic tail: it doesn't start at boot).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use trino_core::replay::{HEADER_LEN, Header, Recorder, Session};
use trino_core::{Game, PlayersInput};

/// How many frames the crash tail keeps (a minute at 60 fps).
pub const TAIL_FRAMES: usize = 3600;

struct Tail {
    game: String,
    screen: (u16, u16),
    players: u8,
    /// Frames noted since the host started.
    total: u64,
    frames: VecDeque<(PlayersInput, f32)>,
}

static TAIL: Mutex<Option<Tail>> = Mutex::new(None);

/// Note one frame's input for the crash tail.
pub fn note_frame(game: &str, screen: (u16, u16), players: &PlayersInput, dt: f32) {
    let Ok(mut tail) = TAIL.lock() else { return };
    let t = tail.get_or_insert_with(|| Tail {
        game: game.to_string(),
        screen,
        players: 1,
        total: 0,
        frames: VecDeque::with_capacity(TAIL_FRAMES),
    });
    if t.frames.len() == TAIL_FRAMES {
        t.frames.pop_front();
    }
    t.players = t.players.max(players_in(players));
    t.frames.push_back((*players, dt));
    t.total += 1;
}

/// The players a frame needs recorded: up to the highest connected one.
fn players_in(p: &PlayersInput) -> u8 {
    (8 - (p.connected & 0x0f).leading_zeros() as u8).max(1)
}

/// The last minute of input as a `TREC` (its `start_frame` is where the
/// tail starts in the session; no hashes, no start state). `None` before
/// the first frame.
pub fn recent_input() -> Option<Vec<u8>> {
    let tail = TAIL.lock().ok()?;
    let t = tail.as_ref()?;
    let mut header = Header::new(&t.game, t.screen, t.players, 0);
    header.start_frame = (t.total - t.frames.len() as u64) as u32;
    let mut out = Vec::new();
    let mut sink = |b: &[u8]| out.extend_from_slice(b);
    let mut rec = Recorder::begin(header, &mut sink);
    for (p, dt) in &t.frames {
        rec.frame(&mut sink, p, *dt, None);
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
        }
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
        })
    }

    /// `TRINO_REPLAY=<file>` (with `TRINO_REPLAY_UNTIL=<frames>`) replays,
    /// `TRINO_RECORD=<file>` records (`TRINO_RECORD_HASH_EVERY`, default
    /// 1: every frame; `TRINO_RECORD_PLAYERS`, default 4), else live.
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

    /// The game before its first frame: a recording stores its hash, a
    /// replay compares it (a different start is `diverged at frame 0`).
    pub fn start(&mut self, game: &dyn Game) {
        if let Some(head) = self.session.start(game) {
            self.out[..HEADER_LEN].copy_from_slice(&head);
        }
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

    #[test]
    fn the_tail_keeps_the_last_minute() {
        for f in 0..(TAIL_FRAMES as u32 + 25) {
            note_frame("tail-test", (320, 240), &live(f), 1.0 / 60.0);
        }
        let bytes = recent_input().unwrap();
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
}
