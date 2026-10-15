//! Per-vertex color slots (TMDL `SLOT`) and the per-draw palette
//! (`ModelParams::palette`) through the public T&L: a slotted vertex's
//! color is its baked color times `palette[slot - 1]`, rounded
//! `(a * b + 127) / 255`, before tint and lighting — on the static, flat,
//! rigid and smooth-skinned paths. Without a palette (or without slots)
//! every draw is bit-identical to an unslotted one. The N64 RSP path and
//! the draw memo refuse palette draws.

mod common;

use common::*;
use trino_core::memo::MemoKey;
use trino_core::render3d::{
    self, Camera3, DEFAULT_LIGHT, Fog, Light, Mesh, MeshError, ScreenTri, TnlContext, TnlScratch,
    tessellate_with,
};
use trino_core::{AnimRef, ClipId, Color, Mat34, ModelParams, Palette, Shading, Vec3};

const PALETTE: Palette = [
    Color::rgb(200, 60, 40),
    Color::rgb(30, 160, 220),
    Color::rgba(128, 128, 128, 9), // alpha is ignored
    Color::rgb(255, 255, 0),
];

/// The documented multiply, written independently of the engine.
fn mul(a: u8, b: u8) -> u8 {
    ((a as u32 * b as u32 + 127) / 255) as u8
}

/// Slot of vertex `i`: every slot 0..=4, several per face.
fn slot_of(i: usize) -> u8 {
    (i % 5) as u8
}

fn slots_for(blob: &[u8]) -> Vec<u8> {
    let n = Mesh::from_tmdl(blob).unwrap().vertex_count;
    (0..n).map(slot_of).collect()
}

/// `blob` with a `SLOT` section appended.
fn with_slots(blob: &[u8], slots: &[u8]) -> Vec<u8> {
    let mut out = blob.to_vec();
    section(&mut out, b"SLOT", slots);
    out
}

/// The unit cube's base arrays with each vertex color pre-multiplied by
/// its palette entry: what a palette draw must look like, baked.
fn premultiplied_cube(palette: &Palette) -> Vec<u8> {
    let base = cube_tmdl();
    let m = Mesh::from_tmdl(&base).unwrap();
    let n = m.vertex_count;
    let colors: Vec<[u8; 4]> = (0..n)
        .map(|i| {
            let c = m.color(i);
            match slot_of(i) {
                0 => [c.r, c.g, c.b, c.a],
                s => {
                    let p = palette[s as usize - 1];
                    [mul(c.r, p.r), mul(c.g, p.g), mul(c.b, p.b), c.a]
                }
            }
        })
        .collect();
    let pos: Vec<Vec3> = (0..n).map(|i| m.position(i)).collect();
    let nrm: Vec<Vec3> = (0..n).map(|i| m.normal(i)).collect();
    let idx: Vec<u16> = (0..m.index_count).map(|i| m.index(i)).collect();
    tmdl(&pos, &nrm, &colors, &idx)
}

/// Sections every variant shares after the base arrays: bounds, plus a
/// two-joint rig with the `lift` clip when `rig` (and SKW2 when `smooth`).
fn decorate(base: &[u8], rig: bool, smooth: bool) -> Vec<u8> {
    let mut blob = base.to_vec();
    if rig {
        // rigged_cube's SKEL + ANIM on top of these base arrays.
        let rigged = rigged_cube();
        let plain = cube_tmdl();
        blob.extend_from_slice(&rigged[plain.len()..]);
        if smooth {
            let m = Mesh::from_tmdl(&plain).unwrap();
            let w: Vec<u8> = (0..m.vertex_count)
                .flat_map(|i| [0, if m.position(i).y > 0.0 { 100 } else { 0 }])
                .collect();
            section(&mut blob, b"SKW2", &w);
        }
    }
    section(&mut blob, b"BNDS", &bnds(Vec3::new(0.0, 1.0, 0.0), 3.0));
    blob
}

/// Every screen triangle, bit for bit (Debug prints f32 round-trip exact).
fn draw(blob: &[u8], ctx: &TnlContext, model: &Mat34, params: &ModelParams) -> Vec<String> {
    // Through the layout, like every backend.
    let layout = Mesh::from_tmdl(blob).unwrap().layout(blob);
    let mesh = Mesh::from_layout(blob, &layout);
    let mut out: Vec<ScreenTri> = Vec::new();
    let mut scratch = TnlScratch::new();
    tessellate_with(ctx, &mut scratch, &mesh, model, params, &mut |t| {
        out.push(t)
    });
    out.iter().map(|t| format!("{t:?}")).collect()
}

fn contexts() -> Vec<TnlContext> {
    let cam = Camera3 {
        eye: Vec3::new(2.0, 3.0, -6.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    };
    let warm = Light {
        color: Color::rgb(255, 220, 180),
        ambient_color: Color::rgb(90, 110, 160),
        fog: Fog {
            color: Color::rgb(20, 30, 40),
            near: 4.0,
            far: 12.0,
        },
        ..DEFAULT_LIGHT
    };
    vec![
        TnlContext::new(&cam, &DEFAULT_LIGHT, SCREEN),
        TnlContext::new(&cam, &warm, SCREEN),
    ]
}

/// Surfaces under test: each shading mode, plus tint and emissive.
fn surfaces() -> Vec<ModelParams> {
    let mut out = Vec::new();
    for shading in [Shading::Gouraud, Shading::Unlit, Shading::Flat] {
        out.push(ModelParams {
            shading,
            ..ModelParams::DEFAULT
        });
        out.push(ModelParams {
            shading,
            tint: Color::rgba(250, 200, 120, 255),
            emissive: Color::rgb(10, 0, 30),
            ..ModelParams::DEFAULT
        });
    }
    out
}

fn models() -> [Mat34; 2] {
    [
        // Rotation + uniform scale (the model-space lighting path)...
        Mat34::from_rotation_scale_translation(Vec3::new(0.3, 0.7, 0.0), Vec3::ONE, Vec3::ZERO),
        // ...and a skewed non-uniform scale (the world-space normal path).
        Mat34 {
            m: [
                [1.0, 0.4, 0.0, 0.0],
                [0.0, 1.5, 0.0, 0.0],
                [0.2, 0.0, 0.8, 0.0],
            ],
        },
    ]
}

#[test]
fn a_palette_multiplies_slotted_vertices_before_tint_and_light() {
    let plain = cube_tmdl();
    let slots = slots_for(&plain);
    let oracle_base = premultiplied_cube(&PALETTE);
    let lift = Some(AnimRef::new(ClipId::from_name("lift"), 0.5));
    let mut checked = 0;
    for (rig, smooth) in [(false, false), (true, false), (true, true)] {
        let slotted = with_slots(&decorate(&plain, rig, smooth), &slots);
        let oracle = decorate(&oracle_base, rig, smooth);
        for ctx in contexts() {
            for model in models() {
                for surface in surfaces() {
                    let anim = if rig { lift } else { None };
                    let want = draw(&oracle, &ctx, &model, &ModelParams { anim, ..surface });
                    let got = draw(
                        &slotted,
                        &ctx,
                        &model,
                        &ModelParams {
                            anim,
                            palette: Some(PALETTE),
                            ..surface
                        },
                    );
                    assert!(!want.is_empty());
                    assert_eq!(got, want, "rig {rig} smooth {smooth} {surface:?}");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 3 * 2 * 2 * 6);
}

#[test]
fn without_a_palette_or_slots_draws_are_unchanged() {
    let plain = cube_tmdl();
    let slots = slots_for(&plain);
    for (rig, smooth) in [(false, false), (true, false), (true, true)] {
        let unslotted = decorate(&plain, rig, smooth);
        let slotted = with_slots(&unslotted, &slots);
        for ctx in contexts() {
            for model in models() {
                for surface in surfaces() {
                    let want = draw(&unslotted, &ctx, &model, &surface);
                    // A slotted mesh drawn without a palette.
                    assert_eq!(draw(&slotted, &ctx, &model, &surface), want);
                    // A palette on a mesh without slots.
                    let pal = ModelParams {
                        palette: Some(PALETTE),
                        ..surface
                    };
                    assert_eq!(draw(&unslotted, &ctx, &model, &pal), want);
                }
            }
        }
    }
}

#[test]
fn unlit_vertices_show_their_palette_entry_exactly() {
    // A strip of five vertices, one per slot. Vertex i's baked color is
    // (255, 255 - i, 255): it names the vertex in an unpaletted draw.
    let n = Vec3::new(0.0, 0.0, -1.0);
    let pos: Vec<Vec3> = (0..5)
        .map(|i| {
            Vec3::new(
                -0.8 + 0.4 * i as f32,
                if i % 2 == 0 { -0.5 } else { 0.5 },
                0.0,
            )
        })
        .collect();
    let colors: Vec<[u8; 4]> = (0..5).map(|i| [255, 255 - i as u8, 255, 255]).collect();
    let base = tmdl(&pos, &[n; 5], &colors, &[0, 1, 2, 1, 3, 2, 2, 3, 4]);
    let blob = with_slots(&base, &[0, 1, 2, 3, 4]);
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    assert!(mesh.has_slots());
    assert_eq!(
        (0..5).map(|i| mesh.slot(i)).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    let draw = |palette| {
        let params = ModelParams {
            shading: Shading::Unlit,
            double_sided: true,
            palette,
            ..ModelParams::DEFAULT
        };
        let mut tris: Vec<ScreenTri> = Vec::new();
        render3d::tessellate(
            &mesh,
            &Mat34::IDENTITY,
            &Camera3::default(),
            &DEFAULT_LIGHT,
            &params,
            SCREEN,
            &mut |t| tris.push(t),
        );
        tris
    };
    let (plain, colored) = (draw(None), draw(Some(PALETTE)));
    assert_eq!(plain.len(), 3);
    assert_eq!(colored.len(), 3);
    let mut seen = [false; 5];
    for (a, b) in plain.iter().zip(&colored) {
        assert_eq!(a.pts, b.pts);
        for k in 0..3 {
            let i = (255 - a.colors[k].g) as usize;
            assert_eq!(a.colors[k], Color::rgba(255, 255 - i as u8, 255, 255));
            seen[i] = true;
            let want = match i {
                0 => a.colors[k],
                // White channels show the entry exactly; alpha stays the
                // vertex's (entry 2's alpha is 9).
                _ => {
                    let e = PALETTE[i - 1];
                    Color::rgba(e.r, mul(255 - i as u8, e.g), e.b, 255)
                }
            };
            assert_eq!(b.colors[k], want, "vertex {i}");
        }
    }
    assert_eq!(seen, [true; 5]);
    // The rounding rule itself.
    assert_eq!(
        (mul(128, 128), mul(255, 77), mul(77, 255), mul(0, 255)),
        (64, 77, 77, 0)
    );
}

#[test]
fn slot_sections_parse_validate_and_survive_the_layout() {
    let plain = cube_tmdl();
    let mesh = Mesh::from_tmdl(&plain).unwrap();
    assert!(!mesh.has_slots());
    assert_eq!(mesh.slot(3), 0);
    let mut slots = slots_for(&plain);
    let blob = with_slots(&plain, &slots);
    let m = Mesh::from_tmdl(&blob).unwrap();
    let back = Mesh::from_layout(&blob, &m.layout(&blob));
    assert!(back.has_slots());
    assert!((0..m.vertex_count).all(|i| back.slot(i) == slot_of(i)));
    // Too short, or a slot past 4: rejected.
    assert_eq!(
        Mesh::from_tmdl(&with_slots(&plain, &slots[1..])).unwrap_err(),
        MeshError::BadSection
    );
    slots[7] = 5;
    assert_eq!(
        Mesh::from_tmdl(&with_slots(&plain, &slots)).unwrap_err(),
        MeshError::BadSection
    );
}

#[test]
fn palette_draws_stay_off_the_rsp_and_out_of_the_memo() {
    let ctx = TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN);
    let bounded = bounded_cube();
    let slotted_blob = with_slots(&bounded, &slots_for(&bounded));
    let slotted = Mesh::from_tmdl(&slotted_blob).unwrap();
    let plain = Mesh::from_tmdl(&bounded).unwrap();
    let pal = ModelParams {
        palette: Some(PALETTE),
        ..ModelParams::DEFAULT
    };
    let m = Mat34::IDENTITY;
    // Slots alone don't change what the overlay can hold or draw.
    assert!(render3d::rsp_mesh_supported(&slotted));
    assert!(render3d::rsp_eligible(&ctx, &slotted, &m, &ModelParams::DEFAULT).is_some());
    // A palette that would recolor it: CPU path (the overlay draws baked
    // colors); on a mesh without slots it changes nothing.
    assert!(render3d::rsp_eligible(&ctx, &slotted, &m, &pal).is_none());
    assert!(render3d::rsp_eligible(&ctx, &plain, &m, &pal).is_some());
    let mv = ctx.projection().0.mul(&m);
    assert!(render3d::rsp_eligible_mv(&ctx, &slotted, &m, &mv, &pal).is_none());
    // The memo key has no room for a palette.
    assert!(MemoKey::new(1, 0, &m, &pal).is_none());
    assert!(MemoKey::new(1, 0, &m, &ModelParams::DEFAULT).is_some());
}
