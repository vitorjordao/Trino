//! `assets/achievements.ron` (issue #61, ADR 0029): a project's
//! achievements, stats and rich-presence keys as text — what the editor's
//! Achievements tab edits and `cargo xtask services check|export|lock`
//! read. Baked to the `services/achievements` blob
//! (`trino_core::services::table`, `TACH`) on every target.
//!
//! ```ron
//! (
//!     version: 1,
//!     achievements: [
//!         (id: "FIRST_COIN", name: "ach.first_coin.name", description: "ach.first_coin.desc",
//!          icon: "sprites/ach_coin"),
//!         (id: "COINS_100", name: "ach.coins_100.name", description: "ach.coins_100.desc",
//!          stat: "COINS", threshold: 100),
//!         (id: "SECRET", name: "ach.secret.name", description: "ach.secret.desc", hidden: true),
//!     ],
//!     stats: [(id: "COINS", min: 0, max: 9999, increment_only: true)],
//!     presence: ["presence.menu", "presence.stage"],
//! )
//! ```
//!
//! An achievement's bit in a save is its place in the list:
//! `assets/achievements.lock` (one id per line, `cargo xtask services lock`)
//! records the order a shipped build used, and `check` fails a list that
//! doesn't start with it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use trino_core::services::table::{self, AchievementRecord, StatRecord};
use trino_core::services::{MAX_ACHIEVEMENTS, MAX_STATS};

use crate::i18n::Langs;
use crate::ui::Level;

pub const VERSION: u32 = 1;
/// The file, under the project root.
pub const FILE: &str = "assets/achievements.ron";
/// The shipped bit order, under the project root.
pub const LOCK_FILE: &str = "assets/achievements.lock";
/// Steam's limit on an API name.
pub const MAX_API_NAME: usize = 128;

/// One achievement.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AchievementDef {
    /// The API name (`FIRST_COIN`): letters, digits and `_`. The store
    /// knows it by this; the game by `AchievementId::new(id)`.
    pub id: String,
    /// String-table keys for its name and description.
    pub name: String,
    pub description: String,
    /// A sprite (`sprites/ach_coin`): drawn at the list's row height on
    /// every target, exported at 64 × 64 for Steam.
    #[serde(
        deserialize_with = "bare",
        serialize_with = "write_bare",
        skip_serializing_if = "Option::is_none"
    )]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Unlocked when this stat reaches `threshold`.
    #[serde(
        deserialize_with = "bare",
        serialize_with = "write_bare",
        skip_serializing_if = "Option::is_none"
    )]
    pub stat: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub threshold: i32,
}

/// One stat (an integer the game sets; Steam's INT stat).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct StatDef {
    pub id: String,
    pub min: i32,
    pub max: i32,
    /// A lower value is refused (counters).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub increment_only: bool,
}

impl Default for StatDef {
    fn default() -> Self {
        StatDef {
            id: String::new(),
            min: 0,
            max: i32::MAX,
            increment_only: false,
        }
    }
}

/// The whole file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AchievementsDef {
    pub version: u32,
    pub achievements: Vec<AchievementDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<StatDef>,
    /// Rich-presence string keys the game may pass to `set_presence`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub presence: Vec<String>,
}

impl Default for AchievementsDef {
    fn default() -> Self {
        AchievementsDef {
            version: VERSION,
            achievements: Vec::new(),
            stats: Vec::new(),
            presence: Vec::new(),
        }
    }
}

fn bare<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// Writes `x` for `Some(x)` (what [`bare`] reads back).
fn write_bare<S: serde::Serializer, T: Serialize>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => x.serialize(s),
        None => s.serialize_none(),
    }
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE)
}

pub fn lock_path(root: &Path) -> PathBuf {
    root.join(LOCK_FILE)
}

impl AchievementsDef {
    pub fn from_ron(text: &str) -> Result<AchievementsDef, String> {
        let def: AchievementsDef = ron::from_str(text).map_err(|e| e.to_string())?;
        if def.version != VERSION {
            return Err(format!(
                "achievements.ron version {} (this engine reads {VERSION})",
                def.version
            ));
        }
        Ok(def)
    }

    pub fn to_ron(&self) -> String {
        let cfg = ron::ser::PrettyConfig::new()
            .depth_limit(3)
            .struct_names(false)
            .indentor("    ".to_string());
        let mut s = ron::ser::to_string_pretty(self, cfg).expect("achievements serialize");
        s.push('\n');
        s
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        let p = path(root);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, self.to_ron()).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// The stat named `id`, by index.
    pub fn stat_index(&self, id: &str) -> Option<usize> {
        self.stats.iter().position(|s| s.id == id)
    }

    /// Every string-table key the file uses.
    pub fn keys(&self) -> BTreeSet<String> {
        self.achievements
            .iter()
            .flat_map(|a| [a.name.clone(), a.description.clone()])
            .chain(self.presence.iter().cloned())
            .filter(|k| !k.is_empty())
            .collect()
    }

    /// The `TACH` blob; `icon_id` maps a sprite's logical path to its id.
    pub fn to_tach(&self) -> Result<Vec<u8>, String> {
        let achievements: Vec<AchievementRecord<'_>> = self
            .achievements
            .iter()
            .map(|a| {
                let stat = match &a.stat {
                    Some(s) => Some((
                        self.stat_index(s)
                            .ok_or_else(|| format!("achievement {}: no stat `{s}`", a.id))?,
                        a.threshold,
                    )),
                    None => None,
                };
                Ok(AchievementRecord {
                    api: &a.id,
                    name: &a.name,
                    description: &a.description,
                    icon: a.icon.as_deref().map_or(0, trino_core::asset_id),
                    hidden: a.hidden,
                    stat,
                })
            })
            .collect::<Result<_, String>>()?;
        let stats: Vec<StatRecord<'_>> = self
            .stats
            .iter()
            .map(|s| StatRecord {
                api: &s.id,
                min: s.min,
                max: s.max,
                increment_only: s.increment_only,
            })
            .collect();
        let presence: Vec<&str> = self.presence.iter().map(String::as_str).collect();
        let mut out = vec![0u8; table::encoded_len(&achievements, &stats, &presence)];
        let n = table::encode(&achievements, &stats, &presence, &mut out)
            .map_err(|e| format!("achievements: {e:?}"))?;
        out.truncate(n);
        Ok(out)
    }
}

/// `assets/achievements.ron`, or `None` when the project has none.
pub fn load(root: &Path) -> Result<Option<AchievementsDef>, String> {
    let p = path(root);
    match std::fs::read_to_string(&p) {
        Ok(text) => AchievementsDef::from_ron(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", p.display())),
    }
}

/// The locked order (`assets/achievements.lock`), or `None`.
pub fn load_lock(root: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(lock_path(root)).ok()?;
    Some(lock_from_text(&text))
}

/// The ids of a lock file's text, in order (comments and blanks skipped).
pub fn lock_from_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Write the current order as the lock.
pub fn write_lock(root: &Path, def: &AchievementsDef) -> Result<PathBuf, String> {
    let p = lock_path(root);
    let mut text = String::from(
        "# The bit order a shipped build used (`cargo xtask services lock`).\n\
         # Append achievements; never reorder or remove a line: saves store bits.\n",
    );
    for a in &def.achievements {
        text.push_str(&a.id);
        text.push('\n');
    }
    std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

/// One problem `services check` (and the editor) reports.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    /// `achievements`, `stats`, `presence`, `strings`, `icons` or `lock`.
    pub area: &'static str,
    /// The id or key it is about.
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: [{}] {}", self.area, self.subject)?;
        if let Some(l) = &self.lang {
            write!(f, " ({l})")?;
        }
        write!(f, ": {}", self.message)
    }
}

fn finding(
    level: Level,
    area: &'static str,
    subject: &str,
    lang: Option<&str>,
    message: String,
) -> Finding {
    Finding {
        level,
        area,
        subject: subject.to_string(),
        lang: lang.map(String::from),
        message,
    }
}

fn valid_api(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_API_NAME
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Everything wrong with a project's achievements: ids, limits, stat
/// references, the lock, the icons (`has_sprite` answers whether the
/// manifest declares a sprite) and the strings in every language of
/// `langs` (an error where a key is missing from a language and its
/// fallbacks, a warning where only a fallback has it).
pub fn check(
    def: &AchievementsDef,
    langs: &Langs,
    lock: Option<&[String]>,
    has_sprite: &dyn Fn(&str) -> bool,
) -> Vec<Finding> {
    use Level::{Error, Warning};
    let mut out = Vec::new();
    if def.achievements.len() > MAX_ACHIEVEMENTS {
        out.push(finding(
            Error,
            "achievements",
            "",
            None,
            format!(
                "{} achievements: at most {MAX_ACHIEVEMENTS}",
                def.achievements.len()
            ),
        ));
    }
    if def.stats.len() > MAX_STATS {
        out.push(finding(
            Error,
            "stats",
            "",
            None,
            format!("{} stats: at most {MAX_STATS}", def.stats.len()),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    for s in &def.stats {
        if !valid_api(&s.id) {
            out.push(finding(
                Error,
                "stats",
                &s.id,
                None,
                "an id is letters, digits and `_` (Steam's API name)".into(),
            ));
        }
        if !ids.insert(s.id.clone()) || !hashes.insert(trino_core::asset_id(&s.id)) {
            out.push(finding(
                Error,
                "stats",
                &s.id,
                None,
                "declared twice".into(),
            ));
        }
        if s.min > s.max {
            out.push(finding(
                Error,
                "stats",
                &s.id,
                None,
                format!("min {} is above max {}", s.min, s.max),
            ));
        }
    }
    for a in &def.achievements {
        if !valid_api(&a.id) {
            out.push(finding(
                Error,
                "achievements",
                &a.id,
                None,
                "an id is letters, digits and `_` (Steam's API name)".into(),
            ));
        }
        if !ids.insert(a.id.clone()) || !hashes.insert(trino_core::asset_id(&a.id)) {
            out.push(finding(
                Error,
                "achievements",
                &a.id,
                None,
                "declared twice (ids are shared with stats)".into(),
            ));
        }
        if a.name.is_empty() || a.description.is_empty() {
            out.push(finding(
                Error,
                "achievements",
                &a.id,
                None,
                "needs a `name` and a `description` key".into(),
            ));
        }
        match (
            &a.stat,
            def.stats.iter().find(|s| Some(&s.id) == a.stat.as_ref()),
        ) {
            (Some(s), None) => out.push(finding(
                Error,
                "achievements",
                &a.id,
                None,
                format!("its stat `{s}` is not declared"),
            )),
            (Some(_), Some(s)) if a.threshold <= s.min || a.threshold > s.max => out.push(finding(
                Error,
                "achievements",
                &a.id,
                None,
                format!(
                    "threshold {} is outside its stat's range ({}, {}]",
                    a.threshold, s.min, s.max
                ),
            )),
            (None, _) if a.threshold != 0 => out.push(finding(
                Warning,
                "achievements",
                &a.id,
                None,
                "a threshold without a `stat` does nothing".into(),
            )),
            _ => {}
        }
        if let Some(icon) = &a.icon
            && !has_sprite(icon)
        {
            out.push(finding(
                Error,
                "icons",
                &a.id,
                None,
                format!("icon `{icon}` is not a sprite in the manifest"),
            ));
        }
    }
    // The shipped order.
    match lock {
        Some(lock) => {
            for (i, id) in lock.iter().enumerate() {
                match def.achievements.get(i) {
                    Some(a) if &a.id == id => {}
                    Some(a) => out.push(finding(
                        Error,
                        "lock",
                        id,
                        None,
                        format!(
                            "bit {i} was `{id}` in achievements.lock and is `{}` now: \
                             saves would read the wrong bits (append new ones at the end)",
                            a.id
                        ),
                    )),
                    None => out.push(finding(
                        Error,
                        "lock",
                        id,
                        None,
                        format!("bit {i} (`{id}`) was removed: saves still hold it"),
                    )),
                }
            }
        }
        None if !def.achievements.is_empty() => out.push(finding(
            Warning,
            "lock",
            "",
            None,
            "no assets/achievements.lock: run `cargo xtask services lock` before you ship, \
             so a later edit can't move a saved bit"
                .into(),
        )),
        None => {}
    }
    // Strings in every language.
    let names = langs.names();
    if names.is_empty() {
        if !def.achievements.is_empty() {
            out.push(finding(
                Warning,
                "strings",
                "",
                None,
                "no string tables (assets/i18n/*.ron): names draw as their keys".into(),
            ));
        }
    } else {
        let presence: BTreeSet<&String> = def.presence.iter().collect();
        for key in def.keys() {
            let area = if presence.contains(&key) {
                "presence"
            } else {
                "strings"
            };
            for lang in &names {
                let own = langs
                    .tables
                    .get(lang)
                    .is_some_and(|t| t.entries.contains_key(&key));
                if own {
                    continue;
                }
                match langs.lookup(lang, &key) {
                    Some(_) => out.push(finding(
                        Warning,
                        area,
                        &key,
                        Some(lang),
                        "only a fallback language has it".into(),
                    )),
                    None => out.push(finding(Error, area, &key, Some(lang), "missing".into())),
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::StringsDef;

    fn sample() -> AchievementsDef {
        AchievementsDef::from_ron(
            r#"(
                version: 1,
                achievements: [
                    (id: "FIRST_COIN", name: "a.n", description: "a.d", icon: "sprites/coin"),
                    (id: "COINS_10", name: "b.n", description: "b.d", stat: "COINS", threshold: 10),
                    (id: "SECRET", name: "c.n", description: "c.d", hidden: true),
                ],
                stats: [(id: "COINS", min: 0, max: 100, increment_only: true)],
                presence: ["p.menu"],
            )"#,
        )
        .unwrap()
    }

    fn langs(keys: &[&str], pt_keys: &[&str]) -> Langs {
        let mut langs = Langs::default();
        let mut en = StringsDef::new("en-US");
        for k in keys {
            en.entries
                .insert(k.to_string(), crate::i18n::EntryDef::Text(format!("{k}!")));
        }
        let mut pt = StringsDef::new("pt-BR");
        pt.fallback = "en-US".into();
        for k in pt_keys {
            pt.entries
                .insert(k.to_string(), crate::i18n::EntryDef::Text(format!("{k}?")));
        }
        langs.tables.insert("en-US".into(), en);
        langs.tables.insert("pt-BR".into(), pt);
        langs
    }

    #[test]
    fn parses_round_trips_and_bakes() {
        let def = sample();
        assert_eq!(def.achievements[0].icon.as_deref(), Some("sprites/coin"));
        assert_eq!(def.achievements[1].stat.as_deref(), Some("COINS"));
        let again = AchievementsDef::from_ron(&def.to_ron()).unwrap();
        assert_eq!(again, def);
        let blob = def.to_tach().unwrap();
        let t = trino_core::AchievementTable::parse(&blob).unwrap();
        assert_eq!((t.len(), t.stat_count(), t.presence_count()), (3, 1, 1));
        let a = t.get(1).unwrap();
        assert_eq!((a.api, a.stat, a.threshold), ("COINS_10", Some(0), 10));
        assert_eq!(
            t.get(0).unwrap().icon,
            Some(trino_core::SpriteId::from_path("sprites/coin"))
        );
        assert!(t.get(2).unwrap().hidden);
    }

    #[test]
    fn refuses_typos_and_wrong_versions() {
        assert!(AchievementsDef::from_ron("(version: 1, achievments: [])").is_err());
        assert!(AchievementsDef::from_ron("(version: 2)").is_err());
        assert!(
            AchievementsDef::from_ron(r#"(version: 1, achievements: [(id: "A", nam: "x")])"#)
                .is_err()
        );
    }

    #[test]
    fn check_reports_each_problem() {
        let def = sample();
        let all = ["a.n", "a.d", "b.n", "b.d", "c.n", "c.d", "p.menu"];
        let lock = vec!["FIRST_COIN".to_string()];
        let ok = check(&def, &langs(&all, &all), Some(&lock), &|_| true);
        assert!(ok.is_empty(), "{ok:?}");

        // A key only English has: a warning for pt-BR; a missing one: an error.
        let partial = check(&def, &langs(&all[..6], &all[..5]), Some(&lock), &|_| true);
        let errors: Vec<_> = partial.iter().filter(|f| f.level == Level::Error).collect();
        assert!(
            errors
                .iter()
                .any(|f| f.subject == "p.menu" && f.area == "presence"),
            "{partial:?}"
        );
        assert!(partial.iter().any(|f| f.subject == "c.d"
            && f.level == Level::Warning
            && f.lang.as_deref() == Some("pt-BR")));

        // Icons, stats, ids, the lock.
        let mut bad = def.clone();
        bad.achievements[1].threshold = 500;
        bad.achievements[2].id = "FIRST COIN".into();
        bad.achievements.push(AchievementDef {
            id: "FIRST_COIN".into(),
            name: "x".into(),
            description: "y".into(),
            stat: Some("NOPE".into()),
            ..Default::default()
        });
        let moved = vec!["SECRET".to_string(), "FIRST_COIN".to_string()];
        let f = check(&bad, &langs(&[], &[]), Some(&moved), &|s| {
            s != "sprites/coin"
        });
        let has = |area: &str, needle: &str| {
            f.iter()
                .any(|x| x.area == area && x.to_string().contains(needle))
        };
        assert!(has("icons", "sprites/coin"), "{f:?}");
        assert!(has("achievements", "outside its stat's range"));
        assert!(has("achievements", "letters, digits"));
        assert!(has("achievements", "declared twice"));
        assert!(has("achievements", "`NOPE` is not declared"));
        assert!(has("lock", "saves would read the wrong bits"));
        assert!(has("strings", "missing"));
        let unlocked = check(&def, &langs(&all, &all), None, &|_| true);
        assert!(
            unlocked
                .iter()
                .any(|x| x.area == "lock" && x.level == Level::Warning)
        );
    }

    #[test]
    fn lock_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        let def = sample();
        assert_eq!(load(dir.path()).unwrap(), None);
        def.save(dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), Some(def.clone()));
        assert_eq!(load_lock(dir.path()), None);
        write_lock(dir.path(), &def).unwrap();
        assert_eq!(
            load_lock(dir.path()).unwrap(),
            ["FIRST_COIN", "COINS_10", "SECRET"]
        );
    }
}
