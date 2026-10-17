//! A scene that uses every authored component, plus the library it
//! resolves against — shared by the scene crate's integration tests.

#![allow(dead_code)] // each test binary uses a different subset

use trino_scene::{
    AnimComponent, ColliderComponent, Entity, Library, LodDef, MaterialDef, ModelComponent,
    Override, Prefab, PrefabInstance, Scene, ShadingDef, ShapeDef, SoundComponent, SpriteComponent,
    VisComponent,
};

pub fn model(path: &str) -> ModelComponent {
    ModelComponent::new(path)
}

pub fn library() -> Library {
    let mut lib = Library::default();
    lib.materials.insert(
        "materials/stone".into(),
        MaterialDef {
            tint: [200, 100, 50, 255],
            shading: ShadingDef::Flat,
            emissive: [1, 2, 3],
            double_sided: true,
            texture: Some("sprites/rock".into()),
            ..MaterialDef::default()
        },
    );
    lib.materials.insert(
        "materials/water".into(),
        MaterialDef {
            tint: [255, 255, 255, 200],
            texture: Some("sprites/water".into()),
            water: Some(trino_scene::WaterDef {
                flow_speed: (0.0, -0.3),
                wave_amplitude: 0.05,
                ..trino_scene::WaterDef::default()
            }),
            ..MaterialDef::default()
        },
    );
    lib.materials.insert(
        "materials/glow".into(),
        MaterialDef {
            shading: ShadingDef::Unlit,
            emissive: [255, 200, 0],
            ..MaterialDef::default()
        },
    );
    let mut tree = Entity::new("tree");
    let mut trunk = Entity::new("trunk").with_model(model("models/trunk"));
    trunk.children.push(
        Entity::new("leaves")
            .at([0.0, 2.0, 0.0])
            .with_model(model("models/leaves")),
    );
    tree.children.push(trunk);
    lib.prefabs.insert("prefabs/tree".into(), Prefab::new(tree));
    lib.model_bounds
        .insert("models/crate".into(), ([-0.5, 0.0, -0.5], [0.5, 1.0, 0.5]));
    lib
}

/// Every component kind, once at least.
pub fn scene() -> Scene {
    let mut s = Scene::new("fixture");
    let env = &mut s.environment;
    env.clear = [10, 20, 30];
    env.camera.eye = [0.0, 5.0, -10.0];
    env.camera.target = [0.0, 0.0, 0.0];
    env.camera.fov = 50.0;
    env.light.dir = [0.0, -1.0, 0.0];
    env.light.color = [250, 240, 230];
    env.light.ambient = 0.3;
    env.light.ambient_color = [100, 110, 120];
    env.fog = Some(trino_scene::FogDef {
        color: [50, 60, 70],
        near: 5.0,
        far: 40.0,
    });

    let mut ground = Entity::new("ground").with_model(ModelComponent {
        material: Some("materials/stone".into()),
        tint: Some([128, 255, 255, 255]),
        ..model("models/ground")
    });
    ground.tags = vec!["floor".into(), "static".into()];
    ground.collider = Some(ColliderComponent {
        shape: ShapeDef::Box {
            size: [10.0, 1.0, 10.0],
        },
        offset: [0.0, -0.5, 0.0],
        trigger: false,
    });
    s.entities.push(ground);

    let mut hero = Entity::new("hero")
        .at([1.0, 0.0, 2.0])
        .with_model(ModelComponent {
            anim: Some(AnimComponent {
                clip: "walk".into(),
                speed: 1.5,
            }),
            ..model("models/hero")
        });
    hero.transform.rot = [0.0, 90.0, 0.0];
    hero.transform.scale = [2.0, 2.0, 2.0];
    hero.collider = Some(ColliderComponent {
        shape: ShapeDef::Capsule {
            radius: 0.4,
            height: 1.8,
        },
        ..ColliderComponent::default()
    });
    hero.children.push(
        Entity::new("hat")
            .at([0.0, 1.0, 0.0])
            .with_model(ModelComponent {
                material: Some("materials/glow".into()),
                lod: vec![
                    LodDef {
                        distance: 30.0,
                        path: None,
                        ..Default::default()
                    },
                    LodDef {
                        distance: 10.0,
                        path: Some("models/hat-far".into()),
                        ..Default::default()
                    },
                ],
                ..model("models/hat")
            }),
    );
    s.entities.push(hero);

    s.entities.push(
        Entity::new("pond")
            .at([0.0, -0.2, 4.0])
            .with_model(ModelComponent {
                material: Some("materials/water".into()),
                ..model("models/pond")
            }),
    );

    let mut coin = Entity::new("coin")
        .at([3.0, 0.0, 0.0])
        .with_model(model("models/coin"));
    coin.collider = Some(ColliderComponent {
        shape: ShapeDef::Sphere { radius: 0.5 },
        trigger: true,
        ..ColliderComponent::default()
    });
    coin.sound = Some(SoundComponent {
        radius: 4.0,
        volume: 0.8,
        ..SoundComponent::new("sounds/chime")
    });
    s.entities.push(coin);

    let mut crate_ = Entity::new("crate")
        .at([-2.0, 0.0, 0.0])
        .with_model(model("models/crate"));
    crate_.collider = Some(ColliderComponent::default()); // Auto
    s.entities.push(crate_);

    let mut house = Entity::new("house").at([20.0, 2.0, 0.0]);
    house.vis = Some(VisComponent::Cell {
        size: [8.0, 4.0, 8.0],
    });
    house
        .children
        .push(Entity::new("lamp").with_model(model("models/lamp")));
    s.entities.push(house);
    let mut yard = Entity::new("yard").at([28.0, 2.0, 0.0]);
    yard.vis = Some(VisComponent::Cell {
        size: [8.0, 4.0, 8.0],
    });
    s.entities.push(yard);
    // The portal comes before... nothing: cells are named, order-free.
    let mut door = Entity::new("door").at([24.0, 1.0, 0.0]);
    door.vis = Some(VisComponent::Portal {
        size: [0.2, 2.0, 1.0],
        a: "house".into(),
        b: "yard".into(),
    });
    s.entities.push(door);

    let mut spawn = Entity::new("spawn");
    spawn.tags.push("spawn".into());
    s.entities.push(spawn);

    let mut tree = Entity::new("tree-1").at([-5.0, 0.0, 0.0]);
    let mut inst = PrefabInstance::new("prefabs/tree");
    inst.overrides.push(Override {
        target: "trunk/leaves".into(),
        tint: Some([10, 200, 10, 255]),
        ..Override::default()
    });
    tree.prefab = Some(inst);
    s.entities.push(tree);

    let mut label = Entity::new("label").at([8.0, 8.0, 0.0]);
    label.sprite = Some(SpriteComponent::new("sprites/label"));
    s.entities.push(label);

    let mut hidden = Entity::new("hidden").with_model(model("models/ghost"));
    hidden.visible = false;
    s.entities.push(hidden);

    s.entities.push(Entity::new("say \"hi\" \\o/"));
    s
}
