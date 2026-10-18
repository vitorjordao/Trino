//! Loading a game's asset pack ([`crate::pack`]) into the renderer, the
//! audio and the platform — and the Web's KTX2 textures (ADR 0017 §6):
//! each is transcoded, once at load, to the best format the browser's GPU
//! samples, in this order:
//!
//! | GPU has | ETC1S opaque | ETC1S alpha | UASTC |
//! |---|---|---|---|
//! | BC (desktops) | BC1 | BC3 | BC7 |
//! | ETC2 (phones) | ETC2 RGB | ETC2 RGBA | ETC2 RGB / RGBA |
//! | ASTC (phones) | ASTC 4×4 | ASTC 4×4 | ASTC 4×4 |
//! | none | RGBA8 | RGBA8 | RGBA8 |
//!
//! The RGBA8 fallback is the transcoder's own decode, the same pixels the
//! bake measured its PSNR on (as the PC decodes BC blocks on a GPU without
//! BC).

use trino_core::bundle::MemoryCategory;
use trino_core::{BlobId, FontId, MixerConfig, ModelId, MusicId, SoundId, SpriteId};
use trino_render_wgpu::{CompressedFormat, PcRenderer};

use crate::pack::{self, Kind};
use crate::{WebAudio, WebPlatform};

/// The Basis transcoder's target formats (`basisu_transcoder.h`).
const TF_ETC1_RGB: u32 = 0;
const TF_ETC2_RGBA: u32 = 1;
const TF_BC1_RGB: u32 = 2;
const TF_BC3_RGBA: u32 = 3;
const TF_BC7_RGBA: u32 = 6;
const TF_ASTC_4X4: u32 = 10;
const TF_RGBA32: u32 = 13;

/// Where a KTX2 texture goes on this GPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureTarget {
    Bc,
    Etc2,
    Astc,
    Rgba8,
}

impl TextureTarget {
    /// The best target `renderer` samples.
    pub fn of(renderer: &PcRenderer) -> TextureTarget {
        if renderer.supports_compressed(CompressedFormat::Bc7) {
            TextureTarget::Bc
        } else if renderer.supports_compressed(CompressedFormat::Etc2Rgba8) {
            TextureTarget::Etc2
        } else if renderer.supports_compressed(CompressedFormat::Astc4x4) {
            TextureTarget::Astc
        } else {
            TextureTarget::Rgba8
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TextureTarget::Bc => "BC",
            TextureTarget::Etc2 => "ETC2",
            TextureTarget::Astc => "ASTC",
            TextureTarget::Rgba8 => "RGBA8",
        }
    }
}

/// What a pack load did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub sprites: u32,
    /// KTX2 textures uploaded block-compressed, and decoded to RGBA8.
    pub ktx2_compressed: u32,
    pub ktx2_decoded: u32,
    pub sounds: u32,
    pub models: u32,
    pub fonts: u32,
    pub blobs: u32,
    /// The pack carried the project's mix (`assets/audio.toml`).
    pub mixer: bool,
    /// Asset groups the pack names (their bundles beside it).
    pub groups: u32,
    /// The pack's bytes (the download).
    pub bytes: u64,
}

impl LoadReport {
    /// The `TRINO_ASSETS:` line.
    pub fn line(&self, target: TextureTarget) -> String {
        format!(
            "TRINO_ASSETS: bytes={} sprites={} ktx2={} ktx2_gpu={} ktx2_rgba8={} target={} \
             sounds={} models={} fonts={} blobs={} mixer={} groups={}",
            self.bytes,
            self.sprites,
            self.ktx2_compressed + self.ktx2_decoded,
            self.ktx2_compressed,
            self.ktx2_decoded,
            target.name(),
            self.sounds,
            self.models,
            self.fonts,
            self.blobs,
            self.mixer,
            self.groups
        )
    }
}

/// Upload every asset of `pack_bytes`.
pub(crate) fn load(
    pack_bytes: &[u8],
    renderer: &mut PcRenderer,
    audio: &mut WebAudio,
    platform: &mut WebPlatform,
) -> Result<LoadReport, String> {
    let entries = pack::parse(pack_bytes)?;
    let target = TextureTarget::of(renderer);
    let mut r = LoadReport {
        bytes: pack_bytes.len() as u64,
        ..LoadReport::default()
    };
    for e in entries {
        match e.kind {
            Kind::Sprite => {
                let (w, h, px) = pack::read_sprite(e.bytes)?;
                renderer.upload_sprite(SpriteId(e.id), w, h, px);
                let n = renderer.sprite_bytes(SpriteId(e.id)) as u32;
                platform.groups.memory.add(MemoryCategory::Textures, n);
                r.sprites += 1;
            }
            Kind::SpriteKtx2 => {
                if upload_ktx2(renderer, SpriteId(e.id), e.bytes, target)? {
                    r.ktx2_compressed += 1;
                } else {
                    r.ktx2_decoded += 1;
                }
                let n = renderer.sprite_bytes(SpriteId(e.id)) as u32;
                platform.groups.memory.add(MemoryCategory::Textures, n);
                r.sprites += 1;
            }
            Kind::Sound | Kind::Music => {
                let blob = pack::sound_blob(e.kind, e.bytes)?;
                let loaded = if e.kind == Kind::Sound {
                    audio.upload_sound_blob(SoundId(e.id), &blob)
                } else {
                    audio.upload_music_blob(MusicId(e.id), &blob)
                };
                loaded.map_err(|err| format!("sound {:08x}: {err}", e.id))?;
                let n = audio.clip_bytes(e.id, e.kind == Kind::Music) as u32;
                platform.groups.memory.add(MemoryCategory::Audio, n);
                r.sounds += 1;
            }
            Kind::Mixer => {
                let mix =
                    MixerConfig::parse(e.bytes).map_err(|err| format!("audio/mixer: {err}"))?;
                mix.apply(audio);
                r.mixer = true;
            }
            Kind::Model => {
                renderer.upload_mesh(ModelId(e.id), e.bytes.to_vec());
                let n = renderer.mesh_bytes(ModelId(e.id)) as u32;
                platform.groups.memory.add(MemoryCategory::Meshes, n);
                r.models += 1;
            }
            Kind::Font => {
                renderer.upload_font(FontId(e.id), e.bytes.to_vec());
                let n = renderer.font_bytes(FontId(e.id)) as u32;
                platform.groups.memory.add(MemoryCategory::Data, n);
                r.fonts += 1;
            }
            Kind::Blob => {
                platform.insert_blob(BlobId(e.id), e.bytes.to_vec());
                platform
                    .groups
                    .memory
                    .add(MemoryCategory::Data, e.bytes.len() as u32);
                r.blobs += 1;
            }
            // A group the pack leaves out: its bundle is fetched beside it
            // when the game loads it (`crate::groups`).
            Kind::Group => {
                let name = std::str::from_utf8(e.bytes)
                    .map_err(|_| format!("group {:08x}: its name is not UTF-8", e.id))?;
                platform.groups.add(e.id, name, None);
                r.groups += 1;
            }
        }
    }
    Ok(r)
}

/// Transcode one KTX2 texture for `target` and upload it: `true` when it
/// went up block-compressed, `false` when decoded to RGBA8.
pub(crate) fn upload_ktx2(
    renderer: &mut PcRenderer,
    id: SpriteId,
    ktx2: &[u8],
    target: TextureTarget,
) -> Result<bool, String> {
    if !crate::js::trino_basis_ready() {
        return Err("a KTX2 texture, but the Basis transcoder isn't loaded".into());
    }
    let uastc = ktx2.get(44) == Some(&0); // supercompression none = UASTC
    let alpha_hint = ktx2_has_alpha(ktx2);
    let (tf, format) = match (target, uastc, alpha_hint) {
        (TextureTarget::Bc, true, _) => (TF_BC7_RGBA, Some(CompressedFormat::Bc7)),
        (TextureTarget::Bc, false, false) => (TF_BC1_RGB, Some(CompressedFormat::Bc1)),
        (TextureTarget::Bc, false, true) => (TF_BC3_RGBA, Some(CompressedFormat::Bc3)),
        (TextureTarget::Etc2, _, false) => (TF_ETC1_RGB, Some(CompressedFormat::Etc2Rgb8)),
        (TextureTarget::Etc2, _, true) => (TF_ETC2_RGBA, Some(CompressedFormat::Etc2Rgba8)),
        (TextureTarget::Astc, _, _) => (TF_ASTC_4X4, Some(CompressedFormat::Astc4x4)),
        (TextureTarget::Rgba8, _, _) => (TF_RGBA32, None),
    };
    let out = crate::js::trino_transcode_ktx2(ktx2, tf)
        .map_err(|e| format!("sprite {:08x}: KTX2: {e:?}", id.0))?;
    let w = out.get(0).as_f64().unwrap_or(0.0) as u32;
    let h = out.get(1).as_f64().unwrap_or(0.0) as u32;
    let alpha = out.get(2).as_bool().unwrap_or(true);
    let data = js_sys::Uint8Array::new(&out.get(3)).to_vec();
    if let Some(format) = format
        && renderer.upload_sprite_compressed(id, w, h, format, &data, Some(alpha))
    {
        return Ok(true);
    }
    let rgba = if format.is_some() {
        // The device refused the format after all: decode instead.
        let out = crate::js::trino_transcode_ktx2(ktx2, TF_RGBA32)
            .map_err(|e| format!("sprite {:08x}: KTX2: {e:?}", id.0))?;
        js_sys::Uint8Array::new(&out.get(3)).to_vec()
    } else {
        data
    };
    renderer.upload_sprite(id, w, h, &rgba);
    Ok(false)
}

/// The KTX2 DFD says the texture has alpha: ETC1S with a second (alpha)
/// sample, or UASTC's RGBA channel id (`texture::ktx2::parse` on the host).
fn ktx2_has_alpha(ktx2: &[u8]) -> bool {
    let u32_at = |at: usize| {
        ktx2.get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap())) as usize
    };
    let (dfd_at, dfd_len) = (u32_at(48), u32_at(52));
    match ktx2.get(44) {
        Some(1) => dfd_len >= 60,
        _ => ktx2.get(dfd_at + 31).is_some_and(|c| c & 0xF == 3),
    }
}
