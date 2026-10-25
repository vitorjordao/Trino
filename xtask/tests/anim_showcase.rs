//! The showcase's robots through their animation graph (issue #65,
//! `assets/anim/robot.anim.ron`): with the `anim/robot` blob the game plays
//! the walker and the greeter through `AnimGraphInstance`s; without it, at
//! its own clock as before. Every draw of both runs must carry the same
//! pose, frame after frame — so every golden that shows the robots (PC,
//! N64, 3DS) stays byte-identical — and a graph that plays the walk a hair
//! faster must not (the comparison sees the graph).

use std::path::PathBuf;

use trino_core::{
    AnimRef, BlobId, Caps, Color, Game, InputState, Material, ModelId, ModelParams, Platform,
    Renderer, SpriteId, SpriteParams, Transform3, Vec2,
};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// A platform whose only blob is the robots' graph.
struct Blobs(Option<&'static [u8]>);
impl Platform for Blobs {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn blob(&self, id: BlobId) -> Option<&'static [u8]> {
        (id == BlobId::from_path("anim/robot"))
            .then_some(self.0)
            .flatten()
    }
}

/// Every animated draw: (model, pose).
#[derive(Default)]
struct Rec(Vec<(ModelId, AnimRef)>);
impl Renderer for Rec {
    fn caps(&self) -> &Caps {
        &Caps::PC
    }
    fn begin_frame(&mut self, _: Color) {}
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn draw_model(&mut self, m: ModelId, _: &Transform3, _: Material, p: &ModelParams) {
        if let Some(a) = p.anim {
            self.0.push((m, a));
        }
    }
    fn end_frame(&mut self) {}
}

fn robot_graph(edit: impl Fn(String) -> String) -> &'static [u8] {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let manifest =
        trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let text = std::fs::read_to_string(root.join("assets/anim/robot.anim.ron")).unwrap();
    let def = trino_scene::anim::AnimGraphDef::from_ron(&edit(text)).unwrap();
    let tang = trino_asset_pipeline::anim_graph::bake(
        &root,
        &manifest,
        trino_asset_pipeline::Platform::Pc,
        &def,
    )
    .unwrap()
    .unwrap();
    Box::leak(tang.into_boxed_slice())
}

/// The animated draws of `frames` frames of the showcase, the hero walking
/// in a circle and the look paused for a while (the clock stops).
fn play(graph: Option<&'static [u8]>, frames: usize) -> (Vec<Vec<(ModelId, AnimRef)>>, usize) {
    let mut g = showcase::ShowcaseGame::new();
    let mut platform = Blobs(graph);
    let mut out = Vec::new();
    for f in 0..frames {
        let mut input = InputState::default();
        input.stick = Vec2::new((f as f32 * 0.05).sin(), (f as f32 * 0.05).cos());
        // Pause the animation clock (Start) for frames 200..260.
        if f == 200 || f == 260 {
            input.set(trino_core::Button::Start, true);
        }
        let dt = 1.0 / 60.0 + (f % 3) as f32 * 1e-4;
        if graph.is_some() {
            g.update_with(&mut platform, &input, &mut NullAudio, dt);
        } else {
            g.update(&input, &mut NullAudio, dt);
        }
        let mut r = Rec::default();
        g.render(&mut r);
        out.push(r.0);
    }
    (out, g.robot_instances().count())
}

#[test]
fn the_graph_draws_the_robots_exactly_as_before() {
    let frames = 600;
    let (old, none) = play(None, frames);
    assert_eq!(none, 0, "no blob: no instances");
    let (new, robots) = play(Some(robot_graph(|t| t)), frames);
    assert_eq!(robots, 2, "the walker and the greeter");
    let posed = old.iter().map(Vec::len).sum::<usize>();
    assert!(posed > frames, "the robots are drawn: {posed} posed draws");
    for (f, (a, b)) in old.iter().zip(&new).enumerate() {
        assert_eq!(a, b, "frame {f}");
    }

    // A graph that plays the walk 0.1 % faster is seen at once.
    let faster = robot_graph(|t| {
        t.replace(
            "\"walk\": Clip(clip: \"walk\")",
            "\"walk\": Clip(clip: \"walk\", speed: 1.001)",
        )
    });
    let (fast, _) = play(Some(faster), 120);
    assert!(
        old.iter().zip(&fast).any(|(a, b)| a != b),
        "a different graph must change the poses"
    );
}
