//! The crash hook end to end (issue #61): a child process (this test
//! binary again, `TRINO_CRASH_CHILD` set) installs the hook, logs, and
//! panics; the parent finds one report with every field, the panic's
//! message and location, a backtrace and the log line. A panic hook is
//! process-wide, so it never runs in the test process itself.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn a_panic_writes_a_report_with_the_log() {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-projects/crash-hook");
    if std::env::var_os("TRINO_CRASH_CHILD").is_some() {
        trino_runner_pc::crash::install(
            trino_runner_pc::crash::CrashInfo {
                game: "orbit-cat".into(),
                version: "1.2.3".into(),
                commit: "3f2a9c1e0b".into(),
                date: "2026-10-23".into(),
                profile: "native".into(),
            },
            &dir,
        );
        trino_runner_pc::crash::set_gpu("Test adapter");
        trino_platform_pc::system::remember_log("TRINO_STEP:right before the crash");
        let v: Vec<u8> = Vec::new();
        let i = std::hint::black_box(7);
        let _ = v[i];
        unreachable!();
    }
    let _ = std::fs::remove_dir_all(&dir);
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "a_panic_writes_a_report_with_the_log",
            "--exact",
            "--nocapture",
        ])
        .env("TRINO_CRASH_CHILD", "1")
        .env("RUST_BACKTRACE", "0")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the child must panic");
    let reports = trino_runner_pc::crash::pending(&trino_runner_pc::crash::dir(&dir));
    assert_eq!(reports.len(), 1, "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&reports[0]).unwrap();
    for want in [
        "Trino crash report 1\n",
        "game: orbit-cat\n",
        "version: 1.2.3\n",
        "commit: 3f2a9c1e0b\n",
        "build_date: 2026-10-23\n",
        "gpu: Test adapter\n",
        "panic: index out of bounds: the len is 0 but the index is 7\n",
        "crash_hook.rs:",
        "\nbacktrace:\n",
        "TRINO_STEP:right before the crash\n",
    ] {
        assert!(text.contains(want), "{want:?} missing from:\n{text}");
    }
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("[trino] crash report: "),
        "the player's console names the file"
    );
}
