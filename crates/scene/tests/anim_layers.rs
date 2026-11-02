//! Animation layers, 2D blend spaces and inertialized transitions end to
//! end (issue #112): RON → `check` → `TANG` v2 → the runtime
//! (`trino_core::anim`), on clips whose facts the test states. `TANG` v1
//! graphs keep baking and playing as #65 made them.

use trino_core::anim::{
    AnimGraph, AnimGraphInstance, AnimLayers, Contribution, JointMask, LayerMode, SampleRole,
};
use trino_core::{Caps, ClipId};
use trino_scene::anim::{self, AnimGraphDef, ClipFacts, Level, ScriptDef, StateDef, WeightDef};

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
        clip("back", 1.0, true),
        clip("left", 1.0, true),
        clip("right", 1.0, true),
        clip("run", 0.6, true),
        clip("aim", 1.0, true),
        clip("shoot", 0.5, false),
        clip("breathe", 3.0, true),
        clip("hop", 1.0, false),
    ]
}

fn joints() -> Vec<String> {
    [
        "hips", "torso", "head", "arm_l", "arm_r", "thigh_l", "shin_l", "foot_l", "thigh_r",
        "shin_r", "foot_r",
    ]
    .map(String::from)
    .to_vec()
}

const GRAPH: &str = r#"
AnimGraph(
    version: 2,
    model: "models/strider",
    params: {
        "x": Float(0.0), "y": Float(0.0), "aiming": Float(0.0),
        "fire": Trigger, "hop": Trigger,
    },
    states: {
        "move": Blend2D(x: "x", y: "y", points: [
            (0.0, 0.0, "idle"), (0.0, 1.0, "walk"), (0.0, -1.0, "back"),
            (-1.0, 0.0, "left"), (1.0, 0.0, "right"),
        ]),
        "hop": Clip(clip: "hop"),
    },
    transitions: [
        (from: "move", to: "hop", when: Trigger("hop"), blend: 0.2, mode: Inertial),
        (from: "hop", to: "move", when: Finished, blend: 0.2),
    ],
    layers: [
        (
            name: "upper",
            mask: ["torso", "head", "arm_l", "arm_r"],
            weight: "aiming",
            states: {
                "aim": Clip(clip: "aim"),
                "shoot": Clip(clip: "shoot"),
            },
            transitions: [
                (from: "aim", to: "shoot", when: Trigger("fire"), blend: 0.05),
                (from: "shoot", to: "aim", when: Finished, blend: 0.1),
            ],
        ),
        (
            name: "breath",
            mode: Additive,
            weight: 0.5,
            states: { "breathe": Clip(clip: "breathe") },
        ),
    ],
)
"#;

fn graph() -> (AnimGraphDef, Vec<u8>) {
    let def = AnimGraphDef::from_ron(GRAPH).unwrap();
    let j = joints();
    let tang = anim::to_tang_with(&def, &clips(), Some(&j), &|_| true).unwrap();
    (def, tang)
}

fn run(inst: &mut AnimGraphInstance, g: &AnimGraph, secs: f32) {
    for _ in 0..(secs * 60.0).round() as usize {
        inst.update(g, 1.0 / 60.0);
    }
}

fn weights(i: &AnimGraphInstance, g: &AnimGraph, l: usize) -> Vec<(ClipId, f32)> {
    let mut c = [Contribution::NONE; 6];
    let n = i.layer_contributions(g, l, &mut c);
    c[..n]
        .iter()
        .map(|x| (g.clip(x.clip).id, x.weight))
        .collect()
}

#[test]
fn layered_text_round_trips_and_bakes_to_v2() {
    let (def, tang) = graph();
    assert!(def.uses_v2());
    assert_eq!(AnimGraphDef::from_ron(&def.to_ron()).unwrap(), def);
    assert_eq!(def.layers[0].weight, WeightDef::Param("aiming".into()));
    assert_eq!(def.layers[1].weight, WeightDef::Value(0.5));
    let g = AnimGraph::parse(&tang).unwrap();
    assert_eq!(u32::from_le_bytes(tang[4..8].try_into().unwrap()), 2);
    assert_eq!(g.layer_count(), 2);
    assert_eq!(g.states_of(0), (0, 2));
    let up = g.layer(1);
    // torso 1, head 2, arm_l 3, arm_r 4.
    assert_eq!(up.mask, JointMask(0b11110));
    assert_eq!(up.mode, LayerMode::Override);
    assert_eq!(up.states, (2, 4));
    assert!(up.weight_param.is_some());
    let breath = g.layer(2);
    assert_eq!(
        (breath.mode, breath.mask),
        (LayerMode::Additive, JointMask::ALL)
    );
    assert!(g.transition_inertial(0) && !g.transition_inertial(1));

    // A graph with none of it still bakes to v1.
    let mut plain = def.clone();
    plain.layers.clear();
    plain.states[0].1 = StateDef::Clip {
        clip: "idle".into(),
        looping: None,
        speed: 1.0,
        speed_param: None,
        root_motion: true,
        events: Vec::new(),
    };
    plain.transitions[0].mode = Default::default();
    let v1 = anim::to_tang(&plain, &clips(), &|_| true).unwrap();
    assert_eq!(u32::from_le_bytes(v1[4..8].try_into().unwrap()), 1);
}

#[test]
fn blend_2d_weights_sum_to_one_and_follow_the_triangles() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let (x, y) = (g.param_named("x").unwrap(), g.param_named("y").unwrap());
    let at = |px: f32, py: f32| {
        let mut i = AnimGraphInstance::new(&g);
        i.set_float(x, px);
        i.set_float(y, py);
        let w = weights(&i, &g, 0);
        let sum: f32 = w.iter().map(|c| c.1).sum();
        assert!((sum - 1.0).abs() < 1e-5, "({px}, {py}): {w:?}");
        w
    };
    let get = |w: &[(ClipId, f32)], n: &str| {
        w.iter()
            .find(|c| c.0 == ClipId::from_name(n))
            .map_or(0.0, |c| c.1)
    };
    // On a point: that clip alone.
    assert_eq!(at(0.0, 1.0), vec![(ClipId::from_name("walk"), 1.0)]);
    // Inside the idle / walk / right triangle: barycentric.
    let w = at(0.25, 0.5);
    assert!((get(&w, "walk") - 0.5).abs() < 1e-5, "{w:?}");
    assert!((get(&w, "right") - 0.25).abs() < 1e-5, "{w:?}");
    assert!((get(&w, "idle") - 0.25).abs() < 1e-5, "{w:?}");
    // Continuous across an edge (x = 0 splits two triangles).
    for k in 0..=10 {
        let py = k as f32 / 10.0;
        let a = at(-1e-4, py);
        let b = at(1e-4, py);
        for n in ["idle", "walk", "left", "right"] {
            assert!((get(&a, n) - get(&b, n)).abs() < 1e-3, "{n} at y {py}");
        }
    }
    // Outside the hull: the nearest edge (walk–right's middle).
    let w = at(2.0, 2.0);
    assert!((get(&w, "walk") - 0.5).abs() < 1e-4 && (get(&w, "right") - 0.5).abs() < 1e-4);
    // The cycle is the weighted durations: phase-synced like Blend1D.
    let mut i = AnimGraphInstance::new(&g);
    i.set_float(x, 0.0);
    i.set_float(y, 0.5);
    run(&mut i, &g, 0.75);
    // idle 2 s and walk 1 s at 50/50: a 1.5 s cycle, half done.
    assert!((i.clock() - 0.5).abs() < 0.02, "{}", i.clock());
}

#[test]
fn layers_play_their_own_machines_on_their_masks() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let aiming = g.param_named("aiming").unwrap();
    let fire = g.param_named("fire").unwrap();
    let mut i = AnimGraphInstance::new(&g);
    run(&mut i, &g, 0.1);
    // Aiming at 0: the upper layer is out of the pose; the breath (0.5) is in.
    let p = i.pose(&g);
    assert_eq!(p.layer_count, 2);
    assert_eq!(p.layers[1].mode, LayerMode::Additive);
    assert!((p.layers[1].weight - 0.5).abs() < 1e-6);
    i.set_float(aiming, 0.8);
    run(&mut i, &g, 0.1);
    let p = i.pose(&g);
    assert_eq!(p.layer_count, 3);
    assert_eq!(p.layers[1].mask, JointMask(0b11110));
    assert!((p.layers[1].weight - 0.8).abs() < 1e-6);
    let upper: Vec<_> = p.samples().iter().filter(|s| s.layer == 1).collect();
    assert_eq!(upper.len(), 1);
    assert_eq!(upper[0].clip, ClipId::from_name("aim"));
    // The layer's trigger moves the layer, not the base.
    let base = i.state();
    i.set_trigger(fire);
    i.update(&g, 1.0 / 60.0);
    assert_eq!(i.state(), base);
    assert_eq!(
        i.layer_state(1),
        g.states_of(1).0 + 1,
        "upper went to shoot"
    );
    // It fades, then plays `shoot` to its end and comes back.
    assert!(i.layer_transition(1).is_some());
    run(&mut i, &g, 0.7);
    assert_eq!(i.layer_state(1), g.states_of(1).0);
    // The weight is clamped to 0..1.
    i.set_float(aiming, 7.0);
    run(&mut i, &g, 0.02);
    assert_eq!(i.pose(&g).layers[1].weight, 1.0);
}

#[test]
fn an_inertialized_transition_plays_the_destination_whole_and_decays() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let (y, hop) = (g.param_named("y").unwrap(), g.param_named("hop").unwrap());
    let mut i = AnimGraphInstance::new(&g);
    i.set_float(y, 1.0);
    run(&mut i, &g, 0.3);
    i.set_trigger(hop);
    i.update(&g, 1.0 / 60.0);
    // No crossfade: `hop` alone in the base, the frozen walk as `From`.
    assert!(i.transition().is_none());
    assert_eq!(weights(&i, &g, 0), vec![(ClipId::from_name("hop"), 1.0)]);
    let d0 = i.inertia(0).unwrap();
    assert!(d0 > 0.99);
    let p = i.pose(&g);
    let from: Vec<_> = p
        .samples()
        .iter()
        .filter(|s| s.layer == 0 && s.role == SampleRole::From)
        .collect();
    let to: Vec<_> = p
        .samples()
        .iter()
        .filter(|s| s.layer == 0 && s.role == SampleRole::To)
        .collect();
    assert_eq!((from.len(), to.len()), (1, 1));
    assert_eq!(from[0].clip, ClipId::from_name("walk"));
    assert_eq!((to[0].clip, to[0].time), (ClipId::from_name("hop"), 0.0));
    assert!((p.layers[0].decay - d0).abs() < 1e-6);
    // It decays to nothing over its 0.2 s, monotonically.
    let mut last = d0;
    for _ in 0..11 {
        i.update(&g, 1.0 / 60.0);
        let d = i.inertia(0).unwrap_or(0.0);
        assert!(d <= last);
        last = d;
    }
    run(&mut i, &g, 0.05);
    assert!(i.inertia(0).is_none());
    assert!(
        i.pose(&g)
            .samples()
            .iter()
            .all(|s| s.role == SampleRole::Pose)
    );
}

#[test]
fn every_tier_folds_the_same_pose_and_says_how() {
    let (def, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let mut i = AnimGraphInstance::new(&g);
    for (p, v) in [("x", 0.3), ("y", 0.6), ("aiming", 1.0)] {
        i.set(&g, p, v);
    }
    run(&mut i, &g, 0.2);
    i.set(&g, "hop", 1.0);
    i.update(&g, 1.0 / 60.0);
    let pose = i.pose(&g);
    // PC: whole. N64 base: an AnimRef's three clips, the same every time.
    let (pc, r) = pose.fold(Caps::PC.anim_layers);
    assert!(r.is_empty() && pc == pose);
    let (n64, r) = pose.fold(Caps::N64.anim_layers);
    assert!(n64.fits_ref() && r.layers == 2 && r.inertial == 1, "{r:?}");
    assert_eq!(i.anim(&g), pose.to_ref());
    let (x, _) = pose.fold(Caps::N64_EXPANDED.anim_layers);
    assert!(x.layer_count <= 2 && x.len <= 6);
    // `anim check --platform` says what each tier drops.
    assert!(anim::fold_report(&def, Caps::PC.anim_layers).is_empty());
    let notes = anim::fold_report(&def, Caps::N64.anim_layers);
    let text: Vec<String> = notes
        .iter()
        .map(|n| format!("{}: {}", n.at, n.message))
        .collect();
    assert!(
        text.iter()
            .any(|t| t.starts_with("layer \"upper\": dropped")),
        "{text:?}"
    );
    assert!(
        text.iter()
            .any(|t| t.starts_with("layer \"breath\": dropped")),
        "{text:?}"
    );
    assert!(text.iter().any(|t| t.contains("inertialized")), "{text:?}");
    assert!(text.iter().any(|t| t.starts_with("pose:")), "{text:?}");
    let x = anim::fold_report(
        &def,
        AnimLayers {
            layers: 2,
            clips: 6,
        },
    );
    assert!(x.iter().any(|n| n.at == "layer \"breath\""));
    assert!(!x.iter().any(|n| n.at == "layer \"upper\""));
}

#[test]
fn check_names_what_is_wrong_in_layers_and_2d_spaces() {
    let j = joints();
    let issues = |text: &str| -> Vec<String> {
        let def = AnimGraphDef::from_ron(text).unwrap();
        anim::check_with(&def, Some(&clips()), Some(&j), &|_| true)
            .into_iter()
            .map(|i| format!("{:?} {i}", i.level))
            .collect()
    };
    let has = |v: &[String], s: &str| v.iter().any(|t| t.contains(s));
    let bad = GRAPH
        .replace("\"head\", \"arm_l\"", "\"neck\", \"arm_l\"")
        .replace("(-1.0, 0.0, \"left\")", "(0.0, 2.0, \"left\")")
        .replace("(1.0, 0.0, \"right\")", "(0.0, -2.0, \"right\")")
        .replace("(0.0, 1.0, \"walk\")", "(0.0, 0.0, \"walk\")")
        .replace("blend: 0.2, mode: Inertial", "blend: 0.0, mode: Inertial")
        .replace("weight: \"aiming\"", "weight: \"fire\"");
    let v = issues(&bad);
    assert!(has(&v, "mask: no joint `neck`"), "{v:#?}");
    assert!(has(&v, "at the same place"), "{v:#?}");
    assert!(has(&v, "Inertial transition needs a blend"), "{v:#?}");
    assert!(has(&v, "weight `fire` must be a Float"), "{v:#?}");
    // All on one line.
    let line = GRAPH
        .replace("(-1.0, 0.0, \"left\")", "(0.0, 3.0, \"left\")")
        .replace("(1.0, 0.0, \"right\")", "(0.0, -3.0, \"right\")");
    assert!(has(&issues(&line), "all on one line"));
    // Two override layers on the same joints: a warning.
    let two = GRAPH.replace(
        "name: \"breath\",\n            mode: Additive,",
        "name: \"breath\",\n            mask: [\"head\"],",
    );
    let v = issues(&two);
    assert!(
        v.iter()
            .any(|t| t.starts_with("Warning") && t.contains("also overrides")),
        "{v:#?}"
    );
    assert!(!v.iter().any(|t| t.starts_with("Error")), "{v:#?}");
    // Masks without the model's joints can't be checked: an error, never
    // a silent all-joints mask.
    let def = AnimGraphDef::from_ron(GRAPH).unwrap();
    let v = anim::check(&def, Some(&clips()), &|_| true);
    assert!(
        v.iter()
            .any(|i| i.level == Level::Error && i.message.contains("joints are unknown"))
    );
}

#[test]
fn the_triangulation_is_delaunay_and_drops_crossings() {
    let pts = |v: &[(f32, f32)]| -> Vec<(f32, f32, String)> {
        v.iter().map(|(x, y)| (*x, *y, "c".to_string())).collect()
    };
    // A square: two triangles (its four corners are on one circle).
    let t = anim::triangulate(&pts(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)])).unwrap();
    assert_eq!(t.len(), 2);
    // Eight directions and the middle: eight triangles around it.
    let mut v = vec![(0.0, 0.0)];
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        v.push((a.cos(), a.sin()));
    }
    let t = anim::triangulate(&pts(&v)).unwrap();
    assert_eq!(t.len(), 8);
    assert!(t.iter().all(|t| t.contains(&0)));
}

#[test]
fn simulate_traces_each_layer() {
    let (def, tang) = graph();
    let names: Vec<String> = clips()
        .iter()
        .map(|_| String::new())
        .zip([
            "idle", "walk", "back", "left", "right", "run", "aim", "shoot", "breathe", "hop",
        ])
        .map(|(_, n)| n.to_string())
        .collect();
    let script = ScriptDef::from_ron(
        "Script(duration: 1.5, steps: [(at: 0.1, set: {\"aiming\": 1.0, \"y\": 1.0}), (at: 0.5, trigger: [\"fire\", \"hop\"])])",
    )
    .unwrap();
    let t = anim::simulate(&def, &tang, &names, &script, 6).unwrap();
    assert!(
        t.transitions
            .iter()
            .any(|x| x.layer.as_deref() == Some("upper") && x.to == "shoot")
    );
    assert!(
        t.transitions
            .iter()
            .any(|x| x.layer.is_none() && x.to == "hop" && x.inertial)
    );
    assert!(
        t.segments
            .iter()
            .any(|s| s.layer.as_deref() == Some("breath"))
    );
    let s = t
        .samples
        .iter()
        .find(|s| s.at > 0.3 && s.at < 0.45)
        .unwrap();
    assert_eq!(s.layers.len(), 2);
    assert_eq!(s.layers[0].clips[0].0, "aim");
    let text = ron::to_string(&t).unwrap();
    assert!(text.contains("\"upper\""));
}

#[test]
fn broken_v2_blobs_are_refused_never_played() {
    let (_, tang) = graph();
    for cut in 0..tang.len() {
        let _ = AnimGraph::parse(&tang[..cut]);
    }
    for at in 0..tang.len() {
        for x in [0x01u8, 0x80, 0xFF] {
            let mut b = tang.clone();
            b[at] ^= x;
            if let Ok(g) = AnimGraph::parse(&b) {
                let mut i = AnimGraphInstance::new(&g);
                for p in 0..g.param_count() {
                    i.set_float(p, if p % 2 == 0 { 1.0 } else { -3.0 });
                }
                for _ in 0..20 {
                    i.update(&g, 1.0 / 30.0);
                    let p = i.pose(&g);
                    let _ = (i.anim(&g), p.fold(AnimLayers::AREF), p.digest(), i.digest());
                }
            }
        }
    }
}

/// The #65 strider graph as main baked it: it parses as v1 and plays to the
/// digest it always did.
#[test]
fn the_v1_fixture_still_loads_and_plays() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../core/tests/fixtures/strider_v1.tang"
    ))
    .unwrap();
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 1);
    let g = AnimGraph::parse(&bytes).unwrap();
    assert_eq!(g.layer_count(), 0);
    let mut i = AnimGraphInstance::new(&g);
    let speed = g.param_named("speed").unwrap();
    for u in 0..120 {
        i.set_float(speed, if u < 60 { 0.8 } else { 2.0 });
        i.update(&g, 1.0 / 60.0);
        // No layers, no inertia: the pose is the AnimRef's.
        assert!(i.pose(&g).fits_ref());
        assert_eq!(i.pose(&g).to_ref().clip, i.anim(&g).clip);
    }
    assert_eq!(i.layer_count(), 0);
}
