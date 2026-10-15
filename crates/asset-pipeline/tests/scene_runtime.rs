//! Authored scene → `TSCN` → `BakedScene` → `draw_nodes`, with the real
//! meshes the manifest bakes: the repo's showcase and interior scenes, as a
//! game loads them at runtime, drawn through a counting renderer that runs
//! the engine's software T&L on every draw.

use std::collections::HashMap;
use std::path::PathBuf;

use trino_asset_pipeline::{Manifest, Platform, bake_model, bake_scene, resolve_source};
use trino_core::render3d::tessellate;
use trino_core::scene::{draw_nodes, draw_nodes_in};
use trino_core::{
    BakedScene, Camera3, Caps, Cells, Color, Light, Mat34, Material, Mesh, ModelId, ModelParams,
    Renderer, SceneNode, SpriteId, SpriteParams, Transform3, Vec2,
};

const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// Every model of the repo manifest, baked for PC, by handle.
fn baked_models() -> HashMap<u32, Vec<u8>> {
    let root = assets();
    let manifest = Manifest::load(&root.join("manifest.toml")).unwrap();
    manifest
        .models
        .iter()
        .map(|(name, decl)| {
            let source = resolve_source(&root, Platform::Pc, &decl.file).unwrap();
            let blob = bake_model(&source, &decl.options()).unwrap();
            (trino_core::asset_id(&format!("models/{name}")), blob)
        })
        .collect()
}

/// A renderer that tessellates every model draw on the CPU and counts.
struct Counting {
    meshes: HashMap<u32, Vec<u8>>,
    camera: Camera3,
    light: Light,
    draws: Vec<ModelId>,
    unknown: Vec<ModelId>,
    tris: u32,
    caps: Caps,
}

impl Counting {
    fn new(meshes: HashMap<u32, Vec<u8>>) -> Self {
        Counting {
            meshes,
            camera: Camera3::default(),
            light: trino_core::DEFAULT_LIGHT,
            draws: Vec::new(),
            unknown: Vec::new(),
            tris: 0,
            caps: Caps::N64,
        }
    }

    fn reset(&mut self) {
        self.draws.clear();
        self.unknown.clear();
        self.tris = 0;
    }
}

impl Renderer for Counting {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _clear: Color) {}
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn set_camera(&mut self, camera: &Camera3) {
        self.camera = *camera;
    }
    fn set_light(&mut self, light: &Light) {
        self.light = *light;
    }
    fn draw_model(&mut self, model: ModelId, t: &Transform3, m: Material, p: &ModelParams) {
        self.draw_model_matrix(model, &t.matrix(), m, p);
    }
    fn draw_model_matrix(&mut self, model: ModelId, matrix: &Mat34, _: Material, p: &ModelParams) {
        self.draws.push(model);
        let Some(blob) = self.meshes.get(&model.0) else {
            self.unknown.push(model);
            return;
        };
        let mesh = Mesh::from_tmdl(blob).unwrap();
        let mut n = 0;
        tessellate(
            &mesh,
            matrix,
            &self.camera,
            &self.light,
            p,
            SCREEN,
            &mut |_| n += 1,
        );
        self.tris += n;
    }
    fn end_frame(&mut self) {}
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

#[test]
fn the_showcase_scene_loads_from_its_blob_and_draws_every_model() {
    let blob = bake_scene(&assets(), "scenes/showcase.scene.ron").unwrap();
    let scene = BakedScene::parse(leak(blob)).unwrap();
    let nodes: Vec<SceneNode> = scene.nodes().collect();
    assert!(nodes.len() > 10, "{} nodes", nodes.len());
    // Names are unique `/`-joined paths, and find() resolves suffixes.
    for n in &nodes {
        assert!(!n.name.is_empty());
        let last = n.name.rsplit('/').next().unwrap();
        assert!(scene.find(last).is_some(), "{}", n.name);
    }
    let camera = scene.camera();
    assert!(camera.fov_y > 0.1 && camera.fov_y < 3.0, "radians");

    let mut r = Counting::new(baked_models());
    r.set_camera(&camera);
    r.set_light(&scene.light());
    r.begin_frame(scene.clear());
    draw_nodes(&mut r, &nodes, &Mat34::IDENTITY, 0.0, camera.eye);
    r.end_frame();
    assert!(
        r.unknown.is_empty(),
        "scene uses unbaked models: {:?}",
        r.unknown
    );
    // One draw per model node whose LOD picks a model from the camera.
    let expected = nodes
        .iter()
        .filter(|n| n.model_from(camera.eye).is_some())
        .count();
    assert_eq!(r.draws.len(), expected);
    assert!(r.tris > 50, "the camera sees the level: {} tris", r.tris);

    // A root transform moves the whole level: pushed far behind the camera
    // nothing reaches the screen.
    let behind = (camera.eye - camera.target).normalized() * 500.0;
    let mut root = Mat34::IDENTITY;
    root.m[0][3] = behind.x;
    root.m[1][3] = behind.y;
    root.m[2][3] = behind.z;
    r.reset();
    draw_nodes(&mut r, &nodes, &root, 0.0, camera.eye);
    assert_eq!(r.tris, 0);
}

#[test]
fn the_interior_scene_culls_rooms_through_its_portals() {
    let blob = bake_scene(&assets(), "scenes/interior.scene.ron").unwrap();
    let scene = BakedScene::parse(leak(blob)).unwrap();
    let nodes: Vec<SceneNode> = scene.nodes().collect();
    let cells: Cells<16> = Cells::from_nodes(&nodes);
    assert_eq!(cells.cell_count(), 3);
    assert!(cells.portal_count() >= 2);

    let mut r = Counting::new(baked_models());
    let all = {
        draw_nodes(&mut r, &nodes, &Mat34::IDENTITY, 0.0, scene.camera().eye);
        assert!(r.unknown.is_empty());
        r.draws.len()
    };
    // Standing in the first room and facing away from the doorways, only
    // that room (and nodes outside every cell) is drawn.
    let eye = scene.camera().eye;
    let camera = Camera3 {
        eye,
        target: eye - (scene.camera().target - eye),
        ..scene.camera()
    };
    let visible = cells.visible(&camera, SCREEN);
    let home = cells.cell_of(eye).unwrap();
    assert_ne!(visible & (1 << home), 0, "the camera's own cell");
    r.reset();
    draw_nodes_in(&mut r, &nodes, &Mat34::IDENTITY, 0.0, eye, visible);
    let in_room = r.draws.len();
    assert!(in_room < all, "{in_room} of {all}");
    assert!(in_room > 0);
    // Every drawn node is in a visible cell or in none.
    let drawn_cells: Vec<u8> = nodes
        .iter()
        .filter(|n| n.model.is_some() && trino_core::vis::in_view(n.cell, visible))
        .map(|n| n.cell)
        .collect();
    assert_eq!(drawn_cells.len(), in_room);
}
