//! UI screens and string tables as generated blobs (issue #58): every
//! `ui/<name>.ui.ron` of the project → a `TUIL` blob at `ui/<name>`, every
//! `assets/i18n/<lang>.ron` → a `TSTR` blob at `i18n/<lang>`, on every
//! target, with no manifest section. A game reads them with
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

/// The project's string tables (`assets/i18n/*.ron`).
pub fn langs(root: &Path) -> Result<Langs, String> {
    Langs::load_dir(&root.join("assets").join(I18N_DIR))
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

/// Every screen and string table (and the project's `access/data`, issue
/// #60), baked for `platform`. A screen with
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
    match langs(root) {
        Ok(langs) => {
            for table in langs.tables.values() {
                match table.to_tstr() {
                    Ok(bytes) => out.push(UiBlob {
                        logical: trino_scene::ui::strings_blob_path(&table.lang),
                        bytes,
                    }),
                    Err(e) => errors.push(e),
                }
            }
        }
        Err(e) => errors.push(e),
    }
    // The accessibility data (issue #60): palettes, captions, icons and the
    // high-contrast theme, resident like the screens that use it.
    match crate::access::bake(root, manifest) {
        Ok(Some(bytes)) => out.push(UiBlob {
            logical: crate::access::LOGICAL.to_string(),
            bytes,
        }),
        Ok(None) => {}
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
