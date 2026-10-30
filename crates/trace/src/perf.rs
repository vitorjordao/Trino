//! Console perf reports (issue #79, ADR 0038): what one `cargo xtask test
//! n64` run measured for one perf scene, as `target/perf/<target>-<scene>.json`,
//! and what `cargo xtask perf ab` measured over code layouts. The editor's
//! Profiler reads the same files.
//!
//! A report is built from the ROM's log lines:
//! - `TRINO_PERF_SCENE:<scene> k=v ...` — stage times, T&L work and (in an
//!   instrumented build) the backend counters, per frame;
//! - `TRINO_PERF_MODEL:<scene> model=.. calls=.. us=..` — per model;
//! - `TRINO_PROF_RSP:<scene> ovl=.. verts=.. tris=.. n=.. us=..` — `--prof
//!   rsp`'s per-command RSP times, fitted here per overlay;
//! - `TRINO_PROF_SAMPLE:<scene> <pc>=<n> ...` — `--prof sample`'s PCs,
//!   symbolized with the ELF's symbol table (`mips64-elf-nm -S -C`);
//! - a trace session `n64-<scene>` — the zones (`--trace`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
pub use serde_json::Value;

/// The report format's version (`"version"` in the file).
pub const VERSION: u32 = 1;
/// Its `"format"` field.
pub const FORMAT: &str = "trino-perf-report";
/// An A/B run's `"format"`.
pub const AB_FORMAT: &str = "trino-perf-ab";

/// The backend counters an instrumented N64 build appends to its scene
/// line (`trino_platform_n64::prof::NAMES`; kept in pairs).
pub const COUNTERS: [&str; 16] = [
    "memo_hit",
    "memo_miss",
    "memo_culled",
    "memo_store",
    "memo_evict",
    "water_hit",
    "water_miss",
    "water_evict",
    "rsp_reject",
    "rspq_words",
    "rsp_cmds",
    "ovl_switch",
    "rsp_flush",
    "tex_upload",
    "tex_resident",
    "cpu_tris",
];

/// Stages of a scene line, µs per frame: the frame (update + render −
/// begin, what the N64 ceilings gate) and the renderer's stages.
pub const STAGES: [&str; 8] = [
    "frame", "update", "render", "tnl", "sort", "submit", "sprites", "end",
];

/// The T&L work fields that must be identical when only the code layout
/// changed (`xtask/src/perf.rs`'s `WORK_FIELDS`).
pub const WORK: [&str; 9] = [
    "verts",
    "tris_in",
    "clipped",
    "splits",
    "backfaced",
    "emitted",
    "culled",
    "rsp_draws",
    "rsp_tris",
];

/// One `key=value` log line: the scene, then its fields.
pub fn parse_fields(line: &str, prefix: &str) -> Option<(String, BTreeMap<String, String>)> {
    let rest = &line[line.find(prefix)? + prefix.len()..];
    let mut parts = rest.split_whitespace();
    let scene = parts.next()?.to_string();
    let fields = parts
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();
    Some((scene, fields))
}

fn num(f: &BTreeMap<String, String>, k: &str) -> u64 {
    f.get(k).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// The frame time a scene line gates on: update + render − begin.
pub fn frame_us(fields: &BTreeMap<String, u64>) -> u64 {
    let g = |k: &str| fields.get(k).copied().unwrap_or(0);
    (g("update") + g("render")).saturating_sub(g("begin"))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelLine {
    pub model: String,
    pub calls: u64,
    pub us: u64,
    pub verts: u64,
    pub tris_in: u64,
    pub emitted: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RspEntry {
    pub ovl: String,
    pub verts: u64,
    pub tris: u64,
    /// Commands of this shape.
    pub n: u64,
    /// Their summed µs (each minus the empty drain).
    pub us: u64,
}

/// µs = per_cmd + per_vert · verts + per_tri · tris, least squares over an
/// overlay's commands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RspFit {
    pub ovl: String,
    pub commands: u64,
    pub mean_us: f64,
    pub per_cmd: f64,
    pub per_vert: f64,
    pub per_tri: f64,
    /// How much of the variance the fit explains (1 = all).
    pub r2: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RspReport {
    pub empty_drain_us: u64,
    pub entries: Vec<RspEntry>,
    pub fits: Vec<RspFit>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FnSamples {
    pub name: String,
    pub samples: u64,
    pub pct: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SampleReport {
    pub samples: u64,
    pub lost: u64,
    pub period_us: u64,
    pub frames: u64,
    /// The interrupt's own cost per sample, µs, and what it adds to the
    /// sampled frames (the ROM measures it with a calibration loop).
    pub handler_us: f64,
    pub overhead_pct: f64,
    /// Functions by self samples, most first.
    pub functions: Vec<FnSamples>,
    /// Samples no symbol covers.
    pub unknown: u64,
}

/// One perf scene of one run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfReport {
    pub format: String,
    pub version: u32,
    /// `n64`, `3ds`.
    pub target: String,
    pub scene: String,
    /// What the ROM was: `plain`, `trace`, `prof-rsp`, `prof-sample`.
    pub build: String,
    /// `TRINO_N64_TEXT_PAD` of the build (0 = none).
    pub pad: u32,
    /// Every field of the scene line, per frame.
    pub fields: BTreeMap<String, u64>,
    pub models: Vec<ModelLine>,
    /// The counters among `fields` (instrumented builds).
    pub counters: BTreeMap<String, u64>,
    /// The session's zones (`trino_trace::stats::SessionStats::to_json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zones: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rsp: Option<RspReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<SampleReport>,
}

impl PerfReport {
    pub fn new(target: &str, scene: &str, build: &str, pad: u32) -> PerfReport {
        PerfReport {
            format: FORMAT.into(),
            version: VERSION,
            target: target.into(),
            scene: scene.into(),
            build: build.into(),
            pad,
            ..PerfReport::default()
        }
    }

    pub fn frame_us(&self) -> u64 {
        frame_us(&self.fields)
    }

    /// A stage's µs per frame (`frame` computed).
    pub fn stage(&self, name: &str) -> u64 {
        if name == "frame" {
            self.frame_us()
        } else {
            self.fields.get(name).copied().unwrap_or(0)
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("plain data")
    }

    pub fn from_json(text: &str) -> Result<PerfReport, String> {
        let r: PerfReport = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if r.format != FORMAT {
            return Err(format!("not a perf report (format {:?})", r.format));
        }
        Ok(r)
    }

    /// The human table `cargo xtask perf report` prints.
    pub fn table(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} {} ({} build{}): frame {} us",
            self.target,
            self.scene,
            self.build,
            if self.pad > 0 {
                format!(", pad {}", self.pad)
            } else {
                String::new()
            },
            self.frame_us()
        );
        let _ = write!(s, "  stages us/frame:");
        for st in STAGES.iter().skip(1) {
            let _ = write!(s, " {st}={}", self.stage(st));
        }
        let _ = writeln!(s);
        let _ = write!(s, "  work/frame:");
        for w in WORK {
            let _ = write!(s, " {w}={}", self.fields.get(w).copied().unwrap_or(0));
        }
        let _ = writeln!(s);
        if !self.counters.is_empty() {
            let _ = write!(s, "  counters/frame:");
            for (k, v) in &self.counters {
                let _ = write!(s, " {k}={v}");
            }
            let _ = writeln!(s);
        }
        if let Some(z) = self.zones.as_ref().and_then(|z| z["zones"].as_array()) {
            let _ = writeln!(s, "  zones (us): name count mean p95 per_frame");
            for zone in z {
                let _ = writeln!(
                    s,
                    "    {:<14} {:>6} {:>8.1} {:>6} {:>8.1}",
                    zone["name"].as_str().unwrap_or("?"),
                    zone["count"].as_u64().unwrap_or(0),
                    zone["mean_us"].as_f64().unwrap_or(0.0),
                    zone["p95_us"].as_u64().unwrap_or(0),
                    zone["per_frame_us"].as_f64().unwrap_or(0.0),
                );
            }
        }
        if let Some(r) = &self.rsp {
            let _ = writeln!(
                s,
                "  RSP per command (empty drain {} us subtracted): overlay cmds mean_us \
                 = per_cmd + per_vert·v + per_tri·t (r²)",
                r.empty_drain_us
            );
            for f in &r.fits {
                let _ = writeln!(
                    s,
                    "    {:<8} {:>5} {:>7.1} = {:.1} + {:.3}·v + {:.3}·t ({:.2})",
                    f.ovl, f.commands, f.mean_us, f.per_cmd, f.per_vert, f.per_tri, f.r2
                );
            }
        }
        if let Some(p) = &self.samples {
            let _ = writeln!(
                s,
                "  CPU samples: {} over {} frames every {} us ({} lost, {} unknown), handler \
                 {:.2} us/sample = {:.1}% of the sampled time",
                p.samples, p.frames, p.period_us, p.lost, p.unknown, p.handler_us, p.overhead_pct
            );
            for f in p.functions.iter().take(25) {
                let _ = writeln!(s, "    {:>5.1}% {:>6}  {}", f.pct, f.samples, f.name);
            }
        }
        s
    }
}

/// A scene's `TRINO_PROF_SAMPLE:` lines: the totals' fields, then the
/// samples per PC.
type SampleLines = (BTreeMap<String, String>, BTreeMap<u32, u64>);

/// One run's scenes: scene → its line's fields.
pub type RunScenes = BTreeMap<String, BTreeMap<String, u64>>;

/// Builds a run's reports from its log lines (`feed` every line).
#[derive(Default)]
pub struct Collector {
    scenes: BTreeMap<String, PerfReport>,
    rsp: BTreeMap<String, RspReport>,
    pcs: BTreeMap<String, SampleLines>,
}

impl Collector {
    /// Take a line if it is one of the report's; `true` when it was.
    pub fn feed(&mut self, target: &str, build: &str, pad: u32, line: &str) -> bool {
        if let Some((scene, f)) = parse_fields(line, "TRINO_PERF_SCENE:") {
            let r = self
                .scenes
                .entry(scene.clone())
                .or_insert_with(|| PerfReport::new(target, &scene, build, pad));
            for (k, v) in &f {
                if let Ok(n) = v.parse::<u64>() {
                    r.fields.insert(k.clone(), n);
                    if COUNTERS.contains(&k.as_str()) {
                        r.counters.insert(k.clone(), n);
                    }
                }
            }
            return true;
        }
        if let Some((scene, f)) = parse_fields(line, "TRINO_PERF_MODEL:") {
            if let Some(r) = self.scenes.get_mut(&scene) {
                r.models.push(ModelLine {
                    model: f.get("model").cloned().unwrap_or_default(),
                    calls: num(&f, "calls"),
                    us: num(&f, "us"),
                    verts: num(&f, "verts"),
                    tris_in: num(&f, "tris_in"),
                    emitted: num(&f, "emitted"),
                });
            }
            return true;
        }
        if let Some((scene, f)) = parse_fields(line, "TRINO_PROF_RSP:") {
            let r = self.rsp.entry(scene).or_default();
            if let Some(e) = f.get("empty_drain_us") {
                r.empty_drain_us = e.parse().unwrap_or(0);
            } else {
                r.entries.push(RspEntry {
                    ovl: f.get("ovl").cloned().unwrap_or_default(),
                    verts: num(&f, "verts"),
                    tris: num(&f, "tris"),
                    n: num(&f, "n"),
                    us: num(&f, "us"),
                });
            }
            return true;
        }
        if let Some(i) = line.find("TRINO_PROF_SAMPLE:") {
            let rest = &line[i + "TRINO_PROF_SAMPLE:".len()..];
            let mut parts = rest.split_whitespace();
            let Some(scene) = parts.next() else {
                return true;
            };
            let e = self.pcs.entry(scene.to_string()).or_default();
            for kv in parts {
                let Some((k, v)) = kv.split_once('=') else {
                    continue;
                };
                match u32::from_str_radix(k, 16) {
                    Ok(pc) if k.len() == 8 => {
                        *e.1.entry(pc).or_insert(0) += v.parse::<u64>().unwrap_or(0);
                    }
                    _ => {
                        e.0.insert(k.to_string(), v.to_string());
                    }
                }
            }
            return true;
        }
        false
    }

    /// The reports, with the RSP fits, the samples symbolized by `syms`
    /// (`nm -S -C` text; `None` leaves addresses) and each scene's zones
    /// from `zones(scene)`.
    pub fn finish(
        mut self,
        syms: Option<&Symbols>,
        zones: impl Fn(&str) -> Option<Value>,
    ) -> Vec<PerfReport> {
        for (scene, mut r) in std::mem::take(&mut self.rsp) {
            r.fits = fit_overlays(&r.entries);
            if let Some(rep) = self.scenes.get_mut(&scene) {
                rep.rsp = Some(r);
            }
        }
        for (scene, (meta, pcs)) in std::mem::take(&mut self.pcs) {
            if let Some(rep) = self.scenes.get_mut(&scene) {
                rep.samples = Some(samples(&meta, &pcs, syms));
            }
        }
        let mut out: Vec<PerfReport> = self.scenes.into_values().collect();
        for r in &mut out {
            r.zones = zones(&r.scene);
        }
        out
    }
}

fn samples(
    meta: &BTreeMap<String, String>,
    pcs: &BTreeMap<u32, u64>,
    syms: Option<&Symbols>,
) -> SampleReport {
    let g = |k: &str| meta.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let gf = |k: &str| {
        meta.get(k)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let total: u64 = pcs.values().sum();
    let mut by: BTreeMap<String, u64> = BTreeMap::new();
    let mut unknown = 0;
    for (&pc, &n) in pcs {
        match syms.and_then(|s| s.lookup(pc)) {
            Some(name) => *by.entry(name.to_string()).or_insert(0) += n,
            None if syms.is_some() => unknown += n,
            None => *by.entry(format!("{pc:08x}")).or_insert(0) += n,
        }
    }
    let mut functions: Vec<FnSamples> = by
        .into_iter()
        .map(|(name, samples)| FnSamples {
            name,
            samples,
            pct: 100.0 * samples as f64 / total.max(1) as f64,
        })
        .collect();
    functions.sort_by(|a, b| b.samples.cmp(&a.samples).then(a.name.cmp(&b.name)));
    SampleReport {
        samples: g("samples").max(total),
        lost: g("lost"),
        period_us: g("period_us"),
        frames: g("frames"),
        handler_us: gf("handler_us"),
        overhead_pct: gf("overhead_pct"),
        functions,
        unknown,
    }
}

/// An ELF's function symbols (`mips64-elf-nm -S -C` lines: address, size,
/// type, name), for symbolizing sampled PCs.
#[derive(Default)]
pub struct Symbols {
    /// (start, end, name), sorted by start.
    ranges: Vec<(u32, u32, String)>,
}

impl Symbols {
    pub fn parse(nm: &str) -> Symbols {
        let mut ranges: Vec<(u32, u32, String)> = nm
            .lines()
            .filter_map(|l| {
                let mut p = l.splitn(4, ' ');
                let addr = u64::from_str_radix(p.next()?, 16).ok()? as u32;
                let size = u64::from_str_radix(p.next()?, 16).ok()? as u32;
                let kind = p.next()?;
                if !matches!(kind, "T" | "t" | "W" | "w") || size == 0 {
                    return None;
                }
                Some((addr, addr.wrapping_add(size), p.next()?.to_string()))
            })
            .collect();
        ranges.sort_by_key(|r| r.0);
        Symbols { ranges }
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The function holding `pc`.
    pub fn lookup(&self, pc: u32) -> Option<&str> {
        let i = self.ranges.partition_point(|r| r.0 <= pc);
        let r = self.ranges.get(i.checked_sub(1)?)?;
        (pc < r.1).then_some(r.2.as_str())
    }
}

/// Least-squares fits of each overlay's commands: µs per command, per
/// vertex and per triangle. A model with fewer distinct shapes than
/// parameters drops the per-triangle term, then the per-vertex one.
pub fn fit_overlays(entries: &[RspEntry]) -> Vec<RspFit> {
    let mut ovls: Vec<&str> = entries.iter().map(|e| e.ovl.as_str()).collect();
    ovls.sort_unstable();
    ovls.dedup();
    ovls.into_iter()
        .map(|ovl| {
            let es: Vec<&RspEntry> = entries.iter().filter(|e| e.ovl == ovl).collect();
            fit(ovl, &es)
        })
        .collect()
}

fn fit(ovl: &str, es: &[&RspEntry]) -> RspFit {
    let n: u64 = es.iter().map(|e| e.n).sum();
    let us: u64 = es.iter().map(|e| e.us).sum();
    let mean = us as f64 / n.max(1) as f64;
    // Weighted rows: each entry is `n` commands of mean `us / n`.
    let rows: Vec<([f64; 3], f64, f64)> = es
        .iter()
        .filter(|e| e.n > 0)
        .map(|e| {
            (
                [1.0, e.verts as f64, e.tris as f64],
                e.us as f64 / e.n as f64,
                e.n as f64,
            )
        })
        .collect();
    let mut best = [mean, 0.0, 0.0];
    for k in [3usize, 2] {
        if let Some(c) = solve(&rows, k) {
            best = [c[0], c[1], if k == 3 { c[2] } else { 0.0 }];
            break;
        }
    }
    let (mut ss_res, mut ss_tot) = (0.0, 0.0);
    for (x, y, w) in &rows {
        let p = best[0] + best[1] * x[1] + best[2] * x[2];
        ss_res += w * (y - p) * (y - p);
        ss_tot += w * (y - mean) * (y - mean);
    }
    // Four decimals: what a µs fit can mean, and what JSON gives back.
    let r = |v: f64| (v * 1e4).round() / 1e4;
    RspFit {
        ovl: ovl.into(),
        commands: n,
        mean_us: r(mean),
        per_cmd: r(best[0]) + 0.0,
        per_vert: r(best[1]) + 0.0,
        per_tri: r(best[2]) + 0.0,
        r2: r(if ss_tot > 0.0 {
            1.0 - ss_res / ss_tot
        } else {
            1.0
        }),
    }
}

/// Weighted least squares on the first `k` columns (normal equations,
/// Gauss–Jordan); `None` when singular.
fn solve(rows: &[([f64; 3], f64, f64)], k: usize) -> Option<Vec<f64>> {
    let mut a = vec![vec![0.0f64; k + 1]; k];
    for (x, y, w) in rows {
        for i in 0..k {
            for j in 0..k {
                a[i][j] += w * x[i] * x[j];
            }
            a[i][k] += w * x[i] * y;
        }
    }
    for c in 0..k {
        let p = (c..k).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-9 * (1.0 + a[c][c].abs()) {
            return None;
        }
        a.swap(c, p);
        let d = a[c][c];
        for v in a[c].iter_mut() {
            *v /= d;
        }
        for r in 0..k {
            if r != c {
                let f = a[r][c];
                let row = a[c].clone();
                for (v, rv) in a[r].iter_mut().zip(row) {
                    *v -= f * rv;
                }
            }
        }
    }
    Some(a.iter().map(|r| r[k]).collect())
}

/// Mean, min and max of a stage over the paddings of one arm, and each
/// padding's value (in padding order).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub mean: f64,
    pub min: u64,
    pub max: u64,
    pub per_pad: Vec<u64>,
}

impl Spread {
    pub fn of(values: &[u64]) -> Spread {
        if values.is_empty() {
            return Spread::default();
        }
        Spread {
            mean: values.iter().sum::<u64>() as f64 / values.len() as f64,
            min: *values.iter().min().unwrap(),
            max: *values.iter().max().unwrap(),
            per_pad: values.to_vec(),
        }
    }
}

/// One arm of an A/B run (`base`, `head` or `control`) on one scene.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AbArm {
    pub arm: String,
    /// Stage → spread over the paddings.
    pub stages: BTreeMap<String, Spread>,
    /// The T&L work, when every padding agreed (else empty and
    /// `work_stable` false).
    pub work: BTreeMap<String, u64>,
    pub work_stable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AbScene {
    pub scene: String,
    pub arms: Vec<AbArm>,
    /// Every arm did the same T&L work (so a time difference is the code's
    /// speed or its layout, not more or less work).
    pub same_work: bool,
}

/// A whole `cargo xtask perf ab` run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AbReport {
    pub format: String,
    pub version: u32,
    pub target: String,
    pub base: String,
    pub head: String,
    /// The control arm: the base with a no-op patch (layout only).
    pub control: bool,
    pub paddings: Vec<u32>,
    pub scenes: Vec<AbScene>,
}

impl AbReport {
    /// Build from each arm's per-padding scene fields:
    /// `runs[arm][pad index]` = scene → fields.
    pub fn build(
        target: &str,
        base: &str,
        head: &str,
        paddings: &[u32],
        runs: &[(String, Vec<RunScenes>)],
    ) -> AbReport {
        let mut names: Vec<String> = runs
            .iter()
            .flat_map(|(_, pads)| pads.iter().flat_map(|p| p.keys().cloned()))
            .collect();
        names.sort();
        names.dedup();
        let scenes = names
            .into_iter()
            .map(|scene| {
                let arms: Vec<AbArm> = runs
                    .iter()
                    .map(|(arm, pads)| {
                        let found: Vec<&BTreeMap<String, u64>> =
                            pads.iter().filter_map(|p| p.get(&scene)).collect();
                        let stages = STAGES
                            .iter()
                            .map(|st| {
                                let v: Vec<u64> = found
                                    .iter()
                                    .map(|f| {
                                        if *st == "frame" {
                                            frame_us(f)
                                        } else {
                                            f.get(*st).copied().unwrap_or(0)
                                        }
                                    })
                                    .collect();
                                (st.to_string(), Spread::of(&v))
                            })
                            .collect();
                        let work_of = |f: &BTreeMap<String, u64>| -> BTreeMap<String, u64> {
                            WORK.iter()
                                .map(|w| (w.to_string(), f.get(*w).copied().unwrap_or(0)))
                                .collect()
                        };
                        let first = found.first().map(|f| work_of(f)).unwrap_or_default();
                        let work_stable = found.iter().all(|f| work_of(f) == first);
                        AbArm {
                            arm: arm.clone(),
                            stages,
                            work: if work_stable { first } else { BTreeMap::new() },
                            work_stable,
                        }
                    })
                    .collect();
                let same_work = arms.iter().all(|a| a.work_stable)
                    && arms.windows(2).all(|w| w[0].work == w[1].work);
                AbScene {
                    scene,
                    arms,
                    same_work,
                }
            })
            .collect();
        AbReport {
            format: AB_FORMAT.into(),
            version: VERSION,
            target: target.into(),
            base: base.into(),
            head: head.into(),
            control: runs.iter().any(|(a, _)| a == "control"),
            paddings: paddings.to_vec(),
            scenes,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("plain data")
    }

    pub fn from_json(text: &str) -> Result<AbReport, String> {
        let r: AbReport = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if r.format != AB_FORMAT {
            return Err(format!("not an A/B report (format {:?})", r.format));
        }
        Ok(r)
    }

    /// The table `perf ab` prints: per scene and stage, each arm's mean
    /// (min–max), the head − base delta, and the control's delta beside it
    /// (what the layout alone moves).
    pub fn table(&self, stages: &[&str]) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} A/B over {} paddings ({}): base {} vs head {}{}",
            self.target,
            self.paddings.len(),
            self.paddings
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(","),
            self.base,
            self.head,
            if self.control {
                ", control = base + a no-op patch"
            } else {
                ""
            }
        );
        for sc in &self.scenes {
            let _ = writeln!(
                s,
                "\n[{}] work {}",
                sc.scene,
                if sc.same_work {
                    "identical in every arm"
                } else {
                    "DIFFERS between arms"
                }
            );
            let base = sc.arms.iter().find(|a| a.arm == "base");
            for st in stages {
                let _ = write!(s, "  {st:<8}");
                for a in &sc.arms {
                    if let Some(v) = a.stages.get(*st) {
                        let _ = write!(s, " {}={:.0} ({}–{})", a.arm, v.mean, v.min, v.max);
                    }
                }
                if let Some(b) = base.and_then(|b| b.stages.get(*st)) {
                    for a in sc.arms.iter().filter(|a| a.arm != "base") {
                        if let Some(v) = a.stages.get(*st) {
                            let d = v.mean - b.mean;
                            let pct = if b.mean > 0.0 {
                                100.0 * d / b.mean
                            } else {
                                0.0
                            };
                            let _ = write!(s, "  {}−base {d:+.0} ({pct:+.1}%)", a.arm);
                        }
                    }
                }
                let _ = writeln!(s);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_becomes_reports_with_counters_fits_and_symbols() {
        let mut c = Collector::default();
        let lines = [
            "[ares] TRINO_PERF_SCENE:hub frames=4 update=300 render=12000 begin=1000 tnl=4000 \
             verts=10 rsp_draws=3 memo_hit=40 rspq_words=900",
            "[ares] TRINO_PERF_MODEL:hub model=0000abcd calls=2 us=150 verts=8 tris_in=4 emitted=4 splits=0",
            "TRINO_PROF_RSP:hub empty_drain_us=45",
            "TRINO_PROF_RSP:hub ovl=white verts=10 tris=10 n=2 us=40",
            "TRINO_PROF_RSP:hub ovl=white verts=20 tris=10 n=1 us=30",
            "TRINO_PROF_RSP:hub ovl=white verts=20 tris=30 n=1 us=50",
            "TRINO_PROF_SAMPLE:hub samples=4 lost=0 period_us=50 frames=4 pcs=2 handler_us=0.5 overhead_pct=1.0",
            "TRINO_PROF_SAMPLE:hub 80001004=3 80002000=1",
            "TRINO_FRAME:hub avg_us=1",
        ];
        let mut taken = 0;
        for l in lines {
            taken += c.feed("n64", "prof-sample", 0, l) as usize;
        }
        assert_eq!(taken, lines.len() - 1, "TRINO_FRAME is not a report line");
        let syms = Symbols::parse(
            "80001000 00000010 T trino_core::render3d::tessellate_joints\n\
             80003000 00000008 D some_data\n",
        );
        let reps = c.finish(Some(&syms), |s| {
            (s == "hub").then(|| serde_json::json!({"zones": []}))
        });
        assert_eq!(reps.len(), 1);
        let r = &reps[0];
        assert_eq!(r.frame_us(), 11_300);
        assert_eq!(r.counters["memo_hit"], 40);
        assert_eq!(r.counters["rspq_words"], 900);
        assert!(!r.counters.contains_key("verts"), "work is not a counter");
        assert_eq!(r.models.len(), 1);
        let rsp = r.rsp.as_ref().unwrap();
        assert_eq!(rsp.empty_drain_us, 45);
        // 10 µs a command + 1 per vertex + 1 per triangle, exactly.
        let f = &rsp.fits[0];
        assert!(f.per_cmd.abs() < 1e-6, "{f:?}");
        assert!((f.per_vert - 1.0).abs() < 1e-6, "{f:?}");
        assert!((f.per_tri - 1.0).abs() < 1e-6, "{f:?}");
        assert!((f.r2 - 1.0).abs() < 1e-9, "{f:?}");
        let smp = r.samples.as_ref().unwrap();
        assert_eq!(
            smp.functions[0].name,
            "trino_core::render3d::tessellate_joints"
        );
        assert_eq!(smp.functions[0].samples, 3);
        assert_eq!(smp.unknown, 1, "a PC no function covers");
        assert!((smp.handler_us - 0.5).abs() < 1e-9);
        assert!(r.zones.is_some());
        let back = PerfReport::from_json(&r.to_json()).unwrap();
        assert_eq!(&back, r);
        let t = r.table();
        assert!(
            t.contains("memo_hit=40") && t.contains("tessellate_joints"),
            "{t}"
        );
    }

    #[test]
    fn rsp_fits_drop_terms_the_data_cannot_tell_apart() {
        let e = |v, t, n, us| RspEntry {
            ovl: "color".into(),
            verts: v,
            tris: t,
            n,
            us,
        };
        // One shape only: the mean, per command.
        let f = &fit_overlays(&[e(10, 10, 4, 400)])[0];
        assert_eq!((f.per_cmd, f.per_vert, f.per_tri), (100.0, 0.0, 0.0));
        // verts == tris always: no per-triangle term, a line in verts.
        let f = &fit_overlays(&[e(10, 10, 1, 30), e(20, 20, 1, 50)])[0];
        assert!(
            (f.per_cmd - 10.0).abs() < 1e-6 && (f.per_vert - 2.0).abs() < 1e-6,
            "{f:?}"
        );
        assert_eq!(f.per_tri, 0.0);
    }

    #[test]
    fn ab_reports_spread_per_padding_and_tell_work_apart() {
        let scene = |frame: u64, verts: u64| {
            let mut f = BTreeMap::new();
            f.insert("update".to_string(), 0);
            f.insert("render".to_string(), frame);
            f.insert("verts".to_string(), verts);
            let mut m = BTreeMap::new();
            m.insert("hub".to_string(), f);
            m
        };
        let runs = vec![
            ("base".to_string(), vec![scene(100, 7), scene(120, 7)]),
            ("head".to_string(), vec![scene(90, 7), scene(110, 7)]),
            ("control".to_string(), vec![scene(105, 7), scene(125, 7)]),
        ];
        let ab = AbReport::build("n64", "main", "HEAD", &[0, 352], &runs);
        let hub = &ab.scenes[0];
        assert!(hub.same_work && ab.control);
        let base = &hub.arms[0].stages["frame"];
        assert_eq!((base.mean, base.min, base.max), (110.0, 100, 120));
        assert_eq!(base.per_pad, vec![100, 120]);
        let t = ab.table(&["frame"]);
        assert!(
            t.contains("head−base -10") && t.contains("control−base +5"),
            "{t}"
        );
        assert!(t.contains("work identical"), "{t}");
        assert_eq!(AbReport::from_json(&ab.to_json()).unwrap(), ab);
        // More work in one arm: not a layout question.
        let runs = vec![
            ("base".to_string(), vec![scene(100, 7)]),
            ("head".to_string(), vec![scene(100, 8)]),
        ];
        let ab = AbReport::build("n64", "main", "HEAD", &[0], &runs);
        assert!(!ab.scenes[0].same_work);
    }
}
