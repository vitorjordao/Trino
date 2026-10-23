//! What build a game is (issue #61): its version, commit and date, stamped
//! at compile time in the **game's** crate by [`build_info!`](crate::build_info)
//! — the same on every target, shown by an About screen and written into
//! crash reports.
//!
//! `cargo xtask package` sets `TRINO_GIT_COMMIT` (`git rev-parse --short=10
//! HEAD`, `-dirty` when the tree has changes), `TRINO_BUILD_DATE`
//! (`YYYY-MM-DD`) and, from `[pc] version`, `TRINO_GAME_VERSION`; a plain
//! `cargo build` reads `dev`, no date and the crate's own version.

/// A build's identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BuildInfo {
    /// The crate that expanded [`build_info!`](crate::build_info).
    pub name: &'static str,
    /// `TRINO_GAME_VERSION`, else the crate's `version`.
    pub version: &'static str,
    /// `TRINO_GIT_COMMIT`, else `dev`.
    pub commit: &'static str,
    /// `TRINO_BUILD_DATE`, else empty.
    pub date: &'static str,
}

impl BuildInfo {
    /// A development build (no stamp): its commit reads `dev`.
    pub fn is_dev(&self) -> bool {
        self.commit == "dev"
    }

    /// `1.2.0 (3f2a9c1e0b, 2026-10-23)`, `1.2.0 (dev)` — into `out`.
    pub fn write_line(&self, out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        write!(out, "{} ({}", self.version, self.commit)?;
        if !self.date.is_empty() {
            write!(out, ", {}", self.date)?;
        }
        out.write_str(")")
    }
}

/// The calling crate's [`BuildInfo`] (a `const` expression):
///
/// ```
/// const BUILD: trino_core::BuildInfo = trino_core::build_info!();
/// assert!(!BUILD.version.is_empty());
/// ```
#[macro_export]
macro_rules! build_info {
    () => {
        $crate::version::BuildInfo {
            name: env!("CARGO_PKG_NAME"),
            version: match option_env!("TRINO_GAME_VERSION") {
                Some(v) => v,
                None => env!("CARGO_PKG_VERSION"),
            },
            commit: match option_env!("TRINO_GIT_COMMIT") {
                Some(c) => c,
                None => "dev",
            },
            date: match option_env!("TRINO_BUILD_DATE") {
                Some(d) => d,
                None => "",
            },
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_the_calling_crate() {
        const B: BuildInfo = crate::build_info!();
        assert_eq!(B.name, "trino-core");
        assert_eq!(B.version, env!("CARGO_PKG_VERSION"));
        let mut s = String::new();
        let dev = BuildInfo {
            commit: "dev",
            date: "",
            ..B
        };
        dev.write_line(&mut s).unwrap();
        assert_eq!(s, format!("{} (dev)", B.version));
        assert!(dev.is_dev());
        let mut s = String::new();
        BuildInfo {
            version: "1.2.0",
            commit: "3f2a9c1e0b",
            date: "2026-10-23",
            ..B
        }
        .write_line(&mut s)
        .unwrap();
        assert_eq!(s, "1.2.0 (3f2a9c1e0b, 2026-10-23)");
    }
}
