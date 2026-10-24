//! The build's thread budget (issue #101): `--threads <auto|N|-N>`, else
//! `$TRINO_THREADS`, else `trino.toml [threads] build`, else all CPUs but
//! one (at least one) — [`trino_core::ThreadSetting`]. Set once in `main`
//! and read by the bakes, the console shader assembly and cargo
//! (`CARGO_BUILD_JOBS`).

use std::sync::atomic::{AtomicU32, Ordering};

use trino_core::ThreadSetting;

static BUILD: AtomicU32 = AtomicU32::new(0);

/// Take `--threads <v>` out of `args`.
pub fn take_flag(args: &mut Vec<String>) -> Result<Option<ThreadSetting>, String> {
    let Some(i) = args.iter().position(|a| a == "--threads") else {
        return Ok(None);
    };
    let Some(v) = args.get(i + 1).cloned() else {
        return Err("xtask: --threads needs a value: auto, a count (4) or all but N (-2)".into());
    };
    args.drain(i..=i + 1);
    ThreadSetting::parse(&v)
        .map(Some)
        .ok_or_else(|| format!("xtask: --threads {v}: use auto, a count (4) or all but N (-2)"))
}

/// The setting that wins: the flag, the environment, the project's.
pub fn choose(
    flag: Option<ThreadSetting>,
    env: Option<&str>,
    project: ThreadSetting,
) -> ThreadSetting {
    flag.or_else(|| env.and_then(ThreadSetting::parse))
        .unwrap_or(project)
}

pub fn cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// Fix the build's budget for this run.
pub fn set(setting: ThreadSetting) -> u32 {
    let n = setting.resolve(cpus());
    BUILD.store(n, Ordering::Relaxed);
    n
}

/// The build's budget (all CPUs but one before [`set`]).
pub fn build() -> u32 {
    match BUILD.load(Ordering::Relaxed) {
        0 => ThreadSetting::Auto.resolve(cpus()),
        n => n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_beats_the_environment_beats_the_project() {
        let p = ThreadSetting::Count(3);
        assert_eq!(choose(None, None, p), p);
        assert_eq!(choose(None, Some("-1"), p), ThreadSetting::AllBut(1));
        // A typo in the environment falls back to the project's value.
        assert_eq!(choose(None, Some("many"), p), p);
        assert_eq!(
            choose(Some(ThreadSetting::Count(1)), Some("8"), p),
            ThreadSetting::Count(1)
        );
        let mut args: Vec<String> = ["build", "n64", "--threads", "2"]
            .map(String::from)
            .to_vec();
        assert_eq!(take_flag(&mut args), Ok(Some(ThreadSetting::Count(2))));
        assert_eq!(args, ["build", "n64"]);
        let mut bad: Vec<String> = ["--threads", "x"].map(String::from).to_vec();
        assert!(take_flag(&mut bad).is_err());
    }
}
