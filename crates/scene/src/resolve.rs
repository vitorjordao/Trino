//! Flatten a scene + library into what gets drawn: world matrices, resolved
//! materials, expanded prefab instances. The editor viewport renders this
//! list every frame; [`crate::codegen`] bakes it into game code.

use trino_core::physics::{Collider, Shape};
use trino_core::{
    AnimRef, ClipId, MAX_CELLS, Mat34, ModelId, ModelParams, NO_CELL, SpriteId, Vec3, VisVolume,
};

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
    /// Collision shape (local to `matrix`), `Auto` already resolved.
    pub collider: Option<Collider>,
    /// LOD levels sorted by distance: (distance, model or culled).
    pub lod: Vec<(f32, Option<(ModelId, String)>)>,
    /// Positional looping sound.
    pub sound: Option<trino_core::SoundSource>,
    /// Interior cell index ([`trino_core::NO_CELL`] = outside every cell).
    pub cell: u8,
    /// Cell or portal volume (world half extents; cell indices resolved).
    pub vis: Option<VisVolume>,
    /// True for parts that come from a prefab (not editable in place).
    pub from_prefab: bool,
}

impl DrawItem {
    /// Whether this item becomes a game-side `SceneNode` (codegen, TSCN):
    /// 2D-only sprites stay in the editor.
    pub fn exported(&self) -> bool {
        self.sprite.is_none() || self.model.is_some() || self.vis.is_some()
    }

    /// Model to draw for a camera at `eye` (LOD applied).
    pub fn model_from(&self, eye: trino_core::Vec3) -> Option<ModelId> {
        let base = self.model.as_ref().map(|(id, _)| *id);
        if self.lod.is_empty() {
            return base;
        }
        let p = self.matrix.transform_point(trino_core::Vec3::ZERO);
        let d = p - eye;
        let dist_sq = d.dot(d);
        let mut model = base;
        for (distance, m) in &self.lod {
            if dist_sq >= distance * distance {
                model = m.as_ref().map(|(id, _)| *id);
            } else {
                break;
            }
        }
        model
    }
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

impl Resolved {
    /// The scene's cells and portals (see `trino_core::vis`).
    pub fn cells(&self) -> trino_core::Cells<256> {
        trino_core::Cells::from_volumes(
            self.items
                .iter()
                .filter_map(|i| i.vis.map(|v| (i.matrix.transform_point(Vec3::ZERO), v))),
        )
    }

    /// Visibility mask from `camera` on a `screen`-sized view — what the
    /// game computes with `Cells::visible` (all set without cells).
    pub fn visible_cells(&self, camera: &trino_core::Camera3, screen: trino_core::Vec2) -> u64 {
        if !self.items.iter().any(|i| i.vis.is_some()) {
            return u64::MAX;
        }
        self.cells().visible(camera, screen)
    }
}

struct Ctx<'a> {
    lib: &'a Library,
    time: f32,
    out: Resolved,
    stack: Vec<String>,
    /// Cell the entities being visited belong to.
    cell: u8,
    /// Name paths of the cells so far, in index order.
    cells: Vec<String>,
    /// Portals waiting for cell names: (item index, a, b).
    portals: Vec<(usize, String, String)>,
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
        cell: NO_CELL,
        cells: Vec::new(),
        portals: Vec::new(),
    };
    let p2 = Parent2D {
        pos: [0.0; 2],
        scale: [1.0; 2],
        rot: 0.0,
    };
    for (i, e) in scene.entities.iter().enumerate() {
        visit(&mut ctx, e, &[i], &Mat34::IDENTITY, p2, "", false);
    }
    link_portals(&mut ctx);
    ctx.out
}

/// World half extents of a local box (`size` = full size) under `m`: the
/// axis-aligned box enclosing the transformed one.
fn world_half(m: &Mat34, size: [f32; 3]) -> Vec3 {
    let h = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    let row = |r: usize| (0..3).map(|c| m.m[r][c].abs() * h[c]).sum::<f32>();
    Vec3::new(row(0), row(1), row(2))
}

fn name_matches(full: &str, name: &str) -> bool {
    full == name || full.ends_with(&format!("/{name}"))
}

/// Resolve portal cell names to indices (cells may come after portals).
fn link_portals(ctx: &mut Ctx) {
    let portals = std::mem::take(&mut ctx.portals);
    for (at, a, b) in portals {
        let find = |n: &str| ctx.cells.iter().position(|c| name_matches(c, n));
        let item = &mut ctx.out.items[at];
        match (find(&a), find(&b)) {
            (Some(ia), Some(ib)) => {
                if let Some(VisVolume::Portal { a, b, .. }) = &mut item.vis {
                    (*a, *b) = (ia as u8, ib as u8);
                }
            }
            (fa, _) => {
                let missing = if fa.is_none() { a } else { b };
                ctx.out.warnings.push(format!(
                    "{}: portal names unknown cell `{missing}` — portal ignored",
                    item.name
                ));
                item.vis = None;
            }
        }
    }
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
            params.anim = Some(AnimRef::new(ClipId::from_name(&a.clip), ctx.time * a.speed));
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
    let mut lod: Vec<(f32, Option<(ModelId, String)>)> = e
        .model
        .as_ref()
        .map(|m| {
            m.lod
                .iter()
                .map(|l| {
                    (
                        l.distance,
                        l.path
                            .as_ref()
                            .map(|p| (ModelId(trino_core::asset_id(p)), p.clone())),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    lod.sort_by(|a, b| a.0.total_cmp(&b.0));
    let collider = e
        .collider
        .as_ref()
        .map(|c| resolve_collider(ctx, &name, e, c));
    let outer_cell = ctx.cell;
    let vis = match &e.vis {
        Some(crate::VisComponent::Cell { size }) => {
            if ctx.cells.len() < MAX_CELLS {
                ctx.cell = ctx.cells.len() as u8;
                ctx.cells.push(name.clone());
                Some(VisVolume::Cell {
                    half: world_half(&matrix, *size),
                })
            } else {
                ctx.out.warnings.push(format!(
                    "{name}: more than {MAX_CELLS} cells — this one is ignored"
                ));
                None
            }
        }
        Some(crate::VisComponent::Portal { size, a, b }) => {
            ctx.portals
                .push((ctx.out.items.len(), a.clone(), b.clone()));
            Some(VisVolume::Portal {
                half: world_half(&matrix, *size),
                a: 0,
                b: 0,
            })
        }
        None => None,
    };
    ctx.out.items.push(DrawItem {
        owner: owner.to_vec(),
        name: name.clone(),
        tags: e.tags.clone(),
        matrix,
        model,
        params,
        material,
        sprite,
        collider,
        lod,
        sound: e.sound.as_ref().map(|s| trino_core::SoundSource {
            sound: trino_core::SoundId::from_path(&s.path),
            radius: s.radius,
            volume: s.volume,
        }),
        cell: ctx.cell,
        vis,
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
    ctx.cell = outer_cell;
}

/// Authored collider -> engine collider (`Auto` = the model's box).
fn resolve_collider(
    ctx: &mut Ctx,
    name: &str,
    e: &Entity,
    c: &crate::ColliderComponent,
) -> Collider {
    let offset = Vec3::new(c.offset[0], c.offset[1], c.offset[2]);
    let (shape, offset) = match c.shape {
        crate::ShapeDef::Box { size } => (
            Shape::Box {
                half: Vec3::new(size[0] * 0.5, size[1] * 0.5, size[2] * 0.5),
            },
            offset,
        ),
        crate::ShapeDef::Sphere { radius } => (Shape::Sphere { radius }, offset),
        crate::ShapeDef::Capsule { radius, height } => (Shape::Capsule { radius, height }, offset),
        crate::ShapeDef::Auto => {
            let bounds = e
                .model
                .as_ref()
                .and_then(|m| ctx.lib.model_bounds.get(&m.path));
            match bounds {
                Some((min, max)) => (
                    Shape::Box {
                        half: Vec3::new(
                            (max[0] - min[0]) * 0.5,
                            (max[1] - min[1]) * 0.5,
                            (max[2] - min[2]) * 0.5,
                        ),
                    },
                    offset
                        + Vec3::new(
                            (max[0] + min[0]) * 0.5,
                            (max[1] + min[1]) * 0.5,
                            (max[2] + min[2]) * 0.5,
                        ),
                ),
                None => {
                    ctx.out.warnings.push(format!(
                        "{name}: `Auto` collider needs a model with known bounds — \
                         using a 1x1x1 box (give it an explicit shape)"
                    ));
                    (
                        Shape::Box {
                            half: Vec3::new(0.5, 0.5, 0.5),
                        },
                        offset,
                    )
                }
            }
        }
    };
    Collider {
        shape,
        offset,
        trigger: c.trigger,
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
    fn colliders_resolve_auto_from_model_bounds() {
        let mut lib = Library::default();
        lib.model_bounds
            .insert("models/crate".into(), ([-1.0, 0.0, -1.0], [1.0, 2.0, 1.0]));
        let mut scene = Scene::new("s");
        let mut a = Entity::new("crate").with_model(ModelComponent::new("models/crate"));
        a.collider = Some(crate::ColliderComponent::default());
        let mut b = Entity::new("coin");
        b.collider = Some(crate::ColliderComponent {
            shape: crate::ShapeDef::Sphere { radius: 0.5 },
            offset: [0.0, 1.0, 0.0],
            trigger: true,
        });
        let mut c = Entity::new("mystery");
        c.collider = Some(crate::ColliderComponent::default());
        scene.entities.extend([a, b, c]);
        let r = resolve(&scene, &lib, 0.0);
        let crate_c = r.items[0].collider.unwrap();
        assert_eq!(
            crate_c.shape,
            Shape::Box {
                half: Vec3::new(1.0, 1.0, 1.0)
            }
        );
        assert_eq!(crate_c.offset, Vec3::new(0.0, 1.0, 0.0));
        assert!(r.items[1].collider.unwrap().trigger);
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("mystery"));
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
