//! A shipped game (issue #61, `cargo xtask package pc`): the package puts
//! `trino-package.txt` next to the executable (in `Contents/Resources` of a
//! macOS `.app`) with the game's identity, and the baked assets beside it.
//! [`detect`] finds it; the runner then
//!
//! - reads the assets from the package instead of baking `<root>/assets`
//!   (the build machine's source tree is not on a player's disk);
//! - keeps saves, settings, achievements and crash reports in the user's
//!   data folder ([`user_data_dir`]): `%APPDATA%\<save_dir>` on Windows,
//!   `~/Library/Application Support/<save_dir>` on macOS,
//!   `$XDG_DATA_HOME/<save_dir>` (`~/.local/share/...`) on Linux — the
//!   paths Steam Auto-Cloud is given;
//! - starts on the package's profile (the native PC look by default).
//!
//! `TRINO_SAVE_DIR` overrides the save folder (tests, a portable install).
//! The marker is plain `key=value` text, one per line:
//!
//! ```text
//! name=orbit-cat
//! title=Orbit Cat
//! version=1.0.0
//! commit=3f2a9c1e0b
//! date=2026-10-23
//! publisher=Example Games
//! identifier=com.example.orbitcat
//! save_dir=Orbit Cat
//! assets=assets/pc
//! profile=native
//! steam_app_id=480
//! crash_upload=https://crash.example.com/report
//! threads=auto
//! ```

use std::path::{Path, PathBuf};

/// The marker's file name.
pub const MARKER: &str = "trino-package.txt";

/// What a package says about itself.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PackageInfo {
    /// Where the marker is (the assets path is relative to it).
    pub dir: PathBuf,
    pub name: String,
    pub title: String,
    pub version: String,
    pub commit: String,
    pub date: String,
    pub publisher: String,
    pub identifier: String,
    /// The folder under the user data dir.
    pub save_dir: String,
    /// The baked assets, relative to [`dir`](Self::dir).
    pub assets: String,
    /// `native`, `n64`, `3ds` or `web` (default native).
    pub profile: String,
    pub steam_app_id: Option<u32>,
    pub crash_upload: Option<String>,
    /// `trino.toml [threads] runtime` (issue #101): the precompile and
    /// load workers; `TRINO_THREADS` still wins.
    pub threads: Option<trino_core::ThreadSetting>,
}

impl PackageInfo {
    pub fn parse(text: &str, dir: &Path) -> PackageInfo {
        let mut p = PackageInfo {
            dir: dir.to_path_buf(),
            assets: "assets/pc".into(),
            profile: "native".into(),
            ..PackageInfo::default()
        };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().to_string();
            match k.trim() {
                "name" => p.name = v,
                "title" => p.title = v,
                "version" => p.version = v,
                "commit" => p.commit = v,
                "date" => p.date = v,
                "publisher" => p.publisher = v,
                "identifier" => p.identifier = v,
                "save_dir" => p.save_dir = v,
                "assets" => p.assets = v,
                "profile" => p.profile = v,
                "steam_app_id" => p.steam_app_id = v.parse().ok(),
                "crash_upload" => p.crash_upload = Some(v).filter(|v| !v.is_empty()),
                "threads" => p.threads = trino_core::ThreadSetting::parse(&v),
                _ => {}
            }
        }
        if p.save_dir.is_empty() {
            p.save_dir = if p.title.is_empty() {
                p.name.clone()
            } else {
                p.title.clone()
            };
        }
        p
    }

    /// The marker's text (what `cargo xtask package` writes).
    pub fn to_text(&self) -> String {
        let mut out = String::from("# Written by `cargo xtask package` (Trino, issue #61).\n");
        let mut kv = |k: &str, v: &str| {
            if !v.is_empty() {
                out.push_str(&format!("{k}={v}\n"));
            }
        };
        kv("name", &self.name);
        kv("title", &self.title);
        kv("version", &self.version);
        kv("commit", &self.commit);
        kv("date", &self.date);
        kv("publisher", &self.publisher);
        kv("identifier", &self.identifier);
        kv("save_dir", &self.save_dir);
        kv("assets", &self.assets);
        kv("profile", &self.profile);
        if let Some(id) = self.steam_app_id {
            kv("steam_app_id", &id.to_string());
        }
        if let Some(u) = &self.crash_upload {
            kv("crash_upload", u);
        }
        if let Some(t) = self.threads {
            let mut s = String::new();
            let _ = t.write(&mut s);
            kv("threads", &s);
        }
        out
    }

    /// The baked assets.
    pub fn assets_dir(&self) -> PathBuf {
        self.dir.join(&self.assets)
    }

    /// The package's profile (`TRINO_SIM` still wins).
    pub fn profile(&self) -> Option<trino_platform_pc::SimProfile> {
        trino_platform_pc::SimProfile::parse(&self.profile)
    }
}

/// The marker for an executable at `exe`: beside it, or in a macOS
/// bundle's `Contents/Resources` (the executable is in `Contents/MacOS`).
pub fn find(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    [dir.join(MARKER), dir.join("../Resources").join(MARKER)]
        .into_iter()
        .find(|p| p.is_file())
}

/// The running executable's package, if it is one.
pub fn detect() -> Option<PackageInfo> {
    let exe = std::env::current_exe().ok()?;
    let marker = find(&exe)?;
    let text = std::fs::read_to_string(&marker).ok()?;
    let dir = marker.parent()?.canonicalize().ok()?;
    Some(PackageInfo::parse(&text, &dir))
}

/// The OS's per-user data folder for `save_dir`, from the environment
/// (`APPDATA`; `HOME`; `XDG_DATA_HOME`, else `HOME/.local/share`).
pub fn user_data_dir(save_dir: &str) -> Option<PathBuf> {
    user_data_dir_from(save_dir, std::env::consts::OS, &|k| std::env::var_os(k))
}

/// [`user_data_dir`] for an `os` and an environment (testable anywhere).
pub fn user_data_dir_from(
    save_dir: &str,
    os: &str,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let some = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = match os {
        "windows" => some("APPDATA")?,
        "macos" => some("HOME")?.join("Library/Application Support"),
        _ => some("XDG_DATA_HOME").or_else(|| some("HOME").map(|h| h.join(".local/share")))?,
    };
    Some(base.join(sanitize(save_dir)))
}

/// A folder name every OS takes: no separators or reserved characters.
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "Trino game".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trips_and_defaults() {
        let p = PackageInfo {
            dir: PathBuf::from("/pkg"),
            name: "orbit-cat".into(),
            title: "Orbit Cat".into(),
            version: "1.0.0".into(),
            commit: "3f2a9c1e0b".into(),
            date: "2026-10-23".into(),
            publisher: "Example".into(),
            identifier: "com.example.orbitcat".into(),
            save_dir: "Orbit Cat".into(),
            assets: "assets/pc".into(),
            profile: "native".into(),
            steam_app_id: Some(480),
            crash_upload: Some("https://x.example/r".into()),
            threads: Some(trino_core::ThreadSetting::AllBut(2)),
        };
        assert_eq!(PackageInfo::parse(&p.to_text(), Path::new("/pkg")), p);
        let bare = PackageInfo::parse("name=g\n# comment\nnoise\n", Path::new("/d"));
        assert_eq!(bare.save_dir, "g");
        assert_eq!(bare.assets_dir(), PathBuf::from("/d/assets/pc"));
        assert_eq!(bare.profile(), Some(trino_platform_pc::SimProfile::Native));
        assert_eq!(bare.steam_app_id, None);
        assert_eq!(bare.threads, None);
    }

    #[test]
    fn user_data_dirs_per_os() {
        let env = |k: &str| -> Option<std::ffi::OsString> {
            match k {
                "APPDATA" => Some(r"C:\Users\p\AppData\Roaming".into()),
                "HOME" => Some("/home/p".into()),
                _ => None,
            }
        };
        assert_eq!(
            user_data_dir_from("Orbit Cat", "windows", &env).unwrap(),
            PathBuf::from(r"C:\Users\p\AppData\Roaming").join("Orbit Cat")
        );
        assert_eq!(
            user_data_dir_from("Orbit Cat", "macos", &env).unwrap(),
            PathBuf::from("/home/p/Library/Application Support/Orbit Cat")
        );
        assert_eq!(
            user_data_dir_from("Orbit Cat", "linux", &env).unwrap(),
            PathBuf::from("/home/p/.local/share/Orbit Cat")
        );
        let xdg = |k: &str| (k == "XDG_DATA_HOME").then(|| "/xdg".into());
        assert_eq!(
            user_data_dir_from("a/b:c", "linux", &xdg).unwrap(),
            PathBuf::from("/xdg/a_b_c")
        );
        assert_eq!(sanitize(" . "), "Trino game");
    }

    #[test]
    fn finds_the_marker_beside_the_exe_or_in_a_bundle() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-projects/package-marker");
        let _ = std::fs::remove_dir_all(&root);
        let flat = root.join("flat");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join(MARKER), "name=a\n").unwrap();
        assert_eq!(find(&flat.join("game")), Some(flat.join(MARKER)));
        let app = root.join("Game.app/Contents");
        std::fs::create_dir_all(app.join("MacOS")).unwrap();
        std::fs::create_dir_all(app.join("Resources")).unwrap();
        std::fs::write(app.join("Resources").join(MARKER), "name=b\n").unwrap();
        assert!(find(&app.join("MacOS/game")).is_some());
        assert_eq!(find(&root.join("nothing/game")), None);
    }
}
