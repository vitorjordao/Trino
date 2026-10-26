//! Crash reports on the PC (issue #61). [`install`] sets a panic hook that
//! writes `crashes/crash-<UTC time>-<pid>.txt` into the save folder (the
//! user's data folder in a package) before the process unwinds:
//!
//! ```text
//! Trino crash report 1
//! game: orbit-cat
//! version: 1.0.0
//! commit: 3f2a9c1e0b
//! build_date: 2026-10-23
//! time: 2026-10-23T12:34:56Z
//! os: linux x86_64
//! gpu: AMD Radeon (RADV RENOIR) (Vulkan)
//! profile: native
//! thread: main
//! panic: index out of bounds: the len is 3 but the index is 7
//! location: game/src/lib.rs:120:9
//!
//! backtrace:
//! ...
//!
//! log (last 200 lines):
//! ...
//! ```
//!
//! The log is `Platform::log`'s ([`trino_platform_pc::system::recent_log`]).
//! Nothing leaves the machine unless the package names an endpoint
//! (`[pc] crash_upload`) **and** the player said yes
//! (`Platform::set_crash_consent`, kept in `<save>.crash`): then the next
//! launch sends every report waiting ([`upload_pending`]) as `text/plain`
//! and renames it `*.sent`. Plain `http://` (a local collector, the tests)
//! needs nothing; `https://` needs the `crash-upload` feature (ureq +
//! rustls).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use trino_core::{CrashConsent, CrashReports};

/// Who crashed: written at the top of every report.
#[derive(Clone, Debug, Default)]
pub struct CrashInfo {
    pub game: String,
    pub version: String,
    pub commit: String,
    pub date: String,
    pub profile: String,
}

/// The folder reports go to under a save folder.
pub fn dir(save_dir: &Path) -> PathBuf {
    save_dir.join("crashes")
}

/// The adapter, set once the renderer is up (a crash before has none).
static GPU: Mutex<String> = Mutex::new(String::new());

pub fn set_gpu(name: &str) {
    if let Ok(mut g) = GPU.lock() {
        *g = name.to_string();
    }
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ` for `secs` since the epoch.
pub fn utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The report's text.
pub fn report(
    info: &CrashInfo,
    time: u64,
    thread: &str,
    message: &str,
    location: &str,
    backtrace: &str,
    log: &[String],
) -> String {
    let gpu = GPU.lock().map(|g| g.clone()).unwrap_or_default();
    let mut s = String::from("Trino crash report 1\n");
    let mut kv = |k: &str, v: &str| s.push_str(&format!("{k}: {v}\n"));
    kv("game", &info.game);
    kv("version", &info.version);
    kv("commit", &info.commit);
    kv("build_date", &info.date);
    kv("time", &utc(time));
    kv(
        "os",
        &format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    );
    kv(
        "gpu",
        if gpu.is_empty() {
            "(not started)"
        } else {
            &gpu
        },
    );
    kv("profile", &info.profile);
    kv("thread", thread);
    kv("panic", message);
    kv("location", location);
    s.push_str("\nbacktrace:\n");
    s.push_str(backtrace);
    if !backtrace.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&format!("\nlog (last {} lines):\n", log.len()));
    for l in log {
        s.push_str(l);
        s.push('\n');
    }
    s
}

/// Write reports for panics from now on (the previous hook still prints).
/// Returns the folder they go to.
pub fn install(info: CrashInfo, save_dir: &Path) -> PathBuf {
    let dir = dir(save_dir);
    let target = dir.clone();
    let previous = std::panic::take_hook();
    let info = Arc::new(info);
    std::panic::set_hook(Box::new(move |p| {
        previous(p);
        let message = p
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-text panic)".into());
        let location = p
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let thread = std::thread::current()
            .name()
            .unwrap_or("(unnamed)")
            .to_string();
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let t = now();
        let text = report(
            &info,
            t,
            &thread,
            &message,
            &location,
            &bt,
            &trino_platform_pc::system::recent_log(),
        );
        let _ = std::fs::create_dir_all(&target);
        let name = format!(
            "crash-{}-{}.txt",
            utc(t).replace([':', '-'], "").trim_end_matches('Z'),
            std::process::id()
        );
        let path = target.join(name);
        // The last minute of input beside it (issue #67, #100): what the
        // player did up to the crash, readable with `cargo xtask replay
        // dump` (a tail: it doesn't replay from boot).
        let mut text = text;
        if let Some(trec) = trino_platform_pc::replay_host::recent_input() {
            let input = path.with_extension("trec");
            if std::fs::write(&input, &trec).is_ok() {
                text.push_str(&input_note(&input, &trec));
            }
        }
        if std::fs::write(&path, text).is_ok() {
            eprintln!("[trino] crash report: {}", path.display());
        }
    }));
    dir
}

/// The report's line about the input tail written beside it.
pub fn input_note(file: &Path, trec: &[u8]) -> String {
    let frames = trino_core::replay::Header::parse(trec).map_or(0, |h| h.frames);
    let name = file
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    format!(
        "\n== input ==\nThe last {frames} frames of input are in {name} \
         (`cargo xtask replay dump {name}`).\n"
    )
}

/// Reports waiting in `dir` (not sent).
pub fn pending(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "txt")
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("crash-"))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The player's answer from `<save>.crash` (`upload=ask|yes|no`).
pub fn load_consent(file: &Path) -> CrashConsent {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.trim().strip_prefix("upload="))
                .and_then(CrashConsent::parse)
        })
        .unwrap_or(CrashConsent::Ask)
}

pub fn save_consent(file: &Path, consent: CrashConsent) {
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(
        file,
        format!(
            "# May the game send its crash reports? (issue #61)\nupload={}\n",
            consent.key()
        ),
    );
}

/// What the platform publishes.
pub fn status(dir: &Path, consent: CrashConsent, endpoint: Option<&str>) -> CrashReports {
    CrashReports {
        pending: pending(dir).len().min(u16::MAX as usize) as u16,
        consent,
        upload: endpoint.is_some(),
    }
}

/// POST `body` to `url` as `text/plain`; the response's status code.
pub fn post(url: &str, body: &str) -> Result<u16, String> {
    if let Some(rest) = url.strip_prefix("http://") {
        return post_http(rest, body);
    }
    #[cfg(feature = "crash-upload")]
    {
        let r = ureq::post(url)
            .header("Content-Type", "text/plain; charset=utf-8")
            .send(body)
            .map_err(|e| e.to_string())?;
        Ok(r.status().as_u16())
    }
    #[cfg(not(feature = "crash-upload"))]
    {
        let _ = body;
        Err(format!(
            "{url}: https needs the `crash-upload` feature of trino-runner-pc"
        ))
    }
}

/// HTTP/1.1 over a plain socket (`host[:port]/path`).
fn post_http(rest: &str, body: &str) -> Result<u16, String> {
    use std::io::{Read, Write};
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    let mut s = std::net::TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    let _ = s.read_to_string(&mut reply);
    reply
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("{addr}: no HTTP status in the reply"))
}

/// Send every waiting report to `endpoint` (only with the player's yes):
/// a 2xx renames it `*.sent`. Returns how many went.
pub fn upload_pending(dir: &Path, endpoint: &str, consent: CrashConsent) -> usize {
    if consent != CrashConsent::Allow {
        return 0;
    }
    let mut sent = 0;
    for p in pending(dir) {
        let Ok(body) = std::fs::read_to_string(&p) else {
            continue;
        };
        match post(endpoint, &body) {
            Ok(code) if (200..300).contains(&code) => {
                let _ = std::fs::rename(&p, p.with_extension("sent"));
                sent += 1;
            }
            Ok(code) => eprintln!("[trino] crash upload: HTTP {code}"),
            Err(e) => {
                eprintln!("[trino] crash upload: {e}");
                break;
            }
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-projects/crash")
            .join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn utc_times() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_761_222_896), "2025-10-23T12:34:56Z");
    }

    #[test]
    fn report_has_every_field() {
        let info = CrashInfo {
            game: "orbit-cat".into(),
            version: "1.0.0".into(),
            commit: "abc".into(),
            date: "2026-10-23".into(),
            profile: "native".into(),
        };
        set_gpu("Test GPU (Vulkan)");
        let r = report(
            &info,
            0,
            "main",
            "boom",
            "a.rs:1:2",
            "0: frame",
            &["TRINO_STEP:x".into()],
        );
        for want in [
            "Trino crash report 1\n",
            "game: orbit-cat\n",
            "version: 1.0.0\n",
            "commit: abc\n",
            "gpu: Test GPU (Vulkan)\n",
            "panic: boom\n",
            "location: a.rs:1:2\n",
            "backtrace:\n0: frame\n",
            "log (last 1 lines):\nTRINO_STEP:x\n",
        ] {
            assert!(r.contains(want), "{want:?} in\n{r}");
        }
    }

    #[test]
    fn consent_round_trips_and_gates_the_upload() {
        let d = scratch("consent");
        let f = d.join("game.crash");
        assert_eq!(load_consent(&f), CrashConsent::Ask);
        save_consent(&f, CrashConsent::Allow);
        assert_eq!(load_consent(&f), CrashConsent::Allow);
        std::fs::write(d.join("crash-1.txt"), "x").unwrap();
        let s = status(&d, CrashConsent::Ask, Some("http://127.0.0.1:9/"));
        assert!(s.should_ask() && s.pending == 1);
        assert!(
            !status(&d, CrashConsent::Ask, None).should_ask(),
            "no endpoint: nothing to ask"
        );
        assert_eq!(
            upload_pending(&d, "http://127.0.0.1:9/", CrashConsent::Deny),
            0
        );
        assert_eq!(
            upload_pending(&d, "http://127.0.0.1:9/", CrashConsent::Ask),
            0
        );
    }

    /// A local collector gets each report once, and only with consent.
    #[test]
    fn uploads_to_a_local_collector() {
        use std::io::{Read, Write};
        let d = scratch("upload");
        std::fs::write(d.join("crash-a.txt"), "report A").unwrap();
        std::fs::write(d.join("crash-b.txt"), "report B").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = vec![0u8; 4096];
                let mut got = Vec::new();
                loop {
                    let n = s.read(&mut buf).unwrap();
                    got.extend_from_slice(&buf[..n]);
                    let t = String::from_utf8_lossy(&got).to_string();
                    if let Some(i) = t.find("\r\n\r\n")
                        && t[i + 4..].len() >= 8
                    {
                        bodies.push(t[i + 4..].to_string());
                        assert!(t.starts_with("POST /crash HTTP/1.1"), "{t}");
                        break;
                    }
                }
                s.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").unwrap();
            }
            bodies
        });
        let url = format!("http://127.0.0.1:{port}/crash");
        assert_eq!(upload_pending(&d, &url, CrashConsent::Allow), 2);
        let bodies = server.join().unwrap();
        assert_eq!(bodies, ["report A", "report B"]);
        assert!(pending(&d).is_empty());
        assert!(d.join("crash-a.sent").is_file());
    }
}
