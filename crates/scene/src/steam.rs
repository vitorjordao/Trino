//! Files for the Steamworks partner site and the Steam client, generated
//! from a project's text (issue #61, ADR 0029): `cargo xtask services
//! export steam` and the editor's Achievements tab write the same bytes.
//!
//! - [`achievements_csv`]: one row per achievement (API name, every
//!   language's name and description, hidden, icons, stat, threshold): what
//!   you type into *Stats & Achievements* (Steam has no bulk import for the
//!   achievements themselves).
//! - [`achievements_loc_vdf`]: the achievement localization file the same
//!   page uploads, keyed by Steam's tokens (`NEW_ACHIEVEMENT_<block>_<bit>`):
//!   they follow the order the achievements were created in, so create them
//!   in the file's order.
//! - [`presence_vdf`]: a rich-presence localization file per language
//!   (*Rich Presence* page); `presence.stage` becomes the token
//!   `#presence_stage` and `{stage}` becomes `%stage%`, as the runtime sends
//!   them ([`presence_token`]).
//! - [`input_manifest_vdf`]: the Steam Input action manifest
//!   (`game_actions_<app id>.vdf`): one action set with every Trino button,
//!   both sticks and the triggers, so Steam Input and the Steam Deck map a
//!   controller to the game's own buttons.
//! - [`app_build_vdf`] / [`depot_build_vdf`]: SteamPipe scripts for
//!   `steamcmd +run_app_build` over `cargo xtask package pc --steam`'s
//!   layout.

use std::fmt::Write as _;

use crate::i18n::Langs;
use crate::services::AchievementsDef;

/// Steam's API language name for a BCP 47 tag (`pt-BR` → `brazilian`), or
/// `None` for a language Steam doesn't list.
pub fn steam_language(tag: &str) -> Option<&'static str> {
    let lower = tag.to_ascii_lowercase();
    let (lang, region) = match lower.split_once(['-', '_']) {
        Some((l, r)) => (l, r),
        None => (lower.as_str(), ""),
    };
    Some(match (lang, region) {
        ("en", _) => "english",
        ("pt", "br") => "brazilian",
        ("pt", _) => "portuguese",
        ("es", "419" | "mx" | "ar" | "co" | "cl" | "pe") => "latam",
        ("es", _) => "spanish",
        ("fr", _) => "french",
        ("de", _) => "german",
        ("it", _) => "italian",
        ("ja", _) => "japanese",
        ("ko", _) => "koreana",
        ("zh", "tw" | "hk" | "hant") => "tchinese",
        ("zh", _) => "schinese",
        ("ru", _) => "russian",
        ("pl", _) => "polish",
        ("nl", _) => "dutch",
        ("tr", _) => "turkish",
        ("sv", _) => "swedish",
        ("da", _) => "danish",
        ("fi", _) => "finnish",
        ("no" | "nb" | "nn", _) => "norwegian",
        ("cs", _) => "czech",
        ("hu", _) => "hungarian",
        ("ro", _) => "romanian",
        ("uk", _) => "ukrainian",
        ("th", _) => "thai",
        ("vi", _) => "vietnamese",
        ("el", _) => "greek",
        ("bg", _) => "bulgarian",
        ("id", _) => "indonesian",
        ("ar", _) => "arabic",
        _ => return None,
    })
}

/// A presence key as Steam's token: `#` + the key with `.` and `-` as `_`.
pub fn presence_token(key: &str) -> String {
    let mut t = String::from("#");
    t.extend(
        key.chars()
            .map(|c| if c == '.' || c == '-' { '_' } else { c }),
    );
    t
}

/// `{name}` and `{name:format}` placeholders as Steam's `%name%`.
pub fn steam_placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => {
                let inner = &rest[i + 1..i + j];
                let name = inner.split(':').next().unwrap_or(inner).trim();
                let _ = write!(out, "%{name}%");
                rest = &rest[i + j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A VDF string literal.
pub fn vdf_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The text of `key` in `lang` (its fallbacks included), or the key.
fn text<'a>(langs: &'a Langs, lang: &str, key: &'a str) -> &'a str {
    langs.lookup(lang, key).map_or(key, |e| e.text())
}

/// The languages Steam knows, in the tables' order, with their Steam names.
fn steam_langs(langs: &Langs) -> Vec<(String, &'static str)> {
    let mut seen = Vec::new();
    langs
        .names()
        .into_iter()
        .filter(|l| !crate::i18n::is_pseudo(l))
        .filter_map(|l| steam_language(&l).map(|s| (l, s)))
        .filter(|(_, s)| {
            let fresh = !seen.contains(s);
            seen.push(*s);
            fresh
        })
        .collect()
}

/// The icon files [`achievements_csv`] names (`icons/<API>.png`,
/// `icons/<API>_locked.png`).
pub fn icon_files(api: &str) -> (String, String) {
    (
        format!("icons/{api}.png"),
        format!("icons/{api}_locked.png"),
    )
}

/// Steam's achievement token for bit `index` of a fresh app.
pub fn achievement_token(index: usize) -> String {
    format!("NEW_ACHIEVEMENT_{}_{}", index / 32 + 1, index % 32)
}

pub fn achievements_csv(def: &AchievementsDef, langs: &Langs) -> String {
    let langs_s = steam_langs(langs);
    let mut out = String::from("api_name,token");
    for (_, s) in &langs_s {
        let _ = write!(out, ",name_{s},description_{s}");
    }
    out.push_str(",hidden,icon,icon_locked,stat,threshold\n");
    for (i, a) in def.achievements.iter().enumerate() {
        let _ = write!(out, "{},{}", csv_field(&a.id), achievement_token(i));
        for (l, _) in &langs_s {
            let _ = write!(
                out,
                ",{},{}",
                csv_field(text(langs, l, &a.name)),
                csv_field(text(langs, l, &a.description))
            );
        }
        let (icon, locked) = icon_files(&a.id);
        let _ = writeln!(
            out,
            ",{},{icon},{locked},{},{}",
            a.hidden as u8,
            a.stat.as_deref().unwrap_or(""),
            if a.stat.is_some() {
                a.threshold.to_string()
            } else {
                String::new()
            }
        );
    }
    out
}

pub fn achievements_loc_vdf(def: &AchievementsDef, langs: &Langs) -> String {
    let mut out = String::from(
        "// Steam achievement localization (Stats & Achievements > Upload localization file).\n\
         // Tokens follow creation order: create the achievements in assets/achievements.ron's order.\n\
         \"lang\"\n{\n",
    );
    for (l, s) in steam_langs(langs) {
        let _ = writeln!(out, "\t{}\n\t{{\n\t\t\"Tokens\"\n\t\t{{", vdf_str(s));
        for (i, a) in def.achievements.iter().enumerate() {
            let t = achievement_token(i);
            let _ = writeln!(
                out,
                "\t\t\t{}\t{}",
                vdf_str(&format!("{t}_NAME")),
                vdf_str(text(langs, &l, &a.name))
            );
            let _ = writeln!(
                out,
                "\t\t\t{}\t{}",
                vdf_str(&format!("{t}_DESC")),
                vdf_str(text(langs, &l, &a.description))
            );
        }
        out.push_str("\t\t}\n\t}\n");
    }
    out.push_str("}\n");
    out
}

/// One rich-presence file per Steam language: `(steam language, text)`.
pub fn presence_vdf(def: &AchievementsDef, langs: &Langs) -> Vec<(&'static str, String)> {
    steam_langs(langs)
        .into_iter()
        .map(|(l, s)| {
            let mut out = String::new();
            let _ = writeln!(
                out,
                "\"lang\"\n{{\n\t\"Language\"\t{}\n\t\"Tokens\"\n\t{{",
                vdf_str(s)
            );
            for key in &def.presence {
                let _ = writeln!(
                    out,
                    "\t\t{}\t{}",
                    vdf_str(&presence_token(key)),
                    vdf_str(&steam_placeholders(text(langs, &l, key)))
                );
            }
            out.push_str("\t}\n}\n");
            (s, out)
        })
        .collect()
}

/// Steam Input's digital actions, one per Trino button (`button_a` …), in
/// `Button::ALL` order: the runtime reads them back by these names.
pub const BUTTON_ACTIONS: [(&str, &str); 16] = [
    ("button_a", "A"),
    ("button_b", "B"),
    ("button_x", "X"),
    ("button_y", "Y"),
    ("button_l", "L"),
    ("button_r", "R"),
    ("button_start", "Start"),
    ("button_select", "Select"),
    ("dpad_up", "Up"),
    ("dpad_down", "Down"),
    ("dpad_left", "Left"),
    ("dpad_right", "Right"),
    ("button_l2", "L2"),
    ("button_r2", "R2"),
    ("button_l3", "L3"),
    ("button_r3", "R3"),
];
/// The action set and the analog actions.
pub const ACTION_SET: &str = "InGameControls";
pub const MOVE_ACTION: &str = "move";
pub const CAMERA_ACTION: &str = "camera";
pub const TRIGGER_L_ACTION: &str = "trigger_l";
pub const TRIGGER_R_ACTION: &str = "trigger_r";

/// The Steam Input action manifest (`game_actions_<app id>.vdf`), English
/// titles (`title` is the game's).
pub fn input_manifest_vdf(title: &str) -> String {
    let mut out = String::from("\"In Game Actions\"\n{\n\t\"actions\"\n\t{\n");
    let _ = writeln!(
        out,
        "\t\t{}\n\t\t{{\n\t\t\t\"title\"\t\"#Set_Ingame\"",
        vdf_str(ACTION_SET)
    );
    out.push_str("\t\t\t\"StickPadGyro\"\n\t\t\t{\n");
    for (a, t) in [(MOVE_ACTION, "Move"), (CAMERA_ACTION, "Camera")] {
        let _ = writeln!(
            out,
            "\t\t\t\t{}\n\t\t\t\t{{\n\t\t\t\t\t\"title\"\t\"#Action_{t}\"\n\t\t\t\t\t\"input_mode\"\t\"joystick_move\"\n\t\t\t\t}}",
            vdf_str(a)
        );
    }
    out.push_str("\t\t\t}\n\t\t\t\"AnalogTrigger\"\n\t\t\t{\n");
    let _ = writeln!(
        out,
        "\t\t\t\t{}\t\"#Action_TriggerL\"",
        vdf_str(TRIGGER_L_ACTION)
    );
    let _ = writeln!(
        out,
        "\t\t\t\t{}\t\"#Action_TriggerR\"",
        vdf_str(TRIGGER_R_ACTION)
    );
    out.push_str("\t\t\t}\n\t\t\t\"Button\"\n\t\t\t{\n");
    for (a, t) in BUTTON_ACTIONS {
        let _ = writeln!(out, "\t\t\t\t{}\t\"#Action_{t}\"", vdf_str(a));
    }
    out.push_str("\t\t\t}\n\t\t}\n\t}\n\t\"localization\"\n\t{\n\t\t\"english\"\n\t\t{\n");
    let _ = writeln!(out, "\t\t\t\"Set_Ingame\"\t{}", vdf_str(title));
    for t in ["Move", "Camera", "TriggerL", "TriggerR"] {
        let _ = writeln!(out, "\t\t\t\"Action_{t}\"\t{}", vdf_str(t));
    }
    for (_, t) in BUTTON_ACTIONS {
        let _ = writeln!(out, "\t\t\t\"Action_{t}\"\t{}", vdf_str(t));
    }
    out.push_str("\t\t}\n\t}\n}\n");
    out
}

/// A depot: its id and the folder `package pc --steam` filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Depot {
    pub id: u32,
    pub os: &'static str,
}

/// `app_build_<app id>.vdf`, written in `steam/scripts/`: builds every depot
/// from `steam/content/<os>`. SteamPipe resolves `ContentRoot` and
/// `BuildOutput` against the script file, hence `../`.
pub fn app_build_vdf(app_id: u32, description: &str, depots: &[Depot]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\"AppBuild\"\n{{\n\t\"AppID\"\t\"{app_id}\"\n\t\"Desc\"\t{}\n\t\"ContentRoot\"\t\"../content\"\n\t\"BuildOutput\"\t\"../output\"\n\t\"Depots\"\n\t{{",
        vdf_str(description)
    );
    for d in depots {
        let _ = writeln!(out, "\t\t\"{}\"\t\"depot_build_{}.vdf\"", d.id, d.id);
    }
    out.push_str("\t}\n}\n");
    out
}

/// `depot_build_<depot>.vdf`: every file of `content/<os>`.
pub fn depot_build_vdf(depot: &Depot) -> String {
    format!(
        "\"DepotBuild\"\n{{\n\t\"DepotID\"\t\"{}\"\n\t\"FileMapping\"\n\t{{\n\t\t\"LocalPath\"\t\"{}/*\"\n\t\t\"DepotPath\"\t\".\"\n\t\t\"Recursive\"\t\"1\"\n\t}}\n}}\n",
        depot.id, depot.os
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{EntryDef, StringsDef};

    fn sample() -> (AchievementsDef, Langs) {
        let def = AchievementsDef::from_ron(
            r#"(version: 1,
                achievements: [
                    (id: "FIRST", name: "a.n", description: "a.d"),
                    (id: "SECOND", name: "b.n", description: "b.d", hidden: true),
                ],
                presence: ["presence.stage"])"#,
        )
        .unwrap();
        let mut langs = Langs::default();
        let mut en = StringsDef::new("en-US");
        for (k, v) in [
            ("a.n", "First, \"quoted\""),
            ("a.d", "Do it"),
            ("b.n", "Second"),
            ("b.d", "Again"),
            ("presence.stage", "Stage {stage} of {total:int}"),
        ] {
            en.entries.insert(k.into(), EntryDef::Text(v.into()));
        }
        let mut pt = StringsDef::new("pt-BR");
        pt.fallback = "en-US".into();
        pt.entries
            .insert("a.n".into(), EntryDef::Text("Primeira".into()));
        langs.tables.insert("en-US".into(), en);
        langs.tables.insert("pt-BR".into(), pt);
        langs
            .tables
            .insert("qps-ploc".into(), StringsDef::new("qps-ploc"));
        (def, langs)
    }

    #[test]
    fn languages_and_placeholders() {
        assert_eq!(steam_language("pt-BR"), Some("brazilian"));
        assert_eq!(steam_language("pt-PT"), Some("portuguese"));
        assert_eq!(steam_language("es-419"), Some("latam"));
        assert_eq!(steam_language("zh-TW"), Some("tchinese"));
        assert_eq!(steam_language("xx"), None);
        assert_eq!(presence_token("presence.in-match"), "#presence_in_match");
        assert_eq!(
            steam_placeholders("Stage {stage} of {total:int}, {broken"),
            "Stage %stage% of %total%, {broken"
        );
    }

    #[test]
    fn exports_every_language_steam_knows() {
        let (def, langs) = sample();
        let csv = achievements_csv(&def, &langs);
        let mut lines = csv.lines();
        assert_eq!(
            lines.next(),
            Some(
                "api_name,token,name_english,description_english,name_brazilian,description_brazilian,hidden,icon,icon_locked,stat,threshold"
            )
        );
        assert_eq!(
            lines.next(),
            Some(
                "FIRST,NEW_ACHIEVEMENT_1_0,\"First, \"\"quoted\"\"\",Do it,Primeira,Do it,0,icons/FIRST.png,icons/FIRST_locked.png,,"
            )
        );
        assert!(lines.next().unwrap().contains(",1,icons/SECOND.png"));
        let vdf = achievements_loc_vdf(&def, &langs);
        assert!(vdf.contains("\"NEW_ACHIEVEMENT_1_1_DESC\"\t\"Again\""));
        assert!(vdf.contains("\"First, \\\"quoted\\\"\""));
        assert!(vdf.contains("\"brazilian\"") && !vdf.contains("qps"));
        let p = presence_vdf(&def, &langs);
        assert_eq!(p.len(), 2);
        assert!(
            p[0].1
                .contains("\"#presence_stage\"\t\"Stage %stage% of %total%\"")
        );
    }

    #[test]
    fn input_manifest_and_build_scripts() {
        let m = input_manifest_vdf("My \"Game\"");
        for (a, _) in BUTTON_ACTIONS {
            assert!(m.contains(&format!("\"{a}\"")), "{a}");
        }
        assert!(m.contains("\"joystick_move\"") && m.contains("My \\\"Game\\\""));
        assert_eq!(m.matches('{').count(), m.matches('}').count());
        let depots = [
            Depot {
                id: 481,
                os: "windows",
            },
            Depot {
                id: 483,
                os: "linux",
            },
        ];
        let app = app_build_vdf(480, "v1", &depots);
        assert!(app.contains("\"481\"\t\"depot_build_481.vdf\""));
        // Relative to the script in steam/scripts/ (SteamPipe's rule).
        assert!(app.contains("\"ContentRoot\"\t\"../content\""), "{app}");
        assert!(app.contains("\"BuildOutput\"\t\"../output\""), "{app}");
        assert!(depot_build_vdf(&depots[1]).contains("\"LocalPath\"\t\"linux/*\""));
    }
}
