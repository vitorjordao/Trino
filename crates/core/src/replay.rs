//! Input recording and replay (issue #67, ADR 0036): a session as the
//! players' input per frame, so any target can play it again and say
//! where it stopped matching.
//!
//! Trino's sim is deterministic: the same state and the same input give the
//! same next state on the PC, the N64, the 3DS and the Web. A recording
//! (`TREC`) is therefore only the input: per frame the frame's `dt` (raw
//! `f32` bits — the PC's is wall-clock), every local player's
//! [`InputState`] (raw bits, delta-coded against the previous frame: an
//! idle frame is one byte) and, every [`Header::hash_every`] frames, the
//! game's [`state hash`](crate::Game::state_hash) after that frame. A replay
//! feeds the same input to a fresh game and compares the hashes: the first
//! one that differs names the frame the run diverged on (`replay: diverged
//! at frame 1234`).
//!
//! - [`Recorder`]: writes a recording through a [`Sink`] (a `Vec` on the
//!   PC, the debug channel on the consoles), no allocation.
//! - [`Reader`]: decodes one, frame by frame ([`Frame`]).
//! - [`Replay`]: drives a [`Game`](crate::Game) with a recording and checks
//!   its hashes ([`Replay::step`]); the hosts' `TRINO_REPLAY`, the
//!   self-test's `replay` step and the editor's scrubber use it.
//! - [`StateHasher`]: FNV-1a 64 with fixed-width little-endian integers, the
//!   same on every CPU (the N64 is big-endian, `usize` is 32 bits on the
//!   consoles).
//!
//! A recording replays from the state its game started in: the header's
//! [`Header::start_hash`] is the hash before the first frame, and a replay
//! that starts elsewhere reports `diverged at frame 0`. A *tail* (the last
//! minute a crash report keeps, [`Header::start_frame`] > 0) has no state to
//! start from: it is a diagnostic of what the player did, not replayable
//! from boot.
//!
//! Format (`TREC` v1, little-endian):
//!
//! | Bytes | Field |
//! |---|---|
//! | 4 | `TREC` |
//! | 2 | version (1) |
//! | 2 | flags (bit 0: the frame count is final) |
//! | 4 | game id ([`crate::asset_id`] of its name) |
//! | 2 + 2 | the screen the game was created with |
//! | 1 | players per frame (1..=4) |
//! | 1 | reserved (0) |
//! | 2 | hash every N frames (0: none) |
//! | 4 | start frame (0: from boot) |
//! | 4 | frame count (0 with flag 0: read to the end) |
//! | 8 | start hash (0: none) |
//!
//! Then one record per frame: a head byte (bit 0 `dt` follows, bit 1 a hash
//! follows, bit 2 the connected mask follows, bits 4..7 which players
//! changed), then `dt` (4), the mask (1), per changed player a 16-bit field
//! mask and the changed fields' raw bits, and the hash (8). Each field of a
//! player: buttons (4), stick (8), right stick (8), triggers (8), pointer
//! (1 tag, 8 if present, 1 screen), pointer buttons (1), wheel (4), keys
//! (16), pad (1).

use crate::input::{InputState, KeySet};
use crate::math::Vec2;
use crate::players::{MAX_PLAYERS, PlayersInput};

/// `TREC`.
pub const MAGIC: [u8; 4] = *b"TREC";
/// The format version this build writes and reads.
pub const VERSION: u16 = 1;
/// Bytes before the first frame.
pub const HEADER_LEN: usize = 36;

/// Header flag: the frame count is final (the recording was finished).
pub const FLAG_FINISHED: u16 = 1;

/// The recording's header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
    /// [`crate::asset_id`] of the game's name (`castle64`, `solid-eleven`):
    /// a replay refuses another game's recording.
    pub game: u32,
    /// The screen the game was created with (its sim may lay out by it).
    pub screen: (u16, u16),
    /// Players per frame (1..=[`MAX_PLAYERS`]).
    pub players: u8,
    /// A hash every this many frames (0: no hashes).
    pub hash_every: u16,
    /// The session frame the first recorded frame was (0: from boot).
    pub start_frame: u32,
    /// Frames recorded (0 with no [`FLAG_FINISHED`]: read to the end).
    pub frames: u32,
    /// The game's state hash before the first frame (0: none).
    pub start_hash: u64,
}

impl Header {
    /// A header for `game` (its name) at `screen`, recording `players`
    /// players, a hash every `hash_every` frames.
    pub fn new(game: &str, screen: (u16, u16), players: u8, hash_every: u16) -> Header {
        Header {
            version: VERSION,
            flags: 0,
            game: crate::asset_id(game),
            screen,
            players: players.clamp(1, MAX_PLAYERS as u8),
            hash_every,
            start_frame: 0,
            frames: 0,
            start_hash: 0,
        }
    }

    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&self.version.to_le_bytes());
        b[6..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..12].copy_from_slice(&self.game.to_le_bytes());
        b[12..14].copy_from_slice(&self.screen.0.to_le_bytes());
        b[14..16].copy_from_slice(&self.screen.1.to_le_bytes());
        b[16] = self.players;
        b[18..20].copy_from_slice(&self.hash_every.to_le_bytes());
        b[20..24].copy_from_slice(&self.start_frame.to_le_bytes());
        b[24..28].copy_from_slice(&self.frames.to_le_bytes());
        b[28..36].copy_from_slice(&self.start_hash.to_le_bytes());
        b
    }

    pub fn parse(bytes: &[u8]) -> Result<Header, ReplayError> {
        if bytes.len() < HEADER_LEN {
            return Err(ReplayError::Truncated);
        }
        if bytes[0..4] != MAGIC {
            return Err(ReplayError::NotARecording);
        }
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let u32_at =
            |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let version = u16_at(4);
        if version != VERSION {
            return Err(ReplayError::Version(version));
        }
        let players = bytes[16];
        if players == 0 || players as usize > MAX_PLAYERS {
            return Err(ReplayError::Corrupt);
        }
        let mut h8 = [0u8; 8];
        h8.copy_from_slice(&bytes[28..36]);
        Ok(Header {
            version,
            flags: u16_at(6),
            game: u32_at(8),
            screen: (u16_at(12), u16_at(14)),
            players,
            hash_every: u16_at(18),
            start_frame: u32_at(20),
            frames: u32_at(24),
            start_hash: u64::from_le_bytes(h8),
        })
    }

    /// Is the frame count final?
    pub fn finished(&self) -> bool {
        self.flags & FLAG_FINISHED != 0
    }
}

/// Why a recording can't be read or replayed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayError {
    NotARecording,
    Version(u16),
    /// Shorter than its header or than a frame it starts.
    Truncated,
    /// A field the format doesn't have.
    Corrupt,
    /// Another game's recording.
    WrongGame,
}

impl ReplayError {
    pub fn message(&self) -> &'static str {
        match self {
            ReplayError::NotARecording => "not a TREC recording",
            ReplayError::Version(_) => "a TREC version this build doesn't read",
            ReplayError::Truncated => "the recording ends inside a frame",
            ReplayError::Corrupt => "the recording is corrupt",
            ReplayError::WrongGame => "the recording is another game's",
        }
    }
}

/// Where a [`Recorder`] writes: anything that takes bytes (a `Vec`'s
/// `extend_from_slice`, the debug channel, a fixed buffer).
pub trait Sink {
    fn put(&mut self, bytes: &[u8]);
}

impl<F: FnMut(&[u8])> Sink for F {
    fn put(&mut self, bytes: &[u8]) {
        self(bytes)
    }
}

/// A fixed buffer as a [`Sink`] (consoles, tests): bytes past its end are
/// dropped and counted.
pub struct SliceSink<'a> {
    pub buf: &'a mut [u8],
    pub len: usize,
    pub dropped: usize,
}

impl<'a> SliceSink<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        SliceSink {
            buf,
            len: 0,
            dropped: 0,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl Sink for SliceSink<'_> {
    fn put(&mut self, bytes: &[u8]) {
        let room = self.buf.len() - self.len;
        let n = bytes.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        self.dropped += bytes.len() - n;
    }
}

/// A [`Sink`] that sends a recording over a text channel (the consoles'
/// debug log: ISViewer, `svcOutputDebugString`, a flashcart's USB) as
/// `TRINO_REC:<hex>` lines of up to 64 bytes each, through `log`. Call
/// [`end`](Self::end) last: it sends what is buffered and `TRINO_REC_END`.
/// `cargo xtask` reassembles the lines into the file ([`decode_line`]).
pub struct LineSink<F: FnMut(&str)> {
    log: F,
    buf: [u8; 64],
    len: usize,
}

impl<F: FnMut(&str)> LineSink<F> {
    pub fn new(log: F) -> Self {
        LineSink {
            log,
            buf: [0; 64],
            len: 0,
        }
    }

    fn flush(&mut self) {
        if self.len == 0 {
            return;
        }
        const HEX: &[u8; 16] = b"0123456789abcdef";
        use core::fmt::Write as _;
        let mut line = crate::TextBuf::<160>::new();
        let _ = line.write_str("TRINO_REC:");
        for b in &self.buf[..self.len] {
            let _ = line.write_char(HEX[(b >> 4) as usize] as char);
            let _ = line.write_char(HEX[(b & 15) as usize] as char);
        }
        let _ = line.write_char('\n');
        (self.log)(line.as_str());
        self.len = 0;
    }

    /// Send the rest and the end marker.
    pub fn end(mut self) {
        self.flush();
        (self.log)("TRINO_REC_END\n");
    }

    /// Any other line through the same channel (what was buffered first).
    pub fn log(&mut self, line: &str) {
        self.flush();
        (self.log)(line);
    }
}

/// A [`Session`] over a console's debug channel (the N64's and the 3DS's
/// hosts, issue #67): a replay of bytes the build put in the image
/// (`replay.trec`), its verdict logged; or a recording of the live input
/// streamed as `TRINO_REC:` lines ([`LineSink`]), which `cargo xtask run
/// n64|3ds --record` reassembles.
pub struct ChannelSession<'a, F: FnMut(&str)> {
    session: Session<'a>,
    channel: LineSink<F>,
    platform: &'static str,
}

impl<'a, F: FnMut(&str)> ChannelSession<'a, F> {
    /// Replay `bytes` (any game's: the hashes decide), stopping after
    /// `until` frames when given.
    pub fn replay(
        bytes: &'a [u8],
        until: Option<u32>,
        platform: &'static str,
        log: F,
    ) -> Result<Self, ReplayError> {
        Ok(ChannelSession {
            session: Session::replay_any(bytes, until)?,
            channel: LineSink::new(log),
            platform,
        })
    }

    /// Record `game` (its name, its screen) from now on, a hash every
    /// `hash_every` frames, the start state's hash in the header.
    pub fn record(
        game: &dyn crate::Game,
        name: &str,
        screen: (u16, u16),
        hash_every: u16,
        platform: &'static str,
        log: F,
    ) -> Self {
        let mut header = Header::new(name, screen, MAX_PLAYERS as u8, hash_every);
        header.start_hash = game.state_hash().unwrap_or(0);
        let mut channel = LineSink::new(log);
        let session = Session::record(header, &mut channel);
        ChannelSession {
            session,
            channel,
            platform,
        }
    }

    pub fn is_replay(&self) -> bool {
        self.session.is_replay()
    }

    /// A replay compares the game's start state.
    pub fn start(&mut self, game: &dyn crate::Game) {
        if self.session.is_replay() {
            self.session.start(game);
        }
    }

    pub fn input(&mut self, live: &PlayersInput, dt: f32) -> Option<(PlayersInput, f32)> {
        self.session.input(live, dt)
    }

    pub fn after(&mut self, game: &dyn crate::Game, players: &PlayersInput, dt: f32) {
        if let Some(d) = self.session.after(game, players, dt, &mut self.channel) {
            let mut line = crate::TextBuf::<128>::new();
            let _ = core::fmt::Write::write_fmt(
                &mut line,
                format_args!(
                    "TRINO_REPLAY: first divergence at frame {} (expected {:#018x}, got {:#018x})\n",
                    d.frame, d.expected, d.got
                ),
            );
            self.channel.log(line.as_str());
        }
    }

    pub fn frames(&self) -> u32 {
        self.session.frames()
    }

    /// Did the replay pass (so far)?
    pub fn passed(&self) -> bool {
        self.session
            .replay_state()
            .is_none_or(|r| r.diverged.is_none())
    }

    /// The verdict line (replay) or the end marker (recording).
    pub fn finish(mut self) {
        if self.session.is_replay() {
            let mut line = crate::TextBuf::<160>::new();
            let _ = self.session.verdict(self.platform, &mut line);
            let _ = core::fmt::Write::write_char(&mut line, '\n');
            self.channel.log(line.as_str());
        } else {
            self.channel.end();
        }
    }
}

impl<F: FnMut(&str)> Sink for LineSink<F> {
    fn put(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = (self.buf.len() - self.len).min(bytes.len());
            self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
            self.len += n;
            bytes = &bytes[n..];
            if self.len == self.buf.len() {
                self.flush();
            }
        }
    }
}

/// The bytes of one [`LineSink`] line (anything before `TRINO_REC:` is
/// ignored: an emulator's prefix). `None` for any other line.
pub fn decode_line(line: &str, out: &mut dyn Sink) -> Option<usize> {
    let hex = line.split_once("TRINO_REC:")?.1.trim();
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let b = hex.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    for pair in b.chunks(2) {
        let v = (digit(pair[0])? << 4) | digit(pair[1])?;
        out.put(&[v]);
    }
    Some(b.len() / 2)
}

// Field bits of a changed player.
const F_BUTTONS: u16 = 1 << 0;
const F_STICK: u16 = 1 << 1;
const F_RIGHT: u16 = 1 << 2;
const F_TRIGGERS: u16 = 1 << 3;
const F_POINTER: u16 = 1 << 4;
const F_POINTER_BITS: u16 = 1 << 5;
const F_WHEEL: u16 = 1 << 6;
const F_KEYS: u16 = 1 << 7;
const F_PAD: u16 = 1 << 8;
const F_ALL: u16 = (1 << 9) - 1;

// Head bits of a frame.
const H_DT: u8 = 1 << 0;
const H_HASH: u8 = 1 << 1;
const H_CONNECTED: u8 = 1 << 2;

fn same_vec(a: Vec2, b: Vec2) -> bool {
    a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits()
}

fn same_pointer(a: &InputState, b: &InputState) -> bool {
    match (a.pointer, b.pointer) {
        (None, None) => a.pointer_screen == b.pointer_screen,
        (Some(p), Some(q)) => same_vec(p, q) && a.pointer_screen == b.pointer_screen,
        _ => false,
    }
}

/// The fields of `now` that differ from `prev`, bit for bit.
// Out of line: called per player (the N64 keeps code small).
#[inline(never)]
fn changed_fields(prev: &InputState, now: &InputState) -> u16 {
    let mut f = 0;
    if prev.button_bits() != now.button_bits() {
        f |= F_BUTTONS;
    }
    if !same_vec(prev.stick, now.stick) {
        f |= F_STICK;
    }
    if !same_vec(prev.right_stick, now.right_stick) {
        f |= F_RIGHT;
    }
    if !same_vec(prev.triggers, now.triggers) {
        f |= F_TRIGGERS;
    }
    if !same_pointer(prev, now) {
        f |= F_POINTER;
    }
    if prev.pointer_bits() != now.pointer_bits() {
        f |= F_POINTER_BITS;
    }
    if prev.wheel.to_bits() != now.wheel.to_bits() {
        f |= F_WHEEL;
    }
    if prev.keys != now.keys {
        f |= F_KEYS;
    }
    if prev.pad != now.pad {
        f |= F_PAD;
    }
    f
}

fn put_vec(sink: &mut dyn Sink, v: Vec2) {
    sink.put(&v.x.to_bits().to_le_bytes());
    sink.put(&v.y.to_bits().to_le_bytes());
}

// Out of line: called per player (the N64 keeps code small).
#[inline(never)]
fn put_fields(sink: &mut dyn Sink, s: &InputState, f: u16) {
    sink.put(&f.to_le_bytes());
    if f & F_BUTTONS != 0 {
        sink.put(&s.button_bits().to_le_bytes());
    }
    if f & F_STICK != 0 {
        put_vec(sink, s.stick);
    }
    if f & F_RIGHT != 0 {
        put_vec(sink, s.right_stick);
    }
    if f & F_TRIGGERS != 0 {
        put_vec(sink, s.triggers);
    }
    if f & F_POINTER != 0 {
        match s.pointer {
            Some(p) => {
                sink.put(&[1]);
                put_vec(sink, p);
            }
            None => sink.put(&[0]),
        }
        sink.put(&[s.pointer_screen]);
    }
    if f & F_POINTER_BITS != 0 {
        sink.put(&[s.pointer_bits()]);
    }
    if f & F_WHEEL != 0 {
        sink.put(&s.wheel.to_bits().to_le_bytes());
    }
    if f & F_KEYS != 0 {
        sink.put(&s.keys.to_bytes());
    }
    if f & F_PAD != 0 {
        sink.put(&[s.pad as u8]);
    }
}

/// Writes a recording: [`begin`](Self::begin) puts the header, then one
/// [`frame`](Self::frame) per update. Delta-coded against the previous
/// frame, so it needs no allocation and a few bytes per frame.
#[derive(Clone, Debug)]
pub struct Recorder {
    header: Header,
    prev: PlayersInput,
    prev_dt: u32,
    frames: u32,
    bytes: usize,
}

impl Recorder {
    /// Write `header` (its `frames` as given; patch it with
    /// [`finish_header`](Self::finish_header) when the sink can seek).
    pub fn begin(header: Header, sink: &mut dyn Sink) -> Recorder {
        sink.put(&header.to_bytes());
        Recorder {
            header,
            prev: PlayersInput::NONE,
            // A dt no game uses, so the first frame always writes its own.
            prev_dt: u32::MAX,
            frames: 0,
            bytes: HEADER_LEN,
        }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Frames written so far.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Bytes written so far (header included).
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Is a hash due after the next frame?
    pub fn hash_due(&self) -> bool {
        let n = self.header.hash_every as u32;
        n != 0 && (self.frames + 1).is_multiple_of(n)
    }

    /// One frame: what the game was given (`players`, `dt`) and, when
    /// [`hash_due`](Self::hash_due), its state hash after the update.
    pub fn frame(
        &mut self,
        sink: &mut dyn Sink,
        players: &PlayersInput,
        dt: f32,
        hash: Option<u64>,
    ) {
        let mut out = Counted { sink, n: 0 };
        let dt_bits = dt.to_bits();
        let mut head = 0u8;
        if dt_bits != self.prev_dt {
            head |= H_DT;
        }
        if hash.is_some() {
            head |= H_HASH;
        }
        if players.connected != self.prev.connected {
            head |= H_CONNECTED;
        }
        let n = self.header.players as usize;
        let mut fields = [0u16; MAX_PLAYERS];
        for (p, f) in fields.iter_mut().enumerate().take(n) {
            *f = changed_fields(&self.prev.players[p], &players.players[p]);
            if *f != 0 {
                head |= 1 << (4 + p);
            }
        }
        out.put(&[head]);
        if head & H_DT != 0 {
            out.put(&dt_bits.to_le_bytes());
        }
        if head & H_CONNECTED != 0 {
            out.put(&[players.connected]);
        }
        for (p, f) in fields.iter().enumerate().take(n) {
            if *f != 0 {
                put_fields(&mut out, &players.players[p], *f);
            }
        }
        if let Some(h) = hash {
            out.put(&h.to_le_bytes());
        }
        self.bytes += out.n;
        self.prev_dt = dt_bits;
        for p in 0..n {
            self.prev.players[p] = players.players[p];
        }
        self.prev.connected = players.connected;
        self.frames += 1;
    }

    /// The header with the final frame count (write it over the first
    /// [`HEADER_LEN`] bytes).
    pub fn finish_header(&self) -> [u8; HEADER_LEN] {
        Header {
            frames: self.frames,
            flags: self.header.flags | FLAG_FINISHED,
            ..self.header
        }
        .to_bytes()
    }
}

/// A [`Sink`] that counts what passes through.
struct Counted<'a> {
    sink: &'a mut dyn Sink,
    n: usize,
}

impl Sink for Counted<'_> {
    fn put(&mut self, bytes: &[u8]) {
        self.n += bytes.len();
        self.sink.put(bytes);
    }
}

/// One decoded frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Frame {
    /// Its index in the recording (0 = the first recorded frame).
    pub index: u32,
    pub dt: f32,
    pub players: PlayersInput,
    /// The game's state hash after this frame, when recorded.
    pub hash: Option<u64>,
}

/// Where a [`Reader`] is, apart from its bytes: a reader can be dropped
/// between frames and resumed ([`Reader::resume`]) when the bytes are only
/// borrowed for a moment (a blob in an asset group, borrowed from the
/// platform the game's update also needs).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cursor {
    header: Header,
    at: usize,
    cur: PlayersInput,
    dt: f32,
    index: u32,
}

/// Decodes a recording frame by frame.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    header: Header,
    bytes: &'a [u8],
    at: usize,
    cur: PlayersInput,
    dt: f32,
    index: u32,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Reader<'a>, ReplayError> {
        let header = Header::parse(bytes)?;
        Ok(Reader {
            header,
            bytes,
            at: HEADER_LEN,
            cur: PlayersInput::NONE,
            dt: 0.0,
            index: 0,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Frames decoded so far.
    pub fn position(&self) -> u32 {
        self.index
    }

    /// Where this reader is (to [`resume`](Self::resume) it later over the
    /// same bytes).
    pub fn cursor(&self) -> Cursor {
        Cursor {
            header: self.header,
            at: self.at,
            cur: self.cur,
            dt: self.dt,
            index: self.index,
        }
    }

    /// A reader over `bytes` (the same recording) at `cursor`.
    pub fn resume(bytes: &'a [u8], cursor: Cursor) -> Reader<'a> {
        Reader {
            header: cursor.header,
            bytes,
            at: cursor.at,
            cur: cursor.cur,
            dt: cursor.dt,
            index: cursor.index,
        }
    }

    /// Back to the first frame.
    pub fn rewind(&mut self) {
        self.at = HEADER_LEN;
        self.cur = PlayersInput::NONE;
        self.dt = 0.0;
        self.index = 0;
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ReplayError> {
        let b = self
            .bytes
            .get(self.at..self.at + N)
            .ok_or(ReplayError::Truncated)?;
        self.at += N;
        let mut out = [0u8; N];
        out.copy_from_slice(b);
        Ok(out)
    }

    fn vec(&mut self) -> Result<Vec2, ReplayError> {
        let x = f32::from_bits(u32::from_le_bytes(self.take::<4>()?));
        let y = f32::from_bits(u32::from_le_bytes(self.take::<4>()?));
        Ok(Vec2::new(x, y))
    }

    // Out of line: called per player (the N64 keeps code small).
    #[inline(never)]
    fn fields(&mut self, s: &mut InputState) -> Result<(), ReplayError> {
        let f = u16::from_le_bytes(self.take::<2>()?);
        if f & !F_ALL != 0 {
            return Err(ReplayError::Corrupt);
        }
        if f & F_BUTTONS != 0 {
            s.set_button_bits(u32::from_le_bytes(self.take::<4>()?));
        }
        if f & F_STICK != 0 {
            s.stick = self.vec()?;
        }
        if f & F_RIGHT != 0 {
            s.right_stick = self.vec()?;
        }
        if f & F_TRIGGERS != 0 {
            s.triggers = self.vec()?;
        }
        if f & F_POINTER != 0 {
            let [tag] = self.take::<1>()?;
            s.pointer = match tag {
                0 => None,
                1 => Some(self.vec()?),
                _ => return Err(ReplayError::Corrupt),
            };
            s.pointer_screen = self.take::<1>()?[0];
        }
        if f & F_POINTER_BITS != 0 {
            s.set_pointer_bits(self.take::<1>()?[0]);
        }
        if f & F_WHEEL != 0 {
            s.wheel = f32::from_bits(u32::from_le_bytes(self.take::<4>()?));
        }
        if f & F_KEYS != 0 {
            s.keys = KeySet::from_bytes(self.take::<16>()?);
        }
        if f & F_PAD != 0 {
            s.pad = self.take::<1>()?[0] != 0;
        }
        Ok(())
    }

    /// The next frame; `None` at the end (the header's count when it is
    /// final, else the end of the bytes).
    pub fn next_frame(&mut self) -> Option<Result<Frame, ReplayError>> {
        if self.header.finished() && self.index >= self.header.frames {
            return None;
        }
        if self.at >= self.bytes.len() {
            return if self.header.finished() {
                Some(Err(ReplayError::Truncated))
            } else {
                None
            };
        }
        Some(self.decode())
    }

    fn decode(&mut self) -> Result<Frame, ReplayError> {
        let [head] = self.take::<1>()?;
        if head & 0b1000 != 0 || (head >> 4) >> self.header.players != 0 {
            return Err(ReplayError::Corrupt);
        }
        if head & H_DT != 0 {
            self.dt = f32::from_bits(u32::from_le_bytes(self.take::<4>()?));
        }
        if head & H_CONNECTED != 0 {
            self.cur.connected = self.take::<1>()?[0];
        }
        for p in 0..self.header.players as usize {
            if head & (1 << (4 + p)) != 0 {
                let mut s = self.cur.players[p];
                self.fields(&mut s)?;
                self.cur.players[p] = s;
            }
        }
        let hash = if head & H_HASH != 0 {
            Some(u64::from_le_bytes(self.take::<8>()?))
        } else {
            None
        };
        let frame = Frame {
            index: self.index,
            dt: self.dt,
            players: self.cur,
            hash,
        };
        self.index += 1;
        Ok(frame)
    }
}

/// How a replayed frame compared with the recording.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    /// The recording has no hash for this frame.
    Unchecked,
    /// The game's hash equals the recording's.
    Match,
    /// It doesn't: the run diverged on this frame.
    Diverged { expected: u64, got: u64 },
    /// The recording has a hash and the game hashes nothing
    /// ([`Game::state_hash`](crate::Game::state_hash) is `None`).
    NoHash,
}

/// Where a replay first stopped matching.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Divergence {
    /// Frames run when the state first differed: 0 is the start state, 1
    /// the state after the first frame (`diverged at frame 1234`).
    pub frame: u32,
    pub expected: u64,
    pub got: u64,
}

/// Drives a game with a recording: [`start`](Self::start) checks the start
/// state, each [`step`](Self::step) feeds one frame and checks its hash.
#[derive(Clone, Debug)]
pub struct Replay<'a> {
    reader: Reader<'a>,
    /// The first divergence (later frames keep running).
    pub diverged: Option<Divergence>,
    /// Hashes compared so far, and the last one the game gave.
    pub checked: u32,
    pub last_hash: Option<u64>,
}

impl<'a> Replay<'a> {
    /// A replay of `bytes` for `game` (its name: [`Header::game`] must
    /// match).
    pub fn new(bytes: &'a [u8], game: &str) -> Result<Replay<'a>, ReplayError> {
        let reader = Reader::new(bytes)?;
        if reader.header().game != crate::asset_id(game) {
            return Err(ReplayError::WrongGame);
        }
        Ok(Replay {
            reader,
            diverged: None,
            checked: 0,
            last_hash: None,
        })
    }

    /// A replay of `bytes` whatever game recorded it (a console host that
    /// doesn't know its game's name; the hashes still decide).
    pub fn new_any(bytes: &'a [u8]) -> Result<Replay<'a>, ReplayError> {
        Ok(Replay {
            reader: Reader::new(bytes)?,
            diverged: None,
            checked: 0,
            last_hash: None,
        })
    }

    pub fn header(&self) -> &Header {
        self.reader.header()
    }

    /// Frames fed so far.
    pub fn position(&self) -> u32 {
        self.reader.position()
    }

    /// Compare the fresh game's state with the recording's start hash
    /// (nothing to compare when either has none).
    pub fn start(&mut self, game: &dyn crate::Game) -> Check {
        let want = self.reader.header().start_hash;
        match (want, game.state_hash()) {
            (0, _) | (_, None) => Check::Unchecked,
            (expected, Some(got)) if expected == got => Check::Match,
            (expected, Some(got)) => {
                self.diverged.get_or_insert(Divergence {
                    frame: 0,
                    expected,
                    got,
                });
                Check::Diverged { expected, got }
            }
        }
    }

    /// Decode the next frame without running it (`None` at the end).
    pub fn next_frame(&mut self) -> Option<Result<Frame, ReplayError>> {
        self.reader.next_frame()
    }

    /// Compare `game`'s state after `frame` with the recording.
    pub fn check(&mut self, frame: &Frame, game: &dyn crate::Game) -> Check {
        let Some(expected) = frame.hash else {
            return Check::Unchecked;
        };
        let Some(got) = game.state_hash() else {
            return Check::NoHash;
        };
        self.checked += 1;
        self.last_hash = Some(got);
        if got == expected {
            Check::Match
        } else {
            self.diverged.get_or_insert(Divergence {
                frame: frame.index + 1,
                expected,
                got,
            });
            Check::Diverged { expected, got }
        }
    }

    /// One frame: publish its players to `platform` (when it hosts
    /// players), run `game.update_with` with player 0 and the recorded
    /// `dt`, then check the hash. `None` at the end of the recording.
    pub fn step(
        &mut self,
        game: &mut dyn crate::Game,
        platform: &mut dyn crate::Platform,
        audio: &mut dyn crate::Audio,
    ) -> Option<Result<(Frame, Check), ReplayError>> {
        let frame = match self.reader.next_frame()? {
            Ok(f) => f,
            Err(e) => return Some(Err(e)),
        };
        if let Some(h) = platform.hosted_players_mut() {
            h.publish_players(&frame.players);
        }
        game.update_with(platform, &frame.players.players[0], audio, frame.dt);
        let check = self.check(&frame, game);
        Some(Ok((frame, check)))
    }
}

/// One run's input as a host drives it: live, recorded or replayed. The
/// PC's and the Web's hosts wrap it (files, `localStorage`, the console
/// lines); each frame they ask [`input`](Self::input) what to feed the
/// update, then hand the game to [`after`](Self::after).
#[derive(Clone, Debug)]
pub struct Session<'a> {
    mode: SessionMode<'a>,
}

// A session is one per host, never in a hot array: the variants' sizes
// don't matter (and boxing needs `alloc`, which the core hasn't).
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
enum SessionMode<'a> {
    Live,
    Record {
        rec: Recorder,
        hashes: u32,
    },
    Replay {
        replay: Replay<'a>,
        until: Option<u32>,
        pending: Option<Frame>,
        ended: bool,
    },
}

impl<'a> Session<'a> {
    /// Live input, nothing recorded.
    pub const fn live() -> Session<'a> {
        Session {
            mode: SessionMode::Live,
        }
    }

    /// Record into `sink` (the header goes there now; write
    /// [`start`](Self::start)'s and [`finish_header`](Self::finish_header)'s
    /// headers over its first [`HEADER_LEN`] bytes when the sink can).
    pub fn record(header: Header, sink: &mut dyn Sink) -> Session<'a> {
        Session {
            mode: SessionMode::Record {
                rec: Recorder::begin(header, sink),
                hashes: 0,
            },
        }
    }

    /// Replay `bytes`, a recording of `game`, stopping after `until`
    /// frames when given.
    pub fn replay(
        bytes: &'a [u8],
        game: &str,
        until: Option<u32>,
    ) -> Result<Session<'a>, ReplayError> {
        Ok(Session {
            mode: SessionMode::Replay {
                replay: Replay::new(bytes, game)?,
                until,
                pending: None,
                ended: false,
            },
        })
    }

    /// [`replay`](Self::replay) without the game check ([`Replay::new_any`]).
    pub fn replay_any(bytes: &'a [u8], until: Option<u32>) -> Result<Session<'a>, ReplayError> {
        Ok(Session {
            mode: SessionMode::Replay {
                replay: Replay::new_any(bytes)?,
                until,
                pending: None,
                ended: false,
            },
        })
    }

    pub fn is_replay(&self) -> bool {
        matches!(self.mode, SessionMode::Replay { .. })
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.mode, SessionMode::Record { .. })
    }

    /// The game before its first frame. Recording: the header with its
    /// start hash (write it over the sink's first bytes). Replay: the start
    /// state is compared (a different start is `diverged at frame 0`).
    pub fn start(&mut self, game: &dyn crate::Game) -> Option<[u8; HEADER_LEN]> {
        match &mut self.mode {
            SessionMode::Live => None,
            SessionMode::Record { rec, .. } => {
                rec.header.start_hash = game.state_hash().unwrap_or(0);
                Some(rec.header.to_bytes())
            }
            SessionMode::Replay { replay, .. } => {
                replay.start(game);
                None
            }
        }
    }

    /// The input to feed this frame: `live` and `dt` (live, recording), or
    /// the recording's (replay; `None` once it ended or reached `until`,
    /// or on a broken frame).
    pub fn input(&mut self, live: &PlayersInput, dt: f32) -> Option<(PlayersInput, f32)> {
        match &mut self.mode {
            SessionMode::Live | SessionMode::Record { .. } => Some((*live, dt)),
            SessionMode::Replay {
                replay,
                until,
                pending,
                ended,
            } => {
                if *ended || until.is_some_and(|u| replay.position() >= u) {
                    *ended = true;
                    return None;
                }
                match replay.next_frame() {
                    Some(Ok(f)) => {
                        *pending = Some(f);
                        Some((f.players, f.dt))
                    }
                    _ => {
                        *ended = true;
                        None
                    }
                }
            }
        }
    }

    /// After the update with `players` and `dt`: record the frame into
    /// `sink` (with the game's hash when due), or check the replayed one.
    /// Returns the replay's first divergence when this frame found it.
    pub fn after(
        &mut self,
        game: &dyn crate::Game,
        players: &PlayersInput,
        dt: f32,
        sink: &mut dyn Sink,
    ) -> Option<Divergence> {
        match &mut self.mode {
            SessionMode::Live => None,
            SessionMode::Record { rec, hashes } => {
                let hash = if rec.hash_due() {
                    game.state_hash()
                } else {
                    None
                };
                *hashes += hash.is_some() as u32;
                rec.frame(sink, players, dt, hash);
                None
            }
            SessionMode::Replay {
                replay, pending, ..
            } => {
                let f = pending.take()?;
                let first = replay.diverged.is_none();
                match replay.check(&f, game) {
                    Check::Diverged { .. } if first => replay.diverged,
                    _ => None,
                }
            }
        }
    }

    /// Has the replay run out?
    pub fn ended(&self) -> bool {
        matches!(self.mode, SessionMode::Replay { ended: true, .. })
    }

    /// Frames recorded or replayed so far.
    pub fn frames(&self) -> u32 {
        match &self.mode {
            SessionMode::Live => 0,
            SessionMode::Record { rec, .. } => rec.frames(),
            SessionMode::Replay { replay, .. } => replay.position(),
        }
    }

    /// Hashes recorded (recording) or compared (replay).
    pub fn hashes(&self) -> u32 {
        match &self.mode {
            SessionMode::Live => 0,
            SessionMode::Record { hashes, .. } => *hashes,
            SessionMode::Replay { replay, .. } => replay.checked,
        }
    }

    /// The recording's final header (frame count, finished flag).
    pub fn finish_header(&self) -> Option<[u8; HEADER_LEN]> {
        match &self.mode {
            SessionMode::Record { rec, .. } => Some(rec.finish_header()),
            _ => None,
        }
    }

    /// The replay, for its verdict.
    pub fn replay_state(&self) -> Option<&Replay<'a>> {
        match &self.mode {
            SessionMode::Replay { replay, .. } => Some(replay),
            _ => None,
        }
    }

    /// The replay's verdict line: `TRINO_REPLAY: ok frames=… checked=…
    /// hash=… on <platform>` or `TRINO_REPLAY: diverged at frame … (expected
    /// …, got …) on <platform>` (`cargo xtask replay` reads it).
    pub fn verdict(&self, platform: &str, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        let Some(replay) = self.replay_state() else {
            return Ok(());
        };
        match replay.diverged {
            Some(d) => write!(
                out,
                "TRINO_REPLAY: diverged at frame {} (expected {:#018x}, got {:#018x}) on {platform}",
                d.frame, d.expected, d.got
            ),
            None => write!(
                out,
                "TRINO_REPLAY: ok frames={} checked={} hash={:#018x} on {platform}",
                replay.position(),
                replay.checked,
                replay.last_hash.unwrap_or(0)
            ),
        }
    }
}

/// The platform a portable replay runs against (issue #67): the host's
/// clock, log, blobs, asset groups and players, and nothing the player's
/// machine keeps — no save, the default accessibility and quality, no
/// language, no services. A game that reads those while it plays would
/// replay differently wherever they differ (an old save on one SD card);
/// a recording made through `Neutral` replays the same everywhere.
pub struct Neutral<'a>(pub &'a mut dyn crate::Platform);

impl crate::Platform for Neutral<'_> {
    fn now_micros(&self) -> u64 {
        self.0.now_micros()
    }
    fn log(&self, message: &str) {
        self.0.log(message)
    }
    fn blob(&self, id: crate::BlobId) -> Option<&'static [u8]> {
        self.0.blob(id)
    }
    fn group_blob(&self, id: crate::BlobId) -> Option<&[u8]> {
        self.0.group_blob(id)
    }
    fn load_group(&mut self, group: crate::GroupId) -> crate::LoadState {
        self.0.load_group(group)
    }
    fn unload_group(&mut self, group: crate::GroupId) {
        self.0.unload_group(group)
    }
    fn group_state(&self, group: crate::GroupId) -> crate::LoadState {
        self.0.group_state(group)
    }
    fn hw_tier(&self) -> crate::HwTier {
        self.0.hw_tier()
    }
    fn hosted_players(&self) -> Option<&crate::HostedPlayers> {
        self.0.hosted_players()
    }
    fn hosted_players_mut(&mut self) -> Option<&mut crate::HostedPlayers> {
        self.0.hosted_players_mut()
    }
}

/// FNV-1a 64 over fixed-width little-endian values: a state hash that is the
/// same on every target (the N64 is big-endian, `usize` is 32 bits on the
/// consoles: [`usize`](Self::usize) widens it to 64). Floats go through
/// [`f32`](Self::f32) (their bits).
///
/// It deliberately does not implement `core::hash::Hasher`: `#[derive(Hash)]`
/// hashes an integer array or slice as its raw memory (`Hash::hash_slice`),
/// in the CPU's byte order, so a derived hash of `[u16; 3]` differs between
/// the PC and the N64. Hash each field by hand.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StateHasher(u64);

impl Default for StateHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StateHasher {
    pub const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    // The write methods stay out of line: a game hashes dozens of fields,
    // and inlined they cost the N64 kilobytes of RDRAM (issue #67).
    pub const fn new() -> StateHasher {
        StateHasher(Self::OFFSET)
    }

    #[inline(never)]
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
        self
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes(&[v])
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    #[inline(never)]
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    #[inline(never)]
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }

    /// An index or a count, as 64 bits on every target.
    pub fn usize(&mut self, v: usize) -> &mut Self {
        self.u64(v as u64)
    }

    /// A float's bits. With the `strict-determinism` feature a NaN or a
    /// subnormal panics: their bits (NaN payloads) and their arithmetic
    /// (the N64 flushes subnormals) differ between CPUs.
    #[inline(never)]
    pub fn f32(&mut self, v: f32) -> &mut Self {
        crate::determinism::check_f32(v);
        self.u32(v.to_bits())
    }

    pub fn vec2(&mut self, v: Vec2) -> &mut Self {
        self.f32(v.x).f32(v.y)
    }

    pub fn vec3(&mut self, v: crate::Vec3) -> &mut Self {
        self.f32(v.x).f32(v.y).f32(v.z)
    }

    pub const fn value(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Button, Key};

    fn session(frames: u32) -> std::vec::Vec<(PlayersInput, f32)> {
        (0..frames)
            .map(|f| {
                let mut p = PlayersInput::single(InputState::IDLE);
                let s = &mut p.players[0];
                if f % 7 < 3 {
                    s.set(Button::A, true);
                }
                s.stick = Vec2::new((f as f32 * 0.37).sin_approx(), -0.25);
                if f == 5 {
                    s.pointer = Some(Vec2::new(10.5, 20.25));
                    s.pointer_screen = 1;
                    s.set_key(Key::Space, true);
                    s.wheel = 1.5;
                }
                if f >= 10 {
                    p.connected = 0b11;
                    p.players[1].set(Button::B, f % 2 == 0);
                    p.players[1].triggers = Vec2::new(0.5, 1.0);
                    p.players[1].pad = true;
                }
                // The PC's wall-clock dt: a new value most frames.
                let dt = 1.0 / 60.0 + (f % 3) as f32 * 1e-4;
                (p, dt)
            })
            .collect()
    }

    trait SinApprox {
        fn sin_approx(self) -> f32;
    }
    impl SinApprox for f32 {
        fn sin_approx(self) -> f32 {
            crate::math3d::sin(self)
        }
    }

    fn record(s: &[(PlayersInput, f32)], hash_every: u16) -> std::vec::Vec<u8> {
        let mut out = std::vec::Vec::new();
        let header = Header::new("test", (320, 240), 2, hash_every);
        let mut sink = |b: &[u8]| out.extend_from_slice(b);
        let mut rec = Recorder::begin(header, &mut sink);
        for (i, (p, dt)) in s.iter().enumerate() {
            let h = rec.hash_due().then_some(i as u64 * 31);
            rec.frame(&mut sink, p, *dt, h);
        }
        let head = rec.finish_header();
        out[..HEADER_LEN].copy_from_slice(&head);
        out
    }

    #[test]
    fn a_recording_decodes_to_exactly_what_was_recorded() {
        let s = session(40);
        let bytes = record(&s, 4);
        let mut r = Reader::new(&bytes).unwrap();
        assert_eq!(r.header().frames, 40);
        assert!(r.header().finished());
        assert_eq!(r.header().game, crate::asset_id("test"));
        for (i, (p, dt)) in s.iter().enumerate() {
            let f = r.next_frame().unwrap().unwrap();
            assert_eq!(f.index, i as u32);
            assert_eq!(f.dt.to_bits(), dt.to_bits());
            assert_eq!(f.players.connected, p.connected);
            assert_eq!(f.players.players[0], p.players[0], "frame {i}");
            assert_eq!(f.players.players[1], p.players[1], "frame {i}");
            let want = ((i + 1) % 4 == 0).then_some(i as u64 * 31);
            assert_eq!(f.hash, want);
        }
        assert!(r.next_frame().is_none());
        r.rewind();
        assert_eq!(r.next_frame().unwrap().unwrap().index, 0);
        // Dropped and resumed between frames: the same frames.
        let mut cursor = Reader::new(&bytes).unwrap().cursor();
        for (i, (p, _)) in s.iter().enumerate() {
            let mut r = Reader::resume(&bytes, cursor);
            let f = r.next_frame().unwrap().unwrap();
            assert_eq!((f.index, f.players), (i as u32, *p));
            cursor = r.cursor();
        }
    }

    #[test]
    fn a_recording_survives_the_debug_channel() {
        let bytes = record(&session(40), 3);
        let mut lines = std::vec::Vec::new();
        let mut sink = LineSink::new(|l: &str| lines.push(std::string::String::from(l)));
        sink.put(&bytes[..10]);
        sink.put(&bytes[10..]);
        sink.end();
        assert_eq!(lines.last().map(|l| l.as_str()), Some("TRINO_REC_END\n"));
        assert!(lines.iter().all(|l| l.len() <= 10 + 128 + 1));
        let mut back = std::vec::Vec::new();
        for l in &lines {
            // An emulator's prefix in front of the channel's text.
            decode_line(&std::format!("[ares] {l}"), &mut |b: &[u8]| {
                back.extend_from_slice(b)
            });
        }
        assert_eq!(back, bytes);
        assert_eq!(decode_line("TRINO_STEP:x", &mut |_: &[u8]| {}), None);
        assert_eq!(decode_line("TRINO_REC:abc", &mut |_: &[u8]| {}), None);
    }

    #[test]
    fn an_idle_frame_is_one_byte() {
        let idle = (PlayersInput::single(InputState::IDLE), 1.0 / 60.0);
        let bytes = record(&[idle; 100], 0);
        // The first frame writes its dt and the connected mask; the rest
        // are a bare head byte each.
        assert_eq!(bytes.len(), HEADER_LEN + (1 + 4 + 1) + 99);
    }

    #[test]
    fn a_truncated_or_corrupt_recording_is_an_error_not_a_panic() {
        let bytes = record(&session(20), 1);
        for cut in HEADER_LEN..bytes.len() {
            let mut r = Reader::new(&bytes[..cut]).unwrap();
            let mut n = 0;
            while let Some(f) = r.next_frame() {
                if f.is_err() {
                    break;
                }
                n += 1;
            }
            assert!(n < 20, "cut at {cut} still read every frame");
        }
        assert_eq!(Reader::new(b"nope").unwrap_err(), ReplayError::Truncated);
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(Reader::new(&bad).unwrap_err(), ReplayError::NotARecording);
        let mut bad = bytes.clone();
        bad[HEADER_LEN] = 0xff; // a third player in a 2-player recording
        assert_eq!(
            Reader::new(&bad).unwrap().next_frame(),
            Some(Err(ReplayError::Corrupt))
        );
    }

    #[test]
    fn an_unfinished_recording_reads_to_its_end() {
        let s = session(12);
        let mut bytes = record(&s, 0);
        let h = Header::new("test", (320, 240), 2, 0);
        bytes[..HEADER_LEN].copy_from_slice(&h.to_bytes());
        let mut r = Reader::new(&bytes).unwrap();
        let mut n = 0;
        while let Some(f) = r.next_frame() {
            f.unwrap();
            n += 1;
        }
        assert_eq!(n, 12);
    }

    #[test]
    fn the_state_hasher_is_fixed_width_little_endian() {
        // Pinned values: a change here changes every recording's hashes.
        let mut h = StateHasher::new();
        h.u32(0x0102_0304).f32(1.5).usize(7);
        assert_eq!(h.value(), {
            let mut r = StateHasher::new();
            r.bytes(&[4, 3, 2, 1])
                .bytes(&1.5f32.to_bits().to_le_bytes())
                .bytes(&7u64.to_le_bytes());
            r.value()
        });
        // FNV-1a 64's published value for "a".
        assert_eq!(
            StateHasher::new().bytes(b"a").value(),
            0xaf63_dc4c_8601_ec8c
        );
    }

    /// A game whose state is a sum of its inputs.
    struct Summer {
        sum: u64,
        dt: f32,
    }

    impl crate::Game for Summer {
        fn update(&mut self, input: &InputState, _: &mut dyn crate::Audio, dt: f32) {
            self.sum = self
                .sum
                .wrapping_mul(31)
                .wrapping_add(input.button_bits() as u64 + input.stick.x.to_bits() as u64);
            self.dt += dt;
        }
        fn render(&mut self, _: &mut dyn crate::Renderer) {}
        fn state_hash(&self) -> Option<u64> {
            Some(*StateHasher::new().u64(self.sum).f32(self.dt)).map(|h| h.value())
        }
    }

    struct Fake {
        hosted: crate::HostedPlayers,
    }
    impl crate::Platform for Fake {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn hosted_players(&self) -> Option<&crate::HostedPlayers> {
            Some(&self.hosted)
        }
        fn hosted_players_mut(&mut self) -> Option<&mut crate::HostedPlayers> {
            Some(&mut self.hosted)
        }
    }
    struct Mute;
    impl crate::Audio for Mute {
        fn play_sound(&mut self, _: crate::SoundId) {}
        fn play_music(&mut self, _: crate::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    fn recorded_game(s: &[(PlayersInput, f32)]) -> std::vec::Vec<u8> {
        let mut out = std::vec::Vec::new();
        let mut g = Summer { sum: 0, dt: 0.0 };
        let mut header = Header::new("summer", (320, 240), 2, 1);
        header.start_hash = crate::Game::state_hash(&g).unwrap();
        let mut sink = |b: &[u8]| out.extend_from_slice(b);
        let mut rec = Recorder::begin(header, &mut sink);
        for (p, dt) in s {
            crate::Game::update(&mut g, &p.players[0], &mut Mute, *dt);
            let h = rec.hash_due().then(|| crate::Game::state_hash(&g).unwrap());
            rec.frame(&mut sink, p, *dt, h);
        }
        let head = rec.finish_header();
        out[..HEADER_LEN].copy_from_slice(&head);
        out
    }

    #[test]
    fn a_replay_matches_its_recording_and_names_the_frame_it_diverges_on() {
        let s = session(30);
        let bytes = recorded_game(&s);
        let mut fake = Fake {
            hosted: crate::HostedPlayers::new(),
        };
        // The same game: every hash matches.
        let mut g = Summer { sum: 0, dt: 0.0 };
        let mut replay = Replay::new(&bytes, "summer").unwrap();
        assert_eq!(replay.start(&g), Check::Match);
        let mut n = 0;
        while let Some(r) = replay.step(&mut g, &mut fake, &mut Mute) {
            let (frame, check) = r.unwrap();
            assert_eq!(check, Check::Match, "frame {}", frame.index);
            assert_eq!(crate::Platform::players(&fake), &frame.players);
            n += 1;
        }
        assert_eq!((n, replay.checked, replay.diverged), (30, 30, None));
        // Disturbed after frame 17 (index): the state after 19 frames is
        // the first that the next update can't hide, and the replay keeps
        // going.
        let mut g = Summer { sum: 0, dt: 0.0 };
        let mut replay = Replay::new(&bytes, "summer").unwrap();
        while let Some(r) = replay.step(&mut g, &mut fake, &mut Mute) {
            let (frame, _) = r.unwrap();
            if frame.index == 17 {
                g.sum ^= 1;
            }
        }
        let d = replay.diverged.unwrap();
        assert_eq!(d.frame, 19);
        assert_ne!(d.expected, d.got);
        // Another game's recording, another start.
        assert_eq!(
            Replay::new(&bytes, "castle64").unwrap_err(),
            ReplayError::WrongGame
        );
        let g = Summer { sum: 9, dt: 0.0 };
        let mut replay = Replay::new(&bytes, "summer").unwrap();
        assert!(matches!(replay.start(&g), Check::Diverged { .. }));
        assert_eq!(replay.diverged.unwrap().frame, 0);
    }
}
