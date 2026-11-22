//! Dynamic meshes ([`Renderer::draw_mesh`]) on the PC renderer: the same
//! pixels as a baked model with the same vertices, on the GPU path and on
//! the CPU path (`TRINO_PC_TNL=cpu`), opaque, lit, translucent, textured
//! and water; the GPU path within the oracle's tolerance of the CPU path;
//! a mesh rewritten every frame shows only this frame's vertices and
//! creates no GPU buffer in steady state; broken meshes and the console
//! profiles draw nothing, counted. GPU-gated like the goldens.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::primitive::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Fog, Mesh};
use trino_core::water::Water;
use trino_core::{
    Color, DynVertex, DynamicMesh, LocalLight, Mat34, Material, ModelId, ModelParams, Renderer,
    Shading, SpriteId, Transform3, Vec2, Vec3, smooth_normals,
};
use trino_platform_pc::{PcRenderer, SimProfile, tnl_oracle};

const BOX: ModelId = ModelId(11);
const BALL: ModelId = ModelId(12);
const FLOOR: ModelId = ModelId(13);
/// Baked from the very vertices a test also draws as a dynamic mesh.
const SAME: ModelId = ModelId(14);
const SAME2: ModelId = ModelId(15);
const CHECKER: SpriteId = SpriteId(21);

/// One GPU test at a time (parallel headless devices crash some drivers).
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(mut r) => {
            if profile == SimProfile::Native {
                r.set_internal_size(480, 270);
            }
            r.set_hw_tnl(true);
            Some(r)
        }
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn prim(shape: Shape, color: [u8; 4], segments: u32) -> Vec<u8> {
    Primitive {
        shape,
        color,
        pivot: Pivot::Bottom,
        segments,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap()
}

/// A baked blob's vertices and indices, as a game would write them.
fn vertices_of(tmdl: &[u8]) -> (Vec<DynVertex>, Vec<u16>) {
    let m = Mesh::from_tmdl(tmdl).unwrap();
    let uv = m.has_uvs();
    let v = (0..m.vertex_count)
        .map(|i| DynVertex {
            uv: if uv { m.uv(i) } else { Vec2::ZERO },
            ..DynVertex::new(m.position(i), m.normal(i), m.color(i))
        })
        .collect();
    let i = (0..m.index_count).map(|i| m.index(i)).collect();
    (v, i)
}

/// The blob a dynamic mesh is drawn from (what `upload_mesh` takes).
fn blob(mesh: &DynamicMesh) -> Vec<u8> {
    let mut b = vec![0; mesh.tmdl_len().unwrap()];
    assert_eq!(mesh.write_tmdl(&mut b), Ok(b.len()));
    b
}

/// A sheet of `n × n` quads over `size` m, rippled by `phase`, its color
/// fading from `a` (one side) to `b` (the other), with UVs: the kind of
/// surface a game rewrites every frame. Smooth normals.
fn sheet(n: usize, size: f32, phase: f32, a: Color, b: Color) -> (Vec<DynVertex>, Vec<u16>) {
    let mut v = Vec::new();
    for z in 0..=n {
        for x in 0..=n {
            let (u, w) = (x as f32 / n as f32, z as f32 / n as f32);
            let y = 0.25 * ((u * 6.0 + phase).sin() * (w * 4.0 - phase).cos());
            let mix = |p: u8, q: u8| (p as f32 + (q as f32 - p as f32) * u).round() as u8;
            let c = Color::rgba(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a));
            v.push(DynVertex {
                uv: Vec2::new(u * 2.0, w * 2.0),
                ..DynVertex::new(
                    Vec3::new((u - 0.5) * size, y, (w - 0.5) * size),
                    Vec3::ZERO,
                    c,
                )
            });
        }
    }
    let mut i = Vec::new();
    let row = (n + 1) as u16;
    for z in 0..n as u16 {
        for x in 0..n as u16 {
            let k = z * row + x;
            i.extend_from_slice(&[k, k + row, k + 1, k + 1, k + row, k + row + 1]);
        }
    }
    smooth_normals(&mut v, &i);
    (v, i)
}

fn camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 3.0, -7.0),
        target: Vec3::new(0.0, 0.4, 0.0),
        fov_y: 60f32.to_radians(),
    }
}

fn begin(r: &mut PcRenderer) {
    r.begin_frame(Color::rgb(40, 60, 90));
    r.set_camera(&camera());
    let mut light = DEFAULT_LIGHT;
    light.fog = Fog {
        color: Color::rgb(140, 160, 190),
        near: 5.0,
        far: 16.0,
    };
    r.set_light(&light);
    r.set_water(1.5, &[]);
}

fn at(x: f32, y: f32, z: f32, yaw: f32) -> Mat34 {
    Transform3 {
        position: Vec3::new(x, y, z),
        rotation: Vec3::new(0.0, yaw, 0.0),
        ..Transform3::default()
    }
    .matrix()
}

fn checker() -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..16 {
        for x in 0..16 {
            let on = (x / 4 + y / 4) % 2 == 0;
            px.extend_from_slice(&if on {
                [250, 250, 250, 255]
            } else {
                [60, 60, 70, 255]
            });
        }
    }
    px
}

/// The surfaces the "same pixels" frame draws: (where, how).
fn surfaces() -> Vec<(Mat34, ModelParams)> {
    let water = Water {
        wave_amplitude: 0.06,
        ..Water::DEFAULT
    };
    vec![
        // Opaque, lit (gloss: the per-pixel lit pipelines), in fog.
        (
            at(-2.2, 0.0, 1.0, 0.4),
            ModelParams {
                gloss: 160,
                ..ModelParams::DEFAULT
            },
        ),
        // Flat shading, a tint and an emissive.
        (
            at(0.0, 0.0, 3.0, -0.3),
            ModelParams {
                shading: Shading::Flat,
                tint: Color::rgb(255, 220, 200),
                emissive: Color::rgb(20, 0, 30),
                ..ModelParams::DEFAULT
            },
        ),
        // Translucent through the tint, both faces.
        (
            at(2.2, 0.3, 0.5, 0.9),
            ModelParams {
                double_sided: true,
                ..ModelParams::tinted(Color::rgba(255, 255, 255, 140))
            },
        ),
        // Textured (the call's texture over the mesh's UVs).
        (
            at(-1.0, 0.6, -1.5, 0.2),
            ModelParams {
                texture: Some(CHECKER),
                ..ModelParams::DEFAULT
            },
        ),
        // Water.
        (
            at(1.2, -0.1, -1.2, 0.0),
            ModelParams {
                water: Some(water),
                gloss: 200,
                ..ModelParams::tinted(Color::rgba(90, 170, 220, 200))
            },
        ),
    ]
}

/// A dynamic mesh draws what a baked model with its vertices draws: the
/// primitives' own bakes and a rippled, fading, partly translucent sheet,
/// under every surface (lit, flat, tinted, translucent, textured, water),
/// on the GPU path and on the CPU path, byte for byte.
#[test]
fn a_dynamic_mesh_draws_the_pixels_of_the_baked_model() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let ball = prim(Shape::Sphere { radius: 0.6 }, [90, 170, 230, 255], 12);
    let boxy = prim(
        Shape::Box {
            size: [1.0, 0.8, 1.2],
        },
        [200, 140, 80, 255],
        2,
    );
    let floor = prim(
        Shape::Box {
            size: [14.0, 0.2, 14.0],
        },
        [120, 150, 90, 255],
        1,
    );
    let (sv, si) = sheet(
        12,
        1.6,
        0.7,
        Color::rgba(250, 250, 250, 255),
        Color::rgba(30, 90, 200, 120),
    );
    let sheet_mesh = DynamicMesh::new(&sv, &si).textured(CHECKER);
    r.upload_mesh(BALL, ball.clone());
    r.upload_mesh(BOX, boxy.clone());
    r.upload_mesh(FLOOR, floor);
    r.upload_mesh(SAME, blob(&sheet_mesh));
    r.upload_sprite(CHECKER, 16, 16, &checker());
    let (bv, bi) = vertices_of(&ball);
    let (xv, xi) = vertices_of(&boxy);
    let dynamic = [
        DynamicMesh::new(&bv, &bi),
        DynamicMesh::new(&xv, &xi),
        sheet_mesh,
    ];
    let baked = [BALL, BOX, SAME];
    let surfaces = surfaces();
    let frame = |r: &mut PcRenderer, dyn_draws: bool| {
        begin(r);
        r.draw_model_matrix(
            FLOOR,
            &at(0.0, -0.3, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        for (m, p) in &surfaces {
            // Each surface over each mesh, side by side.
            for (j, (d, b)) in dynamic.iter().zip(baked).enumerate() {
                let m = m.mul(&at(0.0, 0.0, j as f32 * 1.6 - 1.6, 0.0));
                if dyn_draws {
                    r.draw_mesh(d, &m, p);
                } else {
                    r.draw_model_matrix(b, &m, Material::VertexLit, p);
                }
            }
        }
        r.end_frame();
        r.read_offscreen()
    };
    for gpu_path in [true, false] {
        r.set_hw_tnl(gpu_path);
        let want = frame(&mut r, false);
        let models = r.stats().models;
        let got = frame(&mut r, true);
        let s = r.stats();
        let (w, h) = r.internal_size();
        let name = if gpu_path {
            "same_pixels_gpu"
        } else {
            "same_pixels_cpu"
        };
        let mut side = want.clone();
        side.extend_from_slice(&got);
        write_png(
            &root().join(format!("target/evidence/dynamic-mesh/{name}.png")),
            w,
            h * 2,
            &side,
        );
        assert_eq!(s.models, models, "{name}: one model draw per dynamic mesh");
        assert_eq!(
            s.dynamic_meshes as usize,
            dynamic.len() * surfaces.len(),
            "{name}"
        );
        assert_eq!(s.dynamic_rejected, 0, "{name}");
        if gpu_path {
            assert!(
                s.tnl.pc_gpu_draws as usize > dynamic.len() * surfaces.len(),
                "{name}: on the GPU path"
            );
        }
        let differ = want.iter().zip(&got).filter(|(a, b)| a != b).count();
        assert_eq!(
            differ, 0,
            "{name}: {differ} bytes differ from the baked models' frame"
        );
        assert!(
            want.chunks(4).any(|p| p[2] > 180 && p[0] < 120),
            "{name}: nothing blue drawn"
        );
    }
}

/// Dynamic meshes on the GPU path, against the CPU path (the oracle of
/// #153/#154): lit under a lamp, translucent sorted among the rest, water,
/// textured. Nothing is rejected from the GPU path.
#[test]
fn dynamic_meshes_on_the_gpu_match_the_cpu_path() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Box {
                size: [14.0, 0.2, 14.0],
            },
            [120, 150, 90, 255],
            1,
        ),
    );
    r.upload_sprite(CHECKER, 16, 16, &checker());
    let lamp = [LocalLight::point(
        Vec3::new(-1.0, 1.4, 0.0),
        Color::rgb(255, 190, 120),
        3.5,
    )];
    let surfaces = surfaces();
    let frame = |r: &mut PcRenderer| {
        begin(r);
        r.set_local_lights(&lamp);
        r.draw_model_matrix(
            FLOOR,
            &at(0.0, -0.3, 2.0, 0.0),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        for (k, (m, p)) in surfaces.iter().enumerate() {
            let (v, i) = sheet(
                10,
                1.8,
                k as f32,
                Color::rgba(255, 240, 220, 255),
                Color::rgba(40, 120, 220, 150),
            );
            let mesh = DynamicMesh::new(&v, &i);
            let mesh = if p.texture.is_some() {
                mesh.textured(CHECKER)
            } else {
                mesh
            };
            r.draw_mesh(&mesh, m, p);
        }
        r.end_frame();
        r.read_offscreen()
    };
    let d = tnl_oracle::compare(&mut r, frame).expect("the native profile has the GPU path");
    eprintln!("dynamic meshes: {d}");
    let (w, h, img) = d.side_by_side();
    write_png(
        &root().join("target/evidence/dynamic-mesh/oracle.png"),
        w,
        h,
        &img,
    );
    d.assert_within("dynamic meshes");
    assert_eq!(
        d.gpu_draws as usize,
        surfaces.len() + 1,
        "every draw on the GPU path"
    );
}

/// A mesh rewritten every frame shows this frame's vertices only: frames
/// alternating two shapes (in the same slot) are each the baked model's
/// frame and repeat exactly, a frame without it shows none of it, and two
/// dynamic meshes in one frame each keep their own vertices (the same
/// pixels as two baked models). On both paths.
#[test]
fn a_rewritten_mesh_shows_only_this_frames_vertices() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let blue = Color::rgba(40, 110, 230, 255);
    let (av, ai) = sheet(16, 3.0, 0.0, Color::WHITE, blue);
    let (bv, bi) = sheet(
        6,
        1.5,
        2.0,
        Color::rgb(230, 60, 40),
        Color::rgb(250, 220, 60),
    );
    let (a, b) = (DynamicMesh::new(&av, &ai), DynamicMesh::new(&bv, &bi));
    r.upload_mesh(SAME, blob(&a));
    r.upload_mesh(SAME2, blob(&b));
    let p = ModelParams::DEFAULT;
    let (ma, mb) = (at(-1.0, 0.0, 0.5, 0.3), at(1.6, 0.6, -0.5, -0.4));
    for gpu_path in [true, false] {
        r.set_hw_tnl(gpu_path);
        let shot = |r: &mut PcRenderer, f: &dyn Fn(&mut PcRenderer)| {
            begin(r);
            f(r);
            r.end_frame();
            r.read_offscreen()
        };
        let only_a = shot(&mut r, &|r| r.draw_mesh(&a, &ma, &p));
        let only_b = shot(&mut r, &|r| r.draw_mesh(&b, &mb, &p));
        let again_a = shot(&mut r, &|r| r.draw_mesh(&a, &ma, &p));
        let again_b = shot(&mut r, &|r| r.draw_mesh(&b, &mb, &p));
        let empty = shot(&mut r, &|_| {});
        let both = shot(&mut r, &|r| {
            r.draw_mesh(&a, &ma, &p);
            r.draw_mesh(&b, &mb, &p);
        });
        let baked = shot(&mut r, &|r| {
            r.draw_model_matrix(SAME, &ma, Material::VertexLit, &p);
            r.draw_model_matrix(SAME2, &mb, Material::VertexLit, &p);
        });
        let baked_a = shot(&mut r, &|r| {
            r.draw_model_matrix(SAME, &ma, Material::VertexLit, &p)
        });
        let baked_b = shot(&mut r, &|r| {
            r.draw_model_matrix(SAME2, &mb, Material::VertexLit, &p)
        });
        let blank = {
            begin(&mut r);
            r.end_frame();
            r.read_offscreen()
        };
        let path = if gpu_path { "GPU" } else { "CPU" };
        assert!(
            only_a != only_b,
            "{path}: the two shapes draw the same frame"
        );
        assert!(
            only_a == baked_a && only_b == baked_b,
            "{path}: a rewritten slot drew another frame's vertices"
        );
        assert!(
            only_a == again_a && only_b == again_b,
            "{path}: a shape drew differently after the other one"
        );
        assert!(empty == blank, "{path}: last frame's mesh still shows");
        assert!(
            both == baked,
            "{path}: two dynamic meshes in one frame are not two models"
        );
        assert!(
            both != only_a && both != only_b,
            "{path}: one of the two meshes is missing"
        );
    }
}

/// The GPU path rewrites a dynamic mesh's stream in its buffer: a sheet
/// rippling for 120 frames (new positions, colors and normals every frame)
/// creates no buffer after the first frame, a bigger one grows its slot
/// once (to the next power of two) and a smaller one reuses it.
#[test]
fn the_gpu_stream_is_rewritten_in_place() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let draw = |r: &mut PcRenderer, n: usize, phase: f32| {
        let (v, i) = sheet(n, 3.0, phase, Color::WHITE, Color::rgba(30, 100, 220, 160));
        begin(r);
        r.draw_mesh(
            &DynamicMesh::new(&v, &i),
            &at(0.0, 0.2, 0.0, 0.0),
            &ModelParams::DEFAULT,
        );
        r.end_frame();
        let s = r.stats();
        assert_eq!(
            (s.dynamic_meshes, s.tnl.pc_gpu_draws),
            (1, 1),
            "on the GPU path"
        );
    };
    draw(&mut r, 20, 0.0);
    let first = r.mesh_buffers_created();
    let resident = r.resident_mesh_bytes();
    for f in 1..120 {
        draw(&mut r, 20, f as f32 * 0.05);
    }
    assert_eq!(r.mesh_buffers_created(), first, "a buffer per frame");
    assert_eq!(r.resident_mesh_bytes(), resident);
    draw(&mut r, 32, 0.0);
    assert_eq!(
        r.mesh_buffers_created(),
        first + 1,
        "growing takes one new buffer"
    );
    for f in 0..30 {
        draw(&mut r, 32 - f % 8, f as f32 * 0.1);
    }
    draw(&mut r, 8, 0.0);
    assert_eq!(
        r.mesh_buffers_created(),
        first + 1,
        "a smaller mesh fits the buffer"
    );
}

/// Meshes that can't be drawn draw nothing and are counted: a NaN
/// position, a NaN normal, an index past the vertices, a broken triangle,
/// more vertices than the target takes, a NaN matrix. The frame is the
/// frame without them, and the next good mesh draws.
#[test]
fn broken_meshes_draw_nothing_and_are_counted() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let (v, i) = sheet(4, 2.0, 0.0, Color::WHITE, Color::rgb(200, 30, 30));
    let good = DynamicMesh::new(&v, &i);
    let mut nan_pos = v.clone();
    nan_pos[3].position.y = f32::NAN;
    let mut nan_normal = v.clone();
    nan_normal[5].normal.x = f32::INFINITY;
    let mut far_index = i.clone();
    far_index[7] = v.len() as u16;
    let broken_tri = &i[..i.len() - 1];
    let huge_v = vec![DynVertex::new(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), Color::WHITE); 65_537];
    let huge_i = [0u16, 1, 2];
    let bad = [
        DynamicMesh::new(&nan_pos, &i),
        DynamicMesh::new(&nan_normal, &i),
        DynamicMesh::new(&v, &far_index),
        DynamicMesh::new(&v, broken_tri),
        DynamicMesh::new(&huge_v, &huge_i),
    ];
    let m = at(0.0, 0.3, 0.0, 0.0);
    let mut nan_m = m;
    nan_m.m[1][3] = f32::NAN;
    let blank = {
        begin(&mut r);
        r.end_frame();
        r.read_offscreen()
    };
    begin(&mut r);
    for b in &bad {
        r.draw_mesh(b, &m, &ModelParams::DEFAULT);
    }
    r.draw_mesh(&good, &nan_m, &ModelParams::DEFAULT);
    // Nothing to draw is not an error.
    r.draw_mesh(&DynamicMesh::new(&[], &[]), &m, &ModelParams::DEFAULT);
    r.end_frame();
    let s = r.stats();
    assert_eq!((s.dynamic_rejected, s.dynamic_meshes, s.models), (6, 0, 0));
    assert!(r.read_offscreen() == blank, "a broken mesh drew something");
    begin(&mut r);
    r.draw_mesh(&good, &m, &ModelParams::DEFAULT);
    r.end_frame();
    assert_eq!(
        (r.stats().dynamic_meshes, r.stats().dynamic_rejected),
        (1, 0)
    );
    assert!(
        r.read_offscreen() != blank,
        "the good mesh after them draws"
    );
}

/// The console profiles draw what the consoles draw: no dynamic meshes
/// (`Caps::max_dynamic_vertices` 0), counted, the frame untouched.
#[test]
fn console_profiles_draw_no_dynamic_meshes() {
    let _gpu = gpu();
    for profile in [SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        assert_eq!(r.caps().max_dynamic_vertices, 0);
        let (v, i) = sheet(4, 2.0, 0.0, Color::WHITE, Color::rgb(200, 30, 30));
        let mut shot = |draw: bool| {
            begin(&mut r);
            if draw {
                r.draw_mesh(
                    &DynamicMesh::new(&v, &i),
                    &at(0.0, 0.5, 0.0, 0.0),
                    &ModelParams::DEFAULT,
                );
            }
            r.end_frame();
            (r.read_offscreen(), r.stats())
        };
        let (blank, _) = shot(false);
        let (got, s) = shot(true);
        assert!(
            got == blank,
            "{profile:?}: a dynamic mesh drew on a console profile"
        );
        assert_eq!(
            (s.dynamic_rejected, s.dynamic_meshes, s.models),
            (1, 0, 0),
            "{profile:?}"
        );
    }
}

/// Strict mode says why a console profile drew nothing.
#[test]
#[should_panic(expected = "Caps::max_dynamic_vertices")]
fn strict_mode_refuses_dynamic_meshes_on_a_console_profile() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::N64) else {
        panic!("skipped: no adapter (Caps::max_dynamic_vertices)");
    };
    r.set_strict(true);
    let (v, i) = sheet(2, 1.0, 0.0, Color::WHITE, Color::WHITE);
    begin(&mut r);
    r.draw_mesh(
        &DynamicMesh::new(&v, &i),
        &Mat34::IDENTITY,
        &ModelParams::DEFAULT,
    );
}

/// What one dynamic mesh costs (`--ignored --nocapture`, release): a 64 ×
/// 64 sheet (4 225 vertices, 8 192 triangles) rewritten and drawn every
/// frame, against the same sheet as a baked model, and against the
/// surface drawn as one instance of a triangle per triangle (how a game
/// without dynamic meshes built a surface that changes), on both paths.
#[test]
#[ignore]
fn dynamic_mesh_cost() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let (v, i) = sheet(64, 6.0, 0.0, Color::WHITE, Color::rgba(30, 100, 220, 255));
    r.upload_mesh(SAME, blob(&DynamicMesh::new(&v, &i)));
    // One unit triangle, placed per copy.
    let tri = [
        DynVertex::new(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), Color::WHITE),
        DynVertex::new(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 0.0),
            Color::WHITE,
        ),
        DynVertex::new(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Color::WHITE,
        ),
    ];
    r.upload_mesh(SAME2, blob(&DynamicMesh::new(&tri, &[0, 1, 2])));
    let copies: Vec<trino_core::Instance> = i
        .chunks(3)
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|k| v[k as usize].position);
            let (x, z) = (c - a, b - a);
            let y = x.cross(z).normalized();
            trino_core::Instance::new(Mat34 {
                m: [
                    [x.x, y.x, z.x, a.x],
                    [x.y, y.y, z.y, a.y],
                    [x.z, y.z, z.z, a.z],
                ],
            })
        })
        .collect();
    let frames = 120;
    {
        // The core's share: writing the blob and reading it back.
        let mesh = DynamicMesh::new(&v, &i);
        let mut out = vec![0; mesh.tmdl_len().unwrap()];
        let t = std::time::Instant::now();
        for _ in 0..frames {
            mesh.write_tmdl(&mut out).unwrap();
        }
        let write = t.elapsed().as_secs_f64() * 1e6 / frames as f64;
        let t = std::time::Instant::now();
        for _ in 0..frames {
            std::hint::black_box(Mesh::from_tmdl(&out).unwrap());
        }
        let read = t.elapsed().as_secs_f64() * 1e6 / frames as f64;
        eprintln!(
            "core write_tmdl {write:.1} µs, Mesh::from_tmdl {read:.1} µs ({} vertices)",
            v.len()
        );
    }
    for gpu_path in [true, false] {
        r.set_hw_tnl(gpu_path);
        let mut run = |name: &str, f: &mut dyn FnMut(&mut PcRenderer, usize)| {
            let mut call = std::time::Duration::ZERO;
            let t0 = std::time::Instant::now();
            for k in 0..frames {
                begin(&mut r);
                let t = std::time::Instant::now();
                f(&mut r, k);
                call += t.elapsed();
                r.end_frame();
            }
            let all = t0.elapsed();
            eprintln!(
                "{} {name:<28} call {:>8.1} µs  frame {:>8.1} µs",
                if gpu_path { "GPU" } else { "CPU" },
                call.as_secs_f64() * 1e6 / frames as f64,
                all.as_secs_f64() * 1e6 / frames as f64,
            );
        };
        let m = at(0.0, 0.0, 1.0, 0.0);
        let p = ModelParams::DEFAULT;
        run("baked model", &mut |r, _| {
            r.draw_model_matrix(SAME, &m, Material::VertexLit, &p)
        });
        run("dynamic mesh (same)", &mut |r, _| {
            r.draw_mesh(&DynamicMesh::new(&v, &i), &m, &p)
        });
        let mut moving = v.clone();
        run("dynamic mesh (rewritten)", &mut |r, k| {
            for (d, s) in moving.iter_mut().zip(&v) {
                d.position.y = s.position.y + 0.1 * ((s.position.x + k as f32 * 0.1).sin());
            }
            smooth_normals(&mut moving, &i);
            r.draw_mesh(&DynamicMesh::new(&moving, &i), &m, &p);
        });
        run("one instance per triangle", &mut |r, _| {
            r.draw_model_instances(
                SAME2,
                &copies,
                &ModelParams {
                    double_sided: true,
                    ..p
                },
            )
        });
    }
}
