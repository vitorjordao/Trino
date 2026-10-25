//! Animation tooling on baked models: measurements per clip (how far a
//! walk cycle carries the character, whether a planted foot slides, whether
//! the mesh dips under the floor, the bounds of every frame) and a pose
//! baked into a static mesh — for a console that should not pay for a
//! skeleton. The editor's Animation panel and `cargo xtask model inspect
//! --anim` / `model pose` call the same functions.
//!
//! Everything is measured on the T&L's own posed vertices
//! (`Mesh::posed_position`), in model space, Y up, the floor at y = 0.

use std::path::Path;

use trino_core::render3d::MAX_JOINTS;
use trino_core::{AnimRef, ClipId, Mat34, Mesh, Vec3};

use crate::model::MeshData;

/// What a planted foot may do before a clip gets a warning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Worst slide of a planted foot in one frame (meters), against the
    /// clip's own stride speed.
    pub slide: f32,
    /// How far under the floor the lowest vertex may go (meters).
    pub below: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            slide: 0.02,
            below: 0.005,
        }
    }
}

/// The model-space box of one sampled frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameBounds {
    pub time: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Measurements of one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipMetrics {
    pub name: String,
    pub duration: f32,
    pub frames: usize,
    pub looping: bool,
    /// How fast a planted foot travels (m/s): an in-place walk moves its
    /// planted foot backward at the speed the game must move the character
    /// so the foot stays put on the ground. 0 for clips that never touch.
    pub speed: f32,
    /// `speed × duration`: ground covered by one cycle.
    pub distance_per_cycle: f32,
    /// Worst frame-to-frame slide of a planted foot against that speed (m).
    pub foot_slide: f32,
    /// Lowest vertex over the clip (y; below 0 = through the floor).
    pub lowest: f32,
    /// Joints that touch the floor at some frame (the feet).
    pub feet: Vec<usize>,
    /// The bake took this clip's root motion out (`root_motion`, issue
    /// #65): it plays in place and the game moves the entity by the track.
    pub root_motion: bool,
    /// Ground the root track covers in one cycle (m; 0 without root motion).
    pub root_distance: f32,
    /// With root motion: the worst frame-to-frame slide of a planted foot
    /// in the world when the entity moves by the track (m). `foot_slide`
    /// is the same foot against a constant speed, what a game that ignores
    /// the track gets.
    pub root_foot_slide: Option<f32>,
    /// Timeline events: (seconds, event name hash). Names are in the
    /// manifest; tools that have it map them back.
    pub events: Vec<(f32, u32)>,
    pub bounds: Vec<FrameBounds>,
    /// Limits the clip breaks, in words.
    pub warnings: Vec<String>,
}

/// Posed positions of every vertex at `anim`.
pub fn posed(mesh: &Mesh, anim: Option<AnimRef>) -> Vec<Vec3> {
    let mut joints = [Mat34::IDENTITY; MAX_JOINTS];
    let count = mesh.pose(anim, &mut joints);
    (0..mesh.vertex_count)
        .map(|i| mesh.posed_position(i, &joints, count))
        .collect()
}

/// Measure clip `name` of a baked model (`tmdl`).
pub fn clip_metrics(tmdl: &[u8], name: &str, limits: Limits) -> Result<ClipMetrics, String> {
    let mesh = Mesh::from_tmdl(tmdl).map_err(|e| format!("model: {e:?}"))?;
    let id = ClipId::from_name(name);
    let clip = mesh
        .find_clip(id)
        .ok_or_else(|| format!("clip `{name}` is not in this model"))?;
    let frames = clip.frame_count.max(1);
    let dt = 1.0 / clip.fps.max(1e-3);
    let poses: Vec<Vec<Vec3>> = (0..frames)
        .map(|f| posed(&mesh, Some(AnimRef::new(id, f as f32 * dt))))
        .collect();
    let height = {
        let (lo, hi) = poses
            .iter()
            .flatten()
            .fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                (lo.min(p.y), hi.max(p.y))
            });
        (hi - lo).max(1e-3)
    };
    let lowest = poses.iter().flatten().fold(f32::MAX, |lo, p| lo.min(p.y));
    // A foot touches when its lowest vertex is this close to the floor the
    // clip stands on: its lowest point (y = 0 for a model built on the
    // floor; a clip that sinks or floats is measured against itself).
    let floor = lowest;
    let eps = (height * 0.02).max(0.01);
    let joints = mesh.joint_count().max(1);
    // Contact of joint j in frame f: the index of its lowest vertex.
    let contact = |f: usize, j: usize| -> Option<usize> {
        (0..mesh.vertex_count)
            .filter(|&i| mesh.joint_of_vertex(i) == j)
            .min_by(|&a, &b| poses[f][a].y.total_cmp(&poses[f][b].y))
    };
    let contacts: Vec<Vec<Option<usize>>> = (0..frames)
        .map(|f| (0..joints).map(|j| contact(f, j)).collect())
        .collect();
    // Planted in frame f: on the floor, and the lowest foot of that frame
    // (or level with it — both feet in double support). A swinging foot
    // that skims the floor is not planted.
    let planted = |f: usize, j: usize| -> Option<usize> {
        let i = contacts[f][j]?;
        let y = poses[f][i].y;
        let lowest_now = contacts[f]
            .iter()
            .flatten()
            .map(|&k| poses[f][k].y)
            .fold(f32::MAX, f32::min);
        (y <= floor + eps && y <= lowest_now + eps * 0.5).then_some(i)
    };
    // Every planted step: the horizontal move of the contact vertex.
    let mut steps: Vec<Vec3> = Vec::new();
    let mut feet = Vec::new();
    // A looping clip's last frame is its first again (time wraps at the
    // duration), so consecutive frames are every pair there is.
    let pairs = frames - 1;
    for j in 0..joints {
        if (0..frames).any(|f| planted(f, j).is_some()) {
            feet.push(j);
        }
        for f in 0..pairs {
            let g = f + 1;
            if let (Some(i), Some(_)) = (planted(f, j), planted(g, j)) {
                // The same vertex in both frames: a foot that rolls from
                // heel to toe is not a slide.
                let (a, b) = (poses[f][i], poses[g][i]);
                steps.push(Vec3::new(b.x - a.x, 0.0, b.z - a.z));
            }
        }
    }
    let (speed, foot_slide) = if steps.is_empty() {
        (0.0, 0.0)
    } else {
        let mean = steps.iter().fold(Vec3::ZERO, |s, d| s + *d) * (1.0 / steps.len() as f32);
        let slide = steps
            .iter()
            .map(|d| {
                let e = *d - mean;
                e.dot(e).sqrt()
            })
            .fold(0.0f32, f32::max);
        (mean.dot(mean).sqrt() / dt, slide)
    };
    let meta = trino_core::anim::ClipMeta::from_tmdl(tmdl).unwrap_or_default();
    let root = meta.roots.root_of(id);
    let events_track = meta.events.events_of(id);
    let events = (0..events_track.len())
        .map(|i| {
            let (t, e, _) = events_track.get(i);
            (t, e.0)
        })
        .collect();
    let (root_distance, root_foot_slide) = match root {
        None => (0.0, None),
        Some(track) => {
            // Each frame's vertex in the world: the in-place pose carried by
            // the root's ground pose at that frame.
            let world = |f: usize, i: usize| {
                let p = poses[f][i];
                let g = track.at(f as f32 * dt);
                let (s, c) = (g.yaw.sin(), g.yaw.cos());
                Vec3::new(g.x + p.x * c + p.z * s, p.y, g.z - p.x * s + p.z * c)
            };
            let mut worst = 0.0f32;
            for j in 0..joints {
                for f in 0..pairs {
                    if let (Some(i), Some(_)) = (planted(f, j), planted(f + 1, j)) {
                        let d = world(f + 1, i) - world(f, i);
                        worst = worst.max((d.x * d.x + d.z * d.z).sqrt());
                    }
                }
            }
            let end = track.at(clip.duration());
            ((end.x * end.x + end.z * end.z).sqrt(), Some(worst))
        }
    };
    let bounds = poses
        .iter()
        .enumerate()
        .map(|(f, ps)| {
            let mut min = [f32::MAX; 3];
            let mut max = [f32::MIN; 3];
            for p in ps {
                for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                    min[k] = min[k].min(v);
                    max[k] = max[k].max(v);
                }
            }
            FrameBounds {
                time: f as f32 * dt,
                min,
                max,
            }
        })
        .collect();
    let mut warnings = Vec::new();
    // A root-motion clip is judged as it plays: carried by its track.
    let slide = root_foot_slide.unwrap_or(foot_slide);
    if slide > limits.slide {
        warnings.push(format!(
            "a planted foot slides {:.1} cm in one frame (limit {:.1} cm)",
            slide * 100.0,
            limits.slide * 100.0
        ));
    }
    if lowest < -limits.below {
        warnings.push(format!(
            "goes {:.1} cm below the floor (lowest vertex y = {lowest:.3})",
            -lowest * 100.0
        ));
    }
    let duration = clip.duration();
    Ok(ClipMetrics {
        name: name.to_string(),
        duration,
        frames,
        looping: clip.looping,
        speed,
        distance_per_cycle: speed * duration,
        foot_slide,
        lowest,
        feet,
        root_motion: root.is_some(),
        root_distance,
        root_foot_slide,
        events,
        bounds,
        warnings,
    })
}

/// Every named clip's metrics (names as `clip_names` reads them from the
/// master; clips the bake dropped are skipped).
pub fn all_metrics(tmdl: &[u8], names: &[String], limits: Limits) -> Vec<ClipMetrics> {
    names
        .iter()
        .filter_map(|n| clip_metrics(tmdl, n, limits).ok())
        .collect()
}

/// The pose of clip `clip` (None = bind pose) at `time` as a static mesh:
/// posed positions and normals, the same colors and triangles, no
/// skeleton — what a console without skinning draws.
pub fn pose_mesh(tmdl: &[u8], clip: Option<&str>, time: f32) -> Result<MeshData, String> {
    let mesh = Mesh::from_tmdl(tmdl).map_err(|e| format!("model: {e:?}"))?;
    let anim = match clip {
        Some(c) => {
            let id = ClipId::from_name(c);
            mesh.find_clip(id)
                .ok_or_else(|| format!("clip `{c}` is not in this model"))?;
            Some(AnimRef::new(id, time))
        }
        None => None,
    };
    let mut joints = [Mat34::IDENTITY; MAX_JOINTS];
    let count = mesh.pose(anim, &mut joints);
    let mut data = MeshData::default();
    for i in 0..mesh.vertex_count {
        let p = mesh.posed_position(i, &joints, count);
        let n = mesh.posed_normal(i, &joints, count).normalized();
        let c = mesh.color(i);
        data.positions.push([p.x, p.y, p.z]);
        data.normals.push([n.x, n.y, n.z]);
        data.colors.push([c.r, c.g, c.b, c.a]);
        if mesh.has_slots() {
            data.slots.push(mesh.slot(i));
        }
    }
    data.indices = (0..mesh.index_count)
        .map(|i| mesh.index(i) as u32)
        .collect();
    Ok(data)
}

/// sRGB 0..255 → linear 0..1 (glTF `COLOR_0` is linear; the bake converts
/// back, so a written pose bakes to its own colors).
fn linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// `data` (a static mesh) as a self-contained glTF 2.0 file (`.gltf`, the
/// buffer inline as base64): positions, normals, linear `COLOR_0`, indices.
pub fn write_gltf(data: &MeshData, name: &str) -> String {
    let n = data.positions.len();
    let mut buf: Vec<u8> = Vec::new();
    let put = |buf: &mut Vec<u8>, v: f32| buf.extend_from_slice(&v.to_le_bytes());
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &data.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
            put(&mut buf, p[k]);
        }
    }
    let normals_at = buf.len();
    for v in &data.normals {
        for x in v {
            put(&mut buf, *x);
        }
    }
    let colors_at = buf.len();
    for c in &data.colors {
        for (k, x) in c.iter().enumerate() {
            put(&mut buf, if k < 3 { linear(*x) } else { *x as f32 / 255.0 });
        }
    }
    let indices_at = buf.len();
    for i in &data.indices {
        buf.extend_from_slice(&i.to_le_bytes());
    }
    let len = buf.len();
    let f = |v: f32| format!("{v:?}");
    format!(
        r#"{{"asset":{{"version":"2.0","generator":"trino model pose"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"name":"{name}"}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3}}]}}],"buffers":[{{"byteLength":{len},"uri":"data:application/octet-stream;base64,{b64}"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{pl}}},{{"buffer":0,"byteOffset":{normals_at},"byteLength":{pl}}},{{"buffer":0,"byteOffset":{colors_at},"byteLength":{cl}}},{{"buffer":0,"byteOffset":{indices_at},"byteLength":{il}}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":{n},"type":"VEC3","min":[{},{},{}],"max":[{},{},{}]}},{{"bufferView":1,"componentType":5126,"count":{n},"type":"VEC3"}},{{"bufferView":2,"componentType":5126,"count":{n},"type":"VEC4"}},{{"bufferView":3,"componentType":5125,"count":{ic},"type":"SCALAR"}}]}}"#,
        f(min[0]),
        f(min[1]),
        f(min[2]),
        f(max[0]),
        f(max[1]),
        f(max[2]),
        b64 = base64(&buf),
        pl = n * 12,
        cl = n * 16,
        il = data.indices.len() * 4,
        ic = data.indices.len(),
    )
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// What [`export_pose`] wrote.
#[derive(Clone, Debug)]
pub struct PoseExport {
    /// `models/<name>`.
    pub logical: String,
    /// The file, relative to the assets root (`shared/models/poses/<name>.gltf`).
    pub file: String,
    pub tris: usize,
    pub verts: usize,
    /// Estimated N64 time of one draw (µs, calibrated cost model).
    pub n64_us: u32,
    pub manifest_changed: bool,
}

/// Bake the pose of `model`'s clip at `time` into a static model `name`:
/// writes `assets/shared/models/poses/<name>.gltf` and declares
/// `[models.<name>]` once. The console variant of the animated model can
/// point at it (scene `platforms`).
pub fn export_pose(
    assets_root: &Path,
    model: &str,
    clip: Option<&str>,
    time: f32,
    name: &str,
) -> Result<PoseExport, String> {
    if !crate::manifest::edit::valid_name(name) {
        return Err(format!(
            "model name `{name}`: lowercase letters, digits, - and _"
        ));
    }
    let manifest_path = assets_root.join("manifest.toml");
    let manifest = crate::Manifest::load(&manifest_path)?;
    let short = model.strip_prefix("models/").unwrap_or(model);
    let decl = manifest
        .models
        .get(short)
        .ok_or_else(|| format!("model `{model}` is not in the manifest"))?;
    let source = crate::resolve_source(assets_root, crate::Platform::Pc, &decl.file)?;
    let tmdl = crate::bake_model(&source, &decl.options())?;
    let data = pose_mesh(&tmdl, clip, time)?;
    let rel = format!("models/poses/{name}.gltf");
    let path = assets_root.join("shared").join(&rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, write_gltf(&data, name))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest_changed = if crate::manifest::edit::has_table(&text, "models", name) {
        false
    } else {
        let text = crate::manifest::edit::add_model(&text, name, &rel)?;
        std::fs::write(&manifest_path, text)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        true
    };
    let (tris, verts) = (data.indices.len() / 3, data.positions.len());
    Ok(PoseExport {
        logical: format!("models/{name}"),
        file: format!("shared/{rel}"),
        tris,
        verts,
        n64_us: trino_core::perf::N64_COST.mesh_draw_us(verts as u32, tris as u32),
        manifest_changed,
    })
}
