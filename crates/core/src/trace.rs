//! Timeline tracing: zones, counters and frame marks on every target, in one
//! format (issue #62, ADR 0030).
//!
//! [`crate::perf`] sums a run into per-stage and per-model totals; a trace
//! keeps **when** each thing happened, so a viewer shows which stage stalled,
//! frame-to-frame variance and spikes (an asset load, a GC-like allocation).
//!
//! - **Recording.** [`zone!`](crate::zone) times the rest of its block,
//!   [`counter`] samples a value, [`frame_mark`] starts a frame. They write
//!   into a [`TraceRing`] installed by the host ([`install`]), timestamped by
//!   the host's microsecond clock (the N64's CPU counter, `svcGetSystemTick`
//!   on the 3DS, `Instant` on the PC, `performance.now()` on the Web).
//! - **Compiled out by default.** Without the crate feature `trace`, the
//!   three expand to nothing ([`Zone`] is zero-sized and has no `Drop`): a
//!   release build is the same machine code with or without them. The types,
//!   the encoder and the decoder are always there (the linker drops them when
//!   unused), so the self-test checks the format on every target and the
//!   host tools read traces with this very code.
//! - **Export.** [`TraceRing::write_lines`] encodes a [`TTRC`](MAGIC) packet
//!   and prints it as `TRINO_TRACE:<session> <seq> <check> <base64>` lines
//!   over the debug channel each target already has (ISViewer/USB on the
//!   N64, `svcOutputDebugString`/3dslink on the 3DS, stdout on the PC, the
//!   console on the Web). `cargo xtask trace collect` turns them into Chrome
//!   Trace Event / Perfetto JSON (`trino-trace`).
//!
//! No allocation, fixed capacity: a full ring overwrites its oldest events
//! and counts them ([`TraceRing::dropped`]).
//!
//! # Packet (`TTRC` v1, little-endian, byte-packed)
//!
//! ```text
//! "TTRC"  u8 version (1)  u8 platform (TracePlatform)  u16 flags (0)
//! u16 name_base  u16 name_count  u32 reserved (0)
//! { u8 len, len bytes of UTF-8 } × name_count
//! u32 dropped (events overwritten or refused since the previous packet)
//! u32 event_count  { u8 kind, u8 name, u8 track, u8 depth, u32 ts, u32 value } × event_count
//! ```
//!
//! Names are sent once: a packet carries the names interned since the
//! previous packet of the same ring (`name_base` = how many came before).
//! `ts` is the host clock in µs (wraps after ~71 minutes); a zone's `value`
//! is its duration, a counter's its value, a frame mark's the frame number.

/// The packet magic.
pub const MAGIC: [u8; 4] = *b"TTRC";
/// The packet format version this crate writes and reads.
pub const VERSION: u8 = 1;
/// Distinct names a ring interns (zones and counters together). A name past
/// it is refused and counted in [`TraceRing::dropped`].
pub const MAX_NAMES: usize = 128;
/// Bytes of packet per `TRINO_TRACE:` line (128 base64 characters).
pub const LINE_BYTES: usize = 96;
/// The line prefix `cargo xtask trace collect` looks for.
pub const LINE_PREFIX: &str = "TRINO_TRACE:";
/// Encoded size of one event.
pub const EVENT_BYTES: usize = 12;

/// What an event is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum EventKind {
    /// A timed span: `ts` its start, `value` its duration (µs).
    Zone = 0,
    /// A sampled value at `ts`.
    Counter = 1,
    /// Frame `value` starts at `ts`.
    Frame = 2,
}

impl EventKind {
    pub fn from_u8(v: u8) -> Option<EventKind> {
        match v {
            0 => Some(EventKind::Zone),
            1 => Some(EventKind::Counter),
            2 => Some(EventKind::Frame),
            _ => None,
        }
    }
}

/// Where an event ran: one timeline row per track in a viewer.
///
/// A track byte is a kind (its low 3 bits: [`CPU`], [`GPU`], ..) and an
/// instance (the high 5 bits, [`on`]): the PC's T&L workers are `worker 1`
/// .. `worker 8`, each a track of its own (issue #130).
pub mod track {
    /// The main thread: the game loop, the renderer's CPU work.
    pub const CPU: u8 = 0;
    /// GPU passes (PC/Web timestamp queries).
    pub const GPU: u8 = 1;
    /// The N64's RSP (work measured from the CPU side).
    pub const RSP: u8 = 2;
    /// An audio thread (the PC's stream worker, the 3DS's audio thread).
    pub const AUDIO: u8 = 3;
    /// Another worker (the 3DS's second core, the PC's T&L workers).
    pub const WORKER: u8 = 4;
    /// The asset-group loader's decode workers (the PC's `GroupLoader`).
    pub const LOADER: u8 = 5;
    /// Instances a kind can have (`0..MAX_INSTANCE`).
    pub const MAX_INSTANCE: u8 = 32;

    /// Instance `n` of `kind` (`n` past [`MAX_INSTANCE`] shares the last).
    pub const fn on(kind: u8, n: u8) -> u8 {
        let n = if n >= MAX_INSTANCE {
            MAX_INSTANCE - 1
        } else {
            n
        };
        (kind & 7) | (n << 3)
    }

    /// A track's kind ([`CPU`] .. [`LOADER`]).
    pub const fn kind(track: u8) -> u8 {
        track & 7
    }

    /// A track's instance (0 for the first, unnumbered one).
    pub const fn instance(track: u8) -> u8 {
        track >> 3
    }

    /// A track kind's display name (`worker` for every worker; see
    /// [`label`] for `worker 3`).
    pub fn name(track: u8) -> &'static str {
        match kind(track) {
            CPU => "cpu",
            GPU => "gpu",
            RSP => "rsp",
            AUDIO => "audio",
            WORKER => "worker",
            LOADER => "loader",
            _ => "other",
        }
    }

    /// A track's display label: its kind's name, and its instance after it
    /// when it has one (`cpu`, `worker`, `worker 3`).
    pub fn label(track: u8) -> Label {
        Label(track)
    }

    /// See [`label`].
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Label(pub u8);

    impl core::fmt::Display for Label {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str(name(self.0))?;
            match instance(self.0) {
                0 => Ok(()),
                n => write!(f, " {n}"),
            }
        }
    }
}

/// The target a trace was recorded on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TracePlatform {
    Pc = 0,
    N64 = 1,
    N3ds = 2,
    Web = 3,
    Unknown = 255,
}

impl TracePlatform {
    pub fn from_u8(v: u8) -> TracePlatform {
        match v {
            0 => TracePlatform::Pc,
            1 => TracePlatform::N64,
            2 => TracePlatform::N3ds,
            3 => TracePlatform::Web,
            _ => TracePlatform::Unknown,
        }
    }

    /// `pc`, `n64`, `3ds`, `web`.
    pub fn name(self) -> &'static str {
        match self {
            TracePlatform::Pc => "pc",
            TracePlatform::N64 => "n64",
            TracePlatform::N3ds => "3ds",
            TracePlatform::Web => "web",
            TracePlatform::Unknown => "unknown",
        }
    }
}

/// One recorded event (12 bytes encoded).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TraceEvent {
    pub kind: u8,
    /// Index into the ring's name table.
    pub name: u8,
    pub track: u8,
    /// Nesting depth of a zone on its track (0 = outermost).
    pub depth: u8,
    pub ts: u32,
    pub value: u32,
}

/// A fixed-capacity event ring and its name table.
pub struct TraceRing<const N: usize> {
    events: [TraceEvent; N],
    /// Next slot written.
    head: usize,
    len: usize,
    dropped: u32,
    names: [&'static str; MAX_NAMES],
    name_count: usize,
    /// Names already written in a packet.
    names_sent: usize,
    /// Open zones per track 0..=7 (their depth).
    depth: [u8; 8],
}

impl<const N: usize> Default for TraceRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TraceRing<N> {
    pub const fn new() -> Self {
        TraceRing {
            events: [TraceEvent {
                kind: 0,
                name: 0,
                track: 0,
                depth: 0,
                ts: 0,
                value: 0,
            }; N],
            head: 0,
            len: 0,
            dropped: 0,
            names: [""; MAX_NAMES],
            name_count: 0,
            names_sent: 0,
            depth: [0; 8],
        }
    }

    /// Events held now.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    /// Events lost since the last packet: overwritten by a full ring,
    /// refused for want of a name slot, or (the global ring) refused while
    /// another thread held it.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Count `n` events lost outside the ring (a busy global ring).
    pub fn add_dropped(&mut self, n: u32) {
        self.dropped = self.dropped.saturating_add(n);
    }

    /// The name table, interned order.
    pub fn names(&self) -> &[&'static str] {
        &self.names[..self.name_count]
    }

    /// `name`'s index, interning it (compared by content: the same literal
    /// in two crates is one name). `None` when the table is full.
    pub fn intern(&mut self, name: &'static str) -> Option<u8> {
        // Pointer first: the common case is the same literal again.
        if let Some(i) = self.names[..self.name_count]
            .iter()
            .position(|n| core::ptr::eq(n.as_ptr(), name.as_ptr()) && n.len() == name.len())
            .or_else(|| {
                self.names[..self.name_count]
                    .iter()
                    .position(|n| *n == name)
            })
        {
            return Some(i as u8);
        }
        if self.name_count == MAX_NAMES {
            return None;
        }
        self.names[self.name_count] = name;
        self.name_count += 1;
        Some((self.name_count - 1) as u8)
    }

    /// Append an event, overwriting the oldest when full.
    pub fn push(&mut self, e: TraceEvent) {
        if N == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.events[self.head] = e;
        self.head = (self.head + 1) % N;
        if self.len == N {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            self.len += 1;
        }
    }

    /// The events held, oldest first.
    pub fn events(&self) -> impl Iterator<Item = &TraceEvent> {
        let start = (self.head + N - self.len) % N.max(1);
        (0..self.len).map(move |i| &self.events[(start + i) % N])
    }

    /// A zone that began at `start` and ended at `end` (µs) on `track`.
    /// Its depth is the track's open zones (see [`Self::open`]).
    pub fn zone(&mut self, name: &'static str, track: u8, start: u32, end: u32) {
        let depth = self.depth[(track & 7) as usize];
        self.zone_at(name, track, depth, start, end.wrapping_sub(start));
    }

    /// A zone with an explicit depth and duration (GPU passes, zones
    /// replayed from another clock).
    pub fn zone_at(&mut self, name: &'static str, track: u8, depth: u8, start: u32, dur: u32) {
        let Some(n) = self.intern(name) else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        self.push(TraceEvent {
            kind: EventKind::Zone as u8,
            name: n,
            track,
            depth,
            ts: start,
            value: dur,
        });
    }

    /// A zone opens on `track` (zones inside it get one more depth).
    pub fn open(&mut self, track: u8) {
        let d = &mut self.depth[(track & 7) as usize];
        *d = d.saturating_add(1);
    }

    /// The innermost open zone on `track` closes.
    pub fn close(&mut self, track: u8) {
        let d = &mut self.depth[(track & 7) as usize];
        *d = d.saturating_sub(1);
    }

    /// A counter sample.
    pub fn counter(&mut self, name: &'static str, track: u8, ts: u32, value: u32) {
        let Some(n) = self.intern(name) else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        self.push(TraceEvent {
            kind: EventKind::Counter as u8,
            name: n,
            track,
            depth: 0,
            ts,
            value,
        });
    }

    /// Frame `frame` starts at `ts`.
    pub fn frame(&mut self, ts: u32, frame: u32) {
        let Some(n) = self.intern("frame") else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        self.push(TraceEvent {
            kind: EventKind::Frame as u8,
            name: n,
            track: track::CPU,
            depth: 0,
            ts,
            value: frame,
        });
    }

    /// Forget the events and the lost count (names stay interned and sent).
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.dropped = 0;
    }

    /// Forget everything, names included: the next packet starts a new
    /// session.
    pub fn reset(&mut self) {
        self.clear();
        self.name_count = 0;
        self.names_sent = 0;
        self.depth = [0; 8];
    }

    /// Bytes [`Self::encode`] writes for the current contents.
    pub fn packet_len(&self) -> usize {
        let names: usize = self.names[self.names_sent..self.name_count]
            .iter()
            .map(|n| 1 + n.len().min(255))
            .sum();
        16 + names + 4 + 4 + self.len * EVENT_BYTES
    }

    /// Encode a packet of the names not sent yet and every event held,
    /// byte by byte into `out`; then clear the ring. The bytes don't depend
    /// on the target's endianness.
    pub fn encode(&mut self, platform: TracePlatform, out: &mut dyn FnMut(u8)) {
        out_all(out, &MAGIC);
        out(VERSION);
        out(platform as u8);
        out_all(out, &0u16.to_le_bytes());
        let base = self.names_sent;
        let count = self.name_count - base;
        out_all(out, &(base as u16).to_le_bytes());
        out_all(out, &(count as u16).to_le_bytes());
        // Reserved: room for a later version's header fields.
        out_all(out, &[0, 0, 0, 0]);
        for name in &self.names[base..self.name_count] {
            let b = &name.as_bytes()[..name.len().min(255)];
            out(b.len() as u8);
            out_all(out, b);
        }
        self.names_sent = self.name_count;
        out_all(out, &self.dropped.to_le_bytes());
        out_all(out, &(self.len as u32).to_le_bytes());
        for e in self.events() {
            out_all(out, &[e.kind, e.name, e.track, e.depth]);
            out_all(out, &e.ts.to_le_bytes());
            out_all(out, &e.value.to_le_bytes());
        }
        self.clear();
    }

    /// Encode a packet ([`Self::encode`]) as `TRINO_TRACE:` lines for
    /// `session` (no spaces), numbered from `*seq` (advanced). Clears the
    /// ring.
    pub fn write_lines(
        &mut self,
        platform: TracePlatform,
        session: &str,
        seq: &mut u32,
        out: &mut dyn core::fmt::Write,
    ) -> core::fmt::Result {
        let mut lines = LineEncoder::new(session, *seq);
        let mut result = Ok(());
        self.encode(platform, &mut |b| {
            if result.is_ok() {
                result = lines.push(b, out);
            }
        });
        result?;
        lines.finish(out)?;
        *seq = lines.seq;
        Ok(())
    }
}

impl<const N: usize> TraceRing<N> {
    /// [`Self::write_lines`] handing each finished line (with its `\n`) to
    /// `out` — a debug channel that takes whole lines (`platform.log`, the
    /// N64's ISViewer). No allocation: one line is buffered at a time.
    pub fn emit_lines(
        &mut self,
        platform: TracePlatform,
        session: &str,
        seq: &mut u32,
        out: &mut dyn FnMut(&str),
    ) -> core::fmt::Result {
        let mut sink = LineSink {
            out,
            buf: [0; LINE_MAX],
            len: 0,
        };
        self.write_lines(platform, session, seq, &mut sink)
    }
}

/// Longest `TRINO_TRACE:` line [`TraceRing::emit_lines`] buffers: the prefix,
/// a session name of up to 40 bytes, the numbers and 128 base64 characters.
pub const LINE_MAX: usize = 200;

struct LineSink<'a> {
    out: &'a mut dyn FnMut(&str),
    buf: [u8; LINE_MAX],
    len: usize,
}

impl core::fmt::Write for LineSink<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len == LINE_MAX {
                return Err(core::fmt::Error);
            }
            self.buf[self.len] = b;
            self.len += 1;
            if b == b'\n' {
                // Only ASCII is written (a session name is the caller's).
                (self.out)(
                    core::str::from_utf8(&self.buf[..self.len]).map_err(|_| core::fmt::Error)?,
                );
                self.len = 0;
            }
        }
        Ok(())
    }
}

/// Print the global ring's events as `TRINO_TRACE:` lines of `session`
/// (numbered from `*seq`) through `out`, and empty it. `false` when nothing
/// is installed.
pub fn flush(session: &str, seq: &mut u32, out: &mut dyn FnMut(&str)) -> bool {
    with_ring(|r, p| r.emit_lines(p, session, seq, out).is_ok()).unwrap_or(false)
}

/// Forget the global ring's events and names: the next [`flush`] starts a
/// self-contained session (its own name table).
pub fn restart() {
    let _ = with_ring(|r, _| r.reset());
}

fn out_all(out: &mut dyn FnMut(u8), bytes: &[u8]) {
    for &b in bytes {
        out(b);
    }
}

/// Splits a byte stream into `TRINO_TRACE:` lines of [`LINE_BYTES`].
pub struct LineEncoder<'s> {
    session: &'s str,
    seq: u32,
    buf: [u8; LINE_BYTES],
    len: usize,
}

impl<'s> LineEncoder<'s> {
    pub fn new(session: &'s str, seq: u32) -> Self {
        LineEncoder {
            session,
            seq,
            buf: [0; LINE_BYTES],
            len: 0,
        }
    }

    /// The next line's sequence number.
    pub fn seq(&self) -> u32 {
        self.seq
    }

    pub fn push(&mut self, b: u8, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        self.buf[self.len] = b;
        self.len += 1;
        if self.len == LINE_BYTES {
            self.flush(out)?;
        }
        Ok(())
    }

    /// Write what is buffered as a last, shorter line.
    pub fn finish(&mut self, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        if self.len > 0 {
            self.flush(out)?;
        }
        Ok(())
    }

    fn flush(&mut self, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        let chunk = &self.buf[..self.len];
        write!(
            out,
            "{LINE_PREFIX}{} {} {:04x} ",
            self.session,
            self.seq,
            fletcher16(chunk)
        )?;
        let mut b64 = [0u8; LINE_BYTES.div_ceil(3) * 4];
        let n = base64_encode(chunk, &mut b64);
        // Base64 output is ASCII.
        out.write_str(core::str::from_utf8(&b64[..n]).unwrap_or(""))?;
        out.write_char('\n')?;
        self.seq = self.seq.wrapping_add(1);
        self.len = 0;
        Ok(())
    }
}

/// Fletcher-16 of `data`: each line's check, so a lost or garbled line is an
/// error in `trace collect` rather than garbage in the timeline.
pub fn fletcher16(data: &[u8]) -> u16 {
    let (mut a, mut b) = (0u16, 0u16);
    for &x in data {
        a = (a + x as u16) % 255;
        b = (b + a) % 255;
    }
    (b << 8) | a
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding; returns the characters written. `out` must
/// hold `data.len().div_ceil(3) * 4`.
pub fn base64_encode(data: &[u8], out: &mut [u8]) -> usize {
    let mut o = 0;
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out[o] = B64[(v >> 18) as usize & 63];
        out[o + 1] = B64[(v >> 12) as usize & 63];
        out[o + 2] = if chunk.len() > 1 {
            B64[(v >> 6) as usize & 63]
        } else {
            b'='
        };
        out[o + 3] = if chunk.len() > 2 {
            B64[v as usize & 63]
        } else {
            b'='
        };
        o += 4;
    }
    o
}

/// Decode standard base64 (padding optional) into `out`; `None` on a bad
/// character or a short `out`. Returns the bytes written.
pub fn base64_decode(text: &str, out: &mut [u8]) -> Option<usize> {
    let mut acc = 0u32;
    let mut bits = 0;
    let mut o = 0;
    for &c in text.trim_end_matches('=').as_bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            *out.get_mut(o)? = (acc >> bits) as u8;
            o += 1;
        }
    }
    Some(o)
}

/// One parsed `TRINO_TRACE:` line (found anywhere in a log line, after an
/// emulator's prefix).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TraceLine<'a> {
    pub session: &'a str,
    pub seq: u32,
    pub check: u16,
    pub data: &'a str,
}

impl<'a> TraceLine<'a> {
    pub fn parse(line: &'a str) -> Option<TraceLine<'a>> {
        let rest = &line[line.find(LINE_PREFIX)? + LINE_PREFIX.len()..];
        let mut parts = rest.split_whitespace();
        let session = parts.next()?;
        let seq = parts.next()?.parse().ok()?;
        let check = u16::from_str_radix(parts.next()?, 16).ok()?;
        let data = parts.next()?;
        Some(TraceLine {
            session,
            seq,
            check,
            data,
        })
    }

    /// The line's bytes into `out` (at least [`LINE_BYTES`]), checked.
    pub fn decode(&self, out: &mut [u8]) -> Result<usize, DecodeError> {
        let n = base64_decode(self.data, out).ok_or(DecodeError::Base64)?;
        if fletcher16(&out[..n]) != self.check {
            return Err(DecodeError::Checksum(self.seq));
        }
        Ok(n)
    }
}

/// Why a trace didn't decode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeError {
    Base64,
    /// Line `seq`'s bytes don't match its check.
    Checksum(u32),
    /// A line is missing before `seq`.
    Gap(u32),
    BadMagic,
    /// A packet of another format version.
    Version(u8),
    Truncated,
    /// A name isn't UTF-8.
    Utf8,
    /// An event names an index past the table.
    UnknownName(u8),
    UnknownKind(u8),
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::Base64 => f.write_str("bad base64"),
            DecodeError::Checksum(s) => write!(f, "line {s}: checksum mismatch"),
            DecodeError::Gap(s) => write!(f, "line(s) missing before {s}"),
            DecodeError::BadMagic => f.write_str("not a TTRC packet"),
            DecodeError::Version(v) => write!(f, "TTRC version {v} (this reader: {VERSION})"),
            DecodeError::Truncated => f.write_str("truncated packet"),
            DecodeError::Utf8 => f.write_str("a name is not UTF-8"),
            DecodeError::UnknownName(n) => write!(f, "event names #{n}, past the table"),
            DecodeError::UnknownKind(k) => write!(f, "unknown event kind {k}"),
        }
    }
}

/// A packet's header, names and events, read in place.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    pub platform: TracePlatform,
    pub name_base: u16,
    pub name_count: u16,
    names: &'a [u8],
    pub dropped: u32,
    pub event_count: u32,
    events: &'a [u8],
}

impl<'a> Packet<'a> {
    /// Parse the packet at the start of `data`; returns it and its length.
    pub fn parse(data: &'a [u8]) -> Result<(Packet<'a>, usize), DecodeError> {
        let get = |r: core::ops::Range<usize>| data.get(r).ok_or(DecodeError::Truncated);
        if get(0..4)? != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let h = get(4..16)?;
        if h[0] != VERSION {
            return Err(DecodeError::Version(h[0]));
        }
        let platform = TracePlatform::from_u8(h[1]);
        let name_base = u16::from_le_bytes([h[4], h[5]]);
        let name_count = u16::from_le_bytes([h[6], h[7]]);
        let mut at = 16;
        let names_start = at;
        for _ in 0..name_count {
            let len = *data.get(at).ok_or(DecodeError::Truncated)? as usize;
            let name = get(at + 1..at + 1 + len)?;
            core::str::from_utf8(name).map_err(|_| DecodeError::Utf8)?;
            at += 1 + len;
        }
        let names = &data[names_start..at];
        let w = get(at..at + 8)?;
        let dropped = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        let event_count = u32::from_le_bytes([w[4], w[5], w[6], w[7]]);
        at += 8;
        let bytes = (event_count as usize)
            .checked_mul(EVENT_BYTES)
            .ok_or(DecodeError::Truncated)?;
        let events = get(at..at + bytes)?;
        for e in events.as_chunks::<EVENT_BYTES>().0 {
            EventKind::from_u8(e[0]).ok_or(DecodeError::UnknownKind(e[0]))?;
        }
        Ok((
            Packet {
                platform,
                name_base,
                name_count,
                names,
                dropped,
                event_count,
                events,
            },
            at + bytes,
        ))
    }

    /// The names this packet adds, in order (indices `name_base..`).
    pub fn names(&self) -> impl Iterator<Item = &'a str> {
        let mut rest = self.names;
        (0..self.name_count).map(move |_| {
            let len = rest[0] as usize;
            let s = core::str::from_utf8(&rest[1..1 + len]).unwrap_or("");
            rest = &rest[1 + len..];
            s
        })
    }

    pub fn events(&self) -> impl Iterator<Item = TraceEvent> + 'a {
        self.events
            .as_chunks::<EVENT_BYTES>()
            .0
            .iter()
            .map(|e| TraceEvent {
                kind: e[0],
                name: e[1],
                track: e[2],
                depth: e[3],
                ts: u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
                value: u32::from_le_bytes([e[8], e[9], e[10], e[11]]),
            })
    }
}

/// Decode a session's lines (in any order) into `out`, checking every
/// line's check and that no line is missing. Returns the bytes written.
/// The host tools (`trino-trace`) do the same with a growing buffer; this
/// one lets a console check its own output.
pub fn decode_lines<'a>(
    lines: impl Iterator<Item = TraceLine<'a>> + Clone,
    out: &mut [u8],
) -> Result<usize, DecodeError> {
    let first = lines.clone().map(|l| l.seq).min().unwrap_or(0);
    let count = lines.clone().count() as u32;
    let mut at = 0;
    let mut chunk = [0u8; LINE_BYTES];
    for seq in first..first + count {
        let line = lines
            .clone()
            .find(|l| l.seq == seq)
            .ok_or(DecodeError::Gap(seq))?;
        let n = line.decode(&mut chunk)?;
        out.get_mut(at..at + n)
            .ok_or(DecodeError::Truncated)?
            .copy_from_slice(&chunk[..n]);
        at += n;
    }
    Ok(at)
}

// --- The global ring: what `zone!`, `counter` and `frame_mark` write. ---

/// The guard [`zone!`](crate::zone) holds: the zone ends when it drops.
/// Zero-sized, with no `Drop`, without the `trace` feature.
#[must_use = "a zone ends when its guard drops: bind it (`zone!` does)"]
pub struct Zone {
    #[cfg(feature = "trace")]
    inner: Option<(&'static str, u8, u32)>,
}

impl Zone {
    /// Open a zone on the main track (prefer [`zone!`](crate::zone)).
    #[inline(always)]
    pub fn begin(name: &'static str) -> Zone {
        Zone::begin_on(track::CPU, name)
    }

    /// End the zone here (instead of at the end of the scope).
    #[inline(always)]
    pub fn end(self) {}

    /// Open a zone on `track`.
    #[inline(always)]
    #[allow(unused_variables)]
    pub fn begin_on(track: u8, name: &'static str) -> Zone {
        #[cfg(feature = "trace")]
        {
            Zone {
                inner: global::begin(track, name),
            }
        }
        #[cfg(not(feature = "trace"))]
        {
            Zone {}
        }
    }
}

#[cfg(feature = "trace")]
impl Drop for Zone {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some((name, track, start)) = self.inner {
            global::end(name, track, start);
        }
    }
}

/// Times the rest of the enclosing block as a zone named by a string
/// literal: `trino_core::zone!("ai");` or `zone!(track::AUDIO, "mix")`.
/// Expands to nothing without `trino-core`'s `trace` feature.
#[macro_export]
macro_rules! zone {
    ($name:expr) => {
        let _trino_zone = $crate::trace::Zone::begin($name);
    };
    ($track:expr, $name:expr) => {
        let _trino_zone = $crate::trace::Zone::begin_on($track, $name);
    };
}

/// Sample a counter (`"draws"`, `"heap"`) on the global ring. A no-op
/// without the `trace` feature.
#[inline(always)]
#[allow(unused_variables)]
pub fn counter(name: &'static str, value: u32) {
    #[cfg(feature = "trace")]
    global::counter(name, value);
}

/// Frame `frame` starts now. A no-op without the `trace` feature.
#[inline(always)]
#[allow(unused_variables)]
pub fn frame_mark(frame: u32) {
    #[cfg(feature = "trace")]
    global::frame(frame);
}

/// The instrumentation is compiled in (`trino-core`'s `trace` feature).
pub const ENABLED: bool = cfg!(feature = "trace");

/// Record into `ring` with `clock` (µs) from now on; returns `false` (and
/// records nothing) without the `trace` feature. The ring stays the
/// caller's: read it back with [`with_ring`].
///
/// Single-threaded consoles take the ring with a plain flag; the PC and the
/// Web with an atomic try-lock: an event from a thread that finds it busy
/// is counted in [`TraceRing::dropped`], never blocks.
///
/// Under hot reload (`watch pc`) the game dylib links its own copy of this
/// crate, so zones in game code go to a ring nobody installed (only the
/// engine's zones record). Trace a statically linked build.
#[allow(unused_variables)]
pub fn install(ring: &'static mut GlobalRing, clock: fn() -> u32, platform: TracePlatform) -> bool {
    #[cfg(feature = "trace")]
    {
        global::install(ring, clock, platform);
        true
    }
    #[cfg(not(feature = "trace"))]
    {
        false
    }
}

/// Stop recording; hands the ring back.
pub fn uninstall() -> Option<&'static mut GlobalRing> {
    #[cfg(feature = "trace")]
    {
        global::uninstall()
    }
    #[cfg(not(feature = "trace"))]
    {
        None
    }
}

/// Recording now (installed, feature on).
pub fn is_on() -> bool {
    #[cfg(feature = "trace")]
    {
        global::is_on()
    }
    #[cfg(not(feature = "trace"))]
    {
        false
    }
}

/// Run `f` on the installed ring (`None` when nothing is installed, or it is
/// busy on another thread), after merging the other threads' lanes into it.
#[allow(unused_variables)]
pub fn with_ring<R>(f: impl FnOnce(&mut GlobalRing, TracePlatform) -> R) -> Option<R> {
    #[cfg(feature = "trace")]
    {
        global::with(|g| {
            let p = g.platform;
            let r = g.ring_mut();
            global::drain_lanes(r);
            f(r, p)
        })
    }
    #[cfg(not(feature = "trace"))]
    {
        None
    }
}

/// Live hooks are set ([`set_hooks`]: Tracy, the Web's marks).
pub fn hooked() -> bool {
    #[cfg(feature = "trace")]
    {
        global::hooked()
    }
    #[cfg(not(feature = "trace"))]
    {
        false
    }
}

/// The installed clock's time, µs (0 when off).
pub fn now() -> u32 {
    #[cfg(feature = "trace")]
    {
        global::now()
    }
    #[cfg(not(feature = "trace"))]
    {
        0
    }
}

/// A zone with explicit times on the global ring (GPU passes read back a
/// frame later, work measured elsewhere). A no-op when off.
#[allow(unused_variables)]
pub fn record_zone(name: &'static str, track: u8, depth: u8, start: u32, dur: u32) {
    #[cfg(feature = "trace")]
    global::with(|g| g.ring_mut().zone_at(name, track, depth, start, dur));
}

/// A zone opens on `track` of the global ring as another copy of this
/// crate saw it begin (a hot-reloaded game's, through [`Hooks::begin`]:
/// [`TraceAttach`]). A no-op when off.
#[allow(unused_variables)]
pub fn forward_begin(track: u8) {
    #[cfg(feature = "trace")]
    global::with(|g| g.ring_mut().open(track));
}

/// The zone [`forward_begin`] opened ends: recorded at its depth. `name`
/// must outlive the ring (a host copies a dylib's names before calling).
#[allow(unused_variables)]
pub fn forward_end(name: &'static str, track: u8, start: u32, end: u32) {
    #[cfg(feature = "trace")]
    global::with(|g| {
        let r = g.ring_mut();
        r.close(track);
        r.zone(name, track, start, end);
    });
}

/// What a host hands a game that links its own copy of this crate (a
/// hot-reloaded dylib, `Platform::trace_hooks`): hooks that record into
/// the host's ring, and the host's clock (issue #79, #105 item 4).
#[derive(Clone, Copy)]
pub struct TraceAttach {
    pub hooks: Hooks,
    pub clock: fn() -> u32,
}

/// In a game's copy of this crate: record through `attach` (the host's
/// ring) unless this copy already records (it is the host's own, a static
/// build) or is already attached. Called by `export_game!` every update;
/// a no-op without the `trace` feature.
#[allow(unused_variables)]
pub fn adopt(attach: Option<TraceAttach>) {
    #[cfg(feature = "trace")]
    if let Some(a) = attach
        && !global::is_on()
        && !global::hooked()
    {
        global::set_clock(a.clock);
        global::set_hooks(Some(a.hooks));
    }
}

/// Events the global ring holds (sized for a few seconds of a busy frame:
/// ~40 events a frame at 60 fps). 48 KB, in the host's `static`.
pub const GLOBAL_EVENTS: usize = 4096;

/// The ring type [`install`] takes.
pub type GlobalRing = TraceRing<GLOBAL_EVENTS>;

/// Live hooks for a profiler that wants zones as they happen (Tracy on the
/// PC, `performance.measure` on the Web): called on the recording thread at
/// each zone's begin and end. A no-op without the `trace` feature.
#[derive(Clone, Copy)]
pub struct Hooks {
    pub begin: fn(track: u8, name: &'static str),
    pub end: fn(track: u8, name: &'static str, start_us: u32, end_us: u32),
    /// A [`counter`] sample.
    pub counter: fn(name: &'static str, value: u32),
    /// A [`frame_mark`].
    pub frame: fn(frame: u32),
}

/// Set (or clear) the live hooks. They run even while no ring is installed,
/// as long as [`install_hooks_clock`] gave them a clock.
#[allow(unused_variables)]
pub fn set_hooks(hooks: Option<Hooks>) {
    #[cfg(feature = "trace")]
    global::set_hooks(hooks);
}

/// Give the hooks a clock without installing a ring (a Tracy-only session).
#[allow(unused_variables)]
pub fn install_hooks_clock(clock: fn() -> u32) {
    #[cfg(feature = "trace")]
    global::set_clock(clock);
}

// --- Other threads (issue #130): a lane per thread, merged by the main one. ---

/// Events a [`Lane`] holds between two drains (the main thread drains every
/// lane at each [`frame_mark`], [`flush`] and [`with_ring`]).
pub const LANE_EVENTS: usize = 512;
/// Lanes the registry holds ([`add_lane`]). A thread that finds none free
/// records on the global ring as before (its events can be refused there
/// and counted in [`TraceRing::dropped`]).
pub const MAX_LANES: usize = 32;

#[derive(Clone, Copy)]
struct LaneEvent {
    name: &'static str,
    kind: u8,
    track: u8,
    depth: u8,
    ts: u32,
    value: u32,
}

const NO_EVENT: LaneEvent = LaneEvent {
    name: "",
    kind: 0,
    track: 0,
    depth: 0,
    ts: 0,
    value: 0,
};

/// One thread's events until the main thread merges them into the global
/// ring: one writer (the thread bound to it, [`bind_thread`]) and one
/// reader (whoever holds the global ring), so recording never waits and
/// never contends. Names stay `&'static str` until the merge interns them.
/// A full lane refuses events and counts them in the next packet's
/// `dropped`.
pub struct Lane {
    events: core::cell::UnsafeCell<[LaneEvent; LANE_EVENTS]>,
    /// Next slot written (the owner's), and next slot read (the reader's).
    head: core::sync::atomic::AtomicU32,
    tail: core::sync::atomic::AtomicU32,
    lost: core::sync::atomic::AtomicU32,
    claimed: core::sync::atomic::AtomicBool,
    /// The owner's track and open zones.
    track: core::cell::UnsafeCell<u8>,
    depth: core::cell::UnsafeCell<u8>,
}

// SAFETY: `events` slots are written only by the claiming thread between
// `tail` and `head`, read only by the ring's holder below `head` (published
// with Release/Acquire); `track` and `depth` only by the claiming thread,
// handed over through `claimed` (Release on release, Acquire on claim).
unsafe impl Sync for Lane {}

impl Default for Lane {
    fn default() -> Self {
        Self::new()
    }
}

impl Lane {
    pub const fn new() -> Lane {
        use core::sync::atomic::{AtomicBool, AtomicU32};
        Lane {
            events: core::cell::UnsafeCell::new([NO_EVENT; LANE_EVENTS]),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            lost: AtomicU32::new(0),
            claimed: AtomicBool::new(false),
            track: core::cell::UnsafeCell::new(0),
            depth: core::cell::UnsafeCell::new(0),
        }
    }

    /// Events waiting for the next merge.
    pub fn pending(&self) -> usize {
        use core::sync::atomic::Ordering;
        let h = self.head.load(Ordering::Acquire);
        h.wrapping_sub(self.tail.load(Ordering::Acquire)) as usize
    }

    /// A thread is bound to it.
    pub fn is_claimed(&self) -> bool {
        self.claimed.load(core::sync::atomic::Ordering::Acquire)
    }

    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn claim(&self, track: u8) -> bool {
        use core::sync::atomic::Ordering;
        if self
            .claimed
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }
        // SAFETY: the claim makes this thread the owner.
        unsafe {
            *self.track.get() = track;
            *self.depth.get() = 0;
        }
        true
    }

    /// Owner only. Its events stay for the next merge.
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn release(&self) {
        // SAFETY: still the owner until the store below.
        unsafe { *self.depth.get() = 0 };
        self.claimed
            .store(false, core::sync::atomic::Ordering::Release);
    }

    /// Owner only.
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn track(&self) -> u8 {
        // SAFETY: owner only.
        unsafe { *self.track.get() }
    }

    /// Owner only: a zone opens (zones inside it nest one deeper).
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn open(&self) {
        // SAFETY: owner only.
        unsafe {
            let d = &mut *self.depth.get();
            *d = d.saturating_add(1);
        }
    }

    /// Owner only: the innermost zone closes; returns its depth.
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn close(&self) -> u8 {
        // SAFETY: owner only.
        unsafe {
            let d = &mut *self.depth.get();
            *d = d.saturating_sub(1);
            *d
        }
    }

    /// Owner only: append, or count it lost when full.
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn push(&self, kind: EventKind, name: &'static str, depth: u8, ts: u32, value: u32) {
        use core::sync::atomic::Ordering;
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) as usize >= LANE_EVENTS {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let e = LaneEvent {
            name,
            kind: kind as u8,
            track: self.track(),
            depth,
            ts,
            value,
        };
        // SAFETY: slot `h` is outside `tail..head`, so the reader isn't on
        // it; a raw write never borrows the rest of the array.
        unsafe {
            self.events
                .get()
                .cast::<LaneEvent>()
                .add(h as usize % LANE_EVENTS)
                .write(e)
        };
        self.head.store(h.wrapping_add(1), Ordering::Release);
    }

    /// Reader only (the global ring's holder): move the events into `ring`,
    /// oldest first, and the lost count into its `dropped`.
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn drain_into<const N: usize>(&self, ring: &mut TraceRing<N>) {
        use core::sync::atomic::Ordering;
        let h = self.head.load(Ordering::Acquire);
        let mut t = self.tail.load(Ordering::Relaxed);
        while t != h {
            // SAFETY: slots below `head` are published and the owner
            // doesn't touch them until `tail` passes them.
            let e = unsafe {
                self.events
                    .get()
                    .cast::<LaneEvent>()
                    .add(t as usize % LANE_EVENTS)
                    .read()
            };
            match EventKind::from_u8(e.kind) {
                Some(EventKind::Zone) => ring.zone_at(e.name, e.track, e.depth, e.ts, e.value),
                Some(EventKind::Counter) => ring.counter(e.name, e.track, e.ts, e.value),
                Some(EventKind::Frame) => ring.frame(e.ts, e.value),
                None => {}
            }
            t = t.wrapping_add(1);
        }
        self.tail.store(h, Ordering::Release);
        ring.add_dropped(self.lost.swap(0, Ordering::Relaxed));
    }

    /// Reader only: forget what it holds (a new session).
    #[cfg_attr(not(feature = "trace"), allow(dead_code))]
    fn discard(&self) {
        use core::sync::atomic::Ordering;
        self.tail
            .store(self.head.load(Ordering::Acquire), Ordering::Release);
        self.lost.store(0, Ordering::Relaxed);
    }
}

/// How the trace finds the calling thread's [`Lane`]. `trino-core` has no
/// threads of its own: the PC keeps the lane in a `thread_local!`
/// (`trino_render_wgpu::trace_host`), the 3DS keys it by the thread's TLS
/// register. Without hooks every thread records on the global ring.
#[derive(Clone, Copy)]
pub struct ThreadHooks {
    /// The calling thread's lane (`None`: the main thread, or not bound).
    pub get: fn() -> Option<&'static Lane>,
    /// Bind the calling thread to a lane, or unbind it.
    pub set: fn(Option<&'static Lane>),
    /// A new lane when every registered one is taken (the PC leaks one;
    /// `None`: the 3DS's static lanes are all there is).
    pub grow: fn() -> Option<&'static Lane>,
}

/// Set (or clear) the thread hooks. A no-op without the `trace` feature.
#[allow(unused_variables)]
pub fn set_thread_hooks(hooks: Option<ThreadHooks>) {
    #[cfg(feature = "trace")]
    global::set_thread_hooks(hooks);
}

/// Register `lane` for [`bind_thread`] to claim and the main thread to
/// merge. `false` without the `trace` feature or with [`MAX_LANES`] taken.
#[allow(unused_variables)]
pub fn add_lane(lane: &'static Lane) -> bool {
    #[cfg(feature = "trace")]
    {
        global::add_lane(lane)
    }
    #[cfg(not(feature = "trace"))]
    {
        false
    }
}

/// Lanes registered.
pub fn lane_count() -> usize {
    #[cfg(feature = "trace")]
    {
        global::lane_count()
    }
    #[cfg(not(feature = "trace"))]
    {
        0
    }
}

/// Record the calling thread's zones, counters and frame marks on a track
/// of their own (`track::on(track::WORKER, 2)`) until the guard drops: in a
/// [`Lane`] it alone writes, merged into the global ring by the main thread.
/// Unbound (zones go to the global ring, as before) without the `trace`
/// feature, without [`ThreadHooks`], when the thread is already bound (the
/// first binding stays), or with every lane taken. Zero-sized without the
/// feature.
#[must_use = "the thread is unbound when the guard drops"]
#[allow(unused_variables)]
pub fn bind_thread(track: u8) -> ThreadLane {
    #[cfg(feature = "trace")]
    {
        ThreadLane {
            lane: global::bind(track),
            _thread: core::marker::PhantomData,
        }
    }
    #[cfg(not(feature = "trace"))]
    {
        ThreadLane {
            _thread: core::marker::PhantomData,
        }
    }
}

/// See [`bind_thread`]. Not `Send`: it unbinds the thread that bound it.
pub struct ThreadLane {
    #[cfg(feature = "trace")]
    lane: Option<&'static Lane>,
    _thread: core::marker::PhantomData<*const ()>,
}

impl ThreadLane {
    /// This guard bound the thread to a lane.
    pub fn is_bound(&self) -> bool {
        #[cfg(feature = "trace")]
        {
            self.lane.is_some()
        }
        #[cfg(not(feature = "trace"))]
        {
            false
        }
    }
}

#[cfg(feature = "trace")]
impl Drop for ThreadLane {
    fn drop(&mut self) {
        if let Some(lane) = self.lane {
            global::unbind(lane);
        }
    }
}

#[cfg(feature = "trace")]
mod global {
    use super::*;
    use core::cell::UnsafeCell;
    #[cfg(not(target_os = "none"))]
    use core::sync::atomic::AtomicU32;
    use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

    pub(super) struct State {
        ring: *mut GlobalRing,
        pub platform: TracePlatform,
    }

    impl State {
        pub fn ring_mut(&mut self) -> &mut GlobalRing {
            // SAFETY: `ring` is non-null while ON is set, and only reached
            // under BUSY.
            unsafe { &mut *self.ring }
        }
    }

    struct Cell(UnsafeCell<State>);
    // SAFETY: every access goes through `with`, which holds BUSY.
    unsafe impl Sync for Cell {}

    static STATE: Cell = Cell(UnsafeCell::new(State {
        ring: core::ptr::null_mut(),
        platform: TracePlatform::Unknown,
    }));
    static ON: AtomicBool = AtomicBool::new(false);
    static BUSY: AtomicBool = AtomicBool::new(false);
    /// Events refused while another thread held the ring (the PC).
    #[cfg(not(target_os = "none"))]
    static CONTENDED: AtomicU32 = AtomicU32::new(0);
    static CLOCK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static HOOK_BEGIN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static HOOK_END: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static HOOK_COUNTER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static HOOK_FRAME: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static THREAD_GET: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static THREAD_SET: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static THREAD_GROW: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
    static LANES: [AtomicPtr<Lane>; MAX_LANES] =
        [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_LANES];
    static LANE_COUNT: AtomicUsize = AtomicUsize::new(0);

    /// Take the ring. The N64 has one thread and no zone in an interrupt:
    /// a plain flag (no read-modify-write atomics on the VR4300 path).
    #[cfg(target_os = "none")]
    fn lock() -> bool {
        if BUSY.load(Ordering::Relaxed) {
            return false;
        }
        BUSY.store(true, Ordering::Relaxed);
        true
    }

    #[cfg(not(target_os = "none"))]
    fn lock() -> bool {
        let ok = BUSY
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok();
        if !ok {
            CONTENDED.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    fn unlock() {
        BUSY.store(false, Ordering::Release);
    }

    pub(super) fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
        if !ON.load(Ordering::Acquire) || !lock() {
            return None;
        }
        // SAFETY: BUSY is held: nobody else touches STATE.
        let s = unsafe { &mut *STATE.0.get() };
        #[cfg(not(target_os = "none"))]
        {
            let lost = CONTENDED.swap(0, Ordering::Relaxed);
            if lost > 0 {
                s.ring_mut().add_dropped(lost);
            }
        }
        let r = f(s);
        unlock();
        Some(r)
    }

    pub(super) fn install(ring: &'static mut GlobalRing, clock: fn() -> u32, p: TracePlatform) {
        set_clock(clock);
        while !lock() {}
        // SAFETY: BUSY is held.
        let s = unsafe { &mut *STATE.0.get() };
        s.ring = ring;
        s.platform = p;
        // A new session: what lanes kept from an earlier one is stale.
        for_lanes(Lane::discard);
        unlock();
        ON.store(true, Ordering::Release);
    }

    pub(super) fn uninstall() -> Option<&'static mut GlobalRing> {
        if !ON.swap(false, Ordering::AcqRel) {
            return None;
        }
        while !lock() {}
        // SAFETY: BUSY is held; the pointer came from a `&'static mut`.
        let s = unsafe { &mut *STATE.0.get() };
        let ring = core::mem::replace(&mut s.ring, core::ptr::null_mut());
        unlock();
        // SAFETY: as above, and no one else holds it now.
        Some(unsafe { &mut *ring })
    }

    pub(super) fn is_on() -> bool {
        ON.load(Ordering::Acquire)
    }

    fn for_lanes(mut f: impl FnMut(&Lane)) {
        let n = LANE_COUNT.load(Ordering::Acquire).min(MAX_LANES);
        for slot in &LANES[..n] {
            let p = slot.load(Ordering::Acquire);
            if !p.is_null() {
                // SAFETY: only `&'static Lane`s are stored.
                f(unsafe { &*p });
            }
        }
    }

    /// Merge every lane into `ring` (its holder only).
    pub(super) fn drain_lanes(ring: &mut GlobalRing) {
        for_lanes(|l| l.drain_into(ring));
    }

    pub(super) fn add_lane(lane: &'static Lane) -> bool {
        let mut known = false;
        for_lanes(|l| known |= core::ptr::eq(l, lane));
        if known {
            return true;
        }
        let mut n = LANE_COUNT.load(Ordering::Relaxed);
        loop {
            if n >= MAX_LANES {
                return false;
            }
            match LANE_COUNT.compare_exchange(n, n + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
        // A reader between the count and this store skips the empty slot.
        LANES[n].store(lane as *const Lane as *mut Lane, Ordering::Release);
        true
    }

    pub(super) fn lane_count() -> usize {
        LANE_COUNT.load(Ordering::Acquire).min(MAX_LANES)
    }

    pub(super) fn set_thread_hooks(h: Option<ThreadHooks>) {
        let null = core::ptr::null_mut();
        let (g, s, w) = match h {
            Some(h) => (h.get as *mut (), h.set as *mut (), h.grow as *mut ()),
            None => (null, null, null),
        };
        THREAD_SET.store(s, Ordering::Release);
        THREAD_GROW.store(w, Ordering::Release);
        THREAD_GET.store(g, Ordering::Release);
    }

    /// The calling thread's lane. The N64 has one thread: never one there,
    /// and no hook load on its zones.
    #[cfg(target_os = "none")]
    #[inline(always)]
    fn current_lane() -> Option<&'static Lane> {
        None
    }

    #[cfg(not(target_os = "none"))]
    #[inline(always)]
    fn current_lane() -> Option<&'static Lane> {
        let g = THREAD_GET.load(Ordering::Acquire);
        if g.is_null() {
            return None;
        }
        // SAFETY: only `ThreadHooks::get` pointers are stored there.
        let f: fn() -> Option<&'static Lane> = unsafe { core::mem::transmute(g) };
        f()
    }

    fn thread_set() -> Option<fn(Option<&'static Lane>)> {
        let s = THREAD_SET.load(Ordering::Acquire);
        // SAFETY: only `ThreadHooks::set` pointers are stored there.
        (!s.is_null()).then(|| unsafe { core::mem::transmute(s) })
    }

    pub(super) fn bind(track: u8) -> Option<&'static Lane> {
        let set = thread_set()?;
        if current_lane().is_some() || THREAD_GET.load(Ordering::Acquire).is_null() {
            return None;
        }
        let mut found = None;
        for_lanes(|l| {
            if found.is_none() && l.claim(track) {
                // SAFETY: `for_lanes` only hands out registered `'static` lanes.
                found = Some(unsafe { &*(l as *const Lane) });
            }
        });
        let lane = match found {
            Some(l) => l,
            None => {
                let g = THREAD_GROW.load(Ordering::Acquire);
                if g.is_null() {
                    return None;
                }
                // SAFETY: only `ThreadHooks::grow` pointers are stored there.
                let grow: fn() -> Option<&'static Lane> = unsafe { core::mem::transmute(g) };
                let l = grow()?;
                if !l.claim(track) {
                    return None;
                }
                if !add_lane(l) {
                    l.release();
                    return None;
                }
                l
            }
        };
        set(Some(lane));
        Some(lane)
    }

    pub(super) fn unbind(lane: &'static Lane) {
        if let Some(set) = thread_set() {
            set(None);
        }
        lane.release();
    }

    pub(super) fn set_clock(clock: fn() -> u32) {
        CLOCK.store(clock as *mut (), Ordering::Release);
    }

    pub(super) fn set_hooks(h: Option<Hooks>) {
        let null = core::ptr::null_mut();
        let (b, e, c, f) = match h {
            Some(h) => (
                h.begin as *mut (),
                h.end as *mut (),
                h.counter as *mut (),
                h.frame as *mut (),
            ),
            None => (null, null, null, null),
        };
        HOOK_BEGIN.store(b, Ordering::Release);
        HOOK_END.store(e, Ordering::Release);
        HOOK_COUNTER.store(c, Ordering::Release);
        HOOK_FRAME.store(f, Ordering::Release);
    }

    pub(super) fn now() -> u32 {
        let c = CLOCK.load(Ordering::Acquire);
        if c.is_null() {
            return 0;
        }
        // SAFETY: only `fn() -> u32` pointers are stored in CLOCK.
        let f: fn() -> u32 = unsafe { core::mem::transmute(c) };
        f()
    }

    pub(super) fn hooked() -> bool {
        !HOOK_BEGIN.load(Ordering::Relaxed).is_null()
    }

    #[inline(never)]
    pub(super) fn begin(track: u8, name: &'static str) -> Option<(&'static str, u8, u32)> {
        let on = is_on();
        if !on && !hooked() {
            return None;
        }
        if let Some(l) = current_lane() {
            l.open();
        } else if on {
            with(|s| s.ring_mut().open(track));
        }
        let b = HOOK_BEGIN.load(Ordering::Acquire);
        if !b.is_null() {
            // SAFETY: only `Hooks::begin` pointers are stored there.
            let f: fn(u8, &'static str) = unsafe { core::mem::transmute(b) };
            f(track, name);
        }
        Some((name, track, now()))
    }

    #[inline(never)]
    pub(super) fn end(name: &'static str, track: u8, start: u32) {
        let end = now();
        if let Some(l) = current_lane() {
            let depth = l.close();
            if is_on() {
                l.push(EventKind::Zone, name, depth, start, end.wrapping_sub(start));
            }
        } else {
            with(|s| {
                let r = s.ring_mut();
                r.close(track);
                r.zone(name, track, start, end);
            });
        }
        let e = HOOK_END.load(Ordering::Acquire);
        if !e.is_null() {
            // SAFETY: only `Hooks::end` pointers are stored there.
            let f: fn(u8, &'static str, u32, u32) = unsafe { core::mem::transmute(e) };
            f(track, name, start, end);
        }
    }

    #[inline(never)]
    pub(super) fn counter(name: &'static str, value: u32) {
        let c = HOOK_COUNTER.load(Ordering::Acquire);
        if !c.is_null() {
            // SAFETY: only `Hooks::counter` pointers are stored there.
            let f: fn(&'static str, u32) = unsafe { core::mem::transmute(c) };
            f(name, value);
        }
        if !is_on() {
            return;
        }
        let t = now();
        if let Some(l) = current_lane() {
            l.push(EventKind::Counter, name, 0, t, value);
            return;
        }
        with(|s| s.ring_mut().counter(name, track::CPU, t, value));
    }

    #[inline(never)]
    pub(super) fn frame(n: u32) {
        let h = HOOK_FRAME.load(Ordering::Acquire);
        if !h.is_null() {
            // SAFETY: only `Hooks::frame` pointers are stored there.
            let f: fn(u32) = unsafe { core::mem::transmute(h) };
            f(n);
        }
        if !is_on() {
            return;
        }
        let t = now();
        if let Some(l) = current_lane() {
            l.push(EventKind::Frame, "frame", 0, t, n);
            return;
        }
        // The main thread's frame: the other threads' events go in first.
        with(|s| {
            let r = s.ring_mut();
            drain_lanes(r);
            r.frame(t, n);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    fn decode_all(text: &str) -> Vec<u8> {
        let lines: Vec<TraceLine> = text.lines().filter_map(TraceLine::parse).collect();
        let mut out = vec![0u8; 1 << 16];
        let n = decode_lines(lines.iter().copied(), &mut out).unwrap();
        out.truncate(n);
        out
    }

    #[test]
    fn events_are_twelve_bytes_and_the_packet_length_is_exact() {
        let mut r = TraceRing::<8>::new();
        r.zone("update", track::CPU, 10, 30);
        r.counter("draws", track::CPU, 31, 7);
        r.frame(40, 1);
        let want = r.packet_len();
        let mut bytes = Vec::new();
        r.encode(TracePlatform::N64, &mut |b| bytes.push(b));
        assert_eq!(bytes.len(), want);
        // 16 header + names ("update", "draws", "frame") + 8 + 3 events.
        assert_eq!(want, 16 + (1 + 6) + (1 + 5) + (1 + 5) + 8 + 3 * EVENT_BYTES);
        assert!(r.is_empty(), "encode clears the ring");
    }

    #[test]
    fn a_packet_round_trips() {
        let mut r = TraceRing::<16>::new();
        r.open(track::CPU);
        r.zone("tnl", track::CPU, 100, 150);
        r.close(track::CPU);
        r.zone("render", track::CPU, 90, 200);
        r.counter("tris", track::CPU, 201, 1234);
        r.zone_at("scene", track::GPU, 0, 120, 60);
        r.frame(80, 3);
        let mut bytes = Vec::new();
        r.encode(TracePlatform::N3ds, &mut |b| bytes.push(b));
        let (p, len) = Packet::parse(&bytes).unwrap();
        assert_eq!(len, bytes.len());
        assert_eq!(p.platform, TracePlatform::N3ds);
        assert_eq!(p.name_base, 0);
        let names: Vec<&str> = p.names().collect();
        assert_eq!(names, ["tnl", "render", "tris", "scene", "frame"]);
        let ev: Vec<TraceEvent> = p.events().collect();
        assert_eq!(ev.len(), 5);
        assert_eq!(
            ev[0],
            TraceEvent {
                kind: 0,
                name: 0,
                track: 0,
                depth: 1,
                ts: 100,
                value: 50
            }
        );
        assert_eq!((ev[1].depth, ev[1].ts, ev[1].value), (0, 90, 110));
        assert_eq!((ev[2].kind, ev[2].value), (EventKind::Counter as u8, 1234));
        assert_eq!((ev[3].track, ev[3].value), (track::GPU, 60));
        assert_eq!((ev[4].kind, ev[4].value), (EventKind::Frame as u8, 3));
    }

    #[test]
    fn names_are_sent_once_per_ring() {
        let mut r = TraceRing::<4>::new();
        r.zone("a", 0, 0, 1);
        let mut first = Vec::new();
        r.encode(TracePlatform::Pc, &mut |b| first.push(b));
        r.zone("a", 0, 1, 2);
        r.zone("b", 0, 2, 3);
        let mut second = Vec::new();
        r.encode(TracePlatform::Pc, &mut |b| second.push(b));
        let (p, _) = Packet::parse(&second).unwrap();
        assert_eq!(p.name_base, 1);
        assert_eq!(p.names().collect::<Vec<_>>(), ["b"]);
        assert_eq!(p.events().map(|e| e.name).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn a_full_ring_keeps_the_newest_and_counts_the_rest() {
        let mut r = TraceRing::<4>::new();
        for i in 0..10 {
            r.counter("n", 0, i, i);
        }
        assert_eq!(r.len(), 4);
        assert_eq!(r.dropped(), 6);
        let kept: Vec<u32> = r.events().map(|e| e.value).collect();
        assert_eq!(kept, [6, 7, 8, 9]);
        let mut bytes = Vec::new();
        r.encode(TracePlatform::Pc, &mut |b| bytes.push(b));
        let (p, _) = Packet::parse(&bytes).unwrap();
        assert_eq!(p.dropped, 6);
        assert_eq!(r.dropped(), 0);
    }

    #[test]
    fn the_name_table_refuses_past_its_capacity() {
        let mut r = TraceRing::<4>::new();
        // Distinct `&'static str`s without allocation: slices of one literal.
        const ALL: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\
                           abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\
                           abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut interned = 0;
        for i in 0..MAX_NAMES + 2 {
            // Unique by length.
            if r.intern(&ALL[..i + 1]).is_some() {
                interned += 1;
            }
        }
        assert_eq!(interned, MAX_NAMES);
        r.counter(&ALL[..MAX_NAMES + 5], 0, 0, 0);
        assert_eq!(r.dropped(), 1);
        // A known name is still fine.
        assert_eq!(r.intern(&ALL[..1]), Some(0));
    }

    #[test]
    fn names_intern_by_content() {
        let mut r = TraceRing::<1>::new();
        let a: &'static str = "update";
        let b: &'static str = &"xupdate"[1..];
        assert_eq!(r.intern(a), r.intern(b));
        assert_eq!(r.names().len(), 1);
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        let cases: [(&[u8], &str); 5] = [
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foobar", "Zm9vYmFy"),
        ];
        for (raw, text) in cases {
            let mut out = [0u8; 16];
            let n = base64_encode(raw, &mut out);
            assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), text);
            let mut back = [0u8; 16];
            let m = base64_decode(text, &mut back).unwrap();
            assert_eq!(&back[..m], raw);
        }
        assert_eq!(base64_decode("Zm9v!", &mut [0u8; 8]), None);
    }

    #[test]
    fn lines_carry_session_sequence_and_check() {
        let mut r = TraceRing::<64>::new();
        for i in 0..40 {
            r.zone("draw", 0, i * 10, i * 10 + 5);
        }
        let mut text = String::new();
        let mut seq = 7;
        r.write_lines(TracePlatform::Pc, "pc-test", &mut seq, &mut text)
            .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // 16 + 5 + 8 + 40 * 12 = 509 bytes = 6 lines of 96.
        assert_eq!(lines.len(), 6);
        assert_eq!(seq, 13);
        let l = TraceLine::parse(&format!("[ares] {}", lines[0]))
            .map(|l| (l.session.to_string(), l.seq))
            .unwrap();
        assert_eq!(l, ("pc-test".to_string(), 7));
        let bytes = decode_all(&text);
        let (p, _) = Packet::parse(&bytes).unwrap();
        assert_eq!(p.event_count, 40);
        assert_eq!(p.events().last().unwrap().ts, 390);
    }

    #[test]
    fn emitted_lines_are_whole_and_match_write_lines() {
        let fill = |r: &mut TraceRing<64>| {
            for i in 0..30 {
                r.zone("emit", 0, i, i + 2);
            }
        };
        let mut a = TraceRing::<64>::new();
        fill(&mut a);
        let mut text = String::new();
        a.write_lines(TracePlatform::N64, "n64-hub", &mut 0, &mut text)
            .unwrap();
        let mut b = TraceRing::<64>::new();
        fill(&mut b);
        let mut got: Vec<String> = Vec::new();
        let mut seq = 0;
        b.emit_lines(TracePlatform::N64, "n64-hub", &mut seq, &mut |l| {
            got.push(l.to_string())
        })
        .unwrap();
        assert!(got.iter().all(|l| l.ends_with('\n') && l.len() <= LINE_MAX));
        assert_eq!(got.concat(), text);
        assert_eq!(seq as usize, got.len());
    }

    #[test]
    fn a_garbled_or_missing_line_is_an_error() {
        let mut r = TraceRing::<64>::new();
        for i in 0..20 {
            r.zone("z", 0, i, i + 1);
        }
        let mut text = String::new();
        r.write_lines(TracePlatform::Pc, "s", &mut 0, &mut text)
            .unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 3);
        // Drop the middle line.
        let gap: Vec<TraceLine> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 1)
            .filter_map(|(_, l)| TraceLine::parse(l))
            .collect();
        let mut out = [0u8; 4096];
        assert_eq!(
            decode_lines(gap.iter().copied(), &mut out),
            Err(DecodeError::Gap(1))
        );
        // Flip a character of the first line's data.
        let mut bad = String::from(lines[0]);
        let last = bad.pop().unwrap();
        bad.push(if last == 'A' { 'B' } else { 'A' });
        let l = TraceLine::parse(&bad).unwrap();
        assert!(matches!(
            l.decode(&mut [0u8; LINE_BYTES]),
            Err(DecodeError::Checksum(0)) | Err(DecodeError::Base64)
        ));
    }

    #[test]
    fn bad_packets_are_refused() {
        assert_eq!(Packet::parse(b"NOPE").unwrap_err(), DecodeError::BadMagic);
        let mut r = TraceRing::<2>::new();
        r.zone("a", 0, 0, 1);
        let mut bytes = Vec::new();
        r.encode(TracePlatform::Pc, &mut |b| bytes.push(b));
        let mut v2 = bytes.clone();
        v2[4] = 2;
        assert_eq!(Packet::parse(&v2).unwrap_err(), DecodeError::Version(2));
        assert_eq!(
            Packet::parse(&bytes[..bytes.len() - 1]).unwrap_err(),
            DecodeError::Truncated
        );
        let mut kind = bytes.clone();
        let at = bytes.len() - EVENT_BYTES;
        kind[at] = 9;
        assert_eq!(
            Packet::parse(&kind).unwrap_err(),
            DecodeError::UnknownKind(9)
        );
    }

    #[test]
    fn packets_are_byte_identical_whatever_the_endianness() {
        // The encoder writes little-endian byte by byte: a known event's
        // bytes are fixed (the N64 is big-endian, the PC little).
        let mut r = TraceRing::<1>::new();
        r.counter("c", 2, 0x0102_0304, 0xA0B0_C0D0);
        let mut bytes = Vec::new();
        r.encode(TracePlatform::Web, &mut |b| bytes.push(b));
        let tail = &bytes[bytes.len() - EVENT_BYTES..];
        assert_eq!(tail, [1, 0, 2, 0, 4, 3, 2, 1, 0xD0, 0xC0, 0xB0, 0xA0]);
    }

    /// The global ring end to end (one test: it is process-wide state).
    #[cfg(feature = "trace")]
    #[test]
    fn the_global_ring_records_nested_zones_counters_and_frames() {
        use core::sync::atomic::{AtomicU32, Ordering};
        static T: AtomicU32 = AtomicU32::new(1000);
        fn clock() -> u32 {
            T.fetch_add(10, Ordering::Relaxed)
        }
        static mut RING: GlobalRing = GlobalRing::new();
        const { assert!(ENABLED) };
        // SAFETY: the test's only reference.
        assert!(install(
            unsafe { &mut *core::ptr::addr_of_mut!(RING) },
            clock,
            TracePlatform::Pc
        ));
        frame_mark(1);
        {
            crate::zone!("render");
            {
                crate::zone!("tnl");
            }
            counter("draws", 12);
        }
        record_zone("scene", track::GPU, 0, 5, 7);
        static HOOKED: AtomicU32 = AtomicU32::new(0);
        set_hooks(Some(Hooks {
            begin: |_, _| {
                HOOKED.fetch_add(1, Ordering::Relaxed);
            },
            end: |_, name, s, e| {
                assert!(e > s, "{name}");
                HOOKED.fetch_add(100, Ordering::Relaxed);
            },
            counter: |_, v| {
                HOOKED.fetch_add(v * 1000, Ordering::Relaxed);
            },
            frame: |_| {
                HOOKED.fetch_add(10_000, Ordering::Relaxed);
            },
        }));
        {
            crate::zone!(track::AUDIO, "mix");
        }
        counter("hooked", 2);
        frame_mark(3);
        set_hooks(None);
        assert_eq!(HOOKED.load(Ordering::Relaxed), 12_101);
        // A hot-reloaded game's zones arrive through the host's forwarders
        // (issue #79), nested like its own.
        forward_begin(track::CPU);
        forward_begin(track::CPU);
        forward_end("game ai", track::CPU, 2000, 2010);
        forward_end("game update", track::CPU, 1990, 2020);
        // A copy that records itself adopts nothing.
        adopt(Some(TraceAttach {
            hooks: Hooks {
                begin: |_, _| {},
                end: |_, _, _, _| {},
                counter: |_, _| {},
                frame: |_| {},
            },
            clock,
        }));
        assert!(!hooked());
        let mut text = String::new();
        with_ring(|r, p| r.write_lines(p, "global", &mut 0, &mut text).unwrap()).unwrap();
        let ring = uninstall().unwrap();
        assert!(ring.is_empty());
        assert!(!is_on());
        let bytes = decode_all(&text);
        let (p, _) = Packet::parse(&bytes).unwrap();
        let names: Vec<&str> = p.names().collect();
        let ev: Vec<(&str, u8, u8, u8)> = p
            .events()
            .map(|e| (names[e.name as usize], e.kind, e.track, e.depth))
            .collect();
        assert_eq!(
            ev,
            [
                ("frame", 2, 0, 0),
                ("tnl", 0, 0, 1),
                ("draws", 1, 0, 0),
                ("render", 0, 0, 0),
                ("scene", 0, 1, 0),
                ("mix", 0, 3, 0),
                ("hooked", 1, 0, 0),
                ("frame", 2, 0, 0),
                ("game ai", 0, 0, 1),
                ("game update", 0, 0, 0),
            ]
        );
        // The inner zone lies inside the outer one.
        let z: Vec<TraceEvent> = p.events().filter(|e| e.kind == 0).collect();
        assert!(z[0].ts >= z[1].ts && z[0].ts + z[0].value <= z[1].ts + z[1].value);
    }

    #[cfg(not(feature = "trace"))]
    #[test]
    fn without_the_feature_zones_are_free() {
        assert_eq!(core::mem::size_of::<Zone>(), 0);
        assert!(!core::mem::needs_drop::<Zone>());
        const { assert!(!ENABLED) };
        static mut RING: GlobalRing = GlobalRing::new();
        // SAFETY: the test's only reference.
        assert!(!install(
            unsafe { &mut *core::ptr::addr_of_mut!(RING) },
            || 1,
            TracePlatform::Pc
        ));
        crate::zone!("nothing");
        counter("x", 1);
        frame_mark(1);
        assert!(!is_on());
        assert!(with_ring(|_, _| ()).is_none());
    }
}
