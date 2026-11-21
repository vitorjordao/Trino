//! Dynamic meshes (`trino_core::dynamic_mesh`): the TMDL blob a
//! `DynamicMesh` writes reads back as the mesh it describes (every vertex
//! field, the indices, bounds, UVs, the default texture), tessellates to
//! the triangles of the baked blob with the same vertices, and every broken
//! mesh is refused before a byte is written, whatever garbage it holds.
//! `smooth_normals` gives a seamless surface one normal per vertex.

mod common;

use common::*;
use trino_core::dynamic_mesh::{MAX_DYNAMIC_INDICES, MAX_DYNAMIC_VERTICES};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Mesh, ScreenTri, tessellate};
use trino_core::{
    Caps, Color, DynVertex, DynamicMesh, DynamicMeshError, Mat34, ModelParams, SpriteId, Vec2,
    Vec3, smooth_normals,
};

fn up() -> Vec3 {
    Vec3::new(0.0, 1.0, 0.0)
}

fn written(mesh: &DynamicMesh) -> Vec<u8> {
    let mut out = vec![0xAB; mesh.tmdl_len().unwrap() + 7];
    let n = mesh.write_tmdl(&mut out).unwrap();
    assert_eq!(n, mesh.tmdl_len().unwrap(), "tmdl_len is what is written");
    out.truncate(n);
    out
}

/// A `w × h` grid of quads, two triangles each, in the XZ plane.
fn grid(w: usize, h: usize) -> (Vec<DynVertex>, Vec<u16>) {
    let mut v = Vec::new();
    for z in 0..=h {
        for x in 0..=w {
            v.push(DynVertex {
                uv: Vec2::new(x as f32 / w as f32, z as f32 / h as f32),
                ..DynVertex::new(
                    Vec3::new(x as f32, 0.0, z as f32),
                    Vec3::ZERO,
                    Color::rgba(10 * x as u8, 20 * z as u8, 200, 100 + x as u8),
                )
            });
        }
    }
    let row = (w + 1) as u16;
    let mut i = Vec::new();
    for z in 0..h as u16 {
        for x in 0..w as u16 {
            let k = z * row + x;
            i.extend_from_slice(&[k, k + row, k + 1, k + 1, k + row, k + row + 1]);
        }
    }
    (v, i)
}

#[test]
fn the_blob_reads_back_as_the_mesh() {
    let (mut v, i) = grid(4, 3);
    smooth_normals(&mut v, &i);
    let mesh = DynamicMesh::new(&v, &i).textured(SpriteId(77));
    let blob = written(&mesh);
    let m = Mesh::from_tmdl(&blob).expect("a valid TMDL blob");
    assert_eq!((m.vertex_count, m.index_count), (v.len(), i.len()));
    for (k, d) in v.iter().enumerate() {
        assert_eq!(m.position(k), d.position);
        assert_eq!(m.normal(k), d.normal);
        assert_eq!(m.color(k), d.color);
        assert_eq!(m.uv(k), d.uv);
    }
    for (k, &x) in i.iter().enumerate() {
        assert_eq!(m.index(k), x);
    }
    assert!(m.has_uvs());
    assert_eq!(m.default_texture(), Some(SpriteId(77)));
    assert!(!m.opaque_colors(), "vertex alpha below 255 is translucent");
    // The bounds hold every vertex: the box's center, the farthest vertex.
    let (c, r) = m.bounds().expect("BNDS");
    assert_eq!(c, Vec3::new(2.0, 0.0, 1.5));
    for d in &v {
        let o = d.position - c;
        assert!(o.dot(o).sqrt() <= r, "a vertex outside the bounds");
    }
    assert!(r < 2.51);
    // Without UVs: no TXUV, no TXID (the texture needs the UVs).
    let plain = written(&DynamicMesh {
        texture: Some(SpriteId(3)),
        ..DynamicMesh::new(&v, &i)
    });
    let m = Mesh::from_tmdl(&plain).unwrap();
    assert!(!m.has_uvs() && m.default_texture().is_none());
    assert_eq!(plain.len() + v.len() * 8 + 8 + 12, blob.len());
}

/// Drawn by the core's T&L, the written blob gives the triangles of the
/// hand-baked blob with the same vertices: the same mesh, whoever wrote it.
#[test]
fn it_tessellates_as_the_baked_blob_with_the_same_vertices() {
    let mut baked = cube_tmdl();
    section(&mut baked, b"BNDS", &bnds(Vec3::ZERO, 0.8664));
    let b = Mesh::from_tmdl(&baked).unwrap();
    let v: Vec<DynVertex> = (0..b.vertex_count)
        .map(|k| DynVertex::new(b.position(k), b.normal(k), b.color(k)))
        .collect();
    let i: Vec<u16> = (0..b.index_count).map(|k| b.index(k)).collect();
    let blob = written(&DynamicMesh::new(&v, &i));
    let d = Mesh::from_tmdl(&blob).unwrap();
    let camera = Camera3 {
        eye: Vec3::new(1.5, 1.2, -2.5),
        target: Vec3::ZERO,
        fov_y: 1.0,
    };
    let screen = Vec2::new(320.0, 240.0);
    let draw = |mesh: &Mesh, params: &ModelParams| {
        let mut tris: Vec<ScreenTri> = Vec::new();
        let m =
            Mat34::from_rotation_scale_translation(Vec3::new(0.2, 0.7, 0.0), Vec3::ONE, Vec3::ZERO);
        let stats = tessellate(
            mesh,
            &m,
            &camera,
            &DEFAULT_LIGHT,
            params,
            screen,
            &mut |t| tris.push(t),
        );
        (format!("{tris:?}"), stats)
    };
    for params in [
        ModelParams::DEFAULT,
        ModelParams::tinted(Color::rgba(255, 200, 200, 120)),
        ModelParams {
            shading: trino_core::Shading::Flat,
            double_sided: true,
            ..ModelParams::DEFAULT
        },
    ] {
        let (want, ws) = draw(&b, &params);
        let (got, gs) = draw(&d, &params);
        assert!(ws.emitted > 0, "the cube drew nothing");
        assert_eq!(got, want, "{params:?}");
        assert_eq!(gs, ws);
    }
}

#[test]
fn broken_meshes_are_refused_before_writing() {
    let (v, i) = grid(2, 2);
    let ok = DynamicMesh::new(&v, &i);
    assert_eq!(ok.check(MAX_DYNAMIC_VERTICES), Ok(()));
    assert_eq!(
        ok.check(4),
        Err(DynamicMeshError::TooManyVertices { count: 9, max: 4 })
    );
    assert_eq!(
        ok.check(0),
        Err(DynamicMeshError::TooManyVertices { count: 9, max: 0 }),
        "a target without dynamic meshes takes none"
    );
    let cases: Vec<(Vec<DynVertex>, Vec<u16>, bool, DynamicMeshError)> = {
        let mut nan = v.clone();
        nan[4].position.z = f32::NAN;
        let mut inf = v.clone();
        inf[2].normal.y = f32::NEG_INFINITY;
        let mut bad_uv = v.clone();
        bad_uv[6].uv.x = f32::NAN;
        let mut far = i.clone();
        far[5] = 9;
        vec![
            (
                nan,
                i.clone(),
                false,
                DynamicMeshError::NonFinite { vertex: 4 },
            ),
            (
                inf,
                i.clone(),
                false,
                DynamicMeshError::NonFinite { vertex: 2 },
            ),
            (
                bad_uv.clone(),
                i.clone(),
                true,
                DynamicMeshError::NonFinite { vertex: 6 },
            ),
            (
                v.clone(),
                far,
                false,
                DynamicMeshError::IndexOutOfRange { at: 5 },
            ),
            (
                v.clone(),
                i[..4].to_vec(),
                false,
                DynamicMeshError::NotTriangles { count: 4 },
            ),
            (
                vec![DynVertex::new(Vec3::ZERO, up(), Color::WHITE); MAX_DYNAMIC_VERTICES + 1],
                vec![0, 1, 2],
                false,
                DynamicMeshError::TooManyVertices {
                    count: MAX_DYNAMIC_VERTICES + 1,
                    max: MAX_DYNAMIC_VERTICES,
                },
            ),
            (
                v.clone(),
                vec![0; MAX_DYNAMIC_INDICES + 3],
                false,
                DynamicMeshError::TooManyIndices {
                    count: MAX_DYNAMIC_INDICES + 3,
                },
            ),
        ]
    };
    for (v, i, uvs, want) in &cases {
        let m = DynamicMesh {
            uvs: *uvs,
            ..DynamicMesh::new(v, i)
        };
        let mut out = vec![0x5A; 1 << 20];
        assert_eq!(m.write_tmdl(&mut out), Err(*want));
        assert!(
            out.iter().all(|&b| b == 0x5A),
            "{want:?}: wrote before refusing"
        );
    }
    // A NaN UV is fine where the UVs aren't read.
    let mut bad_uv = v.clone();
    bad_uv[6].uv.x = f32::NAN;
    assert_eq!(
        DynamicMesh::new(&bad_uv, &i).check(MAX_DYNAMIC_VERTICES),
        Ok(())
    );
    // A short buffer.
    let need = ok.tmdl_len().unwrap();
    assert_eq!(
        ok.write_tmdl(&mut vec![0; need - 1]),
        Err(DynamicMeshError::BufferTooSmall { need })
    );
    // Nothing to draw is not an error.
    assert!(DynamicMesh::new(&[], &[]).is_empty());
    assert_eq!(DynamicMesh::new(&[], &[]).check(0), Ok(()));
    assert!(DynamicMesh::new(&v, &[]).is_empty());
}

/// Garbage of every size and value (a fixed-seed generator): `check` never
/// panics, and whatever passes it writes a blob `Mesh::from_tmdl` accepts.
#[test]
fn any_garbage_is_refused_or_written_valid() {
    let mut seed = 0x1234_5678u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut accepted = 0;
    for _ in 0..2000 {
        let n = (next() % 40) as usize;
        let odd = |x: u32| match x % 401 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => -1e30,
            _ => (x % 1000) as f32 * 0.01 - 5.0,
        };
        let v: Vec<DynVertex> = (0..n)
            .map(|_| DynVertex {
                position: Vec3::new(odd(next()), odd(next()), odd(next())),
                normal: Vec3::new(odd(next()), odd(next()), odd(next())),
                color: Color::rgba(next() as u8, next() as u8, next() as u8, next() as u8),
                uv: Vec2::new(odd(next()), odd(next())),
            })
            .collect();
        // Mostly whole triangles over the vertices; now and then a broken
        // triangle or an index past the end.
        let k = 3 * (next() % 20) as usize + (next() % 9 == 0) as usize;
        let past = if next() % 6 == 0 { 3 } else { 0 };
        let i: Vec<u16> = (0..k)
            .map(|_| (next() % (n as u32 + past).max(1)) as u16)
            .collect();
        let mesh = DynamicMesh {
            uvs: next() % 2 == 0,
            texture: (next() % 3 == 0).then_some(SpriteId(next())),
            ..DynamicMesh::new(&v, &i)
        };
        let mut out = vec![0; mesh.tmdl_len().unwrap()];
        if let Ok(len) = mesh.write_tmdl(&mut out) {
            accepted += 1;
            Mesh::from_tmdl(&out[..len]).expect("a checked mesh writes a valid blob");
        }
    }
    assert!(
        accepted > 50,
        "the generator never made a valid mesh ({accepted})"
    );
}

/// One normal per vertex from its triangles, area-weighted: a flat grid is
/// +Y everywhere; a ridge's shared vertices lean half and half (no seam in
/// the light); a vertex in no triangle, or only in degenerate ones, is +Y.
#[test]
fn smooth_normals_follow_the_surface() {
    let (mut v, i) = grid(3, 3);
    for d in v.iter_mut() {
        d.normal = Vec3::new(9.0, 9.0, 9.0);
    }
    smooth_normals(&mut v, &i);
    assert!(
        v.iter()
            .all(|d| (d.normal - up()).dot(d.normal - up()) < 1e-10)
    );
    // A roof: two slopes meeting at x = 1.
    let p = |x: f32, y: f32, z: f32| DynVertex::new(Vec3::new(x, y, z), Vec3::ZERO, Color::WHITE);
    let mut roof = vec![
        p(0.0, 0.0, 0.0),
        p(1.0, 1.0, 0.0),
        p(2.0, 0.0, 0.0),
        p(0.0, 0.0, 1.0),
        p(1.0, 1.0, 1.0),
        p(2.0, 0.0, 1.0),
        p(5.0, 5.0, 5.0),
    ];
    // (each ridge vertex in as much of one slope as of the other)
    let tris = [0u16, 3, 1, 1, 3, 4, 1, 4, 5, 1, 5, 2];
    smooth_normals(&mut roof, &tris);
    for ridge in [roof[1].normal, roof[4].normal] {
        assert!(
            ridge.x.abs() < 1e-5 && (ridge.y - 1.0).abs() < 1e-5,
            "ridge {ridge:?}"
        );
    }
    assert!(
        roof[0].normal.x < -0.5 && roof[2].normal.x > 0.5,
        "the slopes lean apart"
    );
    assert_eq!(roof[6].normal, up(), "a lone vertex");
    let mut flat = vec![p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(2.0, 0.0, 0.0)];
    smooth_normals(&mut flat, &[0, 1, 2, 0, 1, 7]);
    assert!(
        flat.iter().all(|d| d.normal == up()),
        "degenerate and out-of-range triangles"
    );
}

/// The targets that draw dynamic meshes: the PC and the Web; the consoles
/// (both tiers) draw none yet.
#[test]
fn the_caps_name_the_targets_that_draw_them() {
    assert_eq!(Caps::PC.max_dynamic_vertices as usize, MAX_DYNAMIC_VERTICES);
    assert_eq!(
        Caps::WEB.max_dynamic_vertices as usize,
        MAX_DYNAMIC_VERTICES
    );
    for c in [Caps::N64, Caps::N64_EXPANDED, Caps::N3DS, Caps::N3DS_NEW] {
        assert_eq!(c.max_dynamic_vertices, 0);
    }
}
