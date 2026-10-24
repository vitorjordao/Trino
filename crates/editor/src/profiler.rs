//! The Profiler tab's state (issue #62): timeline traces read through
//! `trino-trace` — the same loader, statistics and diff as `cargo xtask
//! trace report|diff` — from `<project>/target/traces/*.json|*.log`.
//!
//! Trace A is the one the timeline and the flame view show; trace B, when
//! loaded, is compared with A zone by zone. Both files are polled once a
//! second and reloaded when they change on disk, so following `pc.log`
//! (what `cargo xtask run pc --trace` writes while the game runs) is the
//! live view of a Play run. A reload that fails (a log caught mid-write)
//! keeps the last good trace and retries on the next poll.
//!
//! The pure parts (the time view's zoom and pan, the lanes per track, the
//! visible zones, the measured N64 session) are unit-tested below.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use trino_trace::stats::{Diff, SessionStats, pair_sessions};
use trino_trace::{Session, Trace, Zone};

/// The traces folder of a project.
pub fn traces_dir(root: &Path) -> PathBuf {
    root.join("target/traces")
}

/// Trace files in `dir` (`*.json`, `*.log`), sorted by name.
pub fn list_traces(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && matches!(
                            p.extension().and_then(|e| e.to_str()),
                            Some("json") | Some("log")
                        )
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Read a trace file: Chrome/Perfetto JSON, or a log of `TRINO_TRACE:` lines.
pub fn load(path: &Path) -> Result<Trace, String> {
    trino_trace::chrome::load(path)
}

/// A file's change stamp: modification time and length.
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The visible time range of a timeline, microseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeView {
    pub t0: f64,
    pub t1: f64,
}

/// The narrowest view: 1 µs across.
pub const MIN_SPAN_US: f64 = 1.0;

impl TimeView {
    /// The whole of `[from, to]`.
    pub fn fit(from: u64, to: u64) -> TimeView {
        let (a, b) = (from as f64, (to as f64).max(from as f64 + MIN_SPAN_US));
        TimeView { t0: a, t1: b }
    }

    pub fn width(&self) -> f64 {
        self.t1 - self.t0
    }

    /// Keep the view inside `bounds` (never wider than it, never narrower
    /// than [`MIN_SPAN_US`]), shifting rather than shrinking.
    pub fn clamped(self, bounds: TimeView) -> TimeView {
        let bw = bounds.width().max(MIN_SPAN_US);
        let w = self.width().clamp(MIN_SPAN_US, bw);
        let t0 = self.t0.clamp(bounds.t0, bounds.t0 + bw - w);
        TimeView { t0, t1: t0 + w }
    }

    /// Zoom by `factor` (> 1 zooms in) keeping the time at `frac` (0 = the
    /// left edge, 1 = the right) under the cursor.
    pub fn zoom_at(self, frac: f64, factor: f64, bounds: TimeView) -> TimeView {
        if !(factor.is_finite() && factor > 0.0) {
            return self;
        }
        let frac = frac.clamp(0.0, 1.0);
        let at = self.t0 + frac * self.width();
        let w = (self.width() / factor).clamp(MIN_SPAN_US, bounds.width().max(MIN_SPAN_US));
        let t0 = at - frac * w;
        TimeView { t0, t1: t0 + w }.clamped(bounds)
    }

    /// Move by `frac` of the view's width (positive: later times come in,
    /// as when dragging the content left).
    pub fn pan(self, frac: f64, bounds: TimeView) -> TimeView {
        let d = frac * self.width();
        TimeView {
            t0: self.t0 + d,
            t1: self.t1 + d,
        }
        .clamped(bounds)
    }

    /// The x of time `t` in a strip `left..left + width` points.
    pub fn x_of(&self, t: f64, left: f32, width: f32) -> f32 {
        left + ((t - self.t0) / self.width()) as f32 * width
    }
}

/// Rows per track: one per zone depth (`max depth + 1`), for the tracks
/// that hold zones, ascending.
pub fn lanes(s: &Session) -> Vec<(u8, u8)> {
    let mut out: Vec<(u8, u8)> = Vec::new();
    for z in &s.zones {
        match out.iter_mut().find(|(t, _)| *t == z.track) {
            Some((_, n)) => *n = (*n).max(z.depth.saturating_add(1)),
            None => out.push((z.track, z.depth.saturating_add(1))),
        }
    }
    out.sort_unstable();
    out
}

/// The zones overlapping `[t0, t1)`. Zones are sorted by start, so only
/// the prefix that starts before `t1` is scanned.
pub fn visible(zones: &[Zone], t0: f64, t1: f64) -> impl Iterator<Item = &Zone> {
    let end = zones.partition_point(|z| (z.start as f64) < t1);
    zones[..end].iter().filter(move |z| (z.end() as f64) > t0)
}

/// `us` as text: microseconds below a millisecond, else milliseconds.
pub fn fmt_us(us: f64) -> String {
    if us.abs() >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.0} µs")
    }
}

/// A signed change, `fmt_us` with its sign.
pub fn fmt_delta_us(us: f64) -> String {
    let s = fmt_us(us.abs());
    if us > 0.05 {
        format!("+{s}")
    } else if us < -0.05 {
        format!("−{s}")
    } else {
        "±0".to_string()
    }
}

/// The N64 session the budget's "measured" line reads for scene `scene`:
/// `n64-<scene>` or `n64-<scene>-…` (`showcase` → `n64-showcase-river`), with
/// at least one whole frame (two marks). No other scene's session: a
/// measurement of another scene beside this one's estimate would mislead.
pub fn measured_session<'a>(trace: &'a Trace, scene: &str) -> Option<&'a Session> {
    if scene.is_empty() {
        return None;
    }
    let exact = format!("n64-{scene}");
    let prefix = format!("{exact}-");
    trace
        .sessions
        .iter()
        .filter(|s| s.frames.len() >= 2)
        .find(|s| s.name == exact)
        .or_else(|| {
            trace
                .sessions
                .iter()
                .filter(|s| s.frames.len() >= 2)
                .find(|s| s.name.starts_with(&prefix))
        })
}

/// A loaded trace file.
pub struct Loaded {
    pub path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    pub trace: Trace,
    /// The last reload's error (the trace above is the last good one).
    pub error: Option<String>,
    /// Bumps on every successful load (keys the caches).
    pub generation: u64,
    /// When the file last changed under us (the "live" pill).
    pub changed: Option<Instant>,
}

impl Loaded {
    pub fn open(path: &Path) -> Result<Loaded, String> {
        let stamp = stamp(path);
        let trace = load(path)?;
        Ok(Loaded {
            path: path.to_path_buf(),
            stamp,
            trace,
            error: None,
            generation: 0,
            changed: None,
        })
    }

    /// Reload when the file changed, or retry after a failed reload.
    /// Returns whether the trace changed.
    fn poll(&mut self) -> bool {
        let now = stamp(&self.path);
        if now == self.stamp && self.error.is_none() {
            return false;
        }
        self.stamp = now;
        match load(&self.path) {
            Ok(t) => {
                self.trace = t;
                self.error = None;
                self.generation += 1;
                self.changed = Some(Instant::now());
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }
}

/// What the budget's "measured" line shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    pub file: PathBuf,
    pub session: String,
    /// Mark to mark, the vsync wait included.
    pub frame_mean_us: f64,
    /// The `begin` zone per frame: waiting for a framebuffer (vsync, the RDP).
    pub wait_us: f64,
    pub frames: u64,
}

impl Measured {
    /// The frame's work: what `N64_COST` estimates and `test n64` gates
    /// (`frame_us` = update + render − begin).
    pub fn work_us(&self) -> f64 {
        (self.frame_mean_us - self.wait_us).max(0.0)
    }
}

/// (A's generation, A's session, B's generation and session, the files).
type CacheKey = (u64, usize, Option<(u64, usize)>, PathBuf, Option<PathBuf>);

/// The `n64*.json` files with their stamps, the scene, what they measured.
type MeasuredCache = (
    Vec<(PathBuf, Option<(SystemTime, u64)>)>,
    String,
    Option<Measured>,
);

/// Caches keyed on (A's generation, A's session, B's generation, B's
/// session): statistics and the diff are computed once per change.
#[derive(Default)]
struct Cache {
    key: Option<CacheKey>,
    stats: Option<SessionStats>,
    diff: Option<Diff>,
    lanes: Vec<(u8, u8)>,
}

pub struct ProfilerState {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub a: Option<Loaded>,
    pub b: Option<Loaded>,
    pub session_a: usize,
    /// B's session; `None` follows A's (`pair_sessions`).
    pub session_b: Option<usize>,
    pub view: Option<TimeView>,
    /// The view shows everything: refit when the trace grows.
    pub fitted: bool,
    pub frame: usize,
    /// Why the last open failed.
    pub error: Option<String>,
    last_poll: Option<Instant>,
    cache: Cache,
    measured: Option<MeasuredCache>,
    measured_poll: Option<Instant>,
}

/// How often files are checked for changes.
pub const POLL: Duration = Duration::from_secs(1);

/// `path[@session]` (the `TRINO_EDITOR_TRACE` hooks).
pub fn parse_hook(v: &str) -> (PathBuf, Option<String>) {
    match v.rsplit_once('@') {
        Some((p, s)) if !s.is_empty() && !s.contains(['/', '\\']) => {
            (PathBuf::from(p), Some(s.to_string()))
        }
        _ => (PathBuf::from(v), None),
    }
}

impl ProfilerState {
    pub fn new(root: &Path) -> ProfilerState {
        let dir = traces_dir(root);
        let mut s = ProfilerState {
            files: list_traces(&dir),
            dir,
            a: None,
            b: None,
            session_a: 0,
            session_b: None,
            view: None,
            fitted: true,
            frame: 0,
            error: None,
            last_poll: None,
            cache: Cache::default(),
            measured: None,
            measured_poll: None,
        };
        // Hooks: TRINO_EDITOR_TRACE=<path>[@session], TRINO_EDITOR_TRACE_B.
        if let Ok(v) = std::env::var("TRINO_EDITOR_TRACE") {
            let (p, session) = parse_hook(&v);
            s.open_a(&root.join(p), session.as_deref());
        }
        if let Ok(v) = std::env::var("TRINO_EDITOR_TRACE_B") {
            let (p, session) = parse_hook(&v);
            s.open_b(&root.join(p), session.as_deref());
        }
        s
    }

    pub fn refresh_files(&mut self) {
        self.files = list_traces(&self.dir);
    }

    /// Open `path` as trace A on `session` (by name; else the first).
    pub fn open_a(&mut self, path: &Path, session: Option<&str>) {
        match Loaded::open(path) {
            Ok(l) => {
                self.session_a = session
                    .and_then(|n| l.trace.sessions.iter().position(|s| s.name == n))
                    .unwrap_or(0);
                self.a = Some(l);
                self.error = None;
                self.reset_view();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Open `path` as trace B (`None`: compare with nothing).
    pub fn open_b(&mut self, path: &Path, session: Option<&str>) {
        match Loaded::open(path) {
            Ok(l) => {
                self.session_b =
                    session.and_then(|n| l.trace.sessions.iter().position(|s| s.name == n));
                self.b = Some(l);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn close_b(&mut self) {
        self.b = None;
        self.session_b = None;
    }

    pub fn set_session_a(&mut self, i: usize) {
        if i != self.session_a {
            self.session_a = i;
            self.reset_view();
        }
    }

    fn reset_view(&mut self) {
        self.view = None;
        self.fitted = true;
        self.frame = 0;
    }

    pub fn session(&self) -> Option<&Session> {
        let a = self.a.as_ref()?;
        a.trace.sessions.get(self.session_a)
    }

    /// B's session index: the one picked, else the one `pair_sessions`
    /// pairs with A's, else the first.
    pub fn session_b_index(&self) -> Option<usize> {
        let b = self.b.as_ref()?;
        if b.trace.sessions.is_empty() {
            return None;
        }
        if let Some(i) = self.session_b.filter(|i| *i < b.trace.sessions.len()) {
            return Some(i);
        }
        let paired = self.a.as_ref().and_then(|a| {
            let name = &a.trace.sessions.get(self.session_a)?.name;
            pair_sessions(&a.trace, &b.trace)
                .into_iter()
                .find(|(x, _)| &x.name == name)
                .and_then(|(_, y)| b.trace.sessions.iter().position(|s| s.name == y.name))
        });
        Some(paired.unwrap_or(0))
    }

    pub fn session_b(&self) -> Option<&Session> {
        let i = self.session_b_index()?;
        self.b.as_ref()?.trace.sessions.get(i)
    }

    /// The whole session's time range.
    pub fn bounds(&self) -> Option<TimeView> {
        let (from, to) = self.session()?.span();
        Some(TimeView::fit(from, to))
    }

    /// The timeline's view (the whole session until zoomed).
    pub fn current_view(&mut self) -> Option<TimeView> {
        let bounds = self.bounds()?;
        let v = match self.view {
            Some(v) if !self.fitted => v.clamped(bounds),
            _ => bounds,
        };
        self.view = Some(v);
        Some(v)
    }

    pub fn set_view(&mut self, v: TimeView) {
        self.fitted = false;
        self.view = Some(v);
    }

    pub fn fit(&mut self) {
        self.fitted = true;
        self.view = self.bounds();
    }

    /// Check both files (and the folder) about once a second. Returns
    /// whether anything reloaded.
    pub fn poll(&mut self, now: Instant) -> bool {
        if self.last_poll.is_some_and(|t| now.duration_since(t) < POLL) {
            return false;
        }
        self.last_poll = Some(now);
        self.refresh_files();
        let mut changed = false;
        let keep = self.session().map(|s| s.name.clone());
        if let Some(a) = &mut self.a
            && a.poll()
        {
            changed = true;
            // The same session by name after a reload.
            if let Some(n) = keep
                && let Some(i) = a.trace.sessions.iter().position(|s| s.name == n)
            {
                self.session_a = i;
            }
            if self.session_a >= a.trace.sessions.len() {
                self.session_a = 0;
            }
        }
        if let Some(b) = &mut self.b {
            changed |= b.poll();
        }
        changed
    }

    fn cache_key(&self) -> Option<CacheKey> {
        let a = self.a.as_ref()?;
        Some((
            a.generation,
            self.session_a,
            self.b
                .as_ref()
                .and_then(|b| Some((b.generation, self.session_b_index()?))),
            a.path.clone(),
            self.b.as_ref().map(|b| b.path.clone()),
        ))
    }

    fn refresh_cache(&mut self) {
        let key = self.cache_key();
        if key == self.cache.key {
            return;
        }
        self.cache = Cache {
            stats: self.session().map(SessionStats::of),
            diff: self
                .session()
                .zip(self.session_b())
                .map(|(a, b)| Diff::between(a, b)),
            lanes: self.session().map(lanes).unwrap_or_default(),
            key,
        };
    }

    /// A's statistics (cached).
    pub fn stats(&mut self) -> Option<&SessionStats> {
        self.refresh_cache();
        self.cache.stats.as_ref()
    }

    /// B against A (cached).
    pub fn diff(&mut self) -> Option<&Diff> {
        self.refresh_cache();
        self.cache.diff.as_ref()
    }

    pub fn lanes(&mut self) -> Vec<(u8, u8)> {
        self.refresh_cache();
        self.cache.lanes.clone()
    }

    /// The N64 frame a trace measured for scene `scene`
    /// (`target/traces/n64*.json`, newest first), re-read when those files
    /// change; checked about once a second.
    pub fn measured(&mut self, scene: &str) -> Option<&Measured> {
        let now = Instant::now();
        let due = self
            .measured_poll
            .is_none_or(|t| now.duration_since(t) >= POLL)
            || self.measured.as_ref().is_some_and(|(_, s, _)| s != scene);
        if due {
            self.measured_poll = Some(now);
            let mut files: Vec<(PathBuf, Option<(SystemTime, u64)>)> = list_traces(&self.dir)
                .into_iter()
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "json")
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("n64"))
                })
                .map(|p| {
                    let st = stamp(&p);
                    (p, st)
                })
                .collect();
            // Newest first.
            files.sort_by(|a, b| b.1.map(|s| s.0).cmp(&a.1.map(|s| s.0)));
            let same = self
                .measured
                .as_ref()
                .is_some_and(|(f, s, _)| *f == files && s == scene);
            if !same {
                let found = files.iter().find_map(|(p, _)| {
                    let t = load(p).ok()?;
                    let s = measured_session(&t, scene)?;
                    let st = SessionStats::of(s);
                    Some(Measured {
                        file: p.clone(),
                        session: s.name.clone(),
                        frame_mean_us: st.frames.mean_us,
                        wait_us: st
                            .zone("begin", trino_core::trace::track::CPU)
                            .map_or(0.0, |z| z.per_frame_us),
                        frames: st.frames.count,
                    })
                });
                self.measured = Some((files, scene.to_string(), found));
            }
        }
        self.measured.as_ref().and_then(|(_, _, m)| m.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_trace::FrameMark;

    fn zone(name: u16, track: u8, depth: u8, start: u64, dur: u64) -> Zone {
        Zone {
            name,
            track,
            depth,
            start,
            dur,
        }
    }

    #[test]
    fn zooming_keeps_the_time_under_the_cursor() {
        let b = TimeView::fit(0, 1000);
        let v = b.zoom_at(0.25, 2.0, b);
        // 250 µs stays at a quarter of the width.
        assert_eq!(v.width(), 500.0);
        assert!((v.t0 + 0.25 * v.width() - 250.0).abs() < 1e-9);
        // Zooming out never goes past the bounds, in nor below 1 µs.
        assert_eq!(v.zoom_at(0.5, 0.01, b), b);
        let tiny = b.zoom_at(0.5, 1e9, b);
        assert_eq!(tiny.width(), MIN_SPAN_US);
        assert!(tiny.t0 >= 0.0 && tiny.t1 <= 1000.0);
        // A bad factor changes nothing.
        assert_eq!(v.zoom_at(0.5, f64::NAN, b), v);
        assert_eq!(v.zoom_at(0.5, 0.0, b), v);
    }

    #[test]
    fn panning_shifts_and_stops_at_the_edges() {
        let b = TimeView::fit(0, 1000);
        let v = TimeView {
            t0: 100.0,
            t1: 300.0,
        };
        let p = v.pan(0.5, b);
        assert_eq!((p.t0, p.t1), (200.0, 400.0));
        let left = v.pan(-10.0, b);
        assert_eq!((left.t0, left.t1), (0.0, 200.0));
        let right = v.pan(10.0, b);
        assert_eq!((right.t0, right.t1), (800.0, 1000.0));
        // A view wider than the bounds becomes the bounds.
        let wide = TimeView {
            t0: -50.0,
            t1: 5000.0,
        }
        .clamped(b);
        assert_eq!(wide, b);
        assert_eq!(v.x_of(200.0, 10.0, 100.0), 60.0);
    }

    #[test]
    fn lanes_count_depths_per_track() {
        let s = Session {
            zones: vec![
                zone(0, 0, 0, 0, 10),
                zone(1, 0, 2, 1, 2),
                zone(2, 2, 0, 0, 5),
                zone(3, 1, 1, 3, 4),
            ],
            ..Session::default()
        };
        assert_eq!(lanes(&s), [(0, 3), (1, 2), (2, 1)]);
        assert!(lanes(&Session::default()).is_empty());
    }

    #[test]
    fn visible_zones_overlap_the_view() {
        let z = [
            zone(0, 0, 0, 0, 100), // long: still overlaps later views
            zone(1, 0, 1, 10, 5),
            zone(2, 0, 1, 50, 5),
            zone(3, 0, 0, 200, 10),
        ];
        let names = |t0, t1| visible(&z, t0, t1).map(|z| z.name).collect::<Vec<_>>();
        assert_eq!(names(40.0, 60.0), [0, 2]);
        assert_eq!(names(0.0, 1000.0), [0, 1, 2, 3]);
        assert_eq!(names(100.0, 200.0), Vec::<u16>::new());
        assert_eq!(names(205.0, 206.0), [3]);
    }

    #[test]
    fn the_measured_session_is_the_scenes_own() {
        let s = |name: &str, marks: usize| Session {
            name: name.into(),
            frames: (0..marks)
                .map(|i| FrameMark {
                    ts: i as u64 * 100,
                    frame: i as u64,
                })
                .collect(),
            ..Session::default()
        };
        let t = Trace {
            sessions: vec![
                s("selftest", 5),
                s("n64-empty", 1),
                s("n64-hub", 3),
                s("n64-throne", 3),
                s("n64-cpu-throne", 3),
                s("n64-showcase-river", 3),
            ],
        };
        assert_eq!(measured_session(&t, "throne").unwrap().name, "n64-throne");
        assert_eq!(measured_session(&t, "hub").unwrap().name, "n64-hub");
        assert_eq!(
            measured_session(&t, "showcase").unwrap().name,
            "n64-showcase-river"
        );
        // Another scene's session never stands in; nor a single mark.
        assert!(measured_session(&t, "castle").is_none());
        assert!(measured_session(&t, "empty").is_none());
        assert!(measured_session(&t, "").is_none());
        assert!(measured_session(&Trace::default(), "hub").is_none());
    }

    #[test]
    fn hooks_files_and_units() {
        assert_eq!(
            parse_hook("target/traces/n64-trace.json@n64-hub"),
            (
                "target/traces/n64-trace.json".into(),
                Some("n64-hub".into())
            )
        );
        assert_eq!(parse_hook("a.json"), ("a.json".into(), None));
        assert_eq!(parse_hook("dir@x/a.json"), ("dir@x/a.json".into(), None));
        let dir = tempfile::tempdir().unwrap();
        for f in ["b.log", "a.json", "notes.txt"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("sub.json")).unwrap();
        let names: Vec<String> = list_traces(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.json", "b.log"]);
        assert!(list_traces(&dir.path().join("missing")).is_empty());
        assert_eq!(fmt_us(830.0), "830 µs");
        assert_eq!(fmt_us(15276.0), "15.28 ms");
        assert_eq!(fmt_delta_us(-1500.0), "−1.50 ms");
        assert_eq!(fmt_delta_us(12.0), "+12 µs");
        assert_eq!(fmt_delta_us(0.0), "±0");
    }

    #[test]
    fn a_followed_file_reloads_and_a_half_written_one_keeps_the_last_trace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.json");
        let write = |zones: u64| {
            let mut s = Session {
                name: "pc-live".into(),
                platform: "pc".into(),
                ..Session::default()
            };
            let n = s.intern("update");
            for i in 0..zones {
                s.zones.push(zone(n, 0, 0, i * 10, 5));
            }
            let t = Trace { sessions: vec![s] };
            std::fs::write(&path, trino_trace::chrome::write(&t).to_string()).unwrap();
        };
        write(2);
        let mut p = ProfilerState::new(dir.path());
        p.open_a(&path, None);
        assert_eq!(p.session().unwrap().zones.len(), 2);
        assert_eq!(p.stats().unwrap().zones[0].count, 2);
        // Grows on disk: the next poll reloads it (and the stats follow).
        write(5);
        let t = Instant::now();
        assert!(p.poll(t));
        assert_eq!(p.stats().unwrap().zones[0].count, 5);
        // Polls are throttled.
        write(6);
        assert!(!p.poll(t + Duration::from_millis(10)));
        // Half-written: the last good trace stays, the error shows, and the
        // next poll retries even though the file no longer changes.
        std::fs::write(&path, "{\"traceEvents\": [").unwrap();
        assert!(!p.poll(t + Duration::from_secs(2)));
        assert!(p.a.as_ref().unwrap().error.is_some());
        assert_eq!(p.session().unwrap().zones.len(), 5);
        write(7);
        assert!(p.poll(t + Duration::from_secs(4)));
        assert!(p.a.as_ref().unwrap().error.is_none());
        assert_eq!(p.session().unwrap().zones.len(), 7);
    }
}
