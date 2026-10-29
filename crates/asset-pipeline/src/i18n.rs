//! Localization in the bake (issue #59, ADR 0026): the manifest's `[i18n]`,
//! glyph subsetting and per-language font atlases.
//!
//! ```toml
//! [i18n]
//! default = "en-US"                  # else the language the others fall back to
//! languages = ["en-US", "pt-BR"]     # shipped; default: every assets/i18n/*.ron
//! extra_chars = "0123456789"         # dynamic text (player names, scores)
//! groups = true                      # per-language atlases in group i18n-<lang> (lowercase)
//! check_fonts = ["fonts/ui"]         # fonts `i18n check` holds to every language
//! blobs = false                      # tables as static code instead (`i18n export --rust`)
//!
//! [i18n.overrides.ja-JP]             # a font only one language draws with
//! "fonts/ui" = { pc = "fonts/ui-ja", web = "fonts/ui-ja" }
//!
//! [fonts.ui]
//! file = "fonts/Roboto.ttf"
//! size = 11
//! chars = "auto"                     # exactly the glyphs the languages use
//! extra_chars = "#*"                 # plus these
//! per_language = true                # one atlas per language
//! ```
//!
//! - `chars = "auto"` (TTF/OTF only) bakes the union of every shipped
//!   language's glyphs ([`trino_scene::i18n::glyphs`]), every UI screen's
//!   literal text, `[i18n] extra_chars`, the font's `extra_chars`, the
//!   space and `?` (the runtime's glyph for a character an atlas lacks).
//!   Pseudo-locales (`qps-*`) never add glyphs.
//! - `per_language = true` (with `auto`) bakes `fonts/<name>` with the
//!   default language's glyphs and one `fonts/<name>@<lang>` per other
//!   language with its own. Each language's `TSTR` names the atlas it draws
//!   the font with, so [`trino_core::Localizer::font`] and the UI swap it at
//!   run time. With `[i18n] groups = true` the other languages' atlases sit
//!   in asset group `i18n-<lang>` (loaded on demand, #56), else in the
//!   font's own groups.
//! - [`Manifest::load`](crate::Manifest::load) resolves all of it, so every
//!   platform's bake, the memory plan and the reports see the concrete
//!   fonts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;
use trino_scene::i18n::{Langs, StringsDef, is_pseudo, shown_glyphs};

use crate::manifest::{Manifest, PerPlatform, Platform};

/// `[i18n]` in the manifest.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct I18nDecl {
    /// The game's default language (the first the runtime loads, the one
    /// the checks compare against).
    #[serde(default)]
    pub default: Option<String>,
    /// The languages a build ships (empty: every table but pseudo-locales).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Characters every `auto` font also bakes (dynamic text).
    #[serde(default)]
    pub extra_chars: String,
    /// Per-language atlases in asset group `i18n-<lang>`.
    #[serde(default)]
    pub groups: bool,
    /// Bake the `i18n/<lang>` blobs (default). `false` for a game that
    /// keeps its tables as static code (`cargo xtask i18n export --rust`).
    #[serde(default = "yes")]
    pub blobs: bool,
    /// Fonts `i18n check` holds to every language's glyphs (default: the
    /// fonts the UI screens use and every `auto` font).
    #[serde(default)]
    pub check_fonts: Vec<String>,
    /// Per language: a font drawn with another (`"fonts/ui" = "fonts/ui-ja"`,
    /// or per platform).
    #[serde(default)]
    pub overrides: BTreeMap<String, BTreeMap<String, PerPlatform<String>>>,
    /// What [`resolve`] derived (not in the file).
    #[serde(skip)]
    pub resolved: Resolved,
}

fn yes() -> bool {
    true
}

impl Default for I18nDecl {
    fn default() -> Self {
        I18nDecl {
            default: None,
            languages: Vec::new(),
            extra_chars: String::new(),
            groups: false,
            blobs: true,
            check_fonts: Vec::new(),
            overrides: BTreeMap::new(),
            resolved: Resolved::default(),
        }
    }
}

/// The `[i18n]` facts every bake needs, from [`resolve`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Resolved {
    pub default: Option<String>,
    /// The shipped languages, the default first.
    pub languages: Vec<String>,
    /// `(base font logical, language, atlas logical)` of every
    /// per-language atlas.
    pub atlases: Vec<(String, String, String)>,
}

/// The group a language's atlases go in with `[i18n] groups = true`
/// (`i18n-pt-br`: group names are lowercase).
pub fn group_name(lang: &str) -> String {
    format!("i18n-{}", lang.to_ascii_lowercase())
}

/// The logical path of `font`'s atlas for `lang` (`fonts/ui@pt-BR`).
pub fn atlas_logical(font_logical: &str, lang: &str) -> String {
    format!("{font_logical}@{lang}")
}

/// The project's tables with the manifest's default language.
pub fn langs(assets_dir: &Path, decl: Option<&I18nDecl>) -> Result<Langs, String> {
    let mut langs = Langs::load_dir(&assets_dir.join("i18n"))?;
    langs.default = decl.and_then(|d| d.default.clone());
    Ok(langs)
}

/// The languages a build ships, the default first: `[i18n] languages`, or
/// every table that is not a pseudo-locale.
pub fn shipped<'a>(langs: &'a Langs, decl: Option<&I18nDecl>) -> Vec<&'a StringsDef> {
    let listed = decl.map(|d| d.languages.as_slice()).unwrap_or(&[]);
    langs
        .ordered()
        .into_iter()
        .filter(|t| {
            if listed.is_empty() {
                !is_pseudo(&t.lang)
            } else {
                listed.contains(&t.lang)
            }
        })
        .collect()
}

/// The languages `platform` bakes: the shipped ones, plus pseudo-locales on
/// the PC and the Web (development builds see them; consoles never do).
pub fn baked_for<'a>(
    langs: &'a Langs,
    decl: Option<&I18nDecl>,
    platform: Platform,
) -> Vec<&'a StringsDef> {
    let mut v = shipped(langs, decl);
    if matches!(platform, Platform::Pc | Platform::Web) {
        for t in langs.ordered() {
            if is_pseudo(&t.lang) && !v.iter().any(|s| s.lang == t.lang) {
                v.push(t);
            }
        }
    }
    v
}

/// Every literal `text` of the project's UI screens (drawn whatever the
/// language).
pub fn screen_texts(project_root: &Path) -> BTreeSet<char> {
    let mut set = BTreeSet::new();
    for path in crate::ui::screen_files(project_root) {
        if let Ok(def) = trino_scene::ui::ScreenDef::load(&path) {
            for (_, n) in def.nodes() {
                let (before, after) =
                    match (!n.bind.is_empty()).then(|| n.text.find("{}")).flatten() {
                        Some(at) => (&n.text[..at], &n.text[at + 2..]),
                        None => (n.text.as_str(), ""),
                    };
                set.extend(before.chars().chain(after.chars()).filter(|c| *c != '\n'));
            }
        }
    }
    set
}

/// Resolves `[i18n]` and every font's `chars = "auto"` / `per_language`
/// in `manifest` (whose `assets/` is `assets_dir`): concrete `chars`, the
/// synthesized `fonts/<name>@<lang>` atlases and their groups.
pub fn resolve(manifest: &mut Manifest, assets_dir: &Path) -> Result<(), String> {
    let wants = manifest.i18n.is_some()
        || manifest
            .fonts
            .values()
            .any(|f| f.chars.as_deref() == Some("auto") || f.per_language);
    if !wants {
        return Ok(());
    }
    let decl = manifest.i18n.clone().unwrap_or_default();
    let langs = langs(assets_dir, Some(&decl))?;
    for l in &decl.languages {
        if !langs.tables.contains_key(l) {
            return Err(format!(
                "[i18n] languages: `{l}` has no assets/i18n/{l}.ron (have: {})",
                langs.names().join(", ")
            ));
        }
    }
    if let Some(d) = &decl.default
        && !langs.tables.contains_key(d)
    {
        return Err(format!("[i18n] default: `{d}` has no assets/i18n/{d}.ron"));
    }
    let shipped = shipped(&langs, Some(&decl));
    let default = langs.default_lang().map(str::to_string);
    let project_root = assets_dir.parent().unwrap_or(Path::new(""));
    let ui_chars = screen_texts(project_root);
    // What each language shows: its own texts and those its fallback
    // chain lends it (drawn with its atlas too).
    let per_lang: BTreeMap<&str, BTreeSet<char>> = shipped
        .iter()
        .map(|t| (t.lang.as_str(), shown_glyphs(&langs, &t.lang)))
        .collect();
    let mut resolved = Resolved {
        default: default.clone(),
        languages: shipped.iter().map(|t| t.lang.clone()).collect(),
        atlases: Vec::new(),
    };
    let mut new_fonts = Vec::new();
    for (name, font) in manifest.fonts.iter_mut() {
        let auto = font.chars.as_deref() == Some("auto");
        if font.per_language && !auto {
            return Err(format!(
                "font `fonts/{name}`: per_language needs chars = \"auto\""
            ));
        }
        if !auto {
            continue;
        }
        if !font.file.to_ascii_lowercase().ends_with(".ttf")
            && !font.file.to_ascii_lowercase().ends_with(".otf")
        {
            return Err(format!(
                "font `fonts/{name}`: chars = \"auto\" needs a TTF/OTF master \
                 (a PNG grid's cells are its glyphs)"
            ));
        }
        let mut base: BTreeSet<char> = ui_chars.clone();
        base.extend(decl.extra_chars.chars());
        base.extend(font.extra_chars.as_deref().unwrap_or("").chars());
        // The space, and `?`: what the runtime draws for a glyph the atlas
        // lacks (dynamic text), which measures it too.
        base.extend([' ', '?']);
        let logical = format!("fonts/{name}");
        let all = |set: &mut BTreeSet<char>, langs: &mut dyn Iterator<Item = &BTreeSet<char>>| {
            for g in langs {
                set.extend(g.iter().copied());
            }
        };
        if font.per_language {
            let mut own = base.clone();
            if let Some(g) = default.as_deref().and_then(|d| per_lang.get(d)) {
                own.extend(g.iter().copied());
            }
            for (lang, g) in &per_lang {
                if Some(*lang) == default.as_deref() {
                    continue;
                }
                let mut set = base.clone();
                set.extend(g.iter().copied());
                let mut atlas = font.clone();
                atlas.per_language = false;
                atlas.auto_resolved = true;
                atlas.chars = Some(set.into_iter().collect());
                if decl.groups {
                    atlas.groups = vec![group_name(lang)];
                }
                let atlas_name = format!("{name}@{lang}");
                resolved.atlases.push((
                    logical.clone(),
                    lang.to_string(),
                    format!("fonts/{atlas_name}"),
                ));
                new_fonts.push((atlas_name, atlas));
            }
            font.chars = Some(own.into_iter().collect());
            font.auto_resolved = true;
        } else {
            let mut set = base;
            all(&mut set, &mut per_lang.values());
            font.chars = Some(set.into_iter().collect());
            font.auto_resolved = true;
        }
    }
    for (name, atlas) in new_fonts {
        if decl.groups {
            for g in &atlas.groups {
                manifest.groups.entry(g.clone()).or_default();
            }
        }
        manifest.fonts.insert(name, atlas);
    }
    for (lang, map) in &decl.overrides {
        if !langs.tables.contains_key(lang) {
            return Err(format!(
                "[i18n.overrides.{lang}]: no assets/i18n/{lang}.ron"
            ));
        }
        for (from, to) in map {
            for logical in std::iter::once(from).chain(per_platform_values(to).iter()) {
                let known = logical
                    .strip_prefix("fonts/")
                    .is_some_and(|n| manifest.fonts.contains_key(n));
                if !known {
                    return Err(format!(
                        "[i18n.overrides.{lang}]: `{logical}` is not a font of the manifest"
                    ));
                }
            }
        }
    }
    let mut decl = decl;
    decl.resolved = resolved;
    manifest.i18n = Some(decl);
    Ok(())
}

fn per_platform_values(v: &PerPlatform<String>) -> Vec<String> {
    match v {
        PerPlatform::All(s) => vec![s.clone()],
        PerPlatform::Each(m) => m.values().cloned().collect(),
    }
}

/// The font remaps a language's `TSTR` carries on `platform`: its
/// per-language atlases and its `[i18n.overrides]`, as `(font, the font it
/// draws with)` hashes; and its fonts' group (0 = none).
pub fn remaps(manifest: &Manifest, lang: &str, platform: Platform) -> (Vec<(u32, u32)>, u32) {
    let Some(decl) = &manifest.i18n else {
        return (Vec::new(), 0);
    };
    let id = trino_core::asset_id;
    let mut out: Vec<(u32, u32)> = decl
        .resolved
        .atlases
        .iter()
        .filter(|(_, l, _)| l == lang)
        .map(|(base, _, atlas)| (id(base), id(atlas)))
        .collect();
    let has_atlas = !out.is_empty();
    if let Some(map) = decl.overrides.get(lang) {
        for (from, to) in map {
            if let Some(to) = to.get(platform) {
                out.retain(|(f, _)| *f != id(from));
                out.push((id(from), id(&to)));
            }
        }
    }
    out.sort_unstable();
    let group = if decl.groups && has_atlas {
        trino_core::GroupId::from_name(&group_name(lang)).0
    } else {
        0
    };
    (out, group)
}

/// `langs` with every language's font remaps on `platform` (what its
/// `TSTR` carries), so host layouts measure with the fonts the runtime
/// draws (`ui check`, `i18n check`, the editor).
pub fn with_remaps(mut langs: Langs, manifest: &Manifest, platform: Platform) -> Langs {
    langs.fonts = langs
        .tables
        .keys()
        .map(|l| (l.clone(), remaps(manifest, l, platform).0))
        .collect();
    langs
}

/// The fonts `i18n check` holds to each language: `[i18n] check_fonts`, or
/// the fonts the UI screens use plus every `auto` font. Per language, the
/// font that language actually draws with (its atlas or override).
pub fn checked_fonts(manifest: &Manifest, project_root: &Path) -> Vec<String> {
    if let Some(d) = &manifest.i18n
        && !d.check_fonts.is_empty()
    {
        return d.check_fonts.clone();
    }
    let mut set = BTreeSet::new();
    for path in crate::ui::screen_files(project_root) {
        if let Ok(def) = trino_scene::ui::ScreenDef::load(&path) {
            set.insert(def.theme.font_path().to_string());
            for (_, n) in def.nodes() {
                if !n.font.is_empty() {
                    set.insert(n.font.clone());
                }
            }
        }
    }
    for (name, f) in &manifest.fonts {
        if f.extra_chars.is_some() || f.per_language || f.auto_resolved {
            set.insert(format!("fonts/{name}"));
        }
    }
    set.retain(|f| {
        f.strip_prefix("fonts/")
            .is_some_and(|n| manifest.fonts.contains_key(n) && !n.contains('@'))
    });
    set.into_iter().collect()
}

/// The font `lang` draws `font` with on `platform` (logical paths).
pub fn font_for(manifest: &Manifest, font: &str, lang: &str, platform: Platform) -> String {
    let (remaps, _) = remaps(manifest, lang, platform);
    let id = trino_core::asset_id(font);
    match remaps.iter().find(|(f, _)| *f == id) {
        Some((_, to)) => manifest
            .fonts
            .keys()
            .map(|n| format!("fonts/{n}"))
            .find(|l| trino_core::asset_id(l) == *to)
            .unwrap_or_else(|| font.to_string()),
        None => font.to_string(),
    }
}

/// One font atlas in the `i18n fonts` report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FontCost {
    pub font: String,
    /// The language it serves (`*` = every language: a shared atlas).
    pub lang: String,
    pub glyphs: usize,
    pub width: u32,
    pub height: u32,
    pub platform: String,
    pub format: String,
    pub bytes: u64,
    /// TMEM on the N64, texture memory on the 3DS, VRAM on the PC.
    pub memory: String,
    /// What it takes of `memory` (N64: its texels in TMEM).
    pub memory_bytes: u64,
    pub budget: Option<u64>,
    pub fits: bool,
    pub group: String,
}

/// The atlases of every font a language draws (`auto` fonts and their
/// per-language copies), their glyphs and their memory on each platform.
pub fn font_costs(
    assets_dir: &Path,
    manifest: &Manifest,
    platforms: &[Platform],
) -> Result<Vec<FontCost>, String> {
    let atlases = manifest
        .i18n
        .as_ref()
        .map(|d| d.resolved.atlases.clone())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        let lang = match atlases.iter().find(|(_, _, a)| *a == logical) {
            Some((_, l, _)) => l.clone(),
            None if atlases.iter().any(|(b, _, _)| *b == logical) => manifest
                .i18n
                .as_ref()
                .and_then(|d| d.resolved.default.clone())
                .unwrap_or_else(|| "*".into()),
            None => "*".into(),
        };
        for &platform in platforms {
            let (settings, m) = crate::texture::master(assets_dir, manifest, &logical, platform)?;
            let choice = crate::texture::choose(
                platform, &logical, m.source, &settings, m.width, m.height, &m.rgba,
            )?;
            let r = crate::texture::report(platform, choice, m.width, m.height);
            out.push(FontCost {
                font: logical.clone(),
                lang: lang.clone(),
                glyphs: decl.chars.as_ref().map_or(95, |c| c.chars().count()),
                width: m.width,
                height: m.height,
                platform: platform.key().into(),
                format: r.format.clone(),
                bytes: r.bytes,
                memory: r.memory.into(),
                memory_bytes: r.memory_bytes,
                budget: r.memory_budget,
                fits: r.memory_budget.is_none_or(|b| r.memory_bytes <= b),
                group: if decl.groups.is_empty() {
                    "boot".into()
                } else {
                    decl.groups.join(",")
                },
            });
        }
    }
    Ok(out)
}
