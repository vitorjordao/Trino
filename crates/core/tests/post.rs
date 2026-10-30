//! The post stack's portable half (issue #64): glows draw a billboard where
//! the target has no bloom (the N64, the 3DS) and nothing on the Full tier
//! unless asked, from game code, from `draw_nodes` (exported and baked
//! scenes) and from live entities.

mod common;

use common::{Call, Recorder};
use trino_core::scene::{Lods, SceneNode, Tags, draw_nodes};
use trino_core::{
    Caps, Color, Entity, EntityStore, Glow, GlowMode, Mat34, ModelParams, Renderer, Vec3, draw_glow,
};

fn billboards(r: &Recorder) -> Vec<(Vec3, f32)> {
    r.calls
        .iter()
        .filter_map(|c| match c {
            Call::Billboard {
                id, pos, params, ..
            } if *id == Glow::SPRITE => Some((*pos, params.size.x)),
            _ => None,
        })
        .collect()
}

fn node(glow: Option<Glow>) -> SceneNode {
    let mut matrix = Mat34::IDENTITY;
    matrix.m[1][3] = 2.0;
    SceneNode {
        name: "lamp",
        tags: Tags::NONE,
        matrix,
        model: None,
        params: ModelParams::DEFAULT,
        anim: None,
        collider: None,
        lod: Lods::NONE,
        sound: None,
        cell: trino_core::NO_CELL,
        vis: None,
        glow,
        character: None,
        semantic: [0; 4],
        outline: None,
    }
}

#[test]
fn glows_draw_below_the_full_tier_from_code_scenes_and_entities() {
    let glow = Glow::new(0.5, Color::rgb(255, 200, 100));
    for (caps, drawn) in [(Caps::N64, true), (Caps::N3DS, true), (Caps::PC, false)] {
        let mut r = Recorder::new();
        r.caps = caps;
        r.begin_frame(Color::BLACK);
        assert_eq!(draw_glow(&mut r, &glow, Vec3::ZERO), drawn, "{caps:?}");
        // An `Always` glow draws on every tier.
        let always = Glow {
            mode: GlowMode::Always,
            ..glow
        };
        assert!(draw_glow(&mut r, &always, Vec3::ZERO));
        // Nothing for a zero radius or a transparent color.
        assert!(!draw_glow(
            &mut r,
            &Glow::new(0.0, Color::WHITE),
            Vec3::ZERO
        ));
        assert!(!draw_glow(
            &mut r,
            &Glow {
                color: Color::rgba(255, 255, 255, 0),
                ..always
            },
            Vec3::ZERO
        ));
        let bb = billboards(&r);
        assert_eq!(bb.len(), 1 + drawn as usize, "{caps:?}");
        assert!(bb.iter().all(|b| b.1 == 1.0), "side = 2 × radius");

        // draw_nodes: at the node's world position, under the root.
        let mut r2 = Recorder::new();
        r2.caps = caps;
        r2.begin_frame(Color::BLACK);
        let mut root = Mat34::IDENTITY;
        root.m[0][3] = 1.0;
        draw_nodes(
            &mut r2,
            &[node(Some(glow)), node(None)],
            &root,
            0.0,
            Vec3::ZERO,
        );
        let want: Vec<(Vec3, f32)> = if drawn {
            vec![(Vec3::new(1.0, 2.0, 0.0), 1.0)]
        } else {
            Vec::new()
        };
        assert_eq!(billboards(&r2), want, "{caps:?}");

        // Live entities copy the node's glow and draw it.
        let mut store: EntityStore<4> = EntityStore::new();
        store.spawn(Entity::from_node(&node(Some(glow)))).unwrap();
        let mut r3 = Recorder::new();
        r3.caps = caps;
        r3.begin_frame(Color::BLACK);
        store.draw(&mut r3, &Mat34::IDENTITY, 0.0, Vec3::ZERO);
        assert_eq!(billboards(&r3).len(), drawn as usize, "{caps:?}");
    }
}
