//! Flatten a scene + library into what gets drawn: world matrices, resolved
//! materials, expanded prefab instances. The editor viewport renders this
//! list every frame; [`crate::codegen`] bakes it into game code.

use trino_core::{AnimRef, ClipId, Mat34, ModelId, ModelParams, SpriteId};

use crate::{Entity, Library, Scene};

/// Prefab nesting deeper than this is treated as a cycle.
pub const MAX_PREFAB_DEPTH: usize = 16;

/// One flattened entity.
#[derive(Clone, Debug)]
pub struct DrawItem {
    /// Path of the **scene** entity this item belongs to (for prefab parts:
    /// the instance entity) — what clicking it selects.
    pub owner: Vec<usize>,
    /// Name path from the scene root (`house/door`, prefab parts included).
    pub name: String,
    pub tags: Vec<String>,
    pub matrix: Mat34,
    pub model: Option<(ModelId, String)>,
    pub params: ModelParams,
    /// Material logical path, when the entity uses one.
    pub material: Option<String>,
    pub sprite: Option<SpriteDraw>,
    /// True for parts that come from a prefab (not editable in place).
    pub from_prefab: bool,
}

/// A sprite in screen space (2D accumulation of the entity chain).
#[derive(Clone, Debug)]
pub struct SpriteDraw {
    pub id: SpriteId,
    pub path: String,
    pub pos: [f32; 2],
    pub scale: [f32; 2],
    /// Radians.
    pub rotation: f32,
    pub tint: [u8; 4],
    pub flip_x: bool,
    pub flip_y: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub items: Vec<DrawItem>,
    /// Missing prefabs/materials, cycles, stale overrides — never fatal.
    pub warnings: Vec<String>,
}

struct Ctx<'a> {
    lib: &'a Library,
    time: f32,
    out: Resolved,
    stack: Vec<String>,
}

#[derive(Clone, Copy)]
struct Parent2D {
    pos: [f32; 2],
    scale: [f32; 2],
    rot: f32,
}

/// Resolve `scene` at animation time `time` (seconds; clips play at
/// `time * speed`).
pub fn resolve(scene: &Scene, lib: &Library, time: f32) -> Resolved {
    let mut ctx = Ctx {
        lib,
        time,
        out: Resolved::default(),
        stack: Vec::new(),
    };
    let p2 = Parent2D {
        pos: [0.0; 2],
        scale: [1.0; 2],
        rot: 0.0,
    };
    for (i, e) in scene.entities.iter().enumerate() {
        visit(&mut ctx, e, &[i], &Mat34::IDENTITY, p2, "", false);
    }
    ctx.out
}

fn visit(
    ctx: &mut Ctx,
    e: &Entity,
    owner: &[usize],
    parent: &Mat34,
    p2: Parent2D,
    parent_name: &str,
    from_prefab: bool,
) {
    if !e.visible {
        return;
    }
    let matrix = parent.mul(&e.transform.matrix());
    let name = if parent_name.is_empty() {
        e.name.clone()
    } else {
        format!("{parent_name}/{}", e.name)
    };
    let t = &e.transform;
    let here = Parent2D {
        pos: [
            p2.pos[0] + t.pos[0] * p2.scale[0],
            p2.pos[1] + t.pos[1] * p2.scale[1],
        ],
        scale: [p2.scale[0] * t.scale[0], p2.scale[1] * t.scale[1]],
        rot: p2.rot + t.rot[2].to_radians(),
    };

    let mut params = ModelParams::DEFAULT;
    let mut material = None;
    let model = e.model.as_ref().map(|m| {
        params = match &m.material {
            Some(path) => match ctx.lib.materials.get(path) {
                Some(def) => def.params(m.tint),
                None => {
                    ctx.out
                        .warnings
                        .push(format!("{name}: unknown material `{path}`"));
                    crate::MaterialDef::default().params(m.tint)
                }
            },
            None => crate::MaterialDef::default().params(m.tint),
        };
        material = m.material.clone();
        if let Some(a) = &m.anim {
            params.anim = Some(AnimRef {
                clip: ClipId::from_name(&a.clip),
                time: ctx.time * a.speed,
            });
        }
        (m.model_id(), m.path.clone())
    });
    let sprite = e.sprite.as_ref().map(|s| SpriteDraw {
        id: s.sprite_id(),
        path: s.path.clone(),
        pos: here.pos,
        scale: here.scale,
        rotation: here.rot,
        tint: s.tint,
        flip_x: s.flip_x,
        flip_y: s.flip_y,
    });
    ctx.out.items.push(DrawItem {
        owner: owner.to_vec(),
        name: name.clone(),
        tags: e.tags.clone(),
        matrix,
        model,
        params,
        material,
        sprite,
        from_prefab,
    });

    if let Some(inst) = &e.prefab {
        if ctx.stack.contains(&inst.path) || ctx.stack.len() >= MAX_PREFAB_DEPTH {
            ctx.out.warnings.push(format!(
                "{name}: prefab cycle through `{}` ({}) — instance skipped",
                inst.path,
                ctx.stack.join(" → ")
            ));
        } else {
            match ctx.lib.expand(inst) {
                Ok((root, stale)) => {
                    for s in stale {
                        ctx.out.warnings.push(format!(
                            "{name}: override target `{s}` not found in `{}`",
                            inst.path
                        ));
                    }
                    ctx.stack.push(inst.path.clone());
                    visit(ctx, &root, owner, &matrix, here, &name, true);
                    ctx.stack.pop();
                }
                Err(err) => ctx.out.warnings.push(format!("{name}: {err}")),
            }
        }
    }

    for (i, child) in e.children.iter().enumerate() {
        // Prefab parts keep pointing at the instance owner.
        let child_owner: Vec<usize> = if from_prefab {
            owner.to_vec()
        } else {
            let mut p = owner.to_vec();
            p.push(i);
            p
        };
        visit(ctx, child, &child_owner, &matrix, here, &name, from_prefab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnimComponent, MaterialDef, ModelComponent, Prefab, PrefabInstance, ShadingDef};

    fn lib() -> Library {
        let mut lib = Library::default();
        lib.materials.insert(
            "materials/glow".into(),
            MaterialDef {
                shading: ShadingDef::Unlit,
                ..MaterialDef::default()
            },
        );
        let mut root = Entity::new("lamp");
        root.children.push(
            Entity::new("bulb")
                .at([0.0, 2.0, 0.0])
                .with_model(ModelComponent {
                    material: Some("materials/glow".into()),
                    ..ModelComponent::new("models/bulb")
                }),
        );
        lib.prefabs.insert("prefabs/lamp".into(), Prefab::new(root));
        lib
    }

    #[test]
    fn prefab_instances_expand_under_the_instance_transform() {
        let lib = lib();
        let mut scene = Scene::new("s");
        let mut inst = Entity::new("lamp-1").at([10.0, 0.0, 0.0]);
        inst.prefab = Some(PrefabInstance::new("prefabs/lamp"));
        scene.entities.push(inst);
        let r = resolve(&scene, &lib, 0.0);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let bulb = r
            .items
            .iter()
            .find(|i| i.name == "lamp-1/lamp/bulb")
            .unwrap();
        assert_eq!(bulb.owner, vec![0], "prefab parts select the instance");
        assert!(bulb.from_prefab);
        assert_eq!(bulb.params.shading, trino_core::Shading::Unlit);
        let p = bulb.matrix.transform_point(trino_core::Vec3::ZERO);
        assert!((p.x - 10.0).abs() < 1e-4 && (p.y - 2.0).abs() < 1e-4);
    }

    #[test]
    fn cycles_and_missing_references_warn_instead_of_failing() {
        let mut lib = lib();
        let mut a = Entity::new("a");
        a.prefab = Some(PrefabInstance::new("prefabs/b"));
        let mut b = Entity::new("b");
        b.prefab = Some(PrefabInstance::new("prefabs/a"));
        lib.prefabs.insert("prefabs/a".into(), Prefab::new(a));
        lib.prefabs.insert("prefabs/b".into(), Prefab::new(b));
        let mut scene = Scene::new("s");
        let mut inst = Entity::new("loop");
        inst.prefab = Some(PrefabInstance::new("prefabs/a"));
        scene.entities.push(inst);
        let mut ghost = Entity::new("ghost").with_model(ModelComponent {
            material: Some("materials/nope".into()),
            ..ModelComponent::new("models/x")
        });
        ghost.prefab = Some(PrefabInstance::new("prefabs/missing"));
        scene.entities.push(ghost);
        let r = resolve(&scene, &lib, 0.0);
        assert!(
            r.warnings.iter().any(|w| w.contains("cycle")),
            "{:?}",
            r.warnings
        );
        assert!(r.warnings.iter().any(|w| w.contains("unknown material")));
        assert!(r.warnings.iter().any(|w| w.contains("unknown prefab")));
    }

    #[test]
    fn hidden_subtrees_are_skipped_and_anim_time_scales() {
        let lib = Library::default();
        let mut scene = Scene::new("s");
        let mut hidden = Entity::new("hidden");
        hidden.visible = false;
        hidden.children.push(Entity::new("child"));
        scene.entities.push(hidden);
        scene
            .entities
            .push(Entity::new("robot").with_model(ModelComponent {
                anim: Some(AnimComponent {
                    clip: "walk".into(),
                    speed: 2.0,
                }),
                ..ModelComponent::new("models/robot")
            }));
        let r = resolve(&scene, &lib, 1.5);
        assert_eq!(r.items.len(), 1);
        let anim = r.items[0].params.anim.unwrap();
        assert_eq!(anim.clip, ClipId::from_name("walk"));
        assert!((anim.time - 3.0).abs() < 1e-6);
    }
}
