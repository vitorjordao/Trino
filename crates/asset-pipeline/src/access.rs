//! The project's accessibility data as one generated blob (issue #60):
//! `access/data` (`TACC`, `trino_core::access::AccessData`) from
//! `assets/palettes.ron` (semantic colors and their color-vision
//! alternatives), the manifest's sound captions and sprite icons, and
//! `ui/themes/high_contrast.ron`. Baked beside the UI blobs on every target
//! (`crate::ui::bake`), with no manifest entry; a project with none of them
//! bakes no blob.

use std::path::Path;

use trino_core::access::data::{self, CaptionInfo, CaptionRecord};
use trino_core::asset_id;
use trino_scene::access::{CaptionDecl, check_palettes, contrast_theme, load_palettes, load_theme};
use trino_scene::ui::Level;

use crate::Manifest;

/// The blob's logical path.
pub const LOGICAL: &str = data::LOGICAL;

/// Every sound and music track with its caption settings, as
/// `trino_scene::access::check_captions` reads them.
pub fn captions(manifest: &Manifest) -> Vec<CaptionDecl> {
    let sounds = manifest
        .sounds
        .iter()
        .map(|(n, d)| (format!("sounds/{n}"), d));
    let music = manifest
        .music
        .iter()
        .map(|(n, d)| (format!("music/{n}"), d));
    sounds
        .chain(music)
        .map(|(logical, d)| CaptionDecl {
            logical,
            caption: d.caption.clone(),
            important: d.important.unwrap_or(false),
        })
        .collect()
}

/// The `access/data` blob, or `None` when the project declares nothing it
/// would hold. Palettes with errors (`cargo xtask access check` names
/// them) fail the bake: a mode would ship colors that don't work.
pub fn bake(root: &Path, manifest: &Manifest) -> Result<Option<Vec<u8>>, String> {
    let palettes = load_palettes(root)?;
    let theme = load_theme(root)?.map(|t| contrast_theme(&t));
    if let Some(p) = &palettes {
        let errors: Vec<String> = check_palettes(p)
            .into_iter()
            .filter(|f| f.level == Level::Error)
            .map(|f| f.to_string())
            .collect();
        if !errors.is_empty() {
            return Err(format!(
                "{}: {}",
                trino_scene::access::palettes_path(root).display(),
                errors.join("; ")
            ));
        }
    }

    let mut colors: Vec<(u32, [trino_core::Color; 5])> = palettes
        .iter()
        .flat_map(|p| p.colors.iter())
        .map(|(name, c)| (asset_id(name), c.values()))
        .collect();
    colors.sort_by_key(|c| c.0);
    duplicates(colors.iter().map(|c| c.0), "semantic color")?;

    let decls = [("sounds", &manifest.sounds), ("music", &manifest.music)];
    let mut owned: Vec<(u32, String, CaptionInfo<'static>)> = Vec::new();
    for (prefix, section) in decls {
        for (name, d) in section {
            let Some(key) = &d.caption else { continue };
            let important = d.important.unwrap_or(false);
            owned.push((
                asset_id(&format!("{prefix}/{name}")),
                key.clone(),
                CaptionInfo {
                    key: "",
                    priority: d
                        .caption_priority
                        .unwrap_or(if important { 100 } else { 0 }),
                    important,
                    direction: d.direction.unwrap_or(false),
                    duration_ms: d
                        .caption_seconds
                        .map_or(0, |s| (s * 1000.0).round().clamp(1.0, 60_000.0) as u16),
                },
            ));
        }
    }
    owned.sort_by_key(|c| c.0);
    duplicates(owned.iter().map(|c| c.0), "caption")?;
    let captions: Vec<CaptionRecord<'_>> = owned
        .iter()
        .map(|(id, key, info)| CaptionRecord {
            sound: *id,
            info: CaptionInfo { key, ..*info },
        })
        .collect();

    let mut icons: Vec<(u32, u32)> = manifest
        .sprites
        .iter()
        .filter_map(|(n, d)| {
            let icon = d.cvd_icon.as_ref()?;
            Some((asset_id(&format!("sprites/{n}")), asset_id(icon)))
        })
        .collect();
    icons.sort_by_key(|c| c.0);

    if colors.is_empty() && captions.is_empty() && icons.is_empty() && theme.is_none() {
        return Ok(None);
    }
    let mut out = vec![0u8; data::encoded_len(colors.len(), &captions, icons.len())];
    let n = data::encode(&colors, &captions, &icons, theme.as_ref(), &mut out)
        .map_err(|e| format!("{LOGICAL}: {e:?}"))?;
    out.truncate(n);
    data::AccessData::parse(&out)
        .map_err(|e| format!("{LOGICAL}: baked blob does not parse: {e:?}"))?;
    Ok(Some(out))
}

fn duplicates(mut ids: impl Iterator<Item = u32>, what: &str) -> Result<(), String> {
    let mut last = None;
    ids.try_for_each(|id| {
        if last == Some(id) {
            return Err(format!("{LOGICAL}: two {what}s hash to {id:08x}"));
        }
        last = Some(id);
        Ok(())
    })
}

/// One line of the checklist (`cargo xtask access check`, the editor's
/// Accessibility tab): a check and whether it passed.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct CheckLine {
    pub area: &'static str,
    pub check: String,
    pub passed: bool,
}

/// Everything `access check` found in a project.
#[derive(serde::Serialize, Clone, Debug, Default)]
pub struct AccessReport {
    pub colors: usize,
    pub distinct_groups: usize,
    pub contrast_pairs: usize,
    pub captions: usize,
    pub important: usize,
    pub icons: usize,
    pub theme: bool,
    pub screens: usize,
    pub languages: Vec<String>,
    /// Every check, passed or not.
    pub checklist: Vec<CheckLine>,
    pub findings: Vec<trino_scene::access::Finding>,
}

impl AccessReport {
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.level == Level::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings.len() - self.errors()
    }
}

/// The text scale `access check` lays every screen out at.
pub const CHECK_TEXT_SCALE: f32 = 2.0;

/// Check a project's accessibility: its palettes (every group apart and
/// every pair readable in every color-vision mode), its high-contrast
/// theme, its captions (a key in every language; `important` sounds
/// captioned), its icons, and every UI screen at 200 % text on each of
/// `families` and in each language (no text past its box, the budgets
/// kept).
pub fn check_project(
    root: &Path,
    manifest: &Manifest,
    families: &[trino_core::Family],
) -> Result<AccessReport, String> {
    use trino_scene::access::{check_captions, check_theme, contrasts, distances};
    use trino_scene::ui::{Flat, ScreenDef, Target, audit, validate};

    let mut r = AccessReport::default();
    let line = |r: &mut AccessReport, area: &'static str, check: String, passed: bool| {
        r.checklist.push(CheckLine {
            area,
            check,
            passed,
        });
    };

    // Palettes.
    if let Some(p) = load_palettes(root)? {
        r.colors = p.colors.len();
        r.distinct_groups = p.distinct.len();
        r.contrast_pairs = p.contrast.len();
        let found = check_palettes(&p);
        for group in &p.distinct {
            for (i, a) in group.iter().enumerate() {
                for b in &group[i + 1..] {
                    let ok = distances(&p, a, b)
                        .is_some_and(|d| d.iter().all(|&x| x >= p.min_delta_e()));
                    line(
                        &mut r,
                        "palettes",
                        format!("{a} and {b} stay apart in every mode"),
                        ok,
                    );
                }
            }
        }
        for pair in &p.contrast {
            let need = pair.min.unwrap_or(trino_scene::access::DEFAULT_CONTRAST);
            let ok =
                contrasts(&p, &pair.fg, &pair.bg).is_some_and(|c| c.iter().all(|&x| x >= need));
            line(
                &mut r,
                "palettes",
                format!("{} on {} keeps {need:.1}:1 in every mode", pair.fg, pair.bg),
                ok,
            );
        }
        r.findings.extend(found);
    }

    // The high-contrast theme (the project's, else the engine's).
    let theme = match load_theme(root)? {
        Some(t) => {
            r.theme = true;
            contrast_theme(&t)
        }
        None => trino_core::ui::Theme::HIGH_CONTRAST,
    };
    let found = check_theme(&theme);
    let which = if r.theme {
        "ui/themes/high_contrast.ron"
    } else {
        "the engine's"
    };
    line(
        &mut r,
        "theme",
        format!("high-contrast theme ({which}) keeps 7:1 for text"),
        found.is_empty(),
    );
    r.findings.extend(found);

    // Captions.
    let langs = crate::ui::langs(root)?;
    r.languages = langs.names();
    let decls = captions(manifest);
    r.captions = decls.iter().filter(|c| c.caption.is_some()).count();
    r.important = decls.iter().filter(|c| c.important).count();
    let found = check_captions(&decls, &langs);
    for c in decls.iter().filter(|c| c.caption.is_some() || c.important) {
        let ok = !found.iter().any(|f| f.subject == c.logical);
        let what = match &c.caption {
            Some(k) => format!("{} has caption \"{k}\" in every language", c.logical),
            None => format!("{} (important) has a caption", c.logical),
        };
        line(&mut r, "captions", what, ok);
    }
    r.findings.extend(found);

    // Icons (the manifest load already refused unknown sprites).
    r.icons = manifest
        .sprites
        .values()
        .filter(|d| d.cvd_icon.is_some())
        .count();
    // Colors a `distinct` group tells apart, shown with no icon (issue
    // #96): UI nodes set to one, sprites whose art is one.
    if let Some(p) = load_palettes(root)?.filter(|p| !p.distinct.is_empty()) {
        let found = icon_hints(root, manifest, &p)?;
        line(
            &mut r,
            "icons",
            "every node and sprite in a distinct group's color has an icon".into(),
            found.is_empty(),
        );
        r.findings.extend(found);
    }

    // Every screen at 200 % text.
    let access = trino_core::ui::UiAccess {
        text_scale: CHECK_TEXT_SCALE,
        ..trino_core::ui::UiAccess::NONE
    };
    let names = r.languages.clone();
    let lang_list: Vec<Option<&str>> = if names.is_empty() {
        vec![None]
    } else {
        names.iter().map(|l| Some(l.as_str())).collect()
    };
    for path in crate::ui::screen_files(root) {
        let def = ScreenDef::load(&path)?;
        if validate(&def).iter().any(|i| i.level == Level::Error) {
            // `ui check`'s to report; it doesn't lay out.
            continue;
        }
        r.screens += 1;
        let mut failed = false;
        for &family in families {
            let platform = match family {
                trino_core::Family::Pc => crate::Platform::Pc,
                trino_core::Family::N64 => crate::Platform::N64,
                trino_core::Family::N3ds => crate::Platform::N3ds,
                trino_core::Family::Web => crate::Platform::Web,
            };
            let fonts = crate::ui::fonts(root, manifest, platform);
            let sizes = |s: &str| crate::ui::sprite_size(root, manifest, platform, s);
            let flat = Flat::new(&def, &sizes);
            for lang in &lang_list {
                let target = Target {
                    lang: *lang,
                    access,
                    ..Target::of(family.caps())
                };
                let report = audit(&flat, target, &fonts, &langs);
                let caps = family.caps();
                let nodes = def.nodes();
                let mut push = |message: String, subject: String| {
                    failed = true;
                    r.findings.push(trino_scene::access::Finding {
                        level: Level::Error,
                        area: "ui",
                        subject,
                        mode: None,
                        message,
                    });
                };
                for &i in &report.overflow {
                    push(
                        format!(
                            "text overflows its box at 200 % [{}{}]",
                            family.key(),
                            lang.map(|l| format!(", {l}")).unwrap_or_default()
                        ),
                        format!("{} {}", def.name, nodes[i].1.label()),
                    );
                }
                for &i in &report.outside_safe {
                    push(
                        format!(
                            "reaches past the safe area at 200 % [{}{}]: let it scroll or shrink it",
                            family.key(),
                            lang.map(|l| format!(", {l}")).unwrap_or_default()
                        ),
                        format!("{} {}", def.name, nodes[i].1.label()),
                    );
                }
                if report.stats.nodes > caps.max_ui_nodes || report.stats.draws > caps.max_ui_draws
                {
                    push(
                        format!(
                            "{} nodes and {} draws at 200 %, over the target's {} / {} [{}]",
                            report.stats.nodes,
                            report.stats.draws,
                            caps.max_ui_nodes,
                            caps.max_ui_draws,
                            family.key()
                        ),
                        def.name.clone(),
                    );
                }
            }
        }
        line(
            &mut r,
            "ui",
            format!(
                "{} fits at 200 % text on every target and language",
                def.name
            ),
            !failed,
        );
    }
    Ok(r)
}

/// The icon hints of `check_project` (issue #96): every UI node whose
/// color is a `distinct` group's (`ICON_NODE_DELTA_E`) and every sprite
/// whose mean opaque color is one (`ICON_SPRITE_DELTA_E`), without a
/// `cvd_icon`. The editor's UI tab and texture inspector ask the same
/// functions per node and per sprite.
pub fn icon_hints(
    root: &Path,
    manifest: &Manifest,
    palettes: &trino_scene::access::PalettesDef,
) -> Result<Vec<trino_scene::access::Finding>, String> {
    use trino_scene::access::{ICON_NODE_DELTA_E, ICON_SPRITE_DELTA_E, icon_hint};
    let mut out = Vec::new();
    for path in crate::ui::screen_files(root) {
        let Ok(def) = trino_scene::ui::ScreenDef::load(&path) else {
            continue;
        };
        let screen = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        for (at, n) in def.nodes() {
            let Some([r, g, b, _]) = n.color else {
                continue;
            };
            let subject = if n.id.is_empty() {
                format!("{screen} node {at:?}")
            } else {
                format!("{screen} #{}", n.id)
            };
            out.extend(icon_hint(
                palettes,
                &subject,
                [r, g, b],
                !n.cvd_icon.is_empty(),
                ICON_NODE_DELTA_E,
            ));
        }
    }
    let assets = root.join("assets");
    for (name, decl) in &manifest.sprites {
        if decl.cvd_icon.is_some() {
            continue;
        }
        let logical = format!("sprites/{name}");
        let Ok((_, m)) = crate::texture::master(&assets, manifest, &logical, crate::Platform::Pc)
        else {
            continue;
        };
        if let Some(rgb) = mean_opaque(&m.rgba) {
            out.extend(icon_hint(
                palettes,
                &logical,
                rgb,
                false,
                ICON_SPRITE_DELTA_E,
            ));
        }
    }
    Ok(out)
}

/// The mean color of the texels at least half opaque (`None`: none are).
pub fn mean_opaque(rgba: &[u8]) -> Option<[u8; 3]> {
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for px in rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128) {
        for c in 0..3 {
            sum[c] += px[c] as u64;
        }
        n += 1;
    }
    (n > 0).then(|| sum.map(|s| (s / n) as u8))
}
