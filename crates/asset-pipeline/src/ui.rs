//! UI screens and string tables as generated blobs (issue #58): every
//! `ui/<name>.ui.ron` of the project → a `TUIL` blob at `ui/<name>`, every
//! shipped `assets/i18n/<lang>.ron` → a `TSTR` blob at `i18n/<lang>` (its
//! locale and font remaps, `crate::i18n`) plus `i18n/languages`, on every
//! target, with no manifest entry. A game reads them with
//! `platform.blob(BlobId::from_path("ui/pause"))` and
//! `UiScreen::parse` / `StringTable::parse`.

use std::path::{Path, PathBuf};

use trino_scene::ui::{Flat, Fonts, Langs, Level, ScreenDef, validate};

use crate::{Manifest, Platform, resolve_source};

/// Screens live in `<project>/ui/`.
pub const UI_DIR: &str = "ui";
/// String tables live in `<project>/assets/i18n/`.
pub const I18N_DIR: &str = "i18n";

/// One generated blob.
pub struct UiBlob {
    pub logical: String,
    pub bytes: Vec<u8>,
}

/// The project's `ui/*.ui.ron` files, sorted.
pub fn screen_files(root: &Path) -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(root.join(UI_DIR)) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.ends_with(".ui.ron"))
        })
        .collect();
    files.sort();
    files
}

/// The project's string tables (`assets/i18n/*.ron`), with the manifest's
/// default language when the project has a readable manifest.
pub fn langs(root: &Path) -> Result<Langs, String> {
    let assets = root.join("assets");
    let decl = std::fs::read_to_string(assets.join("manifest.toml"))
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
        .and_then(|v| v.get("i18n")?.get("default")?.as_str().map(str::to_string));
    let mut langs = Langs::load_dir(&assets.join(I18N_DIR))?;
    langs.default = decl;
    Ok(langs)
}

/// A sprite's size on `platform`, from its master (or override) PNG's
/// header, for `Image` nodes that do not give `src`.
pub fn sprite_size(
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
    logical: &str,
) -> Option<(u16, u16)> {
    let name = logical.strip_prefix("sprites/")?;
    let decl = manifest.sprites.get(name)?;
    let path = resolve_source(&root.join("assets"), platform, &decl.file).ok()?;
    png_size(&path)
}

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(path: &Path) -> Option<(u16, u16)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    Some((w.min(u16::MAX as u32) as u16, h.min(u16::MAX as u32) as u16))
}

/// Every screen and string table, baked for `platform`. A screen with
/// structural errors ([`validate`]) fails the bake; layout warnings are
/// `cargo xtask ui check`'s.
pub fn bake(root: &Path, manifest: &Manifest, platform: Platform) -> Result<Vec<UiBlob>, String> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for path in screen_files(root) {
        let result = (|| -> Result<UiBlob, String> {
            let def = ScreenDef::load(&path)?;
            let problems: Vec<String> = validate(&def)
                .into_iter()
                .filter(|i| i.level == Level::Error)
                .map(|i| i.to_string())
                .collect();
            if !problems.is_empty() {
                return Err(format!("{}:\n  {}", path.display(), problems.join("\n  ")));
            }
            let sizes = |sprite: &str| sprite_size(root, manifest, platform, sprite);
            let flat = Flat::new(&def, &sizes);
            let bytes = trino_scene::ui::to_tuil(&flat)?;
            Ok(UiBlob {
                logical: trino_scene::ui::blob_path(&def.name),
                bytes,
            })
        })();
        match result {
            Ok(b) => out.push(b),
            Err(e) => errors.push(e),
        }
    }
    match crate::i18n::langs(&root.join("assets"), manifest.i18n.as_ref()) {
        Ok(langs) => {
            // The languages this platform ships (pseudo-locales only on the
            // PC and the Web), each with the fonts it draws with, and the
            // list the runtime loads them by (`i18n/languages`).
            let blobs = manifest.i18n.as_ref().is_none_or(|d| d.blobs);
            let tables = if blobs {
                crate::i18n::baked_for(&langs, manifest.i18n.as_ref(), platform)
            } else {
                Vec::new()
            };
            for table in &tables {
                let (fonts, group) = crate::i18n::remaps(manifest, &table.lang, platform);
                match table.to_tstr_with(&fonts, group) {
                    Ok(bytes) => out.push(UiBlob {
                        logical: trino_scene::ui::strings_blob_path(&table.lang),
                        bytes,
                    }),
                    Err(e) => errors.push(e),
                }
            }
            if !tables.is_empty() {
                let list: String = tables.iter().map(|t| format!("{}\n", t.lang)).collect();
                out.push(UiBlob {
                    logical: trino_scene::i18n::LANGUAGES_BLOB_PATH.into(),
                    bytes: list.into_bytes(),
                });
            }
        }
        Err(e) => errors.push(e),
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors.join("\n"))
    }
}

/// The project's fonts, baked (for measuring text the way the target
/// will). A font that fails to bake is still known: it measures as the
/// engine font's cell.
pub fn fonts(root: &Path, manifest: &Manifest, platform: Platform) -> Fonts {
    let mut fonts = Fonts::default();
    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        let baked = resolve_source(&root.join("assets"), platform, &decl.file)
            .and_then(|source| crate::bake_font(&source, decl));
        match baked {
            Ok(b) => fonts.insert(&logical, b.table),
            Err(_) => fonts.assume(&logical),
        }
    }
    fonts
}
