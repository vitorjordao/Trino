//! Impostor bake: a model rendered from `views` yaws into one sprite atlas
//! plus its `TIMP` table (`trino_core::impostor`), with the engine's own
//! software T&L and a small CPU rasterizer — deterministic bytes, no GPU, no
//! Blender, the same projection and lighting the game uses.
//!
//! The settings are text, so a person (the editor's model inspector) and an
//! agent (`cargo xtask impostor bake`) drive the same bake:
//!
//! ```text
//! assets/impostors/<name>.impostor.ron        the settings (ImpostorDef)
//! assets/shared/impostors/<name>.png           PC/3DS atlas, RGBA
//! assets/shared/impostors/<name>.timp          its table (blob)
//! assets/n64/impostors/<name>.png              N64 atlas: an indexed CI4 PNG
//!                                              that fits TMEM (index 0 clear)
//! assets/n64/impostors/<name>.timp             its table (smaller tiles)
//! manifest: [sprites.impostor-<name>] (n64 = "CI4") and [blobs.impostor-<name>]
//! ```
//!
//! Each `TIMP` records a hash of the model it was rendered from, so
//! [`check`] spots an impostor that a model edit made stale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use trino_core::render3d::{Camera3, Light, ScreenTri, tessellate};
use trino_core::{
    AnimRef, ClipId, Color, Impostor, Mat34, Mesh, ModelParams, SpriteId, Vec2, Vec3,
};

use crate::Platform;

/// Settings files live in `assets/impostors/` (text, not baked directly).
pub const DEF_DIR: &str = "impostors";
pub const DEF_EXT: &str = ".impostor.ron";
pub const DEF_VERSION: u32 = 1;
/// N64 TMEM holds 4 KB; a CI4 texture with its palette gets 2 KB of
/// texels: 4096 of them.
pub const N64_CI4_TEXELS: u32 = 4096;
/// Samples per texel edge (4x4 supersampling).
const SS: u32 = 4;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ImpostorDef {
    pub version: u32,
    /// The model to render (`models/tree`).
    pub model: String,
    /// Yaws around the model (2..=64). 8 is plenty for most props.
    #[serde(default = "default_views")]
    pub views: u8,
    /// Camera elevation above the horizon, degrees (what the game's
    /// camera mostly looks down at).
    #[serde(default = "default_pitch")]
    pub pitch: f32,
    /// Tile edge in texels on PC and 3DS (power of two).
    #[serde(default = "default_tile")]
    pub tile: u16,
    /// Tile edge on the N64; default: the largest power of two (≤ `tile`)
    /// whose atlas fits TMEM as CI4 ([`n64_tile`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n64_tile: Option<u16>,
    /// The light baked into the pixels (billboards are unlit); default
    /// `trino_core::DEFAULT_LIGHT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<BakeLight>,
    /// Pose an animated model: clip name and time (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub time: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn default_views() -> u8 {
    8
}

fn default_pitch() -> f32 {
    20.0
}

fn default_tile() -> u16 {
    64
}

/// A directional light for the bake (direction it travels, ambient floor).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BakeLight {
    pub dir: [f32; 3],
    pub ambient: f32,
}

impl ImpostorDef {
    pub fn new(model: impl Into<String>) -> Self {
        ImpostorDef {
            version: DEF_VERSION,
            model: model.into(),
            views: default_views(),
            pitch: default_pitch(),
            tile: default_tile(),
            n64_tile: None,
            light: None,
            clip: None,
            time: 0.0,
        }
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let d: ImpostorDef =
            ron::from_str(text).map_err(|e| format!("impostor parse error: {e}"))?;
        if d.version > DEF_VERSION {
            return Err(format!(
                "impostor version {} is newer than this engine understands ({DEF_VERSION})",
                d.version
            ));
        }
        d.validate()?;
        Ok(d)
    }

    pub fn to_ron(&self) -> String {
        let config = ron::ser::PrettyConfig::default().struct_names(false);
        ron::ser::to_string_pretty(self, config).expect("serialization cannot fail")
    }

    /// Ranges the bake needs: 2..=64 views, power-of-two tiles of 8..=256,
    /// a pitch within ±85°.
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=trino_core::impostor::MAX_VIEWS).contains(&self.views) {
            return Err(format!("views {} (2..=64)", self.views));
        }
        for (what, t) in [("tile", Some(self.tile)), ("n64_tile", self.n64_tile)] {
            if let Some(t) = t
                && (!t.is_power_of_two() || !(8..=256).contains(&t))
            {
                return Err(format!("{what} {t}: a power of two in 8..=256"));
            }
        }
        if !(-85.0..=85.0).contains(&self.pitch) {
            return Err(format!("pitch {}° (-85..=85)", self.pitch));
        }
        if !self.model.starts_with("models/") {
            return Err(format!(
                "model `{}`: a logical path like models/tree",
                self.model
            ));
        }
        Ok(())
    }

    fn core_light(&self) -> Light {
        match self.light {
            Some(l) => Light {
                dir: Vec3::new(l.dir[0], l.dir[1], l.dir[2]),
                ambient: l.ambient,
                ..trino_core::DEFAULT_LIGHT
            },
            None => trino_core::DEFAULT_LIGHT,
        }
    }
}

/// `assets/impostors/<name>.impostor.ron`.
pub fn def_file(assets_root: &Path, name: &str) -> PathBuf {
    assets_root.join(DEF_DIR).join(format!("{name}{DEF_EXT}"))
}

/// Every impostor with a settings file, by name.
pub fn names(assets_root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(assets_root.join(DEF_DIR))
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.strip_suffix(DEF_EXT).map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

pub fn load_def(assets_root: &Path, name: &str) -> Result<ImpostorDef, String> {
    let path = def_file(assets_root, name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    ImpostorDef::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save_def(assets_root: &Path, name: &str, def: &ImpostorDef) -> Result<PathBuf, String> {
    def.validate()?;
    let path = def_file(assets_root, name);
    write(&path, def.to_ron().as_bytes())?;
    Ok(path)
}

/// The atlas sprite's logical path (`sprites/impostor-<name>`).
pub fn sprite_logical(name: &str) -> String {
    format!("sprites/impostor-{name}")
}

/// The table blob's logical path (`blobs/impostor-<name>`).
pub fn blob_logical(name: &str) -> String {
    format!("blobs/impostor-{name}")
}

/// Atlas grid for `views`: columns and rows, both powers of two (TMEM and
/// the 3DS GPU want power-of-two textures), as square as possible.
pub fn grid(views: u8) -> (u8, u8) {
    let v = views.max(1) as u32;
    let mut cols = 1u32;
    while cols * cols < v {
        cols *= 2;
    }
    let rows = v.div_ceil(cols).next_power_of_two();
    (cols as u8, rows as u8)
}

/// The N64 tile: the largest power of two ≤ `tile` (and ≥ 8) whose atlas
/// fits TMEM as CI4.
pub fn n64_tile(views: u8, tile: u16) -> u16 {
    let (c, r) = grid(views);
    let mut t = tile.max(8);
    while t > 8 && (c as u32 * t as u32) * (r as u32 * t as u32) > N64_CI4_TEXELS {
        t /= 2;
    }
    t
}

/// FNV-1a over a TMDL: the model identity a `TIMP` records.
pub fn model_hash(tmdl: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in tmdl {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A rendered atlas: RGBA texels (alpha 0 or 255) and its descriptor.
#[derive(Clone, Debug)]
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub impostor: Impostor,
}

impl Atlas {
    /// Opaque texels inside view `i`'s tile.
    pub fn opaque_in(&self, i: u8) -> usize {
        let r = self.impostor.region(i);
        let (x0, y0) = (r.pos.x as u32, r.pos.y as u32);
        let t = self.impostor.tile as u32;
        let mut n = 0;
        for y in y0..y0 + t {
            for x in x0..x0 + t {
                if self.rgba[((y * self.width + x) * 4 + 3) as usize] != 0 {
                    n += 1;
                }
            }
        }
        n
    }

    /// RGBA of texel `(x, y)`.
    pub fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Render `tmdl` per `def` with `tile`-texel tiles into an atlas whose
/// sprite is `sprite`.
pub fn render(
    tmdl: &[u8],
    def: &ImpostorDef,
    tile: u16,
    sprite: SpriteId,
) -> Result<Atlas, String> {
    def.validate()?;
    let mesh = Mesh::from_tmdl(tmdl).map_err(|e| format!("model: {e:?}"))?;
    let (center, radius) = mesh
        .bounds()
        .filter(|(_, r)| *r > 0.0)
        .ok_or("model: empty mesh (no bounds)")?;
    let (cols, rows) = grid(def.views);
    let t = tile as u32;
    let (width, height) = (cols as u32 * t, rows as u32 * t);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    // The model's sphere fills the tile inside a 1-texel clear margin (the
    // billboard samples bilinearly on PC and 3DS).
    let half = radius * 1.02 * t as f32 / (t as f32 - 2.0);
    let fov: f32 = 4.0f32.to_radians();
    let dist = half / (fov * 0.5).tan();
    let pitch = def.pitch.to_radians();
    let light = def.core_light();
    let params = ModelParams {
        anim: def
            .clip
            .as_ref()
            .map(|c| AnimRef::new(ClipId::from_name(c), def.time)),
        ..ModelParams::DEFAULT
    };
    let n = t * SS;
    let screen = Vec2::new(n as f32, n as f32);
    let mut color = vec![[0u8; 4]; (n * n) as usize];
    let mut depth = vec![f32::INFINITY; (n * n) as usize];
    for v in 0..def.views {
        let yaw = v as f32 * core::f32::consts::TAU / def.views as f32;
        let dir = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let camera = Camera3 {
            eye: center + dir * dist,
            target: center,
            fov_y: fov,
        };
        color.fill([0; 4]);
        depth.fill(f32::INFINITY);
        tessellate(
            &mesh,
            &Mat34::IDENTITY,
            &camera,
            &light,
            &params,
            screen,
            &mut |tri| raster(&tri, n, &mut color, &mut depth),
        );
        let r = Impostor {
            sprite,
            views: def.views,
            cols,
            tile,
            size: 0.0,
            center,
        }
        .region(v);
        resolve_tile(
            &color,
            n,
            &mut rgba,
            width,
            r.pos.x as u32,
            r.pos.y as u32,
            t,
        );
    }
    bleed(&mut rgba, width, height);
    Ok(Atlas {
        width,
        height,
        rgba,
        impostor: Impostor {
            sprite,
            views: def.views,
            cols,
            tile,
            size: half * 2.0,
            center,
        },
    })
}

/// Fill one screen triangle into the sample buffers (z-tested, Gouraud).
fn raster(tri: &ScreenTri, n: u32, color: &mut [[u8; 4]], depth: &mut [f32]) {
    let [a, b, c] = tri.pts;
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area.abs() < 1e-12 {
        return;
    }
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as u32;
    let max_x = (a.x.max(b.x).max(c.x).ceil() as i64).clamp(0, n as i64) as u32;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as u32;
    let max_y = (a.y.max(b.y).max(c.y).ceil() as i64).clamp(0, n as i64) as u32;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((b.x - p.x) * (c.y - p.y) - (b.y - p.y) * (c.x - p.x)) / area;
            let w1 = ((c.x - p.x) * (a.y - p.y) - (c.y - p.y) * (a.x - p.x)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * tri.z[0] + w1 * tri.z[1] + w2 * tri.z[2];
            let i = (y * n + x) as usize;
            if z >= depth[i] {
                continue;
            }
            depth[i] = z;
            let ch = |k: fn(Color) -> u8| {
                let [c0, c1, c2] = tri.colors;
                (w0 * k(c0) as f32 + w1 * k(c1) as f32 + w2 * k(c2) as f32)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            color[i] = [ch(|c| c.r), ch(|c| c.g), ch(|c| c.b), 255];
        }
    }
}

/// Box-filter the supersampled view into its tile: a texel is opaque when
/// at least half its samples are covered, colored by their average. The
/// tile's outer ring stays clear.
fn resolve_tile(
    samples: &[[u8; 4]],
    n: u32,
    rgba: &mut [u8],
    width: u32,
    x0: u32,
    y0: u32,
    t: u32,
) {
    for ty in 1..t - 1 {
        for tx in 1..t - 1 {
            let (mut sum, mut hit) = ([0u32; 3], 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let s = samples[((ty * SS + sy) * n + tx * SS + sx) as usize];
                    if s[3] != 0 {
                        hit += 1;
                        for k in 0..3 {
                            sum[k] += s[k] as u32;
                        }
                    }
                }
            }
            if hit * 2 >= SS * SS {
                let at = (((y0 + ty) * width + x0 + tx) * 4) as usize;
                for k in 0..3 {
                    rgba[at + k] = ((sum[k] + hit / 2) / hit) as u8;
                }
                rgba[at + 3] = 255;
            }
        }
    }
}

/// Clear texels take their opaque neighbours' average color (alpha stays
/// 0), so bilinear filtering at a silhouette doesn't pull in black.
fn bleed(rgba: &mut [u8], w: u32, h: u32) {
    for _ in 0..2 {
        let src = rgba.to_vec();
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if src[i + 3] != 0 || (src[i] | src[i + 1] | src[i + 2]) != 0 {
                    continue;
                }
                let (mut sum, mut k) = ([0u32; 3], 0u32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ((ny as u32 * w + nx as u32) * 4) as usize;
                    if src[j + 3] != 0 || (src[j] | src[j + 1] | src[j + 2]) != 0 {
                        for c in 0..3 {
                            sum[c] += src[j + c] as u32;
                        }
                        k += 1;
                    }
                }
                for c in 0..3 {
                    if let Some(v) = sum[c].checked_div(k) {
                        rgba[i + c] = v as u8;
                    }
                }
            }
        }
    }
}

/// The atlas as an RGBA PNG.
pub fn rgba_png(atlas: &Atlas) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, atlas.width, atlas.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&atlas.rgba).expect("png data");
    }
    out
}

/// The atlas quantized to 15 colors plus a clear index 0: an indexed
/// 4-bit PNG the N64 bake turns into a CI4 sprite as is (it would drop
/// alpha quantizing RGBA itself). Returns (png, palette).
pub fn ci4_png(atlas: &Atlas) -> (Vec<u8>, Vec<[u8; 3]>) {
    let opaque: Vec<[u8; 3]> = atlas
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] != 0)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    let palette = median_cut(&opaque, 15);
    let index = |c: [u8; 3]| -> u8 {
        let d = |p: &[u8; 3]| -> i32 { (0..3).map(|k| (p[k] as i32 - c[k] as i32).pow(2)).sum() };
        (palette
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| d(p))
            .map_or(0, |(i, _)| i)
            + 1) as u8
    };
    let (w, h) = (atlas.width, atlas.height);
    let row_bytes = w.div_ceil(2) as usize;
    let mut data = vec![0u8; row_bytes * h as usize];
    for y in 0..h {
        for x in 0..w {
            let p = atlas.texel(x, y);
            let i = if p[3] == 0 {
                0
            } else {
                index([p[0], p[1], p[2]])
            };
            let at = y as usize * row_bytes + (x / 2) as usize;
            data[at] |= if x % 2 == 0 { i << 4 } else { i };
        }
    }
    let mut plte = vec![0u8, 0, 0];
    let mut trns = vec![0u8];
    for c in &palette {
        plte.extend_from_slice(c);
        trns.push(255);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Four);
        enc.set_palette(plte);
        enc.set_trns(trns);
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(&data).expect("png data");
    }
    (out, palette)
}

/// Deterministic median cut: split the box with the widest channel range
/// at its median until there are `n` boxes; each color is its box's mean.
fn median_cut(colors: &[[u8; 3]], n: usize) -> Vec<[u8; 3]> {
    if colors.is_empty() {
        return vec![[0, 0, 0]];
    }
    let mut boxes: Vec<Vec<[u8; 3]>> = vec![colors.to_vec()];
    while boxes.len() < n {
        let range = |b: &Vec<[u8; 3]>, k: usize| {
            let (lo, hi) = b
                .iter()
                .fold((255u8, 0u8), |(lo, hi), c| (lo.min(c[k]), hi.max(c[k])));
            hi as i32 - lo as i32
        };
        let Some((at, k, r)) = boxes
            .iter()
            .enumerate()
            .flat_map(|(i, b)| (0..3).map(move |k| (i, k, range(b, k))))
            .max_by_key(|&(i, k, r)| (r, std::cmp::Reverse(i), std::cmp::Reverse(k)))
        else {
            break;
        };
        if r == 0 {
            break;
        }
        let mut b = boxes.remove(at);
        b.sort_by_key(|c| (c[k], c[(k + 1) % 3], c[(k + 2) % 3]));
        let tail = b.split_off(b.len() / 2);
        boxes.insert(at, tail);
        boxes.insert(at, b);
    }
    boxes
        .iter()
        .map(|b| {
            let mut s = [0u64; 3];
            for c in b {
                for k in 0..3 {
                    s[k] += c[k] as u64;
                }
            }
            let l = b.len().max(1) as u64;
            [(s[0] / l) as u8, (s[1] / l) as u8, (s[2] / l) as u8]
        })
        .collect()
}

/// The `TIMP` blob of an atlas.
pub fn timp(atlas: &Atlas, model_hash: u32, pitch_deg: f32) -> Vec<u8> {
    let mut out = vec![0u8; trino_core::impostor::TIMP_HEADER + atlas.impostor.views as usize * 8];
    let n = atlas
        .impostor
        .write(model_hash, pitch_deg.to_radians(), &mut out);
    out.truncate(n);
    out
}

/// What [`bake`] did.
#[derive(Clone, Debug)]
pub struct BakeReport {
    pub name: String,
    /// The shared (PC/3DS) atlas and the N64 one.
    pub pc: Atlas,
    pub n64: Atlas,
    /// Files written (or rewritten), relative to the assets root.
    pub written: Vec<String>,
    /// Whether manifest entries were added.
    pub manifest_changed: bool,
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The model's TMDL as `platform` bakes it (its override master if any).
fn model_tmdl(assets_root: &Path, model: &str, platform: Platform) -> Result<Vec<u8>, String> {
    let manifest = crate::Manifest::load(&assets_root.join("manifest.toml"))?;
    let name = model.strip_prefix("models/").unwrap_or(model);
    let decl = manifest
        .models
        .get(name)
        .ok_or_else(|| format!("model `{model}` is not in the manifest"))?;
    let source = crate::resolve_source(assets_root, platform, &decl.file)?;
    crate::bake_model(&source, &decl.options())
}

/// Bake impostor `name` from its settings file: render both atlases, write
/// the PNGs and `TIMP` tables, and declare the sprite and the blob in the
/// manifest (once). The editor's "Bake impostor" and `cargo xtask impostor
/// bake` both call this.
pub fn bake(assets_root: &Path, name: &str) -> Result<BakeReport, String> {
    if !crate::manifest::edit::valid_name(name) {
        return Err(format!(
            "impostor name `{name}`: lowercase letters, digits, - and _"
        ));
    }
    let def = load_def(assets_root, name)?;
    let sprite = SpriteId::from_path(&sprite_logical(name));
    let pc_tmdl = model_tmdl(assets_root, &def.model, Platform::Pc)?;
    let n64_tmdl = model_tmdl(assets_root, &def.model, Platform::N64)?;
    let pc = render(&pc_tmdl, &def, def.tile, sprite)?;
    let n64_t = def
        .n64_tile
        .unwrap_or_else(|| n64_tile(def.views, def.tile));
    let n64 = render(&n64_tmdl, &def, n64_t, sprite)?;
    let (cw, ch) = (n64.width, n64.height);
    if cw * ch > N64_CI4_TEXELS {
        return Err(format!(
            "the N64 atlas is {cw}x{ch} texels, past TMEM's {N64_CI4_TEXELS} for CI4: \
             lower n64_tile or views"
        ));
    }
    let rel = |dir: &str, ext: &str| format!("{dir}/impostors/{name}.{ext}");
    let files: [(String, Vec<u8>); 4] = [
        (rel("shared", "png"), rgba_png(&pc)),
        (
            rel("shared", "timp"),
            timp(&pc, model_hash(&pc_tmdl), def.pitch),
        ),
        (rel("n64", "png"), ci4_png(&n64).0),
        (
            rel("n64", "timp"),
            timp(&n64, model_hash(&n64_tmdl), def.pitch),
        ),
    ];
    let mut written = Vec::new();
    for (path, bytes) in &files {
        let full = assets_root.join(path);
        if std::fs::read(&full).ok().as_deref() != Some(bytes.as_slice()) {
            write(&full, bytes)?;
            written.push(path.clone());
        }
    }
    // Declare the sprite and the blob once.
    let manifest_path = assets_root.join("manifest.toml");
    let mut text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let entry = format!("impostor-{name}");
    let mut manifest_changed = false;
    if !crate::manifest::edit::has_table(&text, "sprites", &entry) {
        text = crate::manifest::edit::add_sprite(
            &text,
            &entry,
            &format!("impostors/{name}.png"),
            "CI4",
        )?;
        manifest_changed = true;
    }
    if !crate::manifest::edit::has_table(&text, "blobs", &entry) {
        text = crate::manifest::edit::add_blob(&text, &entry, &format!("impostors/{name}.timp"))?;
        manifest_changed = true;
    }
    if manifest_changed {
        write(&manifest_path, text.as_bytes())?;
    }
    Ok(BakeReport {
        name: name.to_string(),
        pc,
        n64,
        written,
        manifest_changed,
    })
}

/// Whether impostor `name`'s baked tables still match its model on every
/// platform (a model edit makes it stale). `Err` says what to do.
pub fn check(assets_root: &Path, name: &str) -> Result<(), String> {
    let def = load_def(assets_root, name)?;
    for platform in [Platform::Pc, Platform::N64] {
        let tmdl = model_tmdl(assets_root, &def.model, platform)?;
        let file = format!("impostors/{name}.timp");
        let path = crate::resolve_source(assets_root, platform, &file).map_err(|_| {
            format!("impostor `{name}` is not baked: cargo xtask impostor bake {name}")
        })?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Impostor::parse(&bytes).map_err(|e| format!("{}: {e:?}", path.display()))?;
        if Impostor::model_hash(&bytes) != model_hash(&tmdl) {
            return Err(format!(
                "impostor `{name}` is stale for {}: `{}` changed since the bake \
                 (cargo xtask impostor bake {name})",
                platform.key(),
                def.model
            ));
        }
    }
    Ok(())
}

/// Every baked impostor's descriptor per platform (`trino_scene::Target`
/// order), keyed `impostors/<name>` — what `trino_scene::Library`
/// resolves LOD levels against. Unbaked ones are left out.
pub fn baked(assets_root: &Path) -> BTreeMap<String, [Option<Impostor>; 4]> {
    let mut out = BTreeMap::new();
    for name in names(assets_root) {
        let file = format!("impostors/{name}.timp");
        let mut per = [None; 4];
        // The Web's: its own override, else the PC's atlas (`resolve_source`).
        for platform in Platform::ALL {
            per[platform.target().index()] = crate::resolve_source(assets_root, platform, &file)
                .ok()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| Impostor::parse(&b).ok());
        }
        if per.iter().any(Option::is_some) {
            out.insert(format!("impostors/{name}"), per);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_are_powers_of_two_and_squarish() {
        assert_eq!(grid(4), (2, 2));
        assert_eq!(grid(8), (4, 2));
        assert_eq!(grid(6), (4, 2));
        assert_eq!(grid(16), (4, 4));
        assert_eq!(grid(2), (2, 1));
        assert_eq!(grid(12), (4, 4));
        for v in 2..=64u8 {
            let (c, r) = grid(v);
            assert!(c.is_power_of_two() && r.is_power_of_two());
            assert!(c as u32 * r as u32 >= v as u32, "{v}");
        }
    }

    #[test]
    fn n64_tiles_fit_tmem() {
        assert_eq!(n64_tile(8, 64), 16);
        assert_eq!(n64_tile(4, 64), 32);
        assert_eq!(n64_tile(4, 16), 16);
        for v in 2..=64u8 {
            let (c, r) = grid(v);
            let t = n64_tile(v, 128) as u32;
            assert!(
                c as u32 * t * r as u32 * t <= N64_CI4_TEXELS || t == 8,
                "{v}"
            );
        }
    }

    #[test]
    fn median_cut_keeps_distinct_colors_and_is_stable() {
        let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 0, 0]];
        let p = median_cut(&colors, 15);
        assert_eq!(p.len(), 3);
        assert_eq!(p, median_cut(&colors, 15));
        assert_eq!(median_cut(&[], 15), vec![[0, 0, 0]]);
    }

    #[test]
    fn settings_validate_and_round_trip() {
        let d = ImpostorDef::new("models/tree");
        assert_eq!(ImpostorDef::from_ron(&d.to_ron()).unwrap(), d);
        let short = ImpostorDef::from_ron(r#"(version: 1, model: "models/x")"#).unwrap();
        assert_eq!((short.views, short.tile, short.pitch), (8, 64, 20.0));
        for bad in [
            r#"(version: 1, model: "models/x", views: 1)"#,
            r#"(version: 1, model: "models/x", tile: 48)"#,
            r#"(version: 1, model: "models/x", pitch: 90.0)"#,
            r#"(version: 1, model: "x")"#,
            r#"(version: 9, model: "models/x")"#,
        ] {
            assert!(ImpostorDef::from_ron(bad).is_err(), "{bad}");
        }
    }
}
