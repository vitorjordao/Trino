//! `trino.toml`'s `[players]` and `[net]` (issue #67) → the `net/config`
//! blob the PC and Web hosts read: how many local players the game takes
//! and how its network sessions run. `key = value` lines, the text
//! `trino_net::NetConfig::apply_text` reads (plus `max_players`).
//!
//! ```toml
//! [players]
//! max = 4
//! [net]
//! mode = "rollback"   # or "delay"
//! max_rollback = 8
//! input_delay = 2
//! ```

use std::path::Path;

use serde::Deserialize;

/// The blob's logical path.
pub const LOGICAL: &str = "net/config";

#[derive(Deserialize, Default)]
struct Root {
    #[serde(default)]
    players: Option<Players>,
    #[serde(default)]
    net: Option<Net>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Players {
    max: u8,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Net {
    mode: Option<String>,
    max_rollback: Option<u8>,
    input_delay: Option<u8>,
    players: Option<u8>,
    desync_every: Option<u16>,
    disconnect_ms: Option<u32>,
}

/// The two sections, validated.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ProjectNet {
    /// `[players] max` (1 when absent).
    pub max_players: u8,
    /// `[net]`'s keys as `key = value` lines (empty without the section).
    pub net: String,
    /// Either section is there (else nothing is baked).
    pub declared: bool,
}

impl ProjectNet {
    pub fn parse(text: &str) -> Result<ProjectNet, String> {
        let r: Root = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut out = ProjectNet {
            max_players: 1,
            net: String::new(),
            declared: r.players.is_some() || r.net.is_some(),
        };
        if let Some(p) = r.players {
            if !(1..=4).contains(&p.max) {
                return Err(format!(
                    "[players] max = {}: 1 to 4 (the N64's ports)",
                    p.max
                ));
            }
            out.max_players = p.max;
        }
        if let Some(n) = r.net {
            let mut line = |k: &str, v: String| {
                out.net.push_str(&format!("{k} = {v}\n"));
            };
            if let Some(m) = n.mode {
                if m != "rollback" && m != "delay" {
                    return Err(format!("[net] mode = \"{m}\": \"rollback\" or \"delay\""));
                }
                line("mode", m);
            }
            let range = |k: &str, v: u32, lo: u32, hi: u32| {
                if (lo..=hi).contains(&v) {
                    Ok(())
                } else {
                    Err(format!("[net] {k} = {v}: {lo} to {hi}"))
                }
            };
            if let Some(v) = n.max_rollback {
                range("max_rollback", v as u32, 1, 16)?;
                line("max_rollback", v.to_string());
            }
            if let Some(v) = n.input_delay {
                range("input_delay", v as u32, 0, 10)?;
                line("input_delay", v.to_string());
            }
            if let Some(v) = n.players {
                range("players", v as u32, 2, 4)?;
                line("players", v.to_string());
            }
            if let Some(v) = n.desync_every {
                range("desync_every", v as u32, 0, 600)?;
                line("desync_every", v.to_string());
            }
            if let Some(v) = n.disconnect_ms {
                range("disconnect_ms", v, 500, 60_000)?;
                line("disconnect_ms", v.to_string());
            }
        }
        Ok(out)
    }

    /// `<root>/trino.toml`'s sections (none without the file).
    pub fn load(root: &Path) -> Result<ProjectNet, String> {
        let path = root.join("trino.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => ProjectNet::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(ProjectNet {
                max_players: 1,
                ..ProjectNet::default()
            }),
        }
    }

    /// The blob's bytes.
    pub fn bake(&self) -> Vec<u8> {
        format!(
            "# trino.toml [players] and [net] (issue #67)\nmax_players = {}\n{}",
            self.max_players, self.net
        )
        .into_bytes()
    }
}

/// The keys `set_in` (the editor's Net tab, `cargo xtask net config set`)
/// writes, with what each takes.
pub const KEYS: [(&str, &str); 6] = [
    ("players.max", "local players, 1 to 4"),
    ("net.mode", "rollback or delay"),
    (
        "net.max_rollback",
        "frames a late input may roll back, 1 to 16",
    ),
    ("net.input_delay", "frames a local input waits, 0 to 10"),
    ("net.players", "players in a network session, 2 to 4"),
    (
        "net.desync_every",
        "frames between state-hash checks, 0 to 600",
    ),
];

/// Set `key` (one of [`KEYS`]) to `value` in `trino.toml`'s text, in place
/// (comments kept); an empty value removes it. The result must validate.
pub fn set(text: &str, key: &str, value: &str) -> Result<String, String> {
    if !KEYS.iter().any(|(k, _)| *k == key) {
        return Err(format!(
            "`{key}`: not one of {}",
            KEYS.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")
        ));
    }
    let (table, field) = key.split_once('.').unwrap_or(("net", key));
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("trino.toml: {e}"))?;
    if value.is_empty() {
        if let Some(t) = doc.get_mut(table).and_then(|t| t.as_table_mut()) {
            t.remove(field);
        }
    } else {
        let v: toml_edit::Value = match value.parse::<i64>() {
            Ok(n) => n.into(),
            Err(_) => value.trim_matches('"').into(),
        };
        if doc.get(table).is_none() {
            doc[table] = toml_edit::table();
        }
        doc[table][field] = toml_edit::value(v);
    }
    let out = doc.to_string();
    ProjectNet::parse(&out)?;
    Ok(out)
}

/// [`set`] on `<root>/trino.toml`.
pub fn set_in(root: &Path, key: &str, value: &str) -> Result<(), String> {
    let p = root.join("trino.toml");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let out = set(&text, key, value)?;
    std::fs::write(&p, out).map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_set_in_place_and_validated() {
        let t = "# my game\n[project]\nname = 'x' # keep me\n";
        let t = set(t, "net.mode", "delay").unwrap();
        let t = set(&t, "players.max", "2").unwrap();
        assert!(t.contains("# keep me") && t.contains("[net]") && t.contains("mode = \"delay\""));
        let n = ProjectNet::parse(&t).unwrap();
        assert_eq!(n.max_players, 2);
        assert!(set(&t, "net.max_rollback", "99").is_err());
        assert!(set(&t, "net.speed", "1").is_err());
        let t = set(&t, "net.mode", "").unwrap();
        assert!(!t.contains("mode ="));
    }

    #[test]
    fn sections_validate_and_bake_as_lines() {
        let n = ProjectNet::parse(
            "[project]\nname='x'\n[players]\nmax = 4\n[net]\nmode = \"delay\"\ninput_delay = 3\n",
        )
        .unwrap();
        assert!(n.declared);
        let text = String::from_utf8(n.bake()).unwrap();
        assert!(
            text.contains("max_players = 4\nmode = delay\ninput_delay = 3\n"),
            "{text}"
        );
        assert!(!ProjectNet::parse("[project]\nname='x'\n").unwrap().declared);
        for bad in [
            "[players]\nmax = 5",
            "[net]\nmode = \"fast\"",
            "[net]\nmax_rollback = 40",
            "[net]\nrollback = 2",
        ] {
            assert!(ProjectNet::parse(bad).is_err(), "{bad}");
        }
    }
}
