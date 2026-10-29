//! The 3DS save folders on Azahar's SD card, for `cargo xtask test 3ds`
//! (GitHub #119).
//!
//! Before booting the engine's self-test the harness seeds the folder every
//! Trino game shared before #119 (`sdmc:/3ds/trino/save.a`) with a known
//! save and empties the self-test's own folder. The app's `save folder` step
//! must then read that seed (the one-time migration), and afterwards the
//! shared files must be byte for byte what was seeded (never written) while
//! the self-test's own folder holds its slot files. Whatever the shared
//! folder held before (a player's save from an older build) is put back.

use std::path::{Path, PathBuf};

/// The save the harness plants in the shared folder.
pub const SEED: &[u8] = b"trino-119-legacy-seed";

/// The SD card's files Azahar keeps beside its log (`<data>/azahar-emu/
/// log/azahar_log.txt` → `<data>/azahar-emu/sdmc`).
pub fn sd_root(log: &Path) -> Option<PathBuf> {
    Some(log.parent()?.parent()?.join("sdmc"))
}

/// The shared folder from before #119 and a game's own one.
pub fn shared_dir(sd: &Path) -> PathBuf {
    sd.join("3ds/trino")
}

pub fn own_dir(sd: &Path, name: &str) -> PathBuf {
    shared_dir(sd).join(name)
}

const SHARED: [&str; 3] = ["save.a", "save.b", "save.bin"];

/// A seeded SD card; dropping it restores what the shared folder held.
pub struct Seeded {
    shared: PathBuf,
    own: PathBuf,
    before: Vec<(PathBuf, Option<Vec<u8>>)>,
    seeded: Vec<(PathBuf, Option<Vec<u8>>)>,
}

/// Plant [`SEED`] as the shared folder's only save and empty the game's
/// own folder `name`.
pub fn seed(sd: &Path, name: &str) -> Result<Seeded, String> {
    let shared = shared_dir(sd);
    let own = own_dir(sd, name);
    std::fs::create_dir_all(&shared).map_err(|e| format!("{}: {e}", shared.display()))?;
    let before = SHARED
        .iter()
        .map(|f| {
            let p = shared.join(f);
            let b = std::fs::read(&p).ok();
            (p, b)
        })
        .collect();
    let seeded = Seeded {
        shared: shared.clone(),
        own: own.clone(),
        before,
        seeded: Vec::new(),
    };
    if own.exists() {
        std::fs::remove_dir_all(&own).map_err(|e| format!("{}: {e}", own.display()))?;
    }
    let mut slot = vec![0u8; trino_core::save::slot_file_len(SEED.len())];
    trino_core::save::encode_slot(1, SEED, &mut slot).ok_or("seed: slot encoding")?;
    let mut seeded = seeded;
    for f in SHARED {
        let p = shared.join(f);
        let _ = std::fs::remove_file(&p);
        let bytes = (f == "save.a").then(|| slot.clone());
        if let Some(b) = &bytes {
            std::fs::write(&p, b).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        seeded.seeded.push((p, bytes));
    }
    Ok(seeded)
}

impl Seeded {
    /// After the run: the step read the seed from the app's own folder, the
    /// shared files are untouched and the own folder holds slot files.
    pub fn check(&self, step: Option<&str>, name: &str) -> Result<String, String> {
        let step = step.ok_or("save folder: the step did not report")?;
        let dir = format!("dir=sdmc:/3ds/trino/{name} ");
        if !step.contains(&dir) {
            return Err(format!("save folder: expected {dir}in `{step}`"));
        }
        if !step.contains("seed=1") {
            return Err(format!(
                "save folder: the shared save from before #119 did not migrate: `{step}`"
            ));
        }
        for (p, want) in &self.seeded {
            if &std::fs::read(p).ok() != want {
                return Err(format!(
                    "save folder: {} changed: the shared folder must never be written",
                    p.display()
                ));
            }
        }
        let own: Vec<String> = ["save.a", "save.b"]
            .iter()
            .filter(|f| self.own.join(f).exists())
            .map(|f| f.to_string())
            .collect();
        if own.is_empty() {
            return Err(format!(
                "save folder: {} holds no slot file after the run",
                self.own.display()
            ));
        }
        Ok(format!(
            "migrated the shared save into {} ({}), {} untouched",
            self.own.display(),
            own.join(", "),
            self.shared.display()
        ))
    }
}

impl Drop for Seeded {
    fn drop(&mut self) {
        for (p, bytes) in &self.before {
            let _ = match bytes {
                Some(b) => std::fs::write(p, b),
                None => std::fs::remove_file(p),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_restores_the_shared_folder_and_checks_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("azahar-emu/log/azahar_log.txt");
        let sd = sd_root(&log).unwrap();
        assert_eq!(sd, dir.path().join("azahar-emu/sdmc"));
        let shared = shared_dir(&sd);
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("save.b"), b"a player's save").unwrap();
        std::fs::create_dir_all(own_dir(&sd, "g-selftest")).unwrap();
        std::fs::write(own_dir(&sd, "g-selftest").join("save.a"), b"stale").unwrap();
        {
            let s = seed(&sd, "g-selftest").unwrap();
            assert!(!own_dir(&sd, "g-selftest").exists(), "own folder emptied");
            assert!(shared.join("save.a").exists() && !shared.join("save.b").exists());
            let line =
                "TRINO_STEP:save folder dir=sdmc:/3ds/trino/g-selftest status=ok len=21 seed=1";
            // No slot file yet: the run did not save.
            assert!(s.check(Some(line), "g-selftest").is_err());
            std::fs::create_dir_all(own_dir(&sd, "g-selftest")).unwrap();
            std::fs::write(own_dir(&sd, "g-selftest").join("save.a"), b"new").unwrap();
            s.check(Some(line), "g-selftest").unwrap();
            assert!(
                s.check(Some(&line.replace("seed=1", "seed=0")), "g-selftest")
                    .is_err()
            );
            assert!(s.check(Some(line), "other").is_err());
            // A write to the shared folder fails the check.
            std::fs::write(shared.join("save.b"), b"x").unwrap();
            assert!(s.check(Some(line), "g-selftest").is_err());
        }
        assert_eq!(
            std::fs::read(shared.join("save.b")).unwrap(),
            b"a player's save"
        );
        assert!(!shared.join("save.a").exists());
    }
}
