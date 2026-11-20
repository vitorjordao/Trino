//! This crate runs on the Web, where `std::time` panics at the first call
//! ("time not implemented on this platform"): every clock goes through
//! `web_time` (`performance.now()` there, `std::time` natively). Clippy
//! can't tell them apart on the host (`web_time` re-exports `std::time`
//! there), so the sources are read instead. A per-job timer in the CPU T&L
//! workers once broke every browser's self-test this way.

use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_std_clock_in_a_crate_the_web_runs() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 10, "found the sources: {files:?}");
    let offenders: Vec<String> = files
        .iter()
        .flat_map(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            text.lines()
                .enumerate()
                .filter(|(_, l)| {
                    let code = l.split("//").next().unwrap_or("");
                    code.contains("std::time::Instant") || code.contains("std::time::SystemTime")
                })
                .map(|(n, l)| format!("{}:{}: {}", f.display(), n + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "use web_time instead:\n{}",
        offenders.join("\n")
    );
}
