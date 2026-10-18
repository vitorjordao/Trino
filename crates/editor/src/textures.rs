//! Texture formats in the editor (issue #52): what each platform bakes a
//! sprite or font atlas to, measured like the bake (`texture::choose`) on a
//! worker thread, and the project's texture memory per platform for the
//! Platform budget. Both are keyed on the manifest's text and the project
//! generation, so a write from the inspector, an agent's edit or a master
//! change recomputes them; nothing is measured on the UI thread.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;
use trino_asset_pipeline::Platform;
use trino_asset_pipeline::texture::{
    self, BcFormat, N3dsFormat, N64Format, Source, TextureReport, TextureSettings,
};

/// What the content is measured at: the project generation and a hash of
/// the manifest text.
pub type Key = (u64, u64);

pub fn key(manifest: &Path, generation: u64) -> Key {
    let text = std::fs::read_to_string(manifest).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    (generation, h.finish())
}

/// One platform's column of the inspector.
pub struct Column {
    pub platform: Platform,
    /// The manifest's `formats` entry for this platform (`None`: default).
    pub declared: Option<String>,
    /// The bake's decision, or why the bake fails.
    pub report: Result<TextureReport, String>,
    /// The master this platform bakes from (an override may differ).
    pub width: u32,
    pub height: u32,
    pub master: Vec<u8>,
    /// What the platform samples: the resolved (or failing declared)
    /// format decoded, or why it isn't modeled.
    pub decoded: Result<Vec<u8>, String>,
}

/// A texture as every platform bakes it.
pub struct Inspection {
    pub logical: String,
    pub settings: TextureSettings,
    /// The manifest's own `quality` (`None`: the default, normal).
    pub quality: Option<String>,
    pub columns: Vec<Column>,
}

/// The project's texture memory per platform (every sprite and font atlas:
/// the consoles load them all at boot).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub textures: usize,
    pub pc_bytes: u64,
    /// The same textures as RGBA8 (what BCn saves).
    pub pc_rgba8: u64,
    pub n3ds_bytes: u64,
    pub n3ds_budget: u64,
    /// The largest single TMEM load and its texture.
    pub n64_max: Option<(String, u64)>,
    /// The Web: what a browser downloads (KTX2 files, raw RGBA8) and the
    /// VRAM once transcoded, against `Caps::WEB`.
    pub web_download: u64,
    pub web_vram: u64,
    pub web_budget: u64,
    /// Textures whose bake fails, per platform (PC, N64, 3DS, Web).
    pub errors: [usize; 4],
}

/// Previews uploaded to egui (master, decoded per column).
pub struct Shown {
    pub master: Vec<egui::TextureHandle>,
    pub decoded: Vec<Option<egui::TextureHandle>>,
}

type Job<T> = (Key, Receiver<T>);

/// The inspector's state and the budget's totals, with their workers.
#[derive(Default)]
pub struct Textures {
    /// The finished inspection and the key it was measured at.
    pub current: Option<(Key, Inspection, Shown)>,
    pending: Option<(String, Job<Inspection>)>,
    pub totals: Option<(Key, Totals)>,
    totals_pending: Option<Job<Totals>>,
    /// The preview's platform (index into `Platform::ALL`) and zoom.
    pub preview: usize,
    pub zoom: f32,
    /// `min_psnr` being typed, before it is written.
    pub min_psnr_edit: Option<f64>,
    /// Errors already written to the Console.
    pub logged: std::collections::HashSet<String>,
}

impl Textures {
    /// The inspector previewing `Platform::ALL[preview]` at 1×.
    pub fn new(preview: usize) -> Self {
        Textures {
            preview,
            zoom: 1.0,
            ..Default::default()
        }
    }

    /// The inspection of `logical`, (re)measured when the key moved.
    /// `None` while the worker runs (it repaints when done). Returns the
    /// errors the Console hasn't shown yet too.
    pub fn inspect(
        &mut self,
        ctx: &egui::Context,
        assets: &Path,
        logical: &str,
        key: Key,
    ) -> (Option<&Inspection>, Vec<String>) {
        let mut new_errors = Vec::new();
        let fresh = self
            .current
            .as_ref()
            .is_some_and(|(k, i, _)| *k == key && i.logical == logical);
        if !fresh {
            let running = self
                .pending
                .as_ref()
                .is_some_and(|(l, (k, _))| l == logical && *k == key);
            if !running {
                let (tx, rx) = channel();
                let (assets, l, ctx2) = (assets.to_path_buf(), logical.to_string(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(measure(&assets, &l));
                    ctx2.request_repaint();
                });
                self.pending = Some((logical.to_string(), (key, rx)));
            }
            if let Some((_, (k, rx))) = &self.pending {
                match rx.try_recv() {
                    Ok(insp) => {
                        let shown = upload(ctx, &insp);
                        for c in &insp.columns {
                            if let Err(e) = &c.report
                                && self.logged.insert(e.clone())
                            {
                                new_errors.push(e.clone());
                            }
                        }
                        self.current = Some((*k, insp, shown));
                        self.pending = None;
                        self.min_psnr_edit = None;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => self.pending = None,
                }
            }
        }
        let shown = self
            .current
            .as_ref()
            .filter(|(_, i, _)| i.logical == logical)
            .map(|(_, i, _)| i);
        (shown, new_errors)
    }

    /// Is a worker measuring a newer state than the one shown?
    pub fn measuring(&self) -> bool {
        self.pending.is_some()
    }

    /// The project's totals, recomputed on a worker when the key moved
    /// (the last ones stay shown meanwhile).
    pub fn totals(&mut self, ctx: &egui::Context, assets: &Path, key: Key) -> Option<&Totals> {
        let stale = self.totals.as_ref().is_none_or(|(k, _)| *k != key);
        let running = self.totals_pending.as_ref().is_some_and(|(k, _)| *k == key);
        if stale && !running {
            let (tx, rx) = channel();
            let (assets, ctx2) = (assets.to_path_buf(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(totals(&assets));
                ctx2.request_repaint();
            });
            self.totals_pending = Some((key, rx));
        }
        if let Some((k, rx)) = &self.totals_pending {
            match rx.try_recv() {
                Ok(t) => {
                    self.totals = Some((*k, t));
                    self.totals_pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.totals_pending = None,
            }
        }
        self.totals.as_ref().map(|(_, t)| t)
    }
}

/// Measure `logical` on every platform, like the bake.
pub fn measure(assets: &Path, logical: &str) -> Inspection {
    let manifest = trino_asset_pipeline::Manifest::load(&assets.join("manifest.toml"));
    let name = logical.split_once('/').map_or(logical, |(_, n)| n);
    let mut settings = TextureSettings::default();
    let quality = manifest.as_ref().ok().and_then(|m| {
        if logical.starts_with("fonts/") {
            m.fonts.get(name).and_then(|d| d.quality.clone())
        } else {
            m.sprites.get(name).and_then(|d| d.quality.clone())
        }
    });
    let columns = Platform::ALL
        .into_iter()
        .map(|platform| {
            let failed = |e: String| Column {
                platform,
                declared: None,
                report: Err(e.clone()),
                width: 0,
                height: 0,
                master: Vec::new(),
                decoded: Err(e),
            };
            let manifest = match &manifest {
                Ok(m) => m,
                Err(e) => return failed(e.clone()),
            };
            let declared = if logical.starts_with("fonts/") {
                manifest
                    .fonts
                    .get(name)
                    .and_then(|d| d.formats.get(platform.key()))
            } else {
                manifest
                    .sprites
                    .get(name)
                    .and_then(|d| d.formats.get(platform.key()))
            }
            .cloned();
            let (s, m) = match texture::master(assets, manifest, logical, platform) {
                Ok(v) => v,
                Err(e) => return failed(e),
            };
            if platform == Platform::Pc {
                settings = s.clone();
            }
            let report =
                texture::choose(platform, logical, m.source, &s, m.width, m.height, &m.rgba)
                    .map(|c| texture::report(platform, c, m.width, m.height));
            // A failing declared format still shows what it would sample.
            let shown = match &report {
                Ok(r) => Some(r.format.clone()),
                Err(_) => s.format.clone().filter(|f| f != "auto"),
            };
            let decoded = match shown {
                Some(f) => decode(platform, m.source, &f, m.width, m.height, &m.rgba),
                None => Err("no format to preview".into()),
            };
            Column {
                platform,
                declared,
                report,
                width: m.width,
                height: m.height,
                master: m.rgba,
                decoded,
            }
        })
        .collect();
    Inspection {
        logical: logical.to_string(),
        settings,
        quality,
        columns,
    }
}

/// What `platform` samples from `rgba` in `format` (staged like the bake).
pub fn decode(
    platform: Platform,
    source: Source,
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Vec<u8>, String> {
    let unknown = || format!("`{format}` is not a {} format", platform.key());
    match platform {
        Platform::Pc if format == "RGBA8" => Ok(rgba.to_vec()),
        Platform::Pc => {
            let f = BcFormat::parse(format).ok_or_else(unknown)?;
            if !BcFormat::fits(width, height) {
                return Err(format!("{width}x{height} is not whole 4x4 blocks"));
            }
            Ok(f.preview(width, height, rgba))
        }
        Platform::N3ds => Ok(N3dsFormat::parse(format)
            .ok_or_else(unknown)?
            .preview(width, height, rgba)),
        Platform::N64 => {
            let f = N64Format::parse(format).ok_or_else(unknown)?;
            f.preview(width, height, &texture::n64::stage(source, f, rgba))
        }
        Platform::Web if format == "RGBA8" => Ok(rgba.to_vec()),
        Platform::Web => {
            let mode = texture::Ktx2Mode::parse(format).ok_or_else(unknown)?;
            if !BcFormat::fits(width, height) {
                return Err(format!("{width}x{height} is not whole 4x4 blocks"));
            }
            Ok(texture::ktx2::encode_cached(mode, width, height, rgba)?.decoded)
        }
    }
}

fn upload(ctx: &egui::Context, insp: &Inspection) -> Shown {
    let image = |name: String, w: u32, h: u32, rgba: &[u8]| {
        ctx.load_texture(
            name,
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba),
            egui::TextureOptions::NEAREST,
        )
    };
    let tag = |p: Platform| p.key();
    Shown {
        master: insp
            .columns
            .iter()
            .map(|c| {
                image(
                    format!("tex-master-{}", tag(c.platform)),
                    c.width.max(1),
                    c.height.max(1),
                    if c.master.is_empty() {
                        &[0; 4]
                    } else {
                        &c.master
                    },
                )
            })
            .collect(),
        decoded: insp
            .columns
            .iter()
            .map(|c| {
                c.decoded.as_ref().ok().map(|d| {
                    image(
                        format!("tex-decoded-{}", tag(c.platform)),
                        c.width,
                        c.height,
                        d,
                    )
                })
            })
            .collect(),
    }
}

/// Every texture of the project on every platform, summed.
pub fn totals(assets: &Path) -> Totals {
    let mut t = Totals {
        n3ds_budget: trino_core::Caps::N3DS.texture_memory_bytes as u64,
        web_budget: trino_core::Caps::WEB.texture_memory_bytes as u64,
        ..Default::default()
    };
    let Ok(entries) = texture::report_project(assets, &Platform::ALL) else {
        return t;
    };
    t.textures = entries.len();
    for e in entries {
        for (i, (platform, r)) in e.platforms.iter().enumerate() {
            let Ok(r) = r else {
                t.errors[i] += 1;
                continue;
            };
            match platform {
                Platform::Pc => {
                    t.pc_bytes += r.bytes;
                    t.pc_rgba8 += r.width as u64 * r.height as u64 * 4;
                }
                Platform::N3ds => t.n3ds_bytes += r.memory_bytes,
                Platform::N64 => {
                    if t.n64_max.as_ref().is_none_or(|(_, b)| r.memory_bytes > *b) {
                        t.n64_max = Some((e.logical.clone(), r.memory_bytes));
                    }
                }
                Platform::Web => {
                    t.web_download += r.bytes;
                    t.web_vram += r.memory_bytes;
                }
            }
        }
    }
    t
}

/// `1.5 KB`, `340 KB`, `2.4 MB`.
pub fn bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// The manifest table of a texture: (`sprites`|`fonts`, name).
pub fn table(logical: &str) -> Result<(&str, &str), String> {
    match logical.split_once('/') {
        Some((kind @ ("sprites" | "fonts"), name)) => Ok((kind, name)),
        _ => Err(format!("`{logical}` is not a sprite or font")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_read_like_people_write_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(3 * 1_048_576), "3.0 MB");
    }

    #[test]
    fn tables_name_sprites_and_fonts_only() {
        assert_eq!(table("sprites/hero"), Ok(("sprites", "hero")));
        assert_eq!(table("fonts/trino5x7"), Ok(("fonts", "trino5x7")));
        assert!(table("models/crate").is_err());
    }

    /// The engine's own photo card on every platform: the declared 3DS
    /// RGBA8 is lossless, the PC's `auto` picks a BCn, and every column
    /// decodes at the master's size.
    #[test]
    fn the_photo_card_is_measured_like_the_bake() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let insp = measure(&assets, "sprites/selftest_photo");
        let col = |p| insp.columns.iter().find(|c| c.platform == p).unwrap();
        let n3ds = col(Platform::N3ds).report.as_ref().unwrap();
        assert_eq!((n3ds.format.as_str(), n3ds.auto), ("RGBA8", false));
        assert_eq!(n3ds.psnr, Some(f64::INFINITY));
        let pc = col(Platform::Pc).report.as_ref().unwrap();
        assert!(pc.auto, "the PC defaults to auto");
        for c in &insp.columns {
            if let Ok(d) = &c.decoded {
                assert_eq!(d.len(), c.master.len(), "{:?}", c.platform);
            }
        }
        // A font atlas measures too (its own source kind).
        let font = measure(&assets, "fonts/trino5x7");
        assert!(font.columns.iter().all(|c| c.width > 0), "atlas decoded");
    }
}
