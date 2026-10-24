//! Chrome Trace Event JSON (the "JSON Object Format"), what
//! `ui.perfetto.dev`, `chrome://tracing` and Speedscope open.
//!
//! One process per session (`pid` from 1, named after the session), one
//! thread per track (`tid` = [`TID_BASE`] + the track number, named `cpu`,
//! `gpu`, ...: Perfetto takes a `tid` equal to the `pid` for the process's
//! main thread and folds `tid` 0 away, which merged the tracks):
//! zones are complete events (`"ph": "X"`), counters `"C"`, frame marks
//! instant events (`"i"`). `otherData` keeps what Chrome has no field for
//! (the platform, the events lost) so [`read`] gets the same [`Trace`] back.

use crate::{Counter, FrameMark, Session, Trace, Zone};
use serde_json::{Map, Value, json};
use trino_core::trace::track;

/// A track's `tid` is this plus its number.
pub const TID_BASE: i64 = 1000;

/// The value of `otherData.format`.
pub const FORMAT: &str = "trino-trace";

/// `trace` as a Chrome Trace Event JSON object.
pub fn write(trace: &Trace) -> Value {
    let mut events = Vec::new();
    let mut sessions = Vec::new();
    for (i, s) in trace.sessions.iter().enumerate() {
        let pid = i + 1;
        events.push(
            json!({"ph": "M", "name": "process_name", "pid": pid, "tid": TID_BASE,
            "args": {"name": s.name}}),
        );
        events.push(
            json!({"ph": "M", "name": "process_sort_index", "pid": pid, "tid": TID_BASE,
            "args": {"sort_index": pid}}),
        );
        for t in s.tracks() {
            events.push(
                json!({"ph": "M", "name": "thread_name", "pid": pid, "tid": TID_BASE + t as i64,
                "args": {"name": track::name(t)}}),
            );
        }
        for z in &s.zones {
            events.push(
                json!({"ph": "X", "name": s.name_of(z.name), "cat": track::name(z.track),
                "pid": pid, "tid": TID_BASE + z.track as i64, "ts": z.start, "dur": z.dur,
                "args": {"depth": z.depth}}),
            );
        }
        for c in &s.counters {
            let name = s.name_of(c.name);
            let mut args = Map::new();
            args.insert(name.to_string(), json!(c.value));
            events.push(
                json!({"ph": "C", "name": name, "pid": pid, "tid": TID_BASE + c.track as i64,
                "ts": c.ts, "args": args}),
            );
        }
        for f in &s.frames {
            events.push(
                json!({"ph": "i", "name": "frame", "s": "p", "pid": pid, "tid": TID_BASE,
                "ts": f.ts, "args": {"frame": f.frame}}),
            );
        }
        sessions.push(
            json!({"name": s.name, "platform": s.platform, "dropped": s.dropped,
            "names": s.names}),
        );
    }
    json!({
        "traceEvents": events,
        "displayTimeUnit": "ms",
        "otherData": {"format": FORMAT, "version": trino_core::trace::VERSION, "sessions": sessions},
    })
}

/// A Chrome Trace Event JSON (an object with `traceEvents`, or a bare
/// array) as a [`Trace`]: this crate's own files exactly, other tools'
/// complete (`X`) and begin/end (`B`/`E`) events, counters and instants.
pub fn read(v: &Value) -> Result<Trace, String> {
    let events = match v {
        Value::Array(a) => a,
        Value::Object(o) => o
            .get("traceEvents")
            .and_then(Value::as_array)
            .ok_or("no traceEvents array")?,
        _ => return Err("not a trace (an object or an array)".into()),
    };
    let meta: Vec<&Value> = v
        .pointer("/otherData/sessions")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let mut by_pid: Vec<(i64, Session)> = Vec::new();
    // Open B events per (pid, tid): (name, start).
    let mut open: std::collections::HashMap<(i64, i64), Vec<(String, u64)>> = Default::default();
    let session = |by_pid: &mut Vec<(i64, Session)>, pid: i64| -> usize {
        match by_pid.iter().position(|(p, _)| *p == pid) {
            Some(i) => i,
            None => {
                by_pid.push((
                    pid,
                    Session {
                        name: format!("pid{pid}"),
                        ..Session::default()
                    },
                ));
                by_pid.len() - 1
            }
        }
    };
    let num =
        |e: &Value, k: &str| -> u64 { e.get(k).and_then(Value::as_f64).unwrap_or(0.0) as u64 };
    for e in events {
        let ph = e.get("ph").and_then(Value::as_str).unwrap_or("");
        let pid = e.get("pid").and_then(Value::as_i64).unwrap_or(0);
        let tid = e.get("tid").and_then(Value::as_i64).unwrap_or(0);
        let name = e.get("name").and_then(Value::as_str).unwrap_or("?");
        // Ours are offset; other tools' tids are taken as they are.
        let track = if tid >= TID_BASE { tid - TID_BASE } else { tid }.clamp(0, 255) as u8;
        let i = session(&mut by_pid, pid);
        let s = &mut by_pid[i].1;
        match ph {
            "M" if name == "process_name" => {
                if let Some(n) = e.pointer("/args/name").and_then(Value::as_str) {
                    s.name = n.to_string();
                    // Our own files list the names in their recorded order.
                    if let Some(names) = meta
                        .iter()
                        .find(|m| m.get("name").and_then(Value::as_str) == Some(n))
                        .and_then(|m| m.get("names"))
                        .and_then(Value::as_array)
                    {
                        for x in names.iter().filter_map(Value::as_str) {
                            s.intern(x);
                        }
                    }
                }
            }
            "X" => {
                let n = s.intern(name);
                let depth = e
                    .pointer("/args/depth")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u8;
                s.zones.push(Zone {
                    name: n,
                    track,
                    depth,
                    start: num(e, "ts"),
                    dur: num(e, "dur"),
                });
            }
            "B" => open
                .entry((pid, tid))
                .or_default()
                .push((name.to_string(), num(e, "ts"))),
            "E" => {
                if let Some((n, start)) = open.get_mut(&(pid, tid)).and_then(Vec::pop) {
                    let depth = open[&(pid, tid)].len() as u8;
                    let idx = s.intern(&n);
                    s.zones.push(Zone {
                        name: idx,
                        track,
                        depth,
                        start,
                        dur: num(e, "ts").saturating_sub(start),
                    });
                }
            }
            "C" => {
                let n = s.intern(name);
                let value = e
                    .get("args")
                    .and_then(Value::as_object)
                    .and_then(|a| a.get(name).or_else(|| a.values().next()))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as u64;
                s.counters.push(Counter {
                    name: n,
                    track,
                    ts: num(e, "ts"),
                    value,
                });
            }
            "i" | "I" if name == "frame" => s.frames.push(FrameMark {
                ts: num(e, "ts"),
                frame: e
                    .pointer("/args/frame")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            }),
            _ => {}
        }
    }
    let mut trace = Trace::default();
    for (_, mut s) in by_pid {
        if s.zones.is_empty() && s.counters.is_empty() && s.frames.is_empty() {
            continue;
        }
        if let Some(m) = meta
            .iter()
            .find(|m| m.get("name").and_then(Value::as_str) == Some(&s.name))
        {
            s.platform = m
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            s.dropped = m.get("dropped").and_then(Value::as_u64).unwrap_or(0);
        }
        s.zones
            .sort_by(|a, b| a.start.cmp(&b.start).then(b.dur.cmp(&a.dur)));
        s.counters.sort_by_key(|c| c.ts);
        s.frames.sort_by_key(|f| f.ts);
        trace.sessions.push(s);
    }
    Ok(trace)
}

/// Read a trace file: Chrome JSON, or a log of `TRINO_TRACE:` lines.
pub fn load(path: &std::path::Path) -> Result<Trace, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A trace's text: JSON when it parses as JSON, else a log of
/// `TRINO_TRACE:` lines (an emulator's `[ares] ...` lines start with `[`
/// too, so the first character can't tell).
pub fn parse(text: &str) -> Result<Trace, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => read(&v),
        Err(_) if text.contains(trino_core::trace::LINE_PREFIX) => {
            crate::collect(text).map_err(|e| e.to_string())
        }
        Err(e) => Err(format!("neither trace JSON nor a TRINO_TRACE log: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Trace {
        let mut s = Session {
            name: "n64-hub".into(),
            platform: "n64".into(),
            dropped: 3,
            ..Session::default()
        };
        let render = s.intern("render");
        let tnl = s.intern("tnl");
        let draws = s.intern("draws");
        let scene = s.intern("scene");
        s.zones = vec![
            Zone {
                name: render,
                track: 0,
                depth: 0,
                start: 0,
                dur: 100,
            },
            Zone {
                name: tnl,
                track: 0,
                depth: 1,
                start: 10,
                dur: 40,
            },
            Zone {
                name: scene,
                track: 1,
                depth: 0,
                start: 20,
                dur: 30,
            },
        ];
        s.counters = vec![Counter {
            name: draws,
            track: 0,
            ts: 100,
            value: 26,
        }];
        s.frames = vec![FrameMark { ts: 0, frame: 1 }];
        Trace { sessions: vec![s] }
    }

    #[test]
    fn our_json_reads_back_identically() {
        let t = sample();
        let v = write(&t);
        assert_eq!(v["otherData"]["format"], FORMAT);
        let back = read(&v).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn logs_and_json_are_told_apart_by_parsing() {
        let mut r = trino_core::trace::TraceRing::<4>::new();
        r.zone("z", 0, 1, 2);
        let mut log = String::new();
        r.write_lines(trino_core::trace::TracePlatform::N64, "s", &mut 0, &mut log)
            .unwrap();
        // An emulator's prefix starts the line with `[`.
        let prefixed: String = log.lines().map(|l| format!("[ares] {l}\n")).collect();
        assert_eq!(parse(&prefixed).unwrap().sessions[0].name, "s");
        let json = write(&parse(&log).unwrap()).to_string();
        assert_eq!(parse(&json).unwrap().sessions[0].zones.len(), 1);
        assert!(parse("[ares] nothing here").is_err());
    }

    #[test]
    fn begin_end_events_from_other_tools_become_zones() {
        let v = json!([
            {"ph": "B", "name": "outer", "pid": 7, "tid": 0, "ts": 0},
            {"ph": "B", "name": "inner", "pid": 7, "tid": 0, "ts": 5},
            {"ph": "E", "pid": 7, "tid": 0, "ts": 8},
            {"ph": "E", "pid": 7, "tid": 0, "ts": 20},
        ]);
        let t = read(&v).unwrap();
        let s = &t.sessions[0];
        assert_eq!(s.name, "pid7");
        let z: Vec<(&str, u8, u64, u64)> = s
            .zones
            .iter()
            .map(|z| (s.name_of(z.name), z.depth, z.start, z.dur))
            .collect();
        assert_eq!(z, [("outer", 0, 0, 20), ("inner", 1, 5, 3)]);
    }
}
