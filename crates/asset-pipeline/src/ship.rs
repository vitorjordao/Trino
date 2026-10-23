//! How a project ships (issue #61): `trino.toml`'s packaging keys — `[pc]`
//! `icon`, `identifier`, `publisher`, `version`, `features`, `profile`,
//! `crash_upload`; `[steam] app_id`; `[itch] target` — read and **edited in
//! place** (comments and order kept, `toml_edit`), so the editor's Project ›
//! Packaging page and `cargo xtask package config set` write the same
//! bytes, and `cargo xtask package pc` reads what either wrote.

use std::path::{Path, PathBuf};

/// The keys `set` accepts, with what each takes.
pub const KEYS: [(&str, &str); 10] = [
    (
        "pc.icon",
        "a square PNG under the project (assets/platform/pc/icon.png)",
    ),
    ("pc.identifier", "reverse-DNS id (com.example.game)"),
    ("pc.publisher", "the company in the .exe's version info"),
    (
        "pc.version",
        "the stamped version (default: the pc crate's)",
    ),
    (
        "pc.features",
        "the pc crate's features in a package (gamepad,steam)",
    ),
    ("pc.profile", "native, n64, 3ds or web"),
    (
        "pc.crash_upload",
        "https:// endpoint for crash reports (players opt in)",
    ),
    ("steam.app_id", "the Steam app id (480 = Spacewar)"),
    ("itch.target", "butler's user/game"),
    (
        "project.title",
        "the game's title (20 ASCII characters: the N64 header)",
    ),
];

/// The packaging keys as `trino.toml` has them (empty = unset).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ship {
    pub name: String,
    pub title: String,
    pub icon: String,
    pub identifier: String,
    pub publisher: String,
    pub version: String,
    pub features: Vec<String>,
    pub profile: String,
    pub crash_upload: String,
    pub steam_app_id: Option<u32>,
    pub itch_target: String,
}

pub fn path(root: &Path) -> PathBuf {
    root.join("trino.toml")
}

impl Ship {
    /// `<root>/trino.toml`'s keys (a project without one: all empty).
    pub fn load(root: &Path) -> Result<Ship, String> {
        match std::fs::read_to_string(path(root)) {
            Ok(t) => Ship::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ship::default()),
            Err(e) => Err(format!("{}: {e}", path(root).display())),
        }
    }

    pub fn parse(text: &str) -> Result<Ship, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("trino.toml: {e}"))?;
        let s = |t: &str, k: &str| -> String {
            doc.get(t)
                .and_then(|t| t.get(k))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let features = doc
            .get("pc")
            .and_then(|t| t.get("features"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Ship {
            name: s("project", "name"),
            title: s("project", "title"),
            icon: s("pc", "icon"),
            identifier: s("pc", "identifier"),
            publisher: s("pc", "publisher"),
            version: s("pc", "version"),
            features,
            profile: s("pc", "profile"),
            crash_upload: s("pc", "crash_upload"),
            steam_app_id: doc
                .get("steam")
                .and_then(|t| t.get("app_id"))
                .and_then(|v| v.as_integer())
                .and_then(|v| u32::try_from(v).ok()),
            itch_target: s("itch", "target"),
        })
    }
}

/// Why `value` can't be `key` (the same checks `cargo xtask package` makes
/// when it reads `trino.toml`).
pub fn validate(key: &str, value: &str) -> Result<(), String> {
    match key {
        "pc.identifier" if !value.is_empty() => {
            if value.split('.').count() < 2
                || !value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            {
                return Err(format!(
                    "[pc] identifier `{value}`: a reverse-DNS id like com.example.game"
                ));
            }
        }
        "pc.crash_upload" if !value.is_empty() => {
            if !value.starts_with("https://")
                && !value.starts_with("http://127.0.0.1")
                && !value.starts_with("http://localhost")
            {
                return Err(format!(
                    "[pc] crash_upload `{value}`: use https:// (plain http only to localhost)"
                ));
            }
        }
        "pc.profile" if !value.is_empty() => {
            if !matches!(value, "native" | "n64" | "3ds" | "web") {
                return Err(format!("[pc] profile `{value}`: native, n64, 3ds or web"));
            }
        }
        "steam.app_id" if !value.is_empty() => {
            value
                .parse::<u32>()
                .map_err(|_| format!("[steam] app_id `{value}`: a number"))?;
        }
        "project.title" => {
            if value.is_empty() || !value.is_ascii() || value.len() > 20 {
                return Err(format!(
                    "project.title `{value}`: the N64 ROM header holds ≤ 20 ASCII characters"
                ));
            }
        }
        k if !KEYS.iter().any(|(x, _)| *x == k) => {
            return Err(format!(
                "unknown key `{k}` ({})",
                KEYS.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")
            ));
        }
        _ => {}
    }
    Ok(())
}

/// `text` with `key` set to `value` (empty removes it; `pc.features` takes
/// a comma-separated list), comments and order kept. A table that doesn't
/// exist yet is appended.
pub fn set(text: &str, key: &str, value: &str) -> Result<String, String> {
    validate(key, value)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("trino.toml: {e}"))?;
    let (table, field) = key.split_once('.').ok_or(format!("unknown key `{key}`"))?;
    if !doc.contains_key(table) {
        if value.is_empty() {
            return Ok(doc.to_string());
        }
        doc[table] = toml_edit::table();
    }
    let t = doc[table]
        .as_table_mut()
        .ok_or(format!("trino.toml: `{table}` is not a table"))?;
    if value.is_empty() {
        t.remove(field);
        return Ok(doc.to_string());
    }
    let item = match key {
        "steam.app_id" => toml_edit::value(value.parse::<i64>().map_err(|e| e.to_string())?),
        "pc.features" => {
            let mut a = toml_edit::Array::new();
            for f in value.split(',').map(str::trim).filter(|f| !f.is_empty()) {
                a.push(f);
            }
            toml_edit::value(a)
        }
        _ => toml_edit::value(value),
    };
    t[field] = item;
    Ok(doc.to_string())
}

/// [`set`] on `<root>/trino.toml`.
pub fn set_in(root: &Path, key: &str, value: &str) -> Result<(), String> {
    let p = path(root);
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let out = set(&text, key, value)?;
    std::fs::write(&p, out).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# My game\n[project]\nname = \"g\"\ntitle = \"G\"\n\n# The desktop host.\n[pc]\npackage = \"g-pc\"\n\n[n64]\npackage = \"g-n64\"\n";

    #[test]
    fn set_keeps_comments_and_reads_back() {
        let t = set(SAMPLE, "pc.identifier", "com.ex.g").unwrap();
        let t = set(&t, "pc.features", "gamepad, steam").unwrap();
        let t = set(&t, "steam.app_id", "480").unwrap();
        let t = set(&t, "itch.target", "me/g").unwrap();
        assert!(t.starts_with("# My game\n[project]"), "{t}");
        assert!(
            t.contains("# The desktop host.\n[pc]\npackage = \"g-pc\"\nidentifier = \"com.ex.g\""),
            "{t}"
        );
        assert!(t.contains("[steam]\napp_id = 480"), "{t}");
        let s = Ship::parse(&t).unwrap();
        assert_eq!(s.identifier, "com.ex.g");
        assert_eq!(s.features, ["gamepad", "steam"]);
        assert_eq!(s.steam_app_id, Some(480));
        assert_eq!(s.itch_target, "me/g");
        assert_eq!((s.name.as_str(), s.title.as_str()), ("g", "G"));
        // Empty removes.
        let t = set(&t, "pc.identifier", "").unwrap();
        assert_eq!(Ship::parse(&t).unwrap().identifier, "");
        assert!(t.contains("[n64]\npackage = \"g-n64\""));
    }

    #[test]
    fn set_refuses_what_package_would() {
        for (k, v, why) in [
            ("pc.identifier", "x", "reverse-DNS"),
            ("pc.crash_upload", "http://ex.com", "https://"),
            ("pc.profile", "gb", "native"),
            ("steam.app_id", "abc", "a number"),
            (
                "project.title",
                "A title far longer than twenty",
                "20 ASCII",
            ),
            ("pc.nope", "1", "unknown key"),
        ] {
            let e = set(SAMPLE, k, v).unwrap_err();
            assert!(e.contains(why), "{k}: {e}");
        }
    }
}
