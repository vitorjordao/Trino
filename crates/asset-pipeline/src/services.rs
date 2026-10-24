//! The project's achievements, stats and presence keys as one generated
//! blob (issue #61): `services/achievements` (`TACH`,
//! `trino_core::services::table`) from `assets/achievements.ron`. Baked
//! beside the UI blobs on every target (`crate::ui::bake`), with no
//! manifest entry; a project without the file bakes no blob.

use std::path::Path;

/// The blob's logical path.
pub const LOGICAL: &str = trino_core::services::table::LOGICAL;

/// The `services/achievements` blob, or `None` without
/// `assets/achievements.ron`. A file that doesn't parse, or names a stat
/// it doesn't declare, fails the bake (`cargo xtask services check` says
/// more).
pub fn bake(root: &Path) -> Result<Option<Vec<u8>>, String> {
    match trino_scene::services::load(root)? {
        Some(def) => def
            .to_tach()
            .map(Some)
            .map_err(|e| format!("{}: {e}", trino_scene::services::path(root).display())),
        None => Ok(None),
    }
}

/// Where `services export steam` (and the editor) write by default.
pub const STEAM_CONFIG_DIR: &str = "dist/steam-config";

/// The text files of a Steam export (relative path, contents): the
/// achievements CSV and localization VDF, the Steam Input action manifest
/// (`game_actions_<app>.vdf`, 0 without an app id) and a rich-presence file
/// per language.
pub fn steam_text_files(
    def: &trino_scene::services::AchievementsDef,
    langs: &trino_scene::i18n::Langs,
    app_id: Option<u32>,
    title: &str,
) -> Vec<(String, String)> {
    use trino_scene::steam;
    let mut files = vec![
        (
            "achievements.csv".to_string(),
            steam::achievements_csv(def, langs),
        ),
        (
            "achievements_loc.vdf".to_string(),
            steam::achievements_loc_vdf(def, langs),
        ),
        (
            format!("game_actions_{}.vdf", app_id.unwrap_or(0)),
            steam::input_manifest_vdf(title),
        ),
    ];
    if !def.presence.is_empty() {
        for (lang, text) in steam::presence_vdf(def, langs) {
            files.push((format!("rich_presence/{lang}.vdf"), text));
        }
    }
    files
}

/// Write a project's Steam export into `out`: [`steam_text_files`] and the
/// icons ([`steam_icons`]). Returns the files written.
pub fn export_steam(
    root: &Path,
    app_id: Option<u32>,
    title: &str,
    out: &Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let def = trino_scene::services::load(root)?.ok_or_else(|| {
        format!(
            "{}: no achievements to export",
            trino_scene::services::path(root).display()
        )
    })?;
    let manifest = crate::Manifest::load(&root.join("assets/manifest.toml"))?;
    let langs = crate::ui::langs(root)?;
    let mut written = Vec::new();
    for (rel, text) in steam_text_files(&def, &langs, app_id, title) {
        let p = out.join(&rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        written.push(p);
    }
    written.extend(steam_icons(root, &manifest, &def, out)?);
    Ok(written)
}

/// Steam's achievement icon size.
pub const STEAM_ICON: u32 = 64;

/// A medal (the overlay's, `trino_core::services` without an icon) as
/// 16 × 16 RGBA.
fn medal16() -> (u32, u32, Vec<u8>) {
    let mut px = vec![0u8; 16 * 16 * 4];
    let mut fill = |x0: u32, y0: u32, w: u32, h: u32, c: [u8; 4]| {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let i = ((y * 16 + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&c);
            }
        }
    };
    let (ribbon, gold) = ([200, 60, 60, 255], [255, 196, 40, 255]);
    fill(4, 0, 2, 6, ribbon);
    fill(10, 0, 2, 6, ribbon);
    fill(4, 6, 8, 10, gold);
    fill(2, 8, 12, 6, gold);
    (16, 16, px)
}

/// Scale `(w, h, rgba)` by whole pixels into a `size` square over `bg`,
/// centered (nearest neighbor: pixel art stays crisp).
fn fit(src: &(u32, u32, Vec<u8>), size: u32, bg: [u8; 3], gray: bool) -> Vec<u8> {
    let (w, h, px) = src;
    let k = (size / (*w).max(*h)).max(1);
    let (ox, oy) = (
        (size - (w * k).min(size)) / 2,
        (size - (h * k).min(size)) / 2,
    );
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (sx, sy) = (x.wrapping_sub(ox) / k, y.wrapping_sub(oy) / k);
            let mut c = [bg[0] as u32, bg[1] as u32, bg[2] as u32];
            if x >= ox && y >= oy && sx < *w && sy < *h {
                let i = ((sy * w + sx) * 4) as usize;
                let a = px[i + 3] as u32;
                for (k, ch) in c.iter_mut().enumerate() {
                    *ch = (px[i + k] as u32 * a + *ch * (255 - a)) / 255;
                }
            }
            if gray {
                let l = (c[0] * 3 + c[1] * 6 + c[2]) / 10 / 2;
                c = [l, l, l];
            }
            out.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
    }
    out
}

/// Write `icons/<API>.png` and `icons/<API>_locked.png` (64 × 64, opaque,
/// the locked one gray and dimmed) for every achievement into `out`: its
/// sprite's master, else the overlay's medal. Returns the files written.
pub fn steam_icons(
    root: &Path,
    manifest: &crate::Manifest,
    def: &trino_scene::services::AchievementsDef,
    out: &Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let dir = out.join("icons");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut written = Vec::new();
    for a in &def.achievements {
        let src = match &a.icon {
            Some(logical) => {
                let name = logical.strip_prefix("sprites/").unwrap_or(logical);
                let decl = manifest
                    .sprites
                    .get(name)
                    .ok_or_else(|| format!("achievement {}: no sprite `{logical}`", a.id))?;
                let file =
                    crate::resolve_source(&root.join("assets"), crate::Platform::Pc, &decl.file)?;
                crate::bake::decode_png(&file)?
            }
            None => medal16(),
        };
        let (icon, locked) = trino_scene::steam::icon_files(&a.id);
        for (rel, gray) in [(icon, false), (locked, true)] {
            let pixels = fit(&src, STEAM_ICON, [27, 40, 56], gray);
            let path = out.join(rel);
            write_png(&path, STEAM_ICON, STEAM_ICON, &pixels)?;
            written.push(path);
        }
    }
    Ok(written)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_scale_by_whole_pixels_and_gray_when_locked() {
        let m = medal16();
        let on = fit(&m, STEAM_ICON, [0, 0, 0], false);
        assert_eq!(on.len(), 64 * 64 * 4);
        // The medal's gold at (8, 10) lands at (32..36, 40..44), ×4.
        let at =
            |px: &[u8], x: usize, y: usize| px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3].to_vec();
        assert_eq!(at(&on, 33, 41), [255, 196, 40]);
        assert_eq!(at(&on, 0, 0), [0, 0, 0], "transparent: the background");
        let off = fit(&m, STEAM_ICON, [0, 0, 0], true);
        let g = at(&off, 33, 41);
        assert!(g[0] == g[1] && g[1] == g[2] && g[0] < 128, "{g:?}");
    }
}
