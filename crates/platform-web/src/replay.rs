//! Recording and replaying input in the browser (issue #67): the Web's
//! `TRINO_RECORD` / `TRINO_REPLAY`.
//!
//! - `?replay=<url>` (with `?replay_until=<frames>`): fetch a `TREC` and
//!   play it back; when it ends the page logs the verdict
//!   (`TRINO_REPLAY: ok … on web` or `… diverged at frame …`) and finishes
//!   (`globalThis.__trino.finished`, its pass flag the verdict).
//! - `?record`: record the session; the recording so far is in
//!   `__trino.info.recording` (base64, refreshed every second and when the
//!   run ends: `?frames=N`), for the harness or devtools to save.
//!
//! The same [`trino_core::replay::Session`] the PC drives, so a recording
//! made in a browser replays on the PC and the consoles, and back.

use trino_core::replay::{HEADER_LEN, Header, Session};
use trino_core::{Game, PlayersInput};

/// One page's input session.
pub struct WebSession {
    session: Session<'static>,
    out: Vec<u8>,
    platform: &'static str,
    done: bool,
}

impl WebSession {
    pub fn live() -> WebSession {
        WebSession {
            session: Session::live(),
            out: Vec::new(),
            platform: "web",
            done: false,
        }
    }

    /// Record `game` at `screen`, a hash every frame.
    pub fn record(game: &str, screen: (u16, u16)) -> WebSession {
        let header = Header::new(game, screen, trino_core::MAX_PLAYERS as u8, 1);
        let mut out = Vec::new();
        let session = Session::record(header, &mut |b: &[u8]| out.extend_from_slice(b));
        WebSession {
            session,
            out,
            platform: "web",
            done: false,
        }
    }

    /// Replay `bytes` of `game` (kept for the page's life).
    pub fn replay(game: &str, bytes: Vec<u8>, until: Option<u32>) -> Result<WebSession, String> {
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let session = Session::replay(bytes, game, until).map_err(|e| e.message().to_string())?;
        Ok(WebSession {
            session,
            out: Vec::new(),
            platform: "web",
            done: false,
        })
    }

    /// From the page's query: `replay=<url>` (fetched), `record`, else live.
    pub async fn from_query(
        query: &crate::Query,
        game: &str,
        screen: (u16, u16),
    ) -> Result<WebSession, String> {
        if let Some(url) = query.get("replay") {
            let bytes = crate::fetch(&url).await?;
            let until = query.get("replay_until").and_then(|v| v.parse().ok());
            return Self::replay(game, bytes, until).map_err(|e| format!("{url}: {e}"));
        }
        if query.has("record") {
            return Ok(Self::record(game, screen));
        }
        Ok(Self::live())
    }

    pub fn is_replay(&self) -> bool {
        self.session.is_replay()
    }

    pub fn start(&mut self, game: &dyn Game) {
        if let Some(head) = self.session.start(game) {
            self.out[..HEADER_LEN].copy_from_slice(&head);
        }
    }

    pub fn input(&mut self, live: &PlayersInput, dt: f32) -> Option<(PlayersInput, f32)> {
        self.session.input(live, dt)
    }

    pub fn after(&mut self, game: &dyn Game, players: &PlayersInput, dt: f32) {
        let out = &mut self.out;
        if let Some(d) = self
            .session
            .after(game, players, dt, &mut |b: &[u8]| out.extend_from_slice(b))
        {
            crate::js::log(&format!(
                "TRINO_REPLAY: first divergence at frame {} (expected {:#018x}, got {:#018x})",
                d.frame, d.expected, d.got
            ));
        }
        if self.session.is_recording() && self.session.frames().is_multiple_of(60) {
            self.publish();
        }
    }

    /// The recording so far as `__trino.info.recording` (base64, its
    /// header final).
    fn publish(&mut self) {
        if let Some(head) = self.session.finish_header() {
            let mut bytes = self.out.clone();
            bytes[..HEADER_LEN].copy_from_slice(&head);
            crate::js::trino_info("recording", &crate::b64::encode(&bytes));
        }
    }

    /// The verdict (replay) or the final recording; `Some(passed)` for a
    /// replay, to finish the page with.
    pub fn finish(&mut self) -> Option<bool> {
        if self.done {
            return None;
        }
        self.done = true;
        if self.session.is_recording() {
            self.publish();
            crate::js::log(&format!(
                "TRINO_RECORD: web frames={} bytes={} hashes={}",
                self.session.frames(),
                self.out.len(),
                self.session.hashes()
            ));
            return None;
        }
        let replay = self.session.replay_state()?;
        let mut line = String::new();
        let _ = self.session.verdict(self.platform, &mut line);
        crate::js::log(&line);
        Some(replay.diverged.is_none())
    }
}
