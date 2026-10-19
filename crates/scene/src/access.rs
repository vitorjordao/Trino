//! Accessibility data as text (issue #60, ADR 0026) and the checks behind
//! `cargo xtask access check` and the editor's Accessibility tab:
//!
//! - `assets/palettes.ron`: semantic colors (`team.home`, `status.danger`)
//!   with an alternative per color-vision mode, the groups that must stay
//!   apart, and the text/background pairs that must stay readable;
//! - `ui/themes/high_contrast.ron`: the project's high-contrast UI theme
//!   (the engine's `Theme::HIGH_CONTRAST` where it leaves a field out);
//! - checks for both, for captions (keys in every language, `important`
//!   sounds captioned) — the UI at 200 % text is `ui::audit` with
//!   [`Target::access`](crate::ui::Target::access).
//!
//! The asset bake turns them (with the manifest's captions and icons) into
//! the `access/data` blob (`trino_core::access::AccessData`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use trino_core::access::color::{MIN_DELTA_E, contrast_ratio, delta_e, simulate};
use trino_core::ui::Theme;
use trino_core::{Color, CvdMode};

use crate::ui::{Langs, Level, ThemeDef};

/// `assets/palettes.ron`.
pub const PALETTES_FILE: &str = "palettes.ron";
/// `ui/themes/high_contrast.ron`.
pub const THEME_FILE: &str = "ui/themes/high_contrast.ron";
/// The current `palettes.ron` version.
pub const VERSION: u32 = 1;
/// The contrast a pair keeps when it names none (WCAG AA for text).
pub const DEFAULT_CONTRAST: f32 = 4.5;
/// What the high-contrast theme's texts keep against their backgrounds
/// (WCAG AAA).
pub const THEME_TEXT_CONTRAST: f32 = 7.0;

/// One semantic color: its base value and what each color-vision mode
/// shows instead (a mode left out keeps the base).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SemanticColor {
    pub base: [u8; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protanopia: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deuteranopia: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tritanopia: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grayscale: Option<[u8; 3]>,
    /// What the color means (shown by the editor).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl SemanticColor {
    pub fn new(base: [u8; 3]) -> SemanticColor {
        SemanticColor {
            base,
            protanopia: None,
            deuteranopia: None,
            tritanopia: None,
            grayscale: None,
            note: String::new(),
        }
    }

    /// The color `mode` shows (its alternative, else the base).
    pub fn get(&self, mode: CvdMode) -> Color {
        rgb(self.alternative(mode).unwrap_or(self.base))
    }

    /// The alternative written for `mode` (`None` for `Off`).
    pub fn alternative(&self, mode: CvdMode) -> Option<[u8; 3]> {
        match mode {
            CvdMode::Off => None,
            CvdMode::Protanopia => self.protanopia,
            CvdMode::Deuteranopia => self.deuteranopia,
            CvdMode::Tritanopia => self.tritanopia,
            CvdMode::Grayscale => self.grayscale,
        }
    }

    pub fn set_alternative(&mut self, mode: CvdMode, value: Option<[u8; 3]>) {
        match mode {
            CvdMode::Off => {
                if let Some(v) = value {
                    self.base = v;
                }
            }
            CvdMode::Protanopia => self.protanopia = value,
            CvdMode::Deuteranopia => self.deuteranopia = value,
            CvdMode::Tritanopia => self.tritanopia = value,
            CvdMode::Grayscale => self.grayscale = value,
        }
    }

    /// The five values the bake writes, in `CvdMode` order.
    pub fn values(&self) -> [Color; 5] {
        CvdMode::ALL.map(|m| self.get(m))
    }
}

/// Two colors text is drawn with: `fg` over `bg`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContrastPair {
    pub fg: String,
    pub bg: String,
    /// The least WCAG ratio, in every mode (default 4.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
}

/// `assets/palettes.ron`. Unknown fields are errors (the editor writes it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PalettesDef {
    pub version: u32,
    /// How far apart (CIE76 ΔE) the colors of a `distinct` group stay in
    /// every mode (default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_delta_e: Option<f32>,
    pub colors: BTreeMap<String, SemanticColor>,
    /// Groups of colors players must tell apart (two teams, danger and
    /// safe).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distinct: Vec<Vec<String>>,
    /// Text that must stay readable on its background.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contrast: Vec<ContrastPair>,
}

impl Default for PalettesDef {
    fn default() -> Self {
        PalettesDef {
            version: VERSION,
            min_delta_e: None,
            colors: BTreeMap::new(),
            distinct: Vec::new(),
            contrast: Vec::new(),
        }
    }
}

impl PalettesDef {
    pub fn from_ron(text: &str) -> Result<PalettesDef, String> {
        // `deuteranopia: (230, 160, 0)` without `Some(..)`: the file is
        // written by hand as often as by the editor.
        let def: PalettesDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .map_err(|e| format!("palettes parse error: {e}"))?;
        if def.version != VERSION {
            return Err(format!(
                "unsupported palettes version {} (expected {VERSION})",
                def.version
            ));
        }
        Ok(def)
    }

    pub fn to_ron(&self) -> String {
        crate::to_ron_pretty(self) + "\n"
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::write_text(path, &self.to_ron())
    }

    pub fn min_delta_e(&self) -> f32 {
        self.min_delta_e.unwrap_or(MIN_DELTA_E)
    }

    /// `name`'s color for `mode`.
    pub fn color(&self, name: &str, mode: CvdMode) -> Option<Color> {
        self.colors.get(name).map(|c| c.get(mode))
    }
}

/// The project's palettes file.
pub fn palettes_path(root: &Path) -> PathBuf {
    root.join("assets").join(PALETTES_FILE)
}

/// The project's high-contrast theme file.
pub fn theme_path(root: &Path) -> PathBuf {
    root.join(THEME_FILE)
}

/// `assets/palettes.ron`, or `None` when the project has none.
pub fn load_palettes(root: &Path) -> Result<Option<PalettesDef>, String> {
    let path = palettes_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => PalettesDef::from_ron(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// `ui/themes/high_contrast.ron` (a `ThemeDef`, as a screen's `theme`), or
/// `None` when the project has none.
pub fn load_theme(root: &Path) -> Result<Option<ThemeDef>, String> {
    let path = theme_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => ron::from_str::<ThemeDef>(&text)
            .map(Some)
            .map_err(|e| format!("{}: high-contrast theme parse error: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// The theme the file describes, over the engine's high-contrast colors.
pub fn contrast_theme(def: &ThemeDef) -> Theme {
    def.core_over(Theme::HIGH_CONTRAST)
}

/// One problem (or a passed check, `Level`-less, for the editor's
/// checklist: see [`Check`]).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Finding {
    pub level: Level,
    /// `palettes`, `theme`, `captions`, `icons` or `ui`.
    pub area: &'static str,
    /// What it is about: a color, a pair, a sound, a screen.
    pub subject: String,
    /// The color-vision mode it fails in, if one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
    pub message: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: [{}] {}", self.area, self.subject)?;
        if let Some(m) = self.mode {
            write!(f, " ({m})")?;
        }
        write!(f, ": {}", self.message)
    }
}

fn finding(
    level: Level,
    area: &'static str,
    subject: impl Into<String>,
    mode: Option<CvdMode>,
    message: String,
) -> Finding {
    Finding {
        level,
        area,
        subject: subject.into(),
        mode: mode.map(CvdMode::key),
        message,
    }
}

/// How two semantic colors look apart in each mode (each shown as that
/// mode's alternative, seen through that mode's simulation).
pub fn distances(def: &PalettesDef, a: &str, b: &str) -> Option<[f32; 5]> {
    let (ca, cb) = (def.colors.get(a)?, def.colors.get(b)?);
    Some(CvdMode::ALL.map(|m| delta_e(simulate(m, ca.get(m)), simulate(m, cb.get(m)))))
}

/// The WCAG ratio of a pair in each mode.
pub fn contrasts(def: &PalettesDef, fg: &str, bg: &str) -> Option<[f32; 5]> {
    let (f, b) = (def.colors.get(fg)?, def.colors.get(bg)?);
    Some(CvdMode::ALL.map(|m| contrast_ratio(simulate(m, f.get(m)), simulate(m, b.get(m)))))
}

/// The palettes' problems: names a group or a pair uses that the file does
/// not define, colors of a `distinct` group a mode shows too close (the
/// alternative to write is named), pairs below their contrast.
pub fn check_palettes(def: &PalettesDef) -> Vec<Finding> {
    let mut out = Vec::new();
    let min = def.min_delta_e();
    for name in def.colors.keys() {
        if name.is_empty() || name.len() > 64 || name.contains(char::is_whitespace) {
            out.push(finding(
                Level::Error,
                "palettes",
                name.clone(),
                None,
                "a color name is 1 to 64 characters without spaces".into(),
            ));
        }
    }
    for group in &def.distinct {
        if group.len() < 2 {
            out.push(finding(
                Level::Warning,
                "palettes",
                group.join(", "),
                None,
                "a distinct group needs two colors or more".into(),
            ));
        }
        for name in group {
            if !def.colors.contains_key(name) {
                out.push(finding(
                    Level::Error,
                    "palettes",
                    name.clone(),
                    None,
                    "a distinct group names a color the file does not define".into(),
                ));
            }
        }
        for (i, a) in group.iter().enumerate() {
            for b in &group[i + 1..] {
                let Some(d) = distances(def, a, b) else {
                    continue;
                };
                for (k, mode) in CvdMode::ALL.into_iter().enumerate() {
                    if d[k] < min {
                        let fix = match mode {
                            CvdMode::Off => "change one of the base colors".to_string(),
                            m => format!("give one of them a `{}` alternative", m.key()),
                        };
                        out.push(finding(
                            Level::Error,
                            "palettes",
                            format!("{a} / {b}"),
                            Some(mode),
                            format!(
                                "they look {:.1} apart (ΔE), under the {min:.0} they must keep: {fix}",
                                d[k]
                            ),
                        ));
                    }
                }
            }
        }
    }
    for pair in &def.contrast {
        let need = pair.min.unwrap_or(DEFAULT_CONTRAST);
        for name in [&pair.fg, &pair.bg] {
            if !def.colors.contains_key(name) {
                out.push(finding(
                    Level::Error,
                    "palettes",
                    name.clone(),
                    None,
                    "a contrast pair names a color the file does not define".into(),
                ));
            }
        }
        let Some(r) = contrasts(def, &pair.fg, &pair.bg) else {
            continue;
        };
        for (k, mode) in CvdMode::ALL.into_iter().enumerate() {
            if r[k] < need {
                out.push(finding(
                    Level::Error,
                    "palettes",
                    format!("{} on {}", pair.fg, pair.bg),
                    Some(mode),
                    format!("contrast {:.2}:1, under {need:.1}:1", r[k]),
                ));
            }
        }
    }
    out
}

/// The high-contrast theme's problems: each text color against what it is
/// drawn on keeps [`THEME_TEXT_CONTRAST`], the dimmed text 4.5 and the
/// focus outline 3 (WCAG's non-text contrast).
pub fn check_theme(t: &Theme) -> Vec<Finding> {
    let pairs: [(&str, Color, Color, f32); 5] = [
        ("text on panel", t.text, t.panel, THEME_TEXT_CONTRAST),
        (
            "button_text on button",
            t.button_text,
            t.button,
            THEME_TEXT_CONTRAST,
        ),
        (
            "focus_text on focus",
            t.focus_text,
            t.focus,
            THEME_TEXT_CONTRAST,
        ),
        ("dim on panel", t.dim, t.panel, DEFAULT_CONTRAST),
        ("accent on panel", t.accent, t.panel, 3.0),
    ];
    pairs
        .into_iter()
        .filter_map(|(what, a, b, need)| {
            let r = contrast_ratio(a, b);
            (r < need).then(|| {
                finding(
                    Level::Error,
                    "theme",
                    what,
                    None,
                    format!("contrast {r:.2}:1, under {need:.1}:1"),
                )
            })
        })
        .collect()
}

/// A sound's caption as the manifest declares it.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionDecl {
    /// `sounds/whistle`, `music/goal`.
    pub logical: String,
    pub caption: Option<String>,
    pub important: bool,
}

/// The captions' problems: an `important` sound without a caption, a key
/// a language lacks (an error with no fallback to show, a warning when the
/// fallback's text shows), captions with no string table at all.
pub fn check_captions(captions: &[CaptionDecl], langs: &Langs) -> Vec<Finding> {
    let mut out = Vec::new();
    let names = langs.names();
    for c in captions {
        let Some(key) = &c.caption else {
            if c.important {
                out.push(finding(
                    Level::Error,
                    "captions",
                    c.logical.clone(),
                    None,
                    "an important sound has no caption (`caption = \"cap.x\"`)".into(),
                ));
            }
            continue;
        };
        if names.is_empty() {
            out.push(finding(
                Level::Warning,
                "captions",
                c.logical.clone(),
                None,
                format!("no assets/i18n table: the caption shows its key \"{key}\""),
            ));
            continue;
        }
        for lang in &names {
            let s = langs.strings(lang);
            if s.has_own(key) {
                continue;
            }
            let fallback = trino_core::ui::UiStrings::get(&s, key).is_some();
            out.push(Finding {
                level: if fallback {
                    Level::Warning
                } else {
                    Level::Error
                },
                area: "captions",
                subject: c.logical.clone(),
                mode: None,
                message: if fallback {
                    format!("{lang} has no \"{key}\": its fallback's text shows")
                } else {
                    format!("{lang} has no \"{key}\": the caption shows the key")
                },
            });
        }
    }
    out
}

fn rgb(c: [u8; 3]) -> Color {
    Color::rgb(c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn teams() -> PalettesDef {
        PalettesDef::from_ron(
            r#"(
                version: 1,
                colors: {
                    "team.home": (base: (200, 40, 40), deuteranopia: (230, 160, 0), protanopia: (230, 160, 0)),
                    "team.away": (base: (40, 160, 60), deuteranopia: (40, 90, 220), protanopia: (40, 90, 220), grayscale: (20, 20, 20)),
                    "hud.text": (base: (255, 255, 255)),
                    "hud.panel": (base: (16, 16, 24)),
                },
                distinct: [["team.home", "team.away"]],
                contrast: [(fg: "hud.text", bg: "hud.panel")],
            )"#,
        )
        .unwrap()
    }

    #[test]
    fn alternatives_keep_red_and_green_teams_apart() {
        let mut def = teams();
        let found = check_palettes(&def);
        assert!(found.is_empty(), "{found:#?}");
        // Without the alternatives, red and green collapse for
        // deuteranopes (ΔE 11.1, the reference implementation's number); a
        // protanope still sees the red darker (ΔE 35), and the other modes
        // keep them apart.
        for c in def.colors.values_mut() {
            c.protanopia = None;
            c.deuteranopia = None;
        }
        let found = check_palettes(&def);
        let modes: Vec<_> = found.iter().filter_map(|f| f.mode).collect();
        assert_eq!(modes, ["deuteranopia"], "{found:#?}");
        let d = distances(&def, "team.home", "team.away").unwrap();
        assert!((d[2] - 11.09).abs() < 0.05, "{d:?}");
        assert!((d[1] - 34.97).abs() < 0.05, "{d:?}");
        assert!(found[0].message.contains("alternative"), "{}", found[0]);
    }

    #[test]
    fn unknown_names_and_low_contrast_are_errors() {
        let mut def = teams();
        def.distinct
            .push(vec!["team.home".into(), "team.ghost".into()]);
        def.contrast.push(ContrastPair {
            fg: "hud.panel".into(),
            bg: "hud.panel".into(),
            min: None,
        });
        let found = check_palettes(&def);
        assert!(found.iter().any(|f| f.subject == "team.ghost"));
        assert!(
            found
                .iter()
                .any(|f| f.subject == "hud.panel on hud.panel" && f.message.contains("1.00:1"))
        );
        // Round trip through the text the editor writes.
        let back = PalettesDef::from_ron(&def.to_ron()).unwrap();
        assert_eq!(back, def);
        assert!(PalettesDef::from_ron("(version: 1, colors: {}, extra: 1)").is_err());
        assert!(PalettesDef::from_ron("(version: 2, colors: {})").is_err());
    }

    /// A node's `cvd_icon` (a sprite path) reaches the flat screen with the
    /// sprite's size from the manifest, the baked TUIL and the static
    /// export; a screen without one exports as before.
    #[test]
    fn cvd_icons_reach_the_bake_and_the_export() {
        use crate::ui::{Flat, ScreenDef, to_rust, to_tuil};
        let def = ScreenDef::from_ron(
            r#"(version: 1, root: (children: [
                (kind: Rect(), id: "kit", size: (Px(16), Px(16)), cvd_icon: "sprites/stripes"),
            ]))"#,
        )
        .unwrap();
        let sizes = |s: &str| (s == "sprites/stripes").then_some((8u16, 8u16));
        let flat = Flat::new(&def, &sizes);
        let icon = flat.nodes[1].cvd_icon.unwrap();
        assert_eq!(
            icon.sprite,
            trino_core::SpriteId::from_path("sprites/stripes")
        );
        assert_eq!(icon.size, (8, 8));
        let bytes = to_tuil(&flat).unwrap();
        let baked = trino_core::ui::UiScreen::parse(&bytes).unwrap();
        assert_eq!(baked.node(1).cvd_icon, Some(icon));
        let code = to_rust(&flat);
        assert!(
            code.contains("cvd_icon: Some(trino_core::ui::CvdIcon { sprite: trino_core::SpriteId::from_path(\"sprites/stripes\"), size: (8, 8) })"),
            "{code}"
        );
        // Written back as the editor saves it, and missing from the manifest.
        assert!(def.to_ron().contains("cvd_icon: \"sprites/stripes\""));
        let missing = crate::ui::missing_assets(&def, &|_| false);
        assert!(
            missing
                .iter()
                .any(|i| i.message.contains("sprites/stripes"))
        );
    }

    #[test]
    fn the_engine_high_contrast_theme_passes_and_a_washed_out_one_does_not() {
        assert!(check_theme(&Theme::HIGH_CONTRAST).is_empty());
        let washed = Theme {
            text: Color::rgb(120, 120, 120),
            focus: Color::rgb(255, 230, 0),
            focus_text: Color::rgb(200, 200, 200),
            ..Theme::HIGH_CONTRAST
        };
        let found = check_theme(&washed);
        let what: Vec<_> = found.iter().map(|f| f.subject.as_str()).collect();
        assert_eq!(what, ["text on panel", "focus_text on focus"], "{found:#?}");
        // A theme file leaves out what it keeps from the engine's.
        let def: ThemeDef = ron::from_str("(focus: Some((0, 200, 255, 255)))").unwrap();
        let t = contrast_theme(&def);
        assert_eq!(t.focus, Color::rgb(0, 200, 255));
        assert_eq!(t.panel, Theme::HIGH_CONTRAST.panel);
    }
}
