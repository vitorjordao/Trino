//! Per-zone statistics of a session and the difference between two: the
//! table a perf PR quotes (`cargo xtask trace report|diff`, the editor's
//! Profiler).

use crate::{Session, Trace};
use serde_json::{Value, json};
use std::fmt::Write as _;
use trino_core::trace::track;

/// One zone name on one track, over a session.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneStats {
    pub name: String,
    pub track: u8,
    pub count: u64,
    pub total_us: u64,
    pub mean_us: f64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
    /// `total_us` over the session's frames (0 without frame marks).
    pub per_frame_us: f64,
}

/// Frame durations (mark to mark).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub count: u64,
    pub mean_us: f64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub max_us: u64,
}

/// A counter's samples.
#[derive(Clone, Debug, PartialEq)]
pub struct CounterStats {
    pub name: String,
    pub samples: u64,
    pub mean: f64,
    pub max: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionStats {
    pub session: String,
    pub platform: String,
    pub dropped: u64,
    pub frames: FrameStats,
    /// Most total time first.
    pub zones: Vec<ZoneStats>,
    pub counters: Vec<CounterStats>,
}

/// Nearest-rank percentile of sorted values.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn frame_stats(s: &Session) -> FrameStats {
    // Whole frames only: mark to next mark.
    let mut d: Vec<u64> = s
        .frames
        .windows(2)
        .map(|w| w[1].ts.saturating_sub(w[0].ts))
        .collect();
    d.sort_unstable();
    if d.is_empty() {
        return FrameStats::default();
    }
    FrameStats {
        count: d.len() as u64,
        mean_us: d.iter().sum::<u64>() as f64 / d.len() as f64,
        p50_us: percentile(&d, 50.0),
        p95_us: percentile(&d, 95.0),
        max_us: *d.last().unwrap(),
    }
}

impl SessionStats {
    pub fn of(s: &Session) -> SessionStats {
        let frames = frame_stats(s);
        let mut by: std::collections::BTreeMap<(u16, u8), Vec<u64>> = Default::default();
        for z in &s.zones {
            by.entry((z.name, z.track)).or_default().push(z.dur);
        }
        let mut zones: Vec<ZoneStats> = by
            .into_iter()
            .map(|((name, track), mut d)| {
                d.sort_unstable();
                let total: u64 = d.iter().sum();
                ZoneStats {
                    name: s.name_of(name).to_string(),
                    track,
                    count: d.len() as u64,
                    total_us: total,
                    mean_us: total as f64 / d.len() as f64,
                    p50_us: percentile(&d, 50.0),
                    p95_us: percentile(&d, 95.0),
                    max_us: *d.last().unwrap(),
                    per_frame_us: if s.frames.is_empty() {
                        0.0
                    } else {
                        total as f64 / s.frames.len() as f64
                    },
                }
            })
            .collect();
        zones.sort_by(|a, b| {
            b.total_us
                .cmp(&a.total_us)
                .then(a.name.cmp(&b.name))
                .then(a.track.cmp(&b.track))
        });
        let mut cby: std::collections::BTreeMap<u16, Vec<u64>> = Default::default();
        for c in &s.counters {
            cby.entry(c.name).or_default().push(c.value);
        }
        let counters = cby
            .into_iter()
            .map(|(n, v)| CounterStats {
                name: s.name_of(n).to_string(),
                samples: v.len() as u64,
                mean: v.iter().sum::<u64>() as f64 / v.len() as f64,
                max: *v.iter().max().unwrap(),
            })
            .collect();
        SessionStats {
            session: s.name.clone(),
            platform: s.platform.clone(),
            dropped: s.dropped,
            frames,
            zones,
            counters,
        }
    }

    pub fn zone(&self, name: &str, track: u8) -> Option<&ZoneStats> {
        self.zones
            .iter()
            .find(|z| z.name == name && z.track == track)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "session": self.session,
            "platform": self.platform,
            "dropped": self.dropped,
            "frames": {
                "count": self.frames.count, "mean_us": round1(self.frames.mean_us),
                "p50_us": self.frames.p50_us, "p95_us": self.frames.p95_us,
                "max_us": self.frames.max_us,
            },
            "zones": self.zones.iter().map(|z| json!({
                "name": z.name, "track": track::name(z.track), "count": z.count,
                "total_us": z.total_us, "mean_us": round1(z.mean_us), "p50_us": z.p50_us,
                "p95_us": z.p95_us, "max_us": z.max_us, "per_frame_us": round1(z.per_frame_us),
            })).collect::<Vec<_>>(),
            "counters": self.counters.iter().map(|c| json!({
                "name": c.name, "samples": c.samples, "mean": round1(c.mean), "max": c.max,
            })).collect::<Vec<_>>(),
        })
    }

    /// A text table: frames, then zones by total time, then counters.
    pub fn table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} ({}): {} frames, mean {:.0} us, p95 {} us, max {} us{}",
            self.session,
            self.platform,
            self.frames.count,
            self.frames.mean_us,
            self.frames.p95_us,
            self.frames.max_us,
            if self.dropped > 0 {
                format!(", {} events lost", self.dropped)
            } else {
                String::new()
            }
        );
        let _ = writeln!(
            out,
            "  {:<24} {:<6} {:>7} {:>10} {:>9} {:>8} {:>8} {:>10}",
            "zone", "track", "count", "mean_us", "p95_us", "max_us", "total", "per_frame"
        );
        for z in &self.zones {
            let _ = writeln!(
                out,
                "  {:<24} {:<6} {:>7} {:>10.1} {:>9} {:>8} {:>8} {:>10.1}",
                z.name,
                track::name(z.track),
                z.count,
                z.mean_us,
                z.p95_us,
                z.max_us,
                z.total_us,
                z.per_frame_us
            );
        }
        for c in &self.counters {
            let _ = writeln!(
                out,
                "  counter {:<16} samples {:>6} mean {:>10.1} max {:>8}",
                c.name, c.samples, c.mean, c.max
            );
        }
        out
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// One zone in both sessions (or in one of them).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneDelta {
    pub name: String,
    pub track: u8,
    pub a: Option<ZoneStats>,
    pub b: Option<ZoneStats>,
}

impl ZoneDelta {
    pub fn mean_delta(&self) -> f64 {
        self.b.as_ref().map_or(0.0, |z| z.mean_us) - self.a.as_ref().map_or(0.0, |z| z.mean_us)
    }

    pub fn p95_delta(&self) -> i64 {
        self.b.as_ref().map_or(0, |z| z.p95_us as i64)
            - self.a.as_ref().map_or(0, |z| z.p95_us as i64)
    }

    pub fn per_frame_delta(&self) -> f64 {
        self.b.as_ref().map_or(0.0, |z| z.per_frame_us)
            - self.a.as_ref().map_or(0.0, |z| z.per_frame_us)
    }

    /// The mean's change in percent of A (`None` when A lacks the zone).
    pub fn mean_pct(&self) -> Option<f64> {
        let a = self.a.as_ref()?.mean_us;
        (a > 0.0).then(|| self.mean_delta() / a * 100.0)
    }
}

/// B against A, zone by zone.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub a: SessionStats,
    pub b: SessionStats,
    /// Largest per-frame change first.
    pub zones: Vec<ZoneDelta>,
}

impl Diff {
    pub fn between(a: &Session, b: &Session) -> Diff {
        let (a, b) = (SessionStats::of(a), SessionStats::of(b));
        let mut keys: Vec<(String, u8)> = a
            .zones
            .iter()
            .chain(&b.zones)
            .map(|z| (z.name.clone(), z.track))
            .collect();
        keys.sort();
        keys.dedup();
        let mut zones: Vec<ZoneDelta> = keys
            .into_iter()
            .map(|(name, track)| ZoneDelta {
                a: a.zone(&name, track).cloned(),
                b: b.zone(&name, track).cloned(),
                name,
                track,
            })
            .collect();
        zones.sort_by(|x, y| {
            y.per_frame_delta()
                .abs()
                .total_cmp(&x.per_frame_delta().abs())
                .then(y.mean_delta().abs().total_cmp(&x.mean_delta().abs()))
                .then(x.name.cmp(&y.name))
        });
        Diff { a, b, zones }
    }

    pub fn to_json(&self) -> Value {
        let f = |s: &SessionStats| {
            json!({"session": s.session, "platform": s.platform, "frames": s.frames.count,
                "frame_mean_us": round1(s.frames.mean_us), "frame_p95_us": s.frames.p95_us})
        };
        json!({
            "a": f(&self.a),
            "b": f(&self.b),
            "frame_mean_delta_us": round1(self.b.frames.mean_us - self.a.frames.mean_us),
            "frame_p95_delta_us": self.b.frames.p95_us as i64 - self.a.frames.p95_us as i64,
            "zones": self.zones.iter().map(|z| json!({
                "name": z.name, "track": track::name(z.track),
                "a_mean_us": z.a.as_ref().map(|s| round1(s.mean_us)),
                "b_mean_us": z.b.as_ref().map(|s| round1(s.mean_us)),
                "mean_delta_us": round1(z.mean_delta()),
                "mean_pct": z.mean_pct().map(round1),
                "a_p95_us": z.a.as_ref().map(|s| s.p95_us),
                "b_p95_us": z.b.as_ref().map(|s| s.p95_us),
                "p95_delta_us": z.p95_delta(),
                "a_per_frame_us": z.a.as_ref().map(|s| round1(s.per_frame_us)),
                "b_per_frame_us": z.b.as_ref().map(|s| round1(s.per_frame_us)),
                "per_frame_delta_us": round1(z.per_frame_delta()),
                "a_count": z.a.as_ref().map(|s| s.count),
                "b_count": z.b.as_ref().map(|s| s.count),
            })).collect::<Vec<_>>(),
        })
    }

    pub fn table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "A = {} ({}), B = {} ({})\nframe mean {:.0} -> {:.0} us ({:+.0}), p95 {} -> {} us",
            self.a.session,
            self.a.platform,
            self.b.session,
            self.b.platform,
            self.a.frames.mean_us,
            self.b.frames.mean_us,
            self.b.frames.mean_us - self.a.frames.mean_us,
            self.a.frames.p95_us,
            self.b.frames.p95_us
        );
        let _ = writeln!(
            out,
            "  {:<24} {:<6} {:>10} {:>10} {:>9} {:>8} {:>8} {:>8} {:>10}",
            "zone", "track", "A mean", "B mean", "delta", "%", "A p95", "B p95", "frame dlt"
        );
        let opt = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.1}"));
        for z in &self.zones {
            let _ = writeln!(
                out,
                "  {:<24} {:<6} {:>10} {:>10} {:>+9.1} {:>8} {:>8} {:>8} {:>+10.1}",
                z.name,
                track::name(z.track),
                opt(z.a.as_ref().map(|s| s.mean_us)),
                opt(z.b.as_ref().map(|s| s.mean_us)),
                z.mean_delta(),
                z.mean_pct()
                    .map_or("-".to_string(), |p| format!("{p:+.0}%")),
                z.a.as_ref()
                    .map_or("-".to_string(), |s| s.p95_us.to_string()),
                z.b.as_ref()
                    .map_or("-".to_string(), |s| s.p95_us.to_string()),
                z.per_frame_delta()
            );
        }
        out
    }
}

/// The sessions to compare between two traces: those with the same name;
/// failing that, the same name past its platform prefix (`n64-hub` and
/// `pc-hub`); failing that, the only session of each.
pub fn pair_sessions<'a>(a: &'a Trace, b: &'a Trace) -> Vec<(&'a Session, &'a Session)> {
    let exact: Vec<_> = a
        .sessions
        .iter()
        .filter_map(|x| b.session(&x.name).map(|y| (x, y)))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let tail = |n: &str| n.split_once('-').map_or(n, |(_, t)| t).to_string();
    let by_tail: Vec<_> = a
        .sessions
        .iter()
        .filter_map(|x| {
            b.sessions
                .iter()
                .find(|y| tail(&y.name) == tail(&x.name))
                .map(|y| (x, y))
        })
        .collect();
    if !by_tail.is_empty() {
        return by_tail;
    }
    match (a.sessions.as_slice(), b.sessions.as_slice()) {
        ([x], [y]) => vec![(x, y)],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameMark, Zone};

    fn session(name: &str, durs: &[(&str, u64)], frame_us: &[u64]) -> Session {
        let mut s = Session {
            name: name.into(),
            platform: "pc".into(),
            ..Session::default()
        };
        let mut t = 0;
        for (n, d) in durs {
            let i = s.intern(n);
            s.zones.push(Zone {
                name: i,
                track: 0,
                depth: 0,
                start: t,
                dur: *d,
            });
            t += d;
        }
        let mut ts = 0;
        for f in frame_us {
            s.frames.push(FrameMark {
                ts,
                frame: s.frames.len() as u64,
            });
            ts += f;
        }
        s.frames.push(FrameMark {
            ts,
            frame: s.frames.len() as u64,
        });
        s
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<u64> = (1..=20).collect();
        assert_eq!(percentile(&v, 50.0), 10);
        assert_eq!(percentile(&v, 95.0), 19);
        assert_eq!(percentile(&v, 100.0), 20);
        assert_eq!(percentile(&[7], 95.0), 7);
        assert_eq!(percentile(&[], 95.0), 0);
    }

    #[test]
    fn zone_and_frame_statistics() {
        let s = session(
            "x",
            &[("tnl", 10), ("tnl", 30), ("sort", 5), ("tnl", 20)],
            &[100, 300, 200],
        );
        let st = SessionStats::of(&s);
        assert_eq!(st.frames.count, 3);
        assert_eq!(st.frames.mean_us, 200.0);
        assert_eq!(st.frames.max_us, 300);
        let tnl = st.zone("tnl", 0).unwrap();
        assert_eq!((tnl.count, tnl.total_us, tnl.max_us), (3, 60, 30));
        assert_eq!(tnl.mean_us, 20.0);
        assert_eq!(tnl.p50_us, 20);
        // 60 us over four frame marks.
        assert_eq!(tnl.per_frame_us, 15.0);
        assert_eq!(st.zones[0].name, "tnl", "most total time first");
        assert!(st.table().contains("tnl"));
        assert_eq!(st.to_json()["zones"][0]["mean_us"], 20.0);
    }

    #[test]
    fn a_diff_is_b_minus_a_largest_change_first() {
        let a = session("pc-hub", &[("tnl", 100), ("sort", 10)], &[1000]);
        let b = session(
            "n64-hub",
            &[("tnl", 150), ("sort", 10), ("rsp", 40)],
            &[1500],
        );
        let d = Diff::between(&a, &b);
        let names: Vec<&str> = d.zones.iter().map(|z| z.name.as_str()).collect();
        assert_eq!(names, ["tnl", "rsp", "sort"]);
        assert_eq!(d.zones[0].mean_delta(), 50.0);
        assert_eq!(d.zones[0].mean_pct(), Some(50.0));
        assert_eq!(d.zones[1].a, None);
        assert_eq!(d.zones[1].mean_pct(), None);
        let j = d.to_json();
        assert_eq!(j["frame_mean_delta_us"], 500.0);
        assert_eq!(j["zones"][0]["p95_delta_us"], 50);
        assert!(d.table().contains("+50.0"));
    }

    #[test]
    fn sessions_pair_by_name_then_by_scene_then_alone() {
        let t = |names: &[&str]| Trace {
            sessions: names.iter().map(|n| session(n, &[], &[])).collect(),
        };
        let a = t(&["n64-hub", "n64-throne"]);
        let b = t(&["pc-throne", "pc-hub"]);
        let p: Vec<(&str, &str)> = pair_sessions(&a, &b)
            .iter()
            .map(|(x, y)| (x.name.as_str(), y.name.as_str()))
            .collect();
        assert_eq!(p, [("n64-hub", "pc-hub"), ("n64-throne", "pc-throne")]);
        assert_eq!(pair_sessions(&a, &a).len(), 2);
        assert_eq!(pair_sessions(&t(&["one"]), &t(&["two"])).len(), 1);
        assert!(pair_sessions(&t(&["a", "b"]), &t(&["c"])).is_empty());
    }
}
