//! Baked scene data for games: what `cargo xtask scene export` (and the
//! editor's *Export to game code*) generates from an editor scene — a
//! `static` slice of flattened [`SceneNode`]s plus the scene's camera,
//! light and clear color as consts. Pure data, `no_std`, zero allocation:
//! editor-made levels run on PC, N64 and 3DS through the same code.
//!
//! ```ignore
//! mod level { include!("level_gen.rs"); }
//! trino_core::scene::draw_nodes(renderer, level::NODES, &Mat34::IDENTITY, t);
//! let spawn = trino_core::scene::find(level::NODES, "spawn").unwrap().position();
//! ```

use crate::math::Vec3;
use crate::math3d::Mat34;
use crate::physics::{Aabb, Collider};
use crate::render::{AnimRef, ClipId, Material, ModelId, ModelParams, Renderer};

/// One flattened entity (prefabs expanded, world matrix precomputed).
#[derive(Clone, Copy, Debug)]
pub struct SceneNode {
    /// `/`-joined name path (`house/door`).
    pub name: &'static str,
    pub tags: &'static [&'static str],
    pub matrix: Mat34,
    /// None = marker (spawn points, triggers, waypoints).
    pub model: Option<ModelId>,
    pub params: ModelParams,
    /// Clip to loop and its playback speed.
    pub anim: Option<(ClipId, f32)>,
    /// Collision shape (local to `matrix`); see [`crate::physics`].
    pub collider: Option<Collider>,
}

impl SceneNode {
    /// World position (the matrix translation).
    pub fn position(&self) -> Vec3 {
        Vec3::new(
            self.matrix.m[0][3],
            self.matrix.m[1][3],
            self.matrix.m[2][3],
        )
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(&tag)
    }

    /// World box of the collider, if any.
    pub fn world_aabb(&self) -> Option<Aabb> {
        self.collider.map(|c| c.world_aabb(&self.matrix))
    }
}

/// World boxes of every **solid** collider in `nodes`, written to `out`;
/// returns how many (stops at `out.len()`). Static level collision without
/// an [`crate::entity::EntityStore`].
pub fn solids(nodes: &[SceneNode], out: &mut [Aabb]) -> usize {
    let mut n = 0;
    for node in nodes {
        let Some(c) = node.collider.filter(|c| !c.trigger) else {
            continue;
        };
        if n == out.len() {
            break;
        }
        out[n] = c.world_aabb(&node.matrix);
        n += 1;
    }
    n
}

/// Draw every model node under `root`, animating clips at `time` seconds.
pub fn draw_nodes(renderer: &mut dyn Renderer, nodes: &[SceneNode], root: &Mat34, time: f32) {
    for node in nodes {
        let Some(model) = node.model else {
            continue;
        };
        let mut params = node.params;
        if let Some((clip, speed)) = node.anim {
            params.anim = Some(AnimRef::new(clip, time * speed));
        }
        renderer.draw_model_matrix(model, &root.mul(&node.matrix), Material::VertexLit, &params);
    }
}

/// First node whose name path is `name` or ends with `/name`.
pub fn find<'a>(nodes: &'a [SceneNode], name: &str) -> Option<&'a SceneNode> {
    nodes.iter().find(|n| {
        n.name == name
            || (n.name.len() > name.len()
                && n.name.ends_with(name)
                && n.name.as_bytes()[n.name.len() - name.len() - 1] == b'/')
    })
}

/// Nodes carrying `tag`.
pub fn with_tag<'a>(nodes: &'a [SceneNode], tag: &'a str) -> impl Iterator<Item = &'a SceneNode> {
    nodes.iter().filter(move |n| n.has_tag(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODES: &[SceneNode] = &[
        SceneNode {
            name: "house/door",
            tags: &["door"],
            matrix: Mat34::IDENTITY,
            model: None,
            params: ModelParams::DEFAULT,
            anim: None,
            collider: None,
        },
        SceneNode {
            name: "spawn",
            tags: &["spawn"],
            matrix: Mat34 {
                m: [
                    [1.0, 0.0, 0.0, 3.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, -2.0],
                ],
            },
            model: None,
            params: ModelParams::DEFAULT,
            anim: None,
            collider: None,
        },
    ];

    #[test]
    fn find_by_name_or_leaf_and_by_tag() {
        assert_eq!(find(NODES, "door").unwrap().name, "house/door");
        assert_eq!(find(NODES, "house/door").unwrap().name, "house/door");
        assert!(find(NODES, "oor").is_none(), "leaf match must align on `/`");
        assert_eq!(
            find(NODES, "spawn").unwrap().position(),
            Vec3::new(3.0, 0.0, -2.0)
        );
        assert_eq!(with_tag(NODES, "door").count(), 1);
    }

    #[test]
    fn solids_skip_triggers() {
        let mut nodes = [NODES[0], NODES[1]];
        nodes[0].collider = Some(Collider::solid_box(Vec3::ONE));
        nodes[1].collider = Some(Collider {
            trigger: true,
            ..Collider::solid_box(Vec3::ONE)
        });
        let mut out = [Aabb::EMPTY; 4];
        assert_eq!(solids(&nodes, &mut out), 1);
        assert_eq!(out[0].max, Vec3::ONE);
        assert_eq!(
            nodes[1].world_aabb().unwrap().min,
            Vec3::new(2.0, -1.0, -3.0)
        );
    }
}
