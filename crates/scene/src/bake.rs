//! Scene → `TSCN` blob (format in `trino_core::scene`): the runtime-loaded
//! twin of [`crate::codegen`]. Same resolve, same node filter, same values —
//! a level baked as data and the same level exported as code draw and
//! collide identically (tested in `xtask/tests/showcase_render.rs`).

use trino_core::scene::{TSCN_ENV_BYTES, TSCN_HEADER, TSCN_MAGIC, TSCN_NODE_BYTES, TSCN_VERSION};
use trino_core::{Color, Shading, Shape};

use crate::{Library, Scene, resolve};

fn f32s(out: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

fn rgba(out: &mut Vec<u8>, c: Color) {
    out.extend_from_slice(&[c.r, c.g, c.b, c.a]);
}

/// Bake `scene` (resolved against `lib`) into a TSCN blob. Returns
/// (blob, warnings).
pub fn to_tscn(scene: &Scene, lib: &Library) -> (Vec<u8>, Vec<String>) {
    let resolved = resolve(scene, lib, 0.0);
    let items: Vec<_> = resolved.items.iter().filter(|i| i.exported()).collect();

    let mut out = Vec::new();
    out.extend_from_slice(TSCN_MAGIC);
    out.extend_from_slice(&TSCN_VERSION.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());

    let env = &scene.environment;
    let cam = env.core_camera();
    let light = env.core_light();
    out.extend_from_slice(&[env.clear[0], env.clear[1], env.clear[2], 0]);
    f32s(
        &mut out,
        &[
            cam.eye.x,
            cam.eye.y,
            cam.eye.z,
            cam.target.x,
            cam.target.y,
            cam.target.z,
            cam.fov_y,
        ],
    );
    f32s(
        &mut out,
        &[light.dir.x, light.dir.y, light.dir.z, light.ambient],
    );
    rgba(&mut out, light.color);
    rgba(&mut out, light.ambient_color);
    rgba(&mut out, light.fog.color);
    f32s(&mut out, &[light.fog.near, light.fog.far]);
    debug_assert_eq!(out.len(), TSCN_HEADER + TSCN_ENV_BYTES);

    let mut strings = Vec::new();
    fn intern(strings: &mut Vec<u8>, s: &str) -> (u32, u32) {
        let at = strings.len() as u32;
        strings.extend_from_slice(s.as_bytes());
        (at, s.len() as u32)
    }
    for item in &items {
        let start = out.len();
        let (no, nl) = intern(&mut strings, &item.name);
        let (to, tl) = intern(&mut strings, &item.tags.join("\n"));
        for v in [no, nl, to, tl] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for row in &item.matrix.m {
            f32s(&mut out, row);
        }
        let p = &item.params;
        // Same rule as codegen: the owner's clip speed.
        let speed = scene
            .get(&item.owner)
            .and_then(|e| e.model.as_ref())
            .and_then(|m| m.anim.as_ref())
            .map_or(1.0, |a| a.speed);
        let mut flags = 0u32;
        if item.model.is_some() {
            flags |= 1;
        }
        if p.anim.is_some() {
            flags |= 2;
        }
        if p.double_sided {
            flags |= 4;
        }
        if p.texture.is_some() {
            flags |= 8;
        }
        if item.sound.is_some() {
            flags |= 16;
        }
        flags |= match p.shading {
            Shading::Gouraud => 0,
            Shading::Flat => 1,
            Shading::Unlit => 2,
        } << 8;
        let (shape_bits, abc) = match item.collider.map(|c| c.shape) {
            Some(Shape::Box { half }) => (1, [half.x, half.y, half.z]),
            Some(Shape::Sphere { radius }) => (2, [radius, 0.0, 0.0]),
            Some(Shape::Capsule { radius, height }) => (3, [radius, height, 0.0]),
            None => (0, [0.0; 3]),
        };
        flags |= shape_bits << 12;
        if item.collider.is_some_and(|c| c.trigger) {
            flags |= 1 << 15;
        }
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&item.model.as_ref().map_or(0, |(id, _)| id.0).to_le_bytes());
        rgba(&mut out, p.tint);
        rgba(&mut out, p.emissive);
        out.extend_from_slice(&p.anim.map_or(0, |a| a.clip.0).to_le_bytes());
        f32s(&mut out, &[if p.anim.is_some() { speed } else { 0.0 }]);
        f32s(&mut out, &abc);
        let off = item.collider.map_or(trino_core::Vec3::ZERO, |c| c.offset);
        f32s(&mut out, &[off.x, off.y, off.z]);
        out.extend_from_slice(&p.texture.map_or(0, |t| t.0).to_le_bytes());
        // LOD records live in the data table (4-aligned offsets are not
        // required: readers decode bytes, never cast).
        let lod_at = strings.len() as u32;
        for (d, m) in &item.lod {
            strings.extend_from_slice(&d.to_le_bytes());
            strings.extend_from_slice(&m.as_ref().map_or(0, |(id, _)| id.0).to_le_bytes());
        }
        out.extend_from_slice(&lod_at.to_le_bytes());
        out.extend_from_slice(&(item.lod.len() as u32).to_le_bytes());
        let snd = item.sound;
        out.extend_from_slice(&snd.map_or(0, |s| s.sound.0).to_le_bytes());
        f32s(
            &mut out,
            &[snd.map_or(0.0, |s| s.radius), snd.map_or(0.0, |s| s.volume)],
        );
        let (kind, a, b, half) = match item.vis {
            Some(trino_core::VisVolume::Cell { half }) => (1u8, 0, 0, half),
            Some(trino_core::VisVolume::Portal { half, a, b }) => (2, a, b, half),
            None => (0, 0, 0, trino_core::Vec3::ZERO),
        };
        out.extend_from_slice(&[item.cell, kind, a, b]);
        f32s(&mut out, &[half.x, half.y, half.z]);
        debug_assert_eq!(out.len() - start, TSCN_NODE_BYTES);
    }
    out.extend_from_slice(&strings);
    (out, resolved.warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnimComponent, ColliderComponent, Entity, ModelComponent, ShapeDef};
    use trino_core::scene::BakedScene;

    #[test]
    fn lod_levels_bake_sorted_and_select_by_distance() {
        let mut scene = Scene::new("t");
        scene
            .entities
            .push(Entity::new("tree").with_model(ModelComponent {
                lod: vec![
                    crate::LodDef {
                        distance: 40.0,
                        path: None,
                    },
                    crate::LodDef {
                        distance: 15.0,
                        path: Some("models/tree-far".into()),
                    },
                ],
                ..ModelComponent::new("models/tree")
            }));
        let (blob, _) = to_tscn(&scene, &Library::default());
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let n = BakedScene::parse(blob).unwrap().find("tree").unwrap();
        assert_eq!(n.lod.len(), 2);
        assert_eq!(n.lod.get(0).unwrap().distance, 15.0, "sorted");
        let eye = |z: f32| trino_core::Vec3::new(0.0, 0.0, z);
        let id = trino_core::ModelId::from_path;
        assert_eq!(n.model_from(eye(5.0)), Some(id("models/tree")));
        assert_eq!(n.model_from(eye(20.0)), Some(id("models/tree-far")));
        assert_eq!(n.model_from(eye(50.0)), None);
        // The editor-side resolve agrees.
        let item = &crate::resolve(&scene, &Library::default(), 0.0).items[0];
        assert_eq!(item.model_from(eye(20.0)), Some(id("models/tree-far")));
    }

    #[test]
    fn round_trips_through_the_core_reader() {
        let mut scene = Scene::new("t");
        scene.environment.clear = [1, 2, 3];
        let mut robot = Entity::new("robot")
            .at([1.0, 2.0, 3.0])
            .with_model(ModelComponent {
                anim: Some(AnimComponent {
                    clip: "walk".into(),
                    speed: 1.5,
                }),
                ..ModelComponent::new("models/robot")
            });
        robot.tags = vec!["npc".into(), "friendly".into()];
        robot.sound = Some(crate::SoundComponent {
            path: "sounds/hum".into(),
            radius: 6.0,
            volume: 0.5,
        });
        robot.collider = Some(ColliderComponent {
            shape: ShapeDef::Capsule {
                radius: 0.4,
                height: 1.8,
            },
            offset: [0.0, 0.9, 0.0],
            trigger: false,
        });
        let mut parent = Entity::new("group");
        parent
            .children
            .push(Entity::new("marker").at([0.0, 5.0, 0.0]));
        scene.entities.extend([robot, parent]);
        let (blob, warnings) = to_tscn(&scene, &Library::default());
        assert!(warnings.is_empty(), "{warnings:?}");
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let baked = BakedScene::parse(blob).unwrap();
        assert_eq!(baked.clear(), Color::rgb(1, 2, 3));
        assert_eq!(baked.node_count(), 3);
        let r = baked.find("robot").unwrap();
        assert!(r.has_tag("npc") && r.has_tag("friendly"));
        assert_eq!(r.position(), trino_core::Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(
            r.model,
            Some(trino_core::ModelId::from_path("models/robot"))
        );
        assert_eq!(r.anim, Some((trino_core::ClipId::from_name("walk"), 1.5)));
        let c = r.collider.unwrap();
        assert_eq!(
            c.shape,
            Shape::Capsule {
                radius: 0.4,
                height: 1.8
            }
        );
        assert!(r.lod.is_empty());
        let snd = r.sound.unwrap();
        assert_eq!(snd.sound, trino_core::SoundId::from_path("sounds/hum"));
        assert_eq!((snd.radius, snd.volume), (6.0, 0.5));
        let m = baked.find("group/marker").unwrap();
        assert!(m.sound.is_none());
        assert!(m.model.is_none() && m.tags.is_empty());
        assert_eq!(m.position().y, 5.0);
    }

    #[test]
    fn cells_and_portals_bake_and_cull() {
        use crate::VisComponent;
        use trino_core::{Camera3, Cells, NO_CELL, Vec2, Vec3, VisVolume};
        let mut scene = Scene::new("t");
        // The doorway comes first: portal names resolve after the walk.
        let mut door = Entity::new("door").at([4.0, 1.0, 2.0]);
        door.vis = Some(VisComponent::Portal {
            size: [0.2, 2.0, 1.0],
            a: "hall".into(),
            b: "house/kitchen".into(),
        });
        scene.entities.push(door);
        let mut bad = Entity::new("bad").at([0.0, 0.0, 0.0]);
        bad.vis = Some(VisComponent::Portal {
            size: [1.0; 3],
            a: "hall".into(),
            b: "attic".into(),
        });
        scene.entities.push(bad);
        let mut house = Entity::new("house");
        for (name, x) in [("hall", 2.0), ("kitchen", 6.0)] {
            let mut room = Entity::new(name).at([x, 1.5, 2.0]);
            room.vis = Some(VisComponent::Cell {
                size: [4.0, 3.0, 4.0],
            });
            room.children.push(
                Entity::new("lamp")
                    .at([0.0, 1.0, 0.0])
                    .with_model(ModelComponent::new("models/lamp")),
            );
            house.children.push(room);
        }
        scene.entities.push(house);
        scene
            .entities
            .push(Entity::new("sky").with_model(ModelComponent::new("models/sky")));

        let (blob, warnings) = to_tscn(&scene, &Library::default());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("attic"), "{warnings:?}");
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let baked = BakedScene::parse(blob).unwrap();
        let nodes: Vec<_> = baked.nodes().collect();
        let node = |n: &str| nodes.iter().find(|x| x.name == n).unwrap();
        assert_eq!(
            node("door").vis,
            Some(VisVolume::Portal {
                half: Vec3::new(0.1, 1.0, 0.5),
                a: 0,
                b: 1
            })
        );
        assert_eq!(node("bad").vis, None);
        assert_eq!(node("house/hall/lamp").cell, 0);
        assert_eq!(node("house/kitchen/lamp").cell, 1);
        assert_eq!(node("sky").cell, NO_CELL);

        let cells: Cells<4> = Cells::from_nodes(&nodes);
        assert_eq!((cells.cell_count(), cells.portal_count()), (2, 1));
        let cam = |target: Vec3| Camera3 {
            eye: Vec3::new(1.0, 1.0, 2.0),
            target,
            fov_y: 1.0,
        };
        let screen = Vec2::new(320.0, 240.0);
        let through = cells.visible(&cam(Vec3::new(8.0, 1.0, 2.0)), screen);
        let away = cells.visible(&cam(Vec3::new(-8.0, 1.0, 2.0)), screen);
        assert_eq!((through & 3, away & 3), (3, 1));
        // The editor-side resolve and codegen agree on the same numbers.
        let r = crate::resolve(&scene, &Library::default(), 0.0);
        let lamp = r
            .items
            .iter()
            .find(|i| i.name == "house/kitchen/lamp")
            .unwrap();
        assert_eq!(lamp.cell, 1);
        let away_cam = cam(Vec3::new(-8.0, 1.0, 2.0));
        assert_eq!(r.visible_cells(&away_cam, screen) & 3, 1);
        let no_cells = crate::resolve(&Scene::new("e"), &Library::default(), 0.0);
        assert_eq!(no_cells.visible_cells(&away_cam, screen), u64::MAX);
        let (code, _) = crate::codegen::to_rust(&scene, &Library::default());
        assert!(
            code.contains(
                "vis: Some(VisVolume::Portal { half: Vec3::new(0.1, 1.0, 0.5), a: 0, b: 1 })"
            ),
            "{code}"
        );
        assert!(code.contains("cell: NO_CELL"));
    }
}
