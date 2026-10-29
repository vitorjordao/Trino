//! `cargo xtask save soak 3ds|n64` (issue #121): cut the power in the
//! middle of saves and check what the next boot reads.
//!
//! The soak build (`TRINO_SAVE_SOAK=1` stages a `save_soak` file) runs
//! `castle64::soak`: it saves a counter every frame and logs `TRINO_SOAK
//! boot seq=<n> status=<s>` at boot and `TRINO_SOAK wrote seq=<n>` after
//! each finished save. After a cut the boot must read at least the last
//! `wrote` it logged (a later save may have finished before the cut, and
//! the log lags), and never `corrupted` or `empty`.
//!
//! - **3DS, automated:** Azahar boots the soak app, and after a varying
//!   number of saves it is killed (SIGKILL: the emulator gets no chance to
//!   flush or close anything, the closest an emulator comes to pulling the
//!   SD card), `--cycles` times. Its SD folder is `<name>-soak`, never the
//!   player's.
//! - **Hardware:** `save soak n64 --device sc64|everdrive` and `save soak
//!   3ds --device network --ip <IP>` build the soak and run it on the
//!   console with its log on the terminal; switch the console off (or pull
//!   the cartridge / SD card) while `wrote` lines scroll, turn it back on and
//!   run the same command again: the `boot` line must hold the last `wrote`
//!   or one more. `docs/HARDWARE.md` › Save power cuts.

use std::process::ExitCode;

use crate::project::Project;

/// One boot's log: what it read, and the last save it finished.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Boot {
    pub read: Option<u32>,
    pub status: String,
    pub wrote: Option<u32>,
}

impl Boot {
    /// Feed one log line; true once it is a `TRINO_SOAK` line.
    pub fn feed(&mut self, line: &str) -> bool {
        let Some(rest) = line.split("TRINO_SOAK ").nth(1) else {
            return false;
        };
        let field = |k: &str| {
            rest.split_whitespace()
                .find_map(|w| w.strip_prefix(k))
                .map(str::to_string)
        };
        if rest.starts_with("boot ") {
            self.read = field("seq=").and_then(|v| v.parse().ok());
            self.status = field("status=").unwrap_or_default();
        } else if rest.starts_with("wrote ") {
            self.wrote = field("seq=").and_then(|v| v.parse().ok());
        }
        true
    }
}

/// The check after a cut: `next` (the boot after) against `before` (the
/// run that was cut).
pub fn check(before: &Boot, next: &Boot) -> Result<String, String> {
    if matches!(next.status.as_str(), "corrupted" | "empty") && before.wrote.is_some() {
        return Err(format!(
            "after a cut the save read as `{}` (the cut run had finished seq {:?})",
            next.status, before.wrote
        ));
    }
    match (before.wrote, next.read) {
        (Some(w), Some(r)) if r < w => Err(format!(
            "the boot read seq {r}, older than the last finished save ({w})"
        )),
        (Some(_), None) => Err("the boot read no soak save after one finished".into()),
        (w, r) => Ok(format!(
            "last finished {} -> read {} ({})",
            w.map_or("-".into(), |v| v.to_string()),
            r.map_or("none".into(), |v| v.to_string()),
            next.status
        )),
    }
}

pub fn run(
    project: &Project,
    rest: &[&str],
    device: Option<&crate::hardware::DeviceArgs>,
) -> ExitCode {
    let value = |flag: &str| {
        rest.iter()
            .position(|a| *a == flag)
            .and_then(|i| rest.get(i + 1))
            .copied()
    };
    let target = rest.get(1).copied().unwrap_or("");
    // SAFETY: set before any thread starts; the builds read it.
    unsafe { std::env::set_var("TRINO_SAVE_SOAK", "1") };
    let result = match (target, device) {
        ("3ds", None) => {
            let cycles = value("--cycles").and_then(|v| v.parse().ok()).unwrap_or(8);
            azahar(project, cycles, !rest.contains(&"--no-build"))
        }
        (t @ ("n64" | "3ds"), Some(d)) if d.device.platform() == t => {
            println!(
                "[soak] the soak runs on the console{}: switch it off (or pull the \
                 cartridge / SD card) while `TRINO_SOAK wrote` lines scroll, turn it on \
                 and run this again: the `TRINO_SOAK boot` line must read the last \
                 `wrote` or one more (docs/HARDWARE.md › Save power cuts).",
                if t == "n64" {
                    ", over the cartridge's own EEPROM save"
                } else {
                    ""
                }
            );
            return if t == "n64" {
                crate::hardware::deploy_n64(project, d, true, false)
            } else {
                crate::hardware::deploy_3ds(project, d, true, false)
            };
        }
        ("n64", None) => Err(
            "the N64 soak needs hardware (--device sc64|everdrive): ares writes the \
             EEPROM file only when it exits, so killing it loses every save of the run"
                .to_string(),
        ),
        _ => Err(
            "usage: cargo xtask save soak 3ds [--cycles N] [--no-build] | \
                  save soak 3ds --device network [--ip IP] | \
                  save soak n64 --device sc64|everdrive"
                .to_string(),
        ),
    };
    match result {
        Ok(msg) => {
            println!("[soak] {msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[soak] FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Boot the soak app in Azahar `cycles` times, killing it after a varying
/// number of saves, and check each boot against the run before it.
fn azahar(p: &Project, cycles: u32, build: bool) -> Result<String, String> {
    if build && crate::n3ds::build(p, false) != ExitCode::SUCCESS {
        return Err("build 3ds failed".into());
    }
    let log = crate::n3ds::azahar_log_path().ok_or("cannot locate the Azahar log directory")?;
    let sd = crate::n3ds_saves::sd_root(&log).ok_or("cannot locate Azahar's SD card")?;
    let name = crate::n3ds::save_name(p, false);
    let own = crate::n3ds_saves::own_dir(&sd, &name);
    let _ = std::fs::remove_dir_all(&own);
    let app = p.app_3dsx();
    let mut before = Boot::default();
    let mut lines = Vec::new();
    for cycle in 0..cycles {
        // 3 to 40 saves before the cut, a different count each cycle.
        let cut_after = 3 + (cycle * 17 + 5) % 38;
        let mut boot = Boot::default();
        let mut seen = 0u32;
        crate::n3ds::boot_for_lines(&app, &mut |line| {
            if boot.feed(line) && line.contains("TRINO_SOAK wrote") {
                seen += 1;
            }
            seen >= cut_after
        })?;
        // The boot after the cut reads what survived; that boot is also the
        // next cycle's run (it saves on and gets cut in turn).
        let next = Boot {
            read: boot.read,
            status: boot.status.clone(),
            wrote: None,
        };
        let msg = if cycle == 0 {
            format!(
                "cycle 1: fresh folder, read {:?} ({})",
                next.read, next.status
            )
        } else {
            format!("cycle {}: {}", cycle + 1, check(&before, &next)?)
        };
        println!("[soak] {msg}");
        lines.push(msg);
        before = boot;
    }
    // The last cut, read back the way the editor and `save inspect` do.
    let mut out = vec![0u8; 256];
    let mut scratch = vec![0u8; trino_core::Caps::N3DS.save_bytes as usize];
    let r = trino_core::save::read_slot_files(
        &mut trino_asset_pipeline::saves::SdFolder::of(&own),
        &mut out,
        &mut scratch,
    )
    .map_err(|e| format!("{}: {e:?}", own.display()))?;
    let tail = Boot {
        read: castle64::soak::counter(&out[..r.len.min(out.len())]),
        status: r.status.label().to_string(),
        wrote: None,
    };
    let last = check(&before, &tail)?;
    println!("[soak] after the last cut: {last} ({})", own.display());
    Ok(format!(
        "{cycles} cuts in Azahar, every boot read the last finished save or a later one"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(lines: &[&str]) -> Boot {
        let mut b = Boot::default();
        for l in lines {
            b.feed(l);
        }
        b
    }

    #[test]
    fn a_cut_may_lose_the_save_in_flight_never_a_finished_one() {
        let run = boot(&[
            "[x] TRINO_SOAK boot seq=4 status=ok",
            "TRINO_SOAK wrote seq=5",
            "TRINO_SOAK wrote seq=6",
        ]);
        assert_eq!((run.read, run.wrote), (Some(4), Some(6)));
        // The next boot reads 6 (the one in flight lost) or 7 (it finished).
        for read in [6, 7] {
            let next = boot(&[&format!("TRINO_SOAK boot seq={read} status=ok")]);
            check(&run, &next).unwrap();
        }
        let recovered = boot(&["TRINO_SOAK boot seq=6 status=recovered-from-backup"]);
        check(&run, &recovered).unwrap();
        // Older than a finished save, nothing, or damage: a failure.
        for bad in [
            "TRINO_SOAK boot seq=5 status=ok",
            "TRINO_SOAK boot seq=none status=empty",
            "TRINO_SOAK boot seq=none status=corrupted",
        ] {
            assert!(check(&run, &boot(&[bad])).is_err(), "{bad}");
        }
        // Before any save finished, an empty medium is fine.
        check(
            &Boot::default(),
            &boot(&["TRINO_SOAK boot seq=none status=empty"]),
        )
        .unwrap();
    }
}
