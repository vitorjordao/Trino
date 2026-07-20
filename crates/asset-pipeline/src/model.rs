//! glTF 2.0 -> TMDL: the 3D model bake, shared by every platform (TMDL is
//! the engine's portable mesh format, parsed no_std by
//! `trino_core::render3d::Mesh`).
//!
//! Real-world low-poly assets (Kenney, Quaternius, ...) are multi-node,
//! multi-primitive and textured, while the engine rasterizes vertex colors
//! only (N64 ceiling). The bake therefore:
//!
//! - walks the default scene and merges **every primitive of every node**,
//!   applying node transforms to positions and normals;
//! - resolves per-vertex colors in priority order: `COLOR_0` when present,
//!   else the material's `baseColorTexture` **sampled at each vertex UV**
//!   (low-poly palette atlases collapse perfectly to flat face colors),
//!   else the material's `baseColorFactor` — always multiplied by the
//!   factor, like glTF specifies;
//! - accepts non-indexed primitives (indices synthesized).

use std::path::Path;

use trino_core::render3d::TMDL_MAGIC;

/// Column-major glTF node matrix helpers.
fn mat_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, cell) in col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

fn mat_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    out
}

fn mat_dir(m: &[[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (r, o) in out.iter_mut().enumerate() {
        *o = m[0][r] * d[0] + m[1][r] * d[1] + m[2][r] * d[2];
    }
    let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
    if len > 1e-8 {
        for v in &mut out {
            *v /= len;
        }
    }
    out
}

/// Nearest-neighbor sample of a glTF image at (u, v), repeat wrapping.
fn sample_image(img: &gltf::image::Data, u: f32, v: f32) -> Option<[u8; 4]> {
    let wrap = |t: f32| {
        let f = t - t.floor();
        if f < 0.0 { f + 1.0 } else { f }
    };
    let x = ((wrap(u) * img.width as f32) as u32).min(img.width - 1) as usize;
    let y = ((wrap(v) * img.height as f32) as u32).min(img.height - 1) as usize;
    let at = y * img.width as usize + x;
    use gltf::image::Format;
    Some(match img.format {
        Format::R8 => {
            let g = img.pixels[at];
            [g, g, g, 255]
        }
        Format::R8G8 => {
            let g = img.pixels[at * 2];
            [g, g, g, img.pixels[at * 2 + 1]]
        }
        Format::R8G8B8 => {
            let p = &img.pixels[at * 3..at * 3 + 3];
            [p[0], p[1], p[2], 255]
        }
        Format::R8G8B8A8 => {
            let p = &img.pixels[at * 4..at * 4 + 4];
            [p[0], p[1], p[2], p[3]]
        }
        _ => return None, // 16-bit formats: fall back to the color factor
    })
}

/// Convert a `.gltf`/`.glb` master into a TMDL blob.
pub fn bake_model_tmdl(path: &Path) -> Result<Vec<u8>, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let (doc, buffers, images) = gltf::import(path).map_err(|e| err(&e))?;

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[u8; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // (node, world transform) worklist from the default scene; models
    // without scenes fall back to their meshes at identity.
    const IDENTITY: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut work: Vec<(gltf::Node, [[f32; 4]; 4])> = Vec::new();
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    if let Some(scene) = scene {
        for node in scene.nodes() {
            work.push((node, IDENTITY));
        }
    }
    let mut instanced: Vec<(gltf::Mesh, [[f32; 4]; 4])> = Vec::new();
    while let Some((node, parent)) = work.pop() {
        let world = mat_mul(parent, node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            instanced.push((mesh, world));
        }
        for child in node.children() {
            work.push((child, world));
        }
    }
    if instanced.is_empty() {
        instanced = doc.meshes().map(|m| (m, IDENTITY)).collect();
    }
    if instanced.is_empty() {
        return Err(err(&"glTF contains no mesh"));
    }

    for (mesh, world) in &instanced {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue; // engine rasterizes triangles only
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let prim_pos: Vec<[f32; 3]> = match reader.read_positions() {
                Some(p) => p.collect(),
                None => continue,
            };
            let prim_nrm: Vec<[f32; 3]> = reader
                .read_normals()
                .ok_or_else(|| err(&"primitive has no NORMAL (export with normals)"))?
                .collect();
            if prim_nrm.len() != prim_pos.len() {
                return Err(err(&"attribute counts do not match POSITION count"));
            }

            // Vertex colors: COLOR_0 > baseColorTexture@UV > baseColorFactor.
            let pbr = primitive.material().pbr_metallic_roughness();
            let factor = pbr.base_color_factor();
            let fmul = |c: [u8; 4]| {
                [
                    (c[0] as f32 * factor[0]) as u8,
                    (c[1] as f32 * factor[1]) as u8,
                    (c[2] as f32 * factor[2]) as u8,
                    (c[3] as f32 * factor[3]) as u8,
                ]
            };
            let prim_col: Vec<[u8; 4]> = if let Some(c) = reader.read_colors(0) {
                c.into_rgba_u8().collect()
            } else {
                let tex = pbr.base_color_texture().and_then(|info| {
                    let image = images.get(info.texture().source().index())?;
                    let uvs: Vec<[f32; 2]> = reader
                        .read_tex_coords(info.tex_coord())?
                        .into_f32()
                        .collect();
                    Some((image, uvs))
                });
                match tex {
                    Some((image, uvs)) if uvs.len() == prim_pos.len() => uvs
                        .iter()
                        .map(|uv| {
                            fmul(sample_image(image, uv[0], uv[1]).unwrap_or([255, 255, 255, 255]))
                        })
                        .collect(),
                    _ => vec![fmul([255, 255, 255, 255]); prim_pos.len()],
                }
            };
            if prim_col.len() != prim_pos.len() {
                return Err(err(&"attribute counts do not match POSITION count"));
            }

            let base = positions.len() as u32;
            positions.extend(prim_pos.iter().map(|p| mat_point(world, *p)));
            normals.extend(prim_nrm.iter().map(|n| mat_dir(world, *n)));
            colors.extend(prim_col);
            match reader.read_indices() {
                Some(idx) => indices.extend(idx.into_u32().map(|i| base + i)),
                None => indices.extend(base..positions.len() as u32),
            }
        }
    }

    if positions.is_empty() {
        return Err(err(&"glTF contains no triangle geometry"));
    }
    if positions.len() > u16::MAX as usize {
        return Err(err(&format!(
            "{} vertices exceed the engine's u16 index space",
            positions.len()
        )));
    }

    let mut blob = Vec::new();
    blob.extend_from_slice(TMDL_MAGIC);
    blob.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for p in &positions {
        for v in p {
            blob.extend_from_slice(&v.to_le_bytes());
        }
    }
    for n in &normals {
        for v in n {
            blob.extend_from_slice(&v.to_le_bytes());
        }
    }
    for c in &colors {
        blob.extend_from_slice(c);
    }
    for i in &indices {
        blob.extend_from_slice(&(*i as u16).to_le_bytes());
    }
    Ok(blob)
}
