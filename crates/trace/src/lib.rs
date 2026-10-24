//! Host-side trace tools (issue #62, ADR 0030).
//!
//! Every target prints its trace the same way: `TRINO_TRACE:` lines holding
//! `TTRC` packets ([`trino_core::trace`]). This crate turns a log of them
//! into a [`Trace`] ([`collect`]), writes and reads Chrome Trace Event JSON
//! ([`chrome`], what `ui.perfetto.dev` and `chrome://tracing` open), and
//! computes the per-zone statistics and the diffs perf PRs quote
//! ([`stats`]). `cargo xtask trace` and the editor's Profiler tab are thin
//! layers over it, so an agent and a human read the same numbers.

pub mod chrome;
pub mod stats;

use std::collections::BTreeMap;
use trino_core::trace::{DecodeError, EventKind, LINE_BYTES, Packet, TraceLine};

pub use stats::{Diff, FrameStats, SessionStats, ZoneDelta, ZoneStats};

/// A timed zone, microseconds from its session's start.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    /// Index into [`Session::names`].
    pub name: u16,
    pub track: u8,
    pub depth: u8,
    pub start: u64,
    pub dur: u64,
}

impl Zone {
    pub fn end(&self) -> u64 {
        self.start + self.dur
    }
}

/// A counter sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Counter {
    pub name: u16,
    pub track: u8,
    pub ts: u64,
    pub value: u64,
}

/// A frame mark: frame `frame` starts at `ts`.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameMark {
    pub ts: u64,
    pub frame: u64,
}

/// One recording: a ring's packets from one run (`n64-hub`, `pc-castle64`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub name: String,
    /// `pc`, `n64`, `3ds`, `web` (or what a foreign trace said).
    pub platform: String,
    pub names: Vec<String>,
    /// Zones sorted by start, then outermost first.
    pub zones: Vec<Zone>,
    pub counters: Vec<Counter>,
    pub frames: Vec<FrameMark>,
    /// Events the target lost (ring full, no name slot, busy ring).
    pub dropped: u64,
}

impl Session {
    pub fn name_of(&self, i: u16) -> &str {
        self.names.get(i as usize).map_or("?", |s| s.as_str())
    }

    /// The index of `name`, interning it.
    pub fn intern(&mut self, name: &str) -> u16 {
        match self.names.iter().position(|n| n == name) {
            Some(i) => i as u16,
            None => {
                self.names.push(name.to_string());
                (self.names.len() - 1) as u16
            }
        }
    }

    /// The tracks that have events, ascending.
    pub fn tracks(&self) -> Vec<u8> {
        let mut t: Vec<u8> = self
            .zones
            .iter()
            .map(|z| z.track)
            .chain(self.counters.iter().map(|c| c.track))
            .collect();
        t.sort_unstable();
        t.dedup();
        t
    }

    /// Start and end of everything recorded, µs.
    pub fn span(&self) -> (u64, u64) {
        let starts = self
            .zones
            .iter()
            .map(|z| z.start)
            .chain(self.counters.iter().map(|c| c.ts))
            .chain(self.frames.iter().map(|f| f.ts));
        let ends = self
            .zones
            .iter()
            .map(|z| z.end())
            .chain(self.counters.iter().map(|c| c.ts))
            .chain(self.frames.iter().map(|f| f.ts));
        (starts.min().unwrap_or(0), ends.max().unwrap_or(0))
    }

    /// Frame `i`'s time range: from its mark to the next one (the last
    /// frame ends with the last event).
    pub fn frame_range(&self, i: usize) -> Option<(u64, u64)> {
        let start = self.frames.get(i)?.ts;
        let end = match self.frames.get(i + 1) {
            Some(n) => n.ts,
            None => self.span().1.max(start),
        };
        Some((start, end))
    }

    /// Zones that overlap `[from, to)`.
    pub fn zones_in(&self, from: u64, to: u64) -> impl Iterator<Item = &Zone> {
        self.zones
            .iter()
            .filter(move |z| z.start < to && z.end() > from)
    }

    /// Sort events and move the earliest to 0.
    pub fn normalize(&mut self) {
        let (t0, _) = self.span();
        for z in &mut self.zones {
            z.start -= t0;
        }
        for c in &mut self.counters {
            c.ts -= t0;
        }
        for f in &mut self.frames {
            f.ts -= t0;
        }
        self.zones
            .sort_by(|a, b| a.start.cmp(&b.start).then(b.dur.cmp(&a.dur)));
        self.counters.sort_by_key(|c| c.ts);
        self.frames.sort_by_key(|f| f.ts);
    }
}

/// Every session of a log or a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub sessions: Vec<Session>,
}

impl Trace {
    pub fn session(&self, name: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.name == name)
    }

    /// Total events (zones, counters, frame marks).
    pub fn event_count(&self) -> usize {
        self.sessions
            .iter()
            .map(|s| s.zones.len() + s.counters.len() + s.frames.len())
            .sum()
    }
}

/// Why a log didn't collect.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectError {
    pub session: String,
    pub error: DecodeError,
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "trace session {}: {}", self.session, self.error)
    }
}

impl std::error::Error for CollectError {}

/// Read every `TRINO_TRACE:` line of `log` (emulator prefixes and other lines
/// are fine) into sessions, in the order they first appear. A session's
/// lines may interleave with other output and other sessions; a line
/// numbered 0 after others of the same name starts a new run of it (named
/// `<name>#2`, ...). A missing or garbled line is an error, never a silently
/// wrong timeline.
pub fn collect(log: &str) -> Result<Trace, CollectError> {
    // (name, run) -> lines; the order of first appearance.
    let mut runs: Vec<(String, Vec<TraceLine>)> = Vec::new();
    let mut current: BTreeMap<&str, usize> = BTreeMap::new();
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for line in log.lines() {
        let Some(l) = TraceLine::parse(line) else {
            continue;
        };
        let idx = match current.get(l.session) {
            Some(&i) if !(l.seq == 0 && !runs[i].1.is_empty()) => i,
            _ => {
                let n = count.entry(l.session).or_insert(0);
                *n += 1;
                let name = if *n == 1 {
                    l.session.to_string()
                } else {
                    format!("{}#{n}", l.session)
                };
                runs.push((name, Vec::new()));
                current.insert(l.session, runs.len() - 1);
                runs.len() - 1
            }
        };
        runs[idx].1.push(l);
    }
    let mut trace = Trace::default();
    for (name, mut lines) in runs {
        let err = |error| CollectError {
            session: name.clone(),
            error,
        };
        lines.sort_by_key(|l| l.seq);
        let mut bytes = Vec::with_capacity(lines.len() * LINE_BYTES);
        let mut chunk = [0u8; LINE_BYTES];
        let first = lines[0].seq;
        for (i, l) in lines.iter().enumerate() {
            if l.seq != first + i as u32 {
                return Err(err(DecodeError::Gap(first + i as u32)));
            }
            let n = l.decode(&mut chunk).map_err(err)?;
            bytes.extend_from_slice(&chunk[..n]);
        }
        trace
            .sessions
            .push(decode_session(&name, &bytes).map_err(err)?);
    }
    Ok(trace)
}

/// The packets of one session, concatenated.
pub fn decode_session(name: &str, mut bytes: &[u8]) -> Result<Session, DecodeError> {
    let mut s = Session {
        name: name.to_string(),
        ..Session::default()
    };
    // Timestamps are u32 µs that wrap (~71 minutes): unwrap each against
    // the previous one.
    let mut last: Option<(u32, u64)> = None;
    let mut unwrap = |ts: u32| -> u64 {
        let t = match last {
            None => ts as u64,
            Some((raw, full)) => (full as i64 + ts.wrapping_sub(raw) as i32 as i64).max(0) as u64,
        };
        last = Some((ts, t));
        t
    };
    while !bytes.is_empty() {
        let (p, len) = Packet::parse(bytes)?;
        if s.platform.is_empty() {
            s.platform = p.platform.name().to_string();
        }
        if p.name_base as usize != s.names.len() {
            // A packet whose names follow ones this log doesn't have.
            return Err(DecodeError::UnknownName(p.name_base.min(255) as u8));
        }
        s.names.extend(p.names().map(str::to_string));
        s.dropped += p.dropped as u64;
        for e in p.events() {
            if e.name as usize >= s.names.len() {
                return Err(DecodeError::UnknownName(e.name));
            }
            let ts = unwrap(e.ts);
            match EventKind::from_u8(e.kind) {
                Some(EventKind::Zone) => s.zones.push(Zone {
                    name: e.name as u16,
                    track: e.track,
                    depth: e.depth,
                    start: ts,
                    dur: e.value as u64,
                }),
                Some(EventKind::Counter) => s.counters.push(Counter {
                    name: e.name as u16,
                    track: e.track,
                    ts,
                    value: e.value as u64,
                }),
                Some(EventKind::Frame) => s.frames.push(FrameMark {
                    ts,
                    frame: e.value as u64,
                }),
                None => return Err(DecodeError::UnknownKind(e.kind)),
            }
        }
        bytes = &bytes[len..];
    }
    s.normalize();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::trace::{TracePlatform, TraceRing, track};

    fn lines(f: impl FnOnce(&mut TraceRing<64>), session: &str) -> String {
        let mut r = TraceRing::<64>::new();
        f(&mut r);
        let mut s = String::new();
        r.write_lines(TracePlatform::N64, session, &mut 0, &mut s)
            .unwrap();
        s
    }

    #[test]
    fn interleaved_sessions_and_reruns_collect_apart() {
        let a = lines(|r| r.zone("a", track::CPU, 100, 110), "x");
        let b = lines(|r| r.zone("b", track::CPU, 5, 9), "y");
        let a2 = lines(|r| r.zone("c", track::CPU, 7, 8), "x");
        let log = format!("[ares] boot\n{a}noise TRINO_TRACE:broken\n{b}{a2}");
        let t = collect(&log).unwrap();
        let names: Vec<&str> = t.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["x", "y", "x#2"]);
        assert_eq!(t.sessions[0].platform, "n64");
        // Normalized: each session starts at 0.
        assert_eq!(t.sessions[0].zones[0].start, 0);
        assert_eq!(t.sessions[0].zones[0].dur, 10);
        assert_eq!(t.event_count(), 3);
    }

    #[test]
    fn a_missing_line_names_its_session_and_sequence() {
        let a = lines(
            |r| {
                for i in 0..30 {
                    r.zone("z", 0, i, i + 1);
                }
            },
            "s",
        );
        let cut: String = a
            .lines()
            .enumerate()
            .filter(|(i, _)| *i != 2)
            .map(|(_, l)| format!("{l}\n"))
            .collect();
        let e = collect(&cut).unwrap_err();
        assert_eq!(e.session, "s");
        assert_eq!(e.error, DecodeError::Gap(2));
        assert!(e.to_string().contains("missing"), "{e}");
    }

    #[test]
    fn timestamps_unwrap_across_the_u32_boundary() {
        let log = lines(
            |r| {
                r.frame(u32::MAX - 10, 1);
                r.zone("late", 0, 5, 25);
            },
            "w",
        );
        let s = &collect(&log).unwrap().sessions[0];
        assert_eq!(s.frames[0].ts, 0);
        // 11 µs to the wrap, then 5 more.
        assert_eq!(s.zones[0].start, 16);
    }

    #[test]
    fn several_packets_of_one_session_share_names() {
        let mut r = TraceRing::<8>::new();
        let mut text = String::new();
        let mut seq = 0;
        r.zone("a", 0, 0, 5);
        r.write_lines(TracePlatform::Pc, "p", &mut seq, &mut text)
            .unwrap();
        r.zone("a", 0, 10, 15);
        r.counter("n", 0, 16, 3);
        r.write_lines(TracePlatform::Pc, "p", &mut seq, &mut text)
            .unwrap();
        let t = collect(&text).unwrap();
        assert_eq!(t.sessions.len(), 1);
        let s = &t.sessions[0];
        assert_eq!(s.names, ["a", "n"]);
        assert_eq!(s.zones.len(), 2);
        assert_eq!(s.counters[0].value, 3);
    }
}
