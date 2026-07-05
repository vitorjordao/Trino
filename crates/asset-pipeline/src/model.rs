//! glTF 2.0 -> TMDL: the 3D model bake, shared by every platform (TMDL is
//! the engine's portable mesh format, parsed no_std by
//! `trino_core::render3d::Mesh`).
//!
//! v1 scope: the first primitive of the first mesh; positions + normals +
//! optional COLOR_0 (defaults to white); u16 indices.

use std::path::Path;

use trino_core::render3d::TMDL_MAGIC;

/// Convert a `.gltf`/`.glb` master into a TMDL blob.
pub fn bake_model_tmdl(path: &Path) -> Result<Vec<u8>, String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let (doc, buffers, _images) = gltf::import(path).map_err(|e| err(&e))?;
    let mesh = doc
        .meshes()
        .next()
        .ok_or_else(|| err(&"glTF contains no mesh"))?;
    let primitive = mesh
        .primitives()
        .next()
        .ok_or_else(|| err(&"mesh contains no primitive"))?;
    let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| err(&"primitive has no POSITION"))?
        .collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .ok_or_else(|| err(&"primitive has no NORMAL (export with normals)"))?
        .collect();
    let colors: Vec<[u8; 4]> = match reader.read_colors(0) {
        Some(c) => c.into_rgba_u8().collect(),
        None => vec![[255, 255, 255, 255]; positions.len()],
    };
    let indices: Vec<u32> = reader
        .read_indices()
        .ok_or_else(|| err(&"primitive is not indexed"))?
        .into_u32()
        .collect();

    if normals.len() != positions.len() || colors.len() != positions.len() {
        return Err(err(&"attribute counts do not match POSITION count"));
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
