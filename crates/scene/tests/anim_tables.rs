//! Animation graphs as tables, crowd instances, synced layers and graphs
//! that serve several rigs (issue #138), end to end: RON → `TANG` → the
//! runtime (`trino_core::anim`), the blob against its decoded tables.

use trino_core::ClipId;
use trino_core::anim::{
    AnimGraph, AnimGraphInstance, GraphInstance, GraphSource, GraphTables, JointMask,
};
use trino_core::mem::Arena;
use trino_scene::anim::{self, AnimGraphDef, BoundModel, ClipFacts, Joint};

fn clip(name: &str, duration: f32, looping: bool) -> ClipFacts {
    ClipFacts {
        id: ClipId::from_name(name),
        duration,
        looping,
        events: None,
        root: None,
        feet: None,
    }
}

fn clips() -> Vec<ClipFacts> {
    vec![
        clip("idle", 2.0, true),
        clip("walk", 1.0, true),
        clip("run", 0.6, true),
        clip("jump", 0.8, false),
        clip("wave", 0.5, true),
        clip("carry", 0.4, true),
        clip("shrug", 0.7, false),
    ]
}

fn joints(names: &[&str]) -> Vec<Joint> {
    let parents: Vec<Option<usize>> = (0..names.len())
        .map(|i| if i == 0 { None } else { Some(0) })
        .collect();
    Joint::list(names, &parents)
}

/// A rig: hips, torso, two arms, two legs.
const RIG: [&str; 6] = ["hips", "torso", "arm_l", "arm_r", "leg_l", "leg_r"];
/// The same rig with a tray joint between the torso and the arms.
const RIG_TRAY: [&str; 7] = ["hips", "torso", "tray", "arm_l", "arm_r", "leg_l", "leg_r"];

/// Conditions that short-circuit (an `Or` of triggers, an `And` that fails
/// first, a trigger under a `Not`), and a layer whose transition reads a
/// trigger the base may have consumed in the same update.
const CONDS: &str = r#"
AnimGraph(
    version: 3,
    model: "models/rig",
    params: {
        "a": Trigger, "b": Trigger, "c": Trigger,
        "on": Bool(false), "x": Float(0.0),
    },
    states: {
        "idle": Clip(clip: "idle"),
        "walk": Clip(clip: "walk"),
        "run": Clip(clip: "run"),
        "jump": Clip(clip: "jump"),
    },
    transitions: [
        (from: "idle", to: "walk", when: Or([Trigger("a"), Trigger("b")]), blend: 0.1),
        (from: "walk", to: "run", when: And([Param("on"), Trigger("c")]), blend: 0.1),
        (from: "run", to: "idle", when: Not(Trigger("a")), blend: 0.2),
        (from: "*", to: "jump", when: Or([Greater("x", 0.5), Trigger("b"), Trigger("c")]), blend: 0.0),
        (from: "jump", to: "idle", when: Finished, blend: 0.1),
    ],
    layers: [
        (
            name: "upper",
            mask: ["torso", "arm_l", "arm_r"],
            states: { "rest": Clip(clip: "idle"), "wave": Clip(clip: "wave") },
            transitions: [
                (from: "rest", to: "wave", when: Trigger("b"), blend: 0.1),
                (from: "wave", to: "rest", when: Or([Trigger("a"), And([Less("x", 0.0), Trigger("c")])]), blend: 0.1),
            ],
        ),
    ],
)
"#;

fn bake(text: &str, rig: &[&str]) -> Vec<u8> {
    let def = AnimGraphDef::from_ron(text).unwrap();
    let j = joints(rig);
    anim::to_tang_with(&def, &clips(), Some(&j), &|_| true).unwrap()
}

fn tables<'s>(g: &AnimGraph<'s>, arena: &'s Arena) -> GraphTables<'s> {
    GraphTables::decode_in(g, arena).unwrap()
}

#[test]
fn short_circuited_conditions_consume_the_same_triggers() {
    let tang = bake(CONDS, &RIG);
    for seed in 1..40 {
        anim::compare_tables(&tang, seed, 3000).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    }
    let g = AnimGraph::parse(&tang).unwrap();
    let mut memory = vec![std::mem::MaybeUninit::uninit(); GraphTables::bytes_for(&g)];
    let arena = Arena::new(&mut memory);
    let t = tables(&g, &arena);
    // `a` and `b` in one update: the base's `Or` takes idle -> walk and
    // consumes both, so the layer's `Trigger("b")` doesn't fire after it.
    let (a, b) = (g.param_named("a").unwrap(), g.param_named("b").unwrap());
    let mut on_blob = AnimGraphInstance::new(&g);
    let mut on_tables = AnimGraphInstance::new(&t);
    for i in [&mut on_blob, &mut on_tables] {
        i.set_trigger(a);
        i.set_trigger(b);
    }
    on_blob.update(&g, 1.0 / 60.0);
    on_tables.update(&t, 1.0 / 60.0);
    for i in [&on_blob, &on_tables] {
        assert_eq!(i.state(), 1, "the base took the Or");
        assert_eq!(
            i.layer_state(1),
            4,
            "the layer saw no `b` left: still at rest"
        );
        assert_eq!(i.transitions, 1);
    }
    assert_eq!(on_blob.digest(), on_tables.digest());
}

#[test]
fn tables_take_what_bytes_for_says_and_hold_the_graph() {
    let tang = bake(CONDS, &RIG);
    let g = AnimGraph::parse(&tang).unwrap();
    let need = GraphTables::bytes_for(&g);
    let mut memory = vec![std::mem::MaybeUninit::uninit(); need];
    let arena = Arena::new(&mut memory);
    let t = tables(&g, &arena);
    assert!(arena.used() <= need, "{} > {need}", arena.used());
    assert!(t.bytes() <= arena.used());
    assert_eq!(t.state_count(), g.state_count());
    assert_eq!(t.layer_count(), 1);
    assert_eq!(t.layer(1).mask, g.layer(1).mask);
    assert_eq!(t.layer_named("upper"), Some(1));
    assert_eq!(t.state_clip(1), Some(ClipId::from_name("walk")));
    // Three triggers, nothing else.
    assert_eq!(t.triggers, 0b111);
    // A node per leaf and per And/Or/Not: 3 + 3 + 2 + 4 + 1 + 1 + 5.
    assert_eq!(t.conds.len(), 19);
    // A smaller arena is refused, not overrun.
    let mut small = vec![std::mem::MaybeUninit::uninit(); need / 2];
    let arena = Arena::new(&mut small);
    assert!(GraphTables::decode_in(&g, &arena).is_none());
}

/// A walking base with a carry layer synced to it, and one free.
const SYNCED: &str = r#"
AnimGraph(
    version: 3,
    model: "models/rig",
    params: { "speed": Float(1.0) },
    states: { "walk": Clip(clip: "walk", speed_param: "speed") },
    layers: [
        (
            name: "carry",
            mask: ["arm_l", "arm_r"],
            sync: true,
            states: { "carry": Clip(clip: "carry", speed: 0.3) },
        ),
        (
            name: "free",
            mask: ["torso"],
            states: { "carry": Clip(clip: "carry", speed: 0.3) },
        ),
    ],
)
"#;

#[test]
fn a_synced_layer_lands_on_the_bases_cycle() {
    let def = AnimGraphDef::from_ron(SYNCED).unwrap();
    assert!(def.uses_v3() && def.layers[0].sync && !def.layers[1].sync);
    assert_eq!(AnimGraphDef::from_ron(&def.to_ron()).unwrap(), def);
    let tang = bake(SYNCED, &RIG);
    assert_eq!(u32::from_le_bytes(tang[4..8].try_into().unwrap()), 3);
    let g = AnimGraph::parse(&tang).unwrap();
    assert!(g.layer(1).sync && !g.layer(2).sync);
    anim::compare_tables(&tang, 9, 2000).unwrap();
    let speed = g.param_named("speed").unwrap();
    let mut i = AnimGraphInstance::new(&g);
    let frac = |v: f32, d: f32| (v / d) - (v / d).floor();
    for k in 0..400 {
        // A walk whose pace changes, and uneven frames.
        i.set_float(speed, 0.5 + (k % 7) as f32 * 0.3);
        i.update(&g, if k % 5 == 0 { 0.05 } else { 1.0 / 60.0 });
        let base = frac(i.clock(), 1.0);
        let carry = frac(i.layer_clock(1), 0.4);
        assert!(
            (base - carry).abs() < 1e-4 || (base - carry).abs() > 1.0 - 1e-4,
            "{k}: {base} vs {carry}"
        );
    }
    // The free layer kept its own pace (0.3 × 0.4 s cycles).
    let free = frac(i.layer_clock(2), 0.4);
    assert!((free - frac(i.clock(), 1.0)).abs() > 0.01);
    // A game can move a layer itself.
    i.seek_layer(2, 0.1);
    assert_eq!(i.layer_clock(2), 0.1);
    i.seek_layer(9, 0.1);
    i.seek_layer(2, f32::NAN);
    assert_eq!(i.layer_clock(2), 0.1);
}

/// One graph for two rigs: the tray joint shifts the arms' indices.
const SERVED: &str = r#"
AnimGraph(
    version: 3,
    model: "models/rig",
    models: ["models/rig_tray"],
    params: { "w": Float(1.0) },
    states: { "walk": Clip(clip: "walk") },
    layers: [
        (
            name: "arms",
            mask: ["arm_l", "arm_r"],
            weight: "w",
            states: { "carry": Clip(clip: "carry") },
        ),
    ],
)
"#;

#[test]
fn one_graph_serves_rigs_that_differ_by_a_prop_joint() {
    let def = AnimGraphDef::from_ron(SERVED).unwrap();
    assert_eq!(def.model_paths(), ["models/rig_tray"]);
    let (rig, tray) = (joints(&RIG), joints(&RIG_TRAY));
    let tray_clips = clips();
    let bound = [BoundModel {
        path: "models/rig_tray".into(),
        clips: &tray_clips,
        joints: Some(&tray),
    }];
    let tang = anim::to_tang_bound(&def, &clips(), Some(&rig), &bound, &|_| true).unwrap();
    let g = AnimGraph::parse(&tang).unwrap();
    assert_eq!(g.bind_count(), 1);
    let own = trino_core::asset_id("models/rig");
    let other = trino_core::asset_id("models/rig_tray");
    assert_eq!(g.bind_model(0), other);
    assert_eq!(g.layer(1).mask, JointMask(0b1100));
    let t = g.for_model(other).unwrap();
    assert_eq!(t.model(), other);
    assert_eq!(t.layer(1).mask, JointMask(0b11000));
    assert_eq!(g.for_model(own).unwrap().layer(1).mask, JointMask(0b1100));
    assert!(g.for_model(trino_core::asset_id("models/other")).is_none());
    // The bound view decodes to tables with its masks, and plays the same
    // clips (only the masks differ).
    let mut memory = vec![std::mem::MaybeUninit::uninit(); GraphTables::bytes_for(&t)];
    let arena = Arena::new(&mut memory);
    let tt = GraphTables::decode_in(&t, &arena).unwrap();
    assert_eq!(tt.model, other);
    assert_eq!(GraphSource::layer(&tt, 1).mask, JointMask(0b11000));
    let mut a = AnimGraphInstance::new(&g);
    let mut b = AnimGraphInstance::new(&tt);
    for _ in 0..90 {
        a.update(&g, 1.0 / 60.0);
        b.update(&tt, 1.0 / 60.0);
    }
    assert_eq!(a.digest(), b.digest());
    let (pa, pb) = (a.pose(&g), b.pose(&tt));
    assert_eq!(pa.layers[1].mask, JointMask(0b1100));
    assert_eq!(pb.layers[1].mask, JointMask(0b11000));
    assert_eq!(pa.samples, pb.samples);

    // A bound model missing a clip, with another duration, or without a
    // mask's joint is refused at the bake.
    let short = [clip("walk", 1.0, true)];
    let mut long = clips();
    long[5].duration = 0.9;
    let legless = joints(&["hips", "torso", "tray"]);
    for (c, j, want) in [
        (&short[..], &tray, "has no clip `carry`"),
        (&long[..], &tray, "lasts 0.900 s"),
        (&tray_clips[..], &legless, "no joint `arm_l`"),
    ] {
        let b = [BoundModel {
            path: "models/rig_tray".into(),
            clips: c,
            joints: Some(j),
        }];
        let e = anim::to_tang_bound(&def, &clips(), Some(&rig), &b, &|_| true).unwrap_err();
        assert!(e.contains(want), "{e}");
    }
}

/// An inertialized hop in a graph played by an instance without inertia
/// slots crossfades over the same time.
const HOP: &str = r#"
AnimGraph(
    version: 2,
    model: "models/rig",
    params: { "hop": Trigger },
    states: { "walk": Clip(clip: "walk"), "jump": Clip(clip: "jump") },
    transitions: [
        (from: "walk", to: "jump", when: Trigger("hop"), blend: 0.3, mode: Inertial),
        (from: "jump", to: "walk", when: Finished, blend: 0.2),
    ],
)
"#;

#[test]
fn a_crowd_instance_is_small_and_crossfades_without_inertia_slots() {
    use std::mem::size_of;
    let tang = bake(HOP, &RIG);
    let g = AnimGraph::parse(&tang).unwrap();
    let hop = g.param_named("hop").unwrap();
    let mut full = AnimGraphInstance::new(&g);
    let mut crowd = GraphInstance::<1, 0, 0>::new(&g);
    for _ in 0..10 {
        full.update(&g, 1.0 / 60.0);
        crowd.update(&g, 1.0 / 60.0);
    }
    full.set_trigger(hop);
    crowd.set_trigger(hop);
    full.update(&g, 1.0 / 60.0);
    crowd.update(&g, 1.0 / 60.0);
    assert!(full.inertia(0).is_some() && full.transition().is_none());
    assert!(crowd.inertia(0).is_none());
    assert_eq!(
        crowd.transition().map(|t| t.0),
        Some(0),
        "a crossfade from walk"
    );
    assert!(crowd.events().is_empty());
    assert!(
        size_of::<GraphInstance<1, 0, 0>>() * 5 < size_of::<AnimGraphInstance>(),
        "{} vs {}",
        size_of::<GraphInstance<1, 0, 0>>(),
        size_of::<AnimGraphInstance>()
    );
}

/// `TRINO_DUMP_V3_SEED=<dir>` writes a v3 blob (a synced layer and a bound
/// model) where the fuzzer's seeds live (`fuzz/seeds/tang`).
#[test]
fn dump_a_v3_fuzz_seed_on_request() {
    let Ok(dir) = std::env::var("TRINO_DUMP_V3_SEED") else {
        return;
    };
    let text = SERVED.replace(
        "name: \"arms\",",
        "name: \"arms\",\n            sync: true,",
    );
    let def = AnimGraphDef::from_ron(&text).unwrap();
    let (rig, tray) = (joints(&RIG), joints(&RIG_TRAY));
    let c = clips();
    let bound = [BoundModel {
        path: "models/rig_tray".into(),
        clips: &c,
        joints: Some(&tray),
    }];
    let tang = anim::to_tang_bound(&def, &c, Some(&rig), &bound, &|_| true).unwrap();
    assert!(AnimGraph::parse(&tang).unwrap().layer(1).sync);
    let name = format!("{:08x}", trino_core::save::checksum(&tang));
    std::fs::write(std::path::Path::new(&dir).join(name), tang).unwrap();
}
