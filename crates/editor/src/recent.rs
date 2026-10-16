//! The project launcher's **recent projects**: a small text file outside
//! any repository, one project per line, newest first:
//!
//! `1792141200<TAB>/home/me/games/my-game` (seconds since the Unix epoch,
//! a tab, the absolute path). Where it lives:
//! `$TRINO_EDITOR_RECENT` (tests, or to keep it elsewhere), else the user's
//! config directory — `$XDG_CONFIG_HOME/trino` or `~/.config/trino` on
//! Linux, `~/Library/Application Support/trino` on macOS,
//! `%APPDATA%\trino` on Windows — as `recent-projects.txt`. A line that
//! does not parse is dropped, never an error: the file is a convenience.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many projects the file keeps.
pub const MAX_RECENT: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    /// Last opened, seconds since the Unix epoch.
    pub opened: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentProjects {
    /// Newest first, one entry per path.
    pub entries: Vec<Recent>,
}

/// Seconds since the Unix epoch, now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The recent-projects file: `$TRINO_EDITOR_RECENT`, else the user's config
/// directory (see the module docs). `None` when neither is known.
pub fn default_file() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if let Some(p) = var("TRINO_EDITOR_RECENT") {
        return Some(PathBuf::from(p));
    }
    let dir = if cfg!(windows) {
        var("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(dir.join("trino").join("recent-projects.txt"))
}

impl RecentProjects {
    pub fn parse(text: &str) -> Self {
        let mut out = RecentProjects::default();
        for line in text.lines() {
            let Some((secs, path)) = line.split_once('\t') else {
                continue;
            };
            let (Ok(opened), path) = (secs.trim().parse::<u64>(), path.trim()) else {
                continue;
            };
            if path.is_empty() || out.entries.iter().any(|e| e.path == Path::new(path)) {
                continue;
            }
            out.entries.push(Recent {
                path: PathBuf::from(path),
                opened,
            });
        }
        out.entries.sort_by_key(|e| std::cmp::Reverse(e.opened));
        out.entries.truncate(MAX_RECENT);
        out
    }

    pub fn to_text(&self) -> String {
        self.entries
            .iter()
            .map(|e| format!("{}\t{}\n", e.opened, e.path.display()))
            .collect()
    }

    /// The file's projects; empty when it is missing or unreadable.
    pub fn load(file: &Path) -> Self {
        std::fs::read_to_string(file)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> Result<(), String> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(file, self.to_text()).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// `path` was opened at `at`: it moves to the top (once).
    pub fn touch(&mut self, path: &Path, at: u64) {
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            Recent {
                path: path.to_path_buf(),
                opened: at,
            },
        );
        self.entries.truncate(MAX_RECENT);
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|e| e.path != path);
    }
}

/// Record that `project` was opened now, in the default file (if any).
/// Errors are returned for the log; they never stop the editor.
pub fn record_open(project: &Path) -> Result<(), String> {
    let Some(file) = default_file() else {
        return Ok(());
    };
    let mut recent = RecentProjects::load(&file);
    recent.touch(project, now());
    recent.save(&file)
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "12 days ago".
pub fn age(opened: u64, now: u64) -> String {
    let s = now.saturating_sub(opened);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_good_lines_newest_first_once() {
        let text = "100\t/a\nnot a line\n300\t/b\nx\t/c\n200\t/a\n\t/d\n50\t\n";
        let r = RecentProjects::parse(text);
        assert_eq!(
            r.entries,
            vec![
                Recent {
                    path: "/b".into(),
                    opened: 300
                },
                Recent {
                    path: "/a".into(),
                    opened: 100
                },
            ]
        );
        assert_eq!(RecentProjects::parse(&r.to_text()), r, "round trip");
    }

    #[test]
    fn touch_moves_to_the_top_and_caps_the_list() {
        let mut r = RecentProjects::default();
        for i in 0..20u64 {
            r.touch(Path::new(&format!("/p{i}")), i);
        }
        assert_eq!(r.entries.len(), MAX_RECENT);
        assert_eq!(r.entries[0].path, Path::new("/p19"));
        r.touch(Path::new("/p10"), 99);
        assert_eq!(r.entries[0].path, Path::new("/p10"));
        assert_eq!(r.entries[0].opened, 99);
        assert_eq!(
            r.entries
                .iter()
                .filter(|e| e.path == Path::new("/p10"))
                .count(),
            1
        );
        r.remove(Path::new("/p10"));
        assert!(r.entries.iter().all(|e| e.path != Path::new("/p10")));
    }

    #[test]
    fn load_and_save_round_trip_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("nested/recent-projects.txt");
        assert_eq!(RecentProjects::load(&file), RecentProjects::default());
        let mut r = RecentProjects::default();
        r.touch(Path::new("/games/my-game"), 1_792_141_200);
        r.save(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "1792141200\t/games/my-game\n"
        );
        assert_eq!(RecentProjects::load(&file), r);
    }

    #[test]
    fn ages_read_like_a_person_says_them() {
        let now = 10_000_000;
        assert_eq!(age(now, now), "just now");
        assert_eq!(age(now + 5, now), "just now", "clock skew");
        assert_eq!(age(now - 300, now), "5 min ago");
        assert_eq!(age(now - 3 * 3600, now), "3 h ago");
        assert_eq!(age(now - 90_000, now), "yesterday");
        assert_eq!(age(now - 12 * 86_400, now), "12 days ago");
    }
}
