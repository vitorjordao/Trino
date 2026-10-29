//! `pc_renderer` (issue #121): what a damaged or modded asset file can make
//! the PC's renderer do. The input is up to six uploads, each `[op, len lo,
//! len hi, payload..]`: an RGBA8 sprite, a block-compressed sprite, a TMDL
//! mesh, a TFNT font (with its atlas), a KTX2 texture through the native
//! transcoder; then one frame draws everything this input uploaded (the
//! mesh on the GPU T&L path and, translucent, on the CPU fallback; textured
//! with the input's sprite; text in the input's font).
//!
//! One headless renderer per thread, created on the first input
//! (`PcRenderer::new_headless`, no debug names: AGENTS.md pitfall #94).
//! Without an adapter every input returns at once and
//! [`renderer_available`] says so (CI machines without a GPU stay green;
//! the nightly fuzz job installs Mesa's lavapipe). Ids are fixed, so a
//! re-upload replaces instead of piling VRAM up, and a draw only uses what
//! this input uploaded (an input's result doesn't depend on the previous).

use std::cell::RefCell;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Mesh};
use trino_core::{
    Color, FontId, Material, ModelId, ModelParams, Renderer, SpriteId, SpriteParams, TextParams,
    Transform3, Vec2, Vec3,
};
use trino_platform_pc::{CompressedFormat, PcRenderer, SimProfile};

const SPRITE: SpriteId = SpriteId(0x5150_0001);
const BC: SpriteId = SpriteId(0x5150_0002);
const KTX: SpriteId = SpriteId(0x5150_0003);
const MODEL: ModelId = ModelId(0x5150_0004);
const FONT: FontId = FontId(0x5150_0005);
/// The largest KTX2 the target transcodes (texels).
const MAX_KTX2_TEXELS: u64 = 256 * 256;

thread_local! {
    /// `None` inside: tried, no adapter. Leaked, never dropped: a
    /// `PcRenderer` dropped by a thread-local destructor at exit polls its
    /// device after the TLS wgpu reads is gone, and panics.
    static RENDERER: RefCell<Option<Option<&'static mut PcRenderer>>> =
        const { RefCell::new(None) };
}

fn with_renderer<R>(f: impl FnOnce(&mut PcRenderer) -> R) -> Option<R> {
    RENDERER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let r = slot.get_or_insert_with(|| {
            match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
                Ok(mut r) => {
                    r.set_internal_size(160, 120);
                    r.set_hw_tnl(true);
                    Some(Box::leak(Box::new(r)))
                }
                Err(e) => {
                    // CI's desktop job has lavapipe and says so.
                    assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
                    eprintln!("pc_renderer: no GPU adapter, every input skipped ({e})");
                    None
                }
            }
        });
        r.as_deref_mut().map(f)
    })
}

/// Whether this thread has a renderer to fuzz (`false`: no adapter here).
pub fn renderer_available() -> bool {
    with_renderer(|_| ()).is_some()
}

/// The input's uploads: `(op, payload)`, at most six.
fn ops(data: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut rest = data;
    while out.len() < 6 && rest.len() >= 3 {
        let n = u16::from_le_bytes([rest[1], rest[2]]) as usize;
        let take = n.min(rest.len() - 3);
        out.push((rest[0], &rest[3..3 + take]));
        rest = &rest[3 + take..];
    }
    out
}

/// `bytes` cycled (or cut) to `n` bytes: data of exactly the size an
/// upload expects, from whatever the fuzzer gave.
fn fill(bytes: &[u8], n: usize) -> Vec<u8> {
    if bytes.is_empty() {
        return vec![0x80; n];
    }
    bytes.iter().copied().cycle().take(n).collect()
}

fn byte(p: &[u8], i: usize) -> u8 {
    p.get(i).copied().unwrap_or(0)
}

/// What one input uploaded (and so may draw).
#[derive(Default)]
struct Uploaded {
    sprite: Option<(u32, u32)>,
    bc: bool,
    ktx: bool,
    mesh: Option<Vec<u8>>,
    font: bool,
    text: String,
}

/// The target body.
pub fn pc_renderer(data: &[u8]) {
    let _ = with_renderer(|r| run(r, data));
}

fn run(r: &mut PcRenderer, data: &[u8]) {
    // The sprites a refused upload leaves in place: the same before every
    // input, so an input's frame doesn't depend on the one before.
    for id in [SPRITE, KTX] {
        r.upload_sprite(id, 1, 1, &[255, 255, 255, 255]);
    }
    let mut up = Uploaded::default();
    for (op, p) in ops(data) {
        match op % 5 {
            // RGBA8: sizes from 0 (refused) to 32, or past the device's
            // limit, or the payload as is (a length that doesn't match).
            0 => {
                let flags = byte(p, 2);
                let (w, h) = if flags & 2 != 0 {
                    (
                        u32::from(byte(p, 0)) << 8 | 0x2001,
                        1 + u32::from(byte(p, 1)),
                    )
                } else {
                    (u32::from(byte(p, 0) % 33), u32::from(byte(p, 1) % 33))
                };
                let px = p.get(3..).unwrap_or(&[]);
                let rgba = if flags & 1 != 0 {
                    px.to_vec()
                } else {
                    fill(px, (w as usize * h as usize * 4).min(1 << 16))
                };
                r.upload_sprite(SPRITE, w, h, &rgba);
                up.sprite = Some((w, h));
            }
            // BC1/BC3/BC7 blocks (any 16 bytes decode): whole blocks, or an
            // edge that isn't, or the payload's own length.
            1 => {
                let format = [
                    CompressedFormat::Bc1,
                    CompressedFormat::Bc3,
                    CompressedFormat::Bc7,
                ][usize::from(byte(p, 0) % 3)];
                let flags = byte(p, 3);
                let w = u32::from(byte(p, 1) % 9) * 4 + u32::from(flags & 4 != 0);
                let h = u32::from(byte(p, 2) % 9) * 4;
                let px = p.get(4..).unwrap_or(&[]);
                let want = (w / 4 * format.block_bytes() * (h / 4)) as usize;
                let blocks = if flags & 1 != 0 {
                    px.to_vec()
                } else {
                    fill(px, want)
                };
                up.bc |= r.upload_sprite_bc(BC, w, h, format, &blocks);
            }
            // A TMDL the parser accepts (the runner validates before it
            // uploads: an invalid one is the caller's contract).
            2 => {
                if Mesh::from_tmdl(p).is_ok() {
                    r.upload_mesh(MODEL, p.to_vec());
                    up.mesh = Some(p.to_vec());
                }
            }
            // A TFNT table and an atlas sized from its first byte; text
            // from the rest of the payload.
            3 => {
                let Some((&n, rest)) = p.split_first() else {
                    continue;
                };
                let (table, text) = rest.split_at(usize::from(n).min(rest.len()));
                if trino_core::text::BitmapFont::parse(table).is_ok() {
                    r.upload_font(FONT, table.to_vec());
                    let side = 1 + u32::from(byte(p, 1) % 64);
                    r.upload_sprite(
                        trino_core::text::atlas_of(FONT),
                        side,
                        side,
                        &vec![255; (side * side * 4) as usize],
                    );
                    up.font = true;
                    up.text = String::from_utf8_lossy(text).chars().take(64).collect();
                }
            }
            // KTX2 (the Web's texture container) through the native
            // transcoder, small ones only.
            _ => {
                if let Ok(info) = trino_asset_pipeline::texture::ktx2::parse(p)
                    && u64::from(info.width) * u64::from(info.height) <= MAX_KTX2_TEXELS
                    && let Ok(rgba) = trino_asset_pipeline::texture::ktx2::decode(p)
                {
                    r.upload_sprite(KTX, info.width, info.height, &rgba);
                    up.ktx = true;
                }
            }
        }
    }
    frame(r, &up, data.first().copied().unwrap_or(0));
}

/// One frame of everything `up` holds.
fn frame(r: &mut PcRenderer, up: &Uploaded, salt: u8) {
    r.begin_frame(Color::rgb(20, 24, 32));
    if let Some(tmdl) = &up.mesh
        && let Ok(mesh) = Mesh::from_tmdl(tmdl)
    {
        let (center, radius) = mesh.bounds().unwrap_or((Vec3::ZERO, 1.0));
        let radius = if radius.is_finite() && radius > 0.0 {
            radius.min(1.0e6)
        } else {
            1.0
        };
        let finite = center.x.is_finite() && center.y.is_finite() && center.z.is_finite();
        let center = if finite { center } else { Vec3::ZERO };
        r.set_camera(&Camera3 {
            eye: center + Vec3::new(0.0, radius, radius * 3.0),
            target: center,
            ..Camera3::default()
        });
        r.set_light(&DEFAULT_LIGHT);
        let anim = mesh
            .clip(0)
            .map(|c| trino_core::AnimRef::new(c.id, f32::from(salt) / 64.0));
        let texture = up.sprite.map(|_| SPRITE);
        for params in [
            // The GPU T&L path where it takes the draw.
            ModelParams {
                anim,
                texture,
                ..ModelParams::DEFAULT
            },
            // Translucent and flat: the CPU fallback.
            ModelParams {
                anim,
                double_sided: true,
                tint: Color::rgba(255, 255, 255, 128),
                shading: trino_core::Shading::Flat,
                ..ModelParams::DEFAULT
            },
        ] {
            r.draw_model(MODEL, &Transform3::default(), Material::VertexLit, &params);
        }
    }
    let params = SpriteParams {
        scale: Vec2::new(1.0 + f32::from(salt % 4), 1.0),
        rotation: f32::from(salt) * 0.1,
        ..SpriteParams::default()
    };
    let mut x = 0.0;
    for (id, on) in [(SPRITE, up.sprite.is_some()), (BC, up.bc), (KTX, up.ktx)] {
        if on {
            r.draw_sprite(id, Vec2::new(x, 8.0), &params);
            x += 40.0;
        }
    }
    if up.font {
        r.draw_text(FONT, Vec2::new(4.0, 90.0), &up.text, &TextParams::DEFAULT);
    }
    r.end_frame();
}
