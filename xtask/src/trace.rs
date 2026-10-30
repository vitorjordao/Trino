//! `cargo xtask trace` (issue #62, ADR 0030): timeline traces from every
//! target in one format.
//!
//! - `trace collect <log..> [--out <file.json>] [--json]` — the
//!   `TRINO_TRACE:` lines of emulator, hardware or PC logs into Chrome Trace
//!   Event / Perfetto JSON (default `target/traces/<log name>.json`).
//! - `trace report <trace> [--session s] [--json]` — per-zone count, mean,
//!   p95, max and time per frame, frame times, counters.
//! - `trace diff <a> <b> [--session a=b] [--json]` — B against A per zone:
//!   the evidence a perf PR quotes.
//!
//! A `<trace>` is a `.json` file (ours, or any Chrome trace) or a log.
//! `cargo xtask test n64|3ds|web` collect their run's lines on their own
//! ([`Capture`]): the self-test's `trace` step on every boot, and with
//! `--trace` the engine's zones (a separate, instrumented build).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use trino_trace::{Diff, SessionStats, Trace, chrome, stats::pair_sessions};

const USAGE: &str = "\
usage:
  cargo xtask trace collect <log..> [--out <file.json>] [--json]
  cargo xtask trace report <trace> [--session <name>] [--json]
  cargo xtask trace diff <a> <b> [--session <a-name>=<b-name>] [--json]
(a <trace> is a .json trace or a log with TRINO_TRACE: lines)";

/// Zone and counter names the castle64 self-test's `trace` step records
/// (`castle64::selftest::TRACE_NAMES`; `xtask/tests/trace_cli.rs` holds the
/// two equal).
pub const SELFTEST_NAMES: [&str; 6] = ["frame", "update", "physics", "render", "draws", "present"];
/// Its session name (`castle64::selftest::TRACE_SESSION`).
pub const SELFTEST_SESSION: &str = "selftest";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let result = match args {
        ["collect", rest @ ..] => collect_cmd(root, rest),
        ["report", rest @ ..] => report_cmd(rest),
        ["diff", rest @ ..] => diff_cmd(rest),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask trace: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Opts<'a> {
    files: Vec<&'a str>,
    out: Option<PathBuf>,
    session: Option<&'a str>,
    json: bool,
}

fn opts<'a>(args: &[&'a str]) -> Result<Opts<'a>, String> {
    let mut o = Opts {
        files: Vec::new(),
        out: None,
        session: None,
        json: false,
    };
    let mut it = args.iter();
    while let Some(&a) = it.next() {
        match a {
            "--json" => o.json = true,
            "--out" => o.out = Some(PathBuf::from(it.next().ok_or("--out needs a file")?)),
            "--session" => o.session = Some(it.next().ok_or("--session needs a name")?),
            f if f.starts_with("--") => return Err(format!("unknown option {f}\n{USAGE}")),
            f => o.files.push(f),
        }
    }
    Ok(o)
}

/// Write `trace` as Chrome JSON to `out` (creating its folder).
pub fn write_json(trace: &Trace, out: &Path) -> Result<(), String> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string(&chrome::write(trace)).map_err(|e| e.to_string())?;
    std::fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))
}

/// One line per session: name, platform, events, frames, lost events.
fn summary(trace: &Trace) -> Vec<Value> {
    trace
        .sessions
        .iter()
        .map(|s| {
            json!({"session": s.name, "platform": s.platform, "zones": s.zones.len(),
                "counters": s.counters.len(), "frames": s.frames.len(), "dropped": s.dropped})
        })
        .collect()
}

fn collect_cmd(root: &Path, args: &[&str]) -> Result<(), String> {
    let o = opts(args)?;
    if o.files.is_empty() {
        return Err(USAGE.into());
    }
    let mut trace = Trace::default();
    for f in &o.files {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        let t = trino_trace::collect(&text).map_err(|e| format!("{f}: {e}"))?;
        trace.sessions.extend(t.sessions);
    }
    if trace.sessions.is_empty() {
        return Err(format!(
            "no {} lines in {}",
            trino_core::trace::LINE_PREFIX,
            o.files.join(", ")
        ));
    }
    let out = o.out.unwrap_or_else(|| {
        let stem = Path::new(o.files[0])
            .file_stem()
            .map_or("trace".into(), |s| s.to_string_lossy().into_owned());
        root.join("target/traces").join(format!("{stem}.json"))
    });
    write_json(&trace, &out)?;
    if o.json {
        println!(
            "{}",
            json!({"out": out.display().to_string(), "sessions": summary(&trace)})
        );
    } else {
        for s in summary(&trace) {
            println!(
                "{}: {} ({} zones, {} counters, {} frames, {} lost)",
                s["session"].as_str().unwrap_or(""),
                s["platform"].as_str().unwrap_or(""),
                s["zones"],
                s["counters"],
                s["frames"],
                s["dropped"]
            );
        }
        println!(
            "wrote {} (open it in https://ui.perfetto.dev or chrome://tracing)",
            out.display()
        );
    }
    Ok(())
}

fn load(file: &str) -> Result<Trace, String> {
    chrome::load(Path::new(file))
}

fn report_cmd(args: &[&str]) -> Result<(), String> {
    let o = opts(args)?;
    let [file] = o.files[..] else {
        return Err(USAGE.into());
    };
    let trace = load(file)?;
    let sessions: Vec<_> = trace
        .sessions
        .iter()
        .filter(|s| o.session.is_none_or(|n| s.name == n))
        .collect();
    if sessions.is_empty() {
        return Err(format!(
            "{file}: no session{}",
            o.session.map_or(String::new(), |s| format!(" named {s}"))
        ));
    }
    let stats: Vec<SessionStats> = sessions.iter().map(|s| SessionStats::of(s)).collect();
    if o.json {
        println!(
            "{}",
            json!({"file": file, "sessions": stats.iter().map(SessionStats::to_json).collect::<Vec<_>>()})
        );
    } else {
        for s in &stats {
            println!("{}", s.table());
        }
    }
    Ok(())
}

fn diff_cmd(args: &[&str]) -> Result<(), String> {
    let o = opts(args)?;
    let [fa, fb] = o.files[..] else {
        return Err(USAGE.into());
    };
    let (a, b) = (load(fa)?, load(fb)?);
    let pairs = match o.session {
        Some(spec) => {
            let (na, nb) = spec.split_once('=').unwrap_or((spec, spec));
            let sa = a.session(na).ok_or(format!("{fa}: no session {na}"))?;
            let sb = b.session(nb).ok_or(format!("{fb}: no session {nb}"))?;
            vec![(sa, sb)]
        }
        None => pair_sessions(&a, &b),
    };
    if pairs.is_empty() {
        return Err(format!(
            "no sessions to pair between {fa} and {fb} (name them: --session <a>=<b>)"
        ));
    }
    let diffs: Vec<Diff> = pairs.iter().map(|(x, y)| Diff::between(x, y)).collect();
    if o.json {
        println!(
            "{}",
            json!({"a": fa, "b": fb, "diffs": diffs.iter().map(Diff::to_json).collect::<Vec<_>>()})
        );
    } else {
        for d in &diffs {
            println!("{}", d.table());
        }
    }
    Ok(())
}

/// After `run pc --trace`: the game's log into `out`, with a summary (a
/// note, never a failure: the game ran either way).
pub fn collect_run(log: &Path, out: &Path) {
    let result = std::fs::read_to_string(log)
        .map_err(|e| format!("{}: {e}", log.display()))
        .and_then(|t| trino_trace::collect(&t).map_err(|e| e.to_string()))
        .and_then(|t| write_json(&t, out).map(|()| t));
    match result {
        Ok(t) => {
            for s in &t.sessions {
                println!("{}", SessionStats::of(s).table());
            }
            println!(
                "[pc] trace: {} events -> {} (https://ui.perfetto.dev)",
                t.event_count(),
                out.display()
            );
        }
        Err(e) => eprintln!("[pc] trace: {e}"),
    }
}

/// The `TRINO_TRACE:` lines of an emulator or browser run, kept apart from
/// the echoed log (a `--trace` run prints thousands).
#[derive(Default)]
pub struct Capture {
    text: String,
    lines: usize,
}

impl Capture {
    /// Keep `line` when it is a trace line; `true` = consumed (don't echo).
    pub fn feed(&mut self, line: &str) -> bool {
        if !line.contains(trino_core::trace::LINE_PREFIX) {
            return false;
        }
        self.text.push_str(line);
        self.text.push('\n');
        self.lines += 1;
        true
    }

    /// The captured lines decoded (`None` without any).
    pub fn trace(&self) -> Option<Trace> {
        if self.lines == 0 {
            return None;
        }
        trino_trace::collect(&self.text).ok()
    }

    /// Decode what was captured, check the self-test's session (`engine`:
    /// castle64's step must be there with its names), and write
    /// `target/traces/<name>.json`. Returns a summary to print.
    pub fn finish(&self, root: &Path, name: &str, engine: bool) -> Result<String, String> {
        let trace = trino_trace::collect(&self.text).map_err(|e| e.to_string())?;
        if engine {
            check_selftest(&trace)?;
        }
        if trace.sessions.is_empty() {
            return Ok("no trace lines".into());
        }
        let out = root.join("target/traces").join(format!("{name}.json"));
        write_json(&trace, &out)?;
        // The raw lines too: `trace collect` reads them again.
        let _ = std::fs::write(out.with_extension("log"), &self.text);
        let mut msg = format!(
            "{} lines, {} sessions, {} events -> {}",
            self.lines,
            trace.sessions.len(),
            trace.event_count(),
            out.display()
        );
        for s in trace.sessions.iter().filter(|s| s.name != SELFTEST_SESSION) {
            let st = SessionStats::of(s);
            msg.push_str(&format!(
                "\n  {}: {} frames, mean {:.0} us, p95 {} us{}",
                s.name,
                st.frames.count,
                st.frames.mean_us,
                st.frames.p95_us,
                if s.dropped > 0 {
                    format!(", {} events lost", s.dropped)
                } else {
                    String::new()
                }
            ));
        }
        Ok(msg)
    }
}

/// The self-test's `trace` session decoded on the host too, with every name
/// the step records.
pub fn check_selftest(trace: &Trace) -> Result<(), String> {
    let s = trace
        .session(SELFTEST_SESSION)
        .ok_or("trace: the self-test printed no `selftest` trace session (the `trace` step)")?;
    for n in SELFTEST_NAMES {
        if !s.names.iter().any(|x| x == n) {
            return Err(format!("trace: the self-test's session lacks `{n}`"));
        }
    }
    if s.zones.len() < 4 || s.frames.len() != 2 || s.counters.len() != 1 {
        return Err(format!(
            "trace: the self-test's session has {} zones, {} frames, {} counters (want 4, 2, 1)",
            s.zones.len(),
            s.frames.len(),
            s.counters.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::trace::{TracePlatform, TraceRing, track};

    fn selftest_lines() -> String {
        let mut r = TraceRing::<16>::new();
        r.frame(0, 1);
        r.open(track::CPU);
        r.zone("physics", 0, 2, 3);
        r.close(track::CPU);
        r.zone("update", 0, 1, 4);
        r.open(track::CPU);
        r.zone("present", 0, 6, 7);
        r.close(track::CPU);
        r.zone("render", 0, 5, 8);
        r.counter("draws", 0, 9, 1);
        r.frame(10, 2);
        let mut s = String::new();
        r.write_lines(TracePlatform::N64, SELFTEST_SESSION, &mut 0, &mut s)
            .unwrap();
        s
    }

    #[test]
    fn the_selftest_names_are_castle64s() {
        assert_eq!(SELFTEST_NAMES, castle64::selftest::TRACE_NAMES);
        assert_eq!(SELFTEST_SESSION, castle64::selftest::TRACE_SESSION);
    }

    #[test]
    fn capture_takes_trace_lines_only_and_checks_the_selftest() {
        let mut c = Capture::default();
        assert!(!c.feed("[ares] TRINO_STEP:trace"));
        for l in selftest_lines().lines() {
            assert!(c.feed(&format!("[ares] {l}")));
        }
        // Never the system temp dir (a RAM disk on some machines).
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/test-projects/trace-capture");
        let msg = c.finish(&root, "n64", true).unwrap();
        assert!(msg.contains("1 sessions"), "{msg}");
        let back = chrome::load(&root.join("target/traces/n64.json")).unwrap();
        assert_eq!(back.sessions[0].platform, "n64");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_selftest_without_its_session_fails_the_engine_run() {
        let c = Capture::default();
        let e = c
            .finish(Path::new("/nonexistent"), "n64", true)
            .unwrap_err();
        assert!(e.contains("selftest"), "{e}");
        // An external project's run has no castle64 step.
        assert_eq!(
            c.finish(Path::new("/nonexistent"), "n64", false).unwrap(),
            "no trace lines"
        );
    }
}
