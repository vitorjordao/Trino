//! Shared by the `ao_shots*` evidence tests (each game crate exports the
//! hot-reload symbols, so the legacy examples link in binaries of their
//! own).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use trino_core::Game;
use trino_platform_pc::{AoView, PcRenderer, SimProfile};

pub struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn renderer(profile: SimProfile, assets: &trino_asset_pipeline::LoadedAssets) -> PcRenderer {
    let mut r = pollster::block_on(PcRenderer::new_headless(profile)).expect("a GPU adapter");
    for s in &assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in &assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl.clone());
    }
    for f in &assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table.clone());
    }
    r
}

/// Panels side by side, with a 4 px dark gutter.
pub fn row(path: &Path, size: (u32, u32), panels: &[Vec<u8>]) {
    let (w, h) = size;
    let gap = 4;
    let total = w * panels.len() as u32 + gap * (panels.len() as u32 - 1);
    let mut out = vec![0u8; (total * h * 4) as usize];
    for px in out.chunks_mut(4) {
        px.copy_from_slice(&[20, 20, 24, 255]);
    }
    for (i, p) in panels.iter().enumerate() {
        let x0 = i as u32 * (w + gap);
        for y in 0..h {
            let src = &p[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
            let at = ((y * total + x0) * 4) as usize;
            out[at..at + src.len()].copy_from_slice(src);
        }
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), total, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&out).unwrap();
    eprintln!("evidence: {}", path.display());
}

/// Off / on / mask frames of whatever `draw` renders.
pub fn three(
    r: &mut PcRenderer,
    on: AoView,
    mut draw: impl FnMut(&mut PcRenderer),
) -> Vec<Vec<u8>> {
    [AoView::Off, on, AoView::Mask]
        .into_iter()
        .map(|view| {
            r.set_ao_view(view);
            draw(r);
            r.read_offscreen()
        })
        .collect()
}

/// The PC assets, baked like the app does.
pub fn assets() -> trino_asset_pipeline::LoadedAssets {
    let root = root();
    let baked = root.join("target/assets/pc");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    trino_asset_pipeline::load_dir(&baked, None).unwrap()
}

pub fn out_dir() -> PathBuf {
    root().join("target/evidence/ao")
}

/// A game after half a second idle, off / forced on / mask, as one row.
pub fn game_row(name: &str, game: &mut dyn Game) {
    let assets = assets();
    let mut r = renderer(SimProfile::Native, &assets);
    let idle = trino_core::InputState::default();
    let mut audio = NullAudio;
    for _ in 0..30 {
        game.update(&idle, &mut audio, 1.0 / 60.0);
    }
    let size = r.internal_size();
    let panels = three(&mut r, AoView::On, |r| game.render(r));
    row(&out_dir().join(format!("{name}.png")), size, &panels);
}

pub fn native_screen() -> trino_core::Vec2 {
    let (w, h) = SimProfile::Native.internal_resolution();
    trino_core::Vec2::new(w as f32, h as f32)
}
