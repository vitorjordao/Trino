//! A UI screen rendered headless the way a target shows it (`cargo xtask
//! ui shot`, the UI goldens): the PC renderer at the target's profile and
//! size, the project's fonts, the sprites and models the screen names.

use std::path::Path;

use trino_asset_pipeline::{Manifest, Platform, resolve_source};
use trino_core::{
    Color, Family, FontId, InputState, ModelId, Renderer, SpriteId, Ui, UiEnv, UiStats,
};
use trino_render_wgpu::{PcRenderer, SimProfile};
use trino_scene::ui::{Flat, Kind, Langs, ScreenDef, apply_preview};

/// What to render.
pub struct ShotOptions<'a> {
    pub family: Family,
    /// The picture's size on the PC and the Web (default 1280 × 720); the
    /// consoles' are fixed.
    pub size: Option<(u32, u32)>,
    pub lang: Option<&'a str>,
    pub interface_scale: f32,
    /// The control to show focused (default: the screen's first).
    pub focus: Option<&'a str>,
}

/// A rendered screen: RGBA8, the 3DS's two screens stacked (top above,
/// bottom centered under it).
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub stats: UiStats,
}

pub const CLEAR: Color = Color::rgb(24, 26, 40);

pub fn profile(family: Family) -> SimProfile {
    match family {
        Family::N64 => SimProfile::N64,
        Family::N3ds => SimProfile::N3ds,
        Family::Pc => SimProfile::Native,
        Family::Web => SimProfile::Web,
    }
}

pub fn pipeline_platform(family: Family) -> Platform {
    match family {
        Family::N64 => Platform::N64,
        Family::N3ds => Platform::N3ds,
        Family::Pc => Platform::Pc,
        Family::Web => Platform::Web,
    }
}

/// Upload the project's fonts and what `def` draws (its sprites and
/// models) into `r`, as `platform` bakes them.
pub fn upload(
    r: &mut PcRenderer,
    root: &Path,
    manifest: &Manifest,
    platform: Platform,
    def: &ScreenDef,
) -> Result<(), String> {
    let assets = root.join("assets");
    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        let source = resolve_source(&assets, platform, &decl.file)?;
        let font = trino_asset_pipeline::bake_font(&source, decl)?;
        let id = trino_core::asset_id(&logical);
        r.upload_sprite(SpriteId(id), font.width, font.height, &font.rgba);
        r.upload_font(FontId(id), font.table);
    }
    let mut sprites = Vec::new();
    let mut models = Vec::new();
    def.root.walk(&mut |n| match &n.kind {
        Kind::Image { sprite, .. } => sprites.push(sprite.clone()),
        Kind::Model3d { model, .. } => models.push(model.clone()),
        _ => {}
    });
    for logical in sprites {
        let Some(decl) = logical
            .strip_prefix("sprites/")
            .and_then(|n| manifest.sprites.get(n))
        else {
            continue;
        };
        let source = resolve_source(&assets, platform, &decl.file)?;
        let (w, h, rgba) = trino_asset_pipeline::decode_png(&source)?;
        r.upload_sprite(SpriteId::from_path(&logical), w, h, &rgba);
    }
    for logical in models {
        let Some(decl) = logical
            .strip_prefix("models/")
            .and_then(|n| manifest.models.get(n))
        else {
            continue;
        };
        let source = resolve_source(&assets, platform, &decl.file)?;
        let tmdl = trino_asset_pipeline::bake_model(&source, &decl.options())?;
        r.upload_mesh(ModelId::from_path(&logical), tmdl);
    }
    Ok(())
}

/// Render `def` for `opts.family` with the screen's preview values.
pub fn render(
    root: &Path,
    manifest: &Manifest,
    langs: &Langs,
    def: &ScreenDef,
    opts: &ShotOptions<'_>,
) -> Result<Shot, String> {
    let platform = pipeline_platform(opts.family);
    let mut r = pollster::block_on(PcRenderer::new_headless(profile(opts.family)))
        .map_err(|e| format!("no GPU renderer: {e}"))?;
    if matches!(opts.family, Family::Pc | Family::Web) {
        let (w, h) = opts.size.unwrap_or((1280, 720));
        r.set_internal_size(w, h);
    }
    upload(&mut r, root, manifest, platform, def)?;
    let sizes = |s: &str| trino_asset_pipeline::ui::sprite_size(root, manifest, platform, s);
    let flat = Flat::new(def, &sizes);
    let screen = flat.screen();
    let strings = opts.lang.map(|l| langs.strings(l));
    let mut ui: Box<Ui<1024>> = Box::new(Ui::new());
    apply_preview(&mut ui, def);
    let idle = InputState::default();
    for frame in 0..3 {
        let env = UiEnv::of(&r).with_interface_scale(opts.interface_scale);
        let env = match &strings {
            Some(s) => env.with_strings(s),
            None => env,
        };
        r.begin_frame(CLEAR);
        ui.draw(&mut r, &screen, &env);
        r.end_frame();
        if frame == 0 {
            ui.update(&screen, &idle, &idle, 0.0);
            if let Some(id) = opts.focus
                && !ui.set_focus(&screen, id)
            {
                return Err(format!("--focus: no control has id \"{id}\""));
            }
        }
    }
    let stats = r.last_ui_stats().unwrap_or_default();
    let (w, h) = r.internal_size();
    let top = r.read_offscreen();
    let Some(bottom) = r.second_screen() else {
        return Ok(Shot {
            width: w,
            height: h,
            rgba: top,
            stats,
        });
    };
    // The 3DS: the touch screen centered under the top one.
    let (bw, bh) = (bottom.x as u32, bottom.y as u32);
    let pixels = r.read_second_screen();
    let width = w.max(bw);
    let height = h + bh;
    // The margin beside the narrower bottom screen: the 3DS's black bezel.
    let mut rgba = [0u8, 0, 0, 255].repeat((width * height) as usize);
    for y in 0..h {
        let from = (y * w * 4) as usize;
        let to = (y * width * 4) as usize;
        rgba[to..to + (w * 4) as usize].copy_from_slice(&top[from..from + (w * 4) as usize]);
    }
    let x0 = (width - bw) / 2;
    for y in 0..bh {
        let from = (y * bw * 4) as usize;
        let to = (((h + y) * width + x0) * 4) as usize;
        rgba[to..to + (bw * 4) as usize].copy_from_slice(&pixels[from..from + (bw * 4) as usize]);
    }
    Ok(Shot {
        width,
        height,
        rgba,
        stats,
    })
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}
