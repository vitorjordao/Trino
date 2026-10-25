//! Animation graphs end to end (issue #65): RON → `check` → `TANG` → the
//! runtime (`trino_core::anim`), on clips whose facts the test states
//! (durations, loops, events, root tracks), where the right answers are
//! known.

use trino_core::anim::{AnimGraph, AnimGraphInstance, Contribution, EventId, GraphError};
use trino_core::{AnimRef, ClipId};
use trino_scene::anim::{self, AnimGraphDef, ClipFacts, CondDef, From, Level, ScriptDef, StateDef};

fn events(list: &[(f32, &str)]) -> Vec<u8> {
    let mut b = (list.len() as u32).to_le_bytes().to_vec();
    for (t, n) in list {
        b.extend_from_slice(&t.to_le_bytes());
        b.extend_from_slice(&EventId::from_name(n).0.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
    }
    b
}

/// A root track moving `speed` m/s forward at `fps` over `duration`.
fn root(duration: f32, speed: f32) -> Vec<u8> {
    let fps = 10.0f32;
    let frames = (duration * fps).round() as usize + 1;
    let mut b = fps.to_le_bytes().to_vec();
    b.extend_from_slice(&(frames as u32).to_le_bytes());
    for f in 0..frames {
        let t = f as f32 / fps;
        for v in [0.0, speed * t, 0.0] {
            b.extend_from_slice(&f32::to_le_bytes(v));
        }
    }
    b
}

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

/// idle 2 s, walk 1 s (root 0.8 m/s, steps at 0 and 0.5), run 0.6 s
/// (root 2 m/s), hop 1 s one-shot (an event at 0.4).
fn clips() -> Vec<ClipFacts> {
    vec![
        clip("idle", 2.0, true),
        ClipFacts {
            events: Some(events(&[(0.0, "foot_l"), (0.5, "foot_r")])),
            root: Some(root(1.0, 0.8)),
            ..clip("walk", 1.0, true)
        },
        ClipFacts {
            events: Some(events(&[(0.0, "foot_l"), (0.3, "foot_r")])),
            root: Some(root(0.6, 2.0)),
            ..clip("run", 0.6, true)
        },
        ClipFacts {
            events: Some(events(&[(0.4, "takeoff")])),
            ..clip("hop", 1.0, false)
        },
    ]
}

const GRAPH: &str = r#"
AnimGraph(
    model: "models/strider",
    params: { "speed": Float(0.0), "grounded": Bool(true), "hop": Trigger, "late": Trigger },
    states: {
        "locomotion": Blend1D(param: "speed", points: [(0.0, "idle"), (0.8, "walk"), (2.0, "run")]),
        "hop": Clip(clip: "hop", root_motion: false, events: [(0.1, "crouch")]),
        "rest": Clip(clip: "idle"),
    },
    transitions: [
        (from: "locomotion", to: "hop", when: And([Trigger("hop"), Param("grounded")]), blend: 0.08),
        (from: "hop", to: "locomotion", when: Finished, blend: 0.2),
        (from: Any, to: "rest", when: Not(Param("grounded")), blend: 0.0),
        (from: "rest", to: "locomotion", when: And([Param("grounded"), After(0.25)])),
    ],
)
"#;

fn graph() -> (AnimGraphDef, Vec<u8>) {
    let def = AnimGraphDef::from_ron(GRAPH).unwrap();
    let tang = anim::to_tang(&def, &clips(), &|_| true).unwrap();
    (def, tang)
}

fn run(inst: &mut AnimGraphInstance, g: &AnimGraph, secs: f32) {
    let n = (secs * 60.0).round() as usize;
    for _ in 0..n {
        inst.update(g, 1.0 / 60.0);
    }
}

#[test]
fn the_text_round_trips_and_keeps_its_order() {
    let (def, _) = graph();
    let again = AnimGraphDef::from_ron(&def.to_ron()).unwrap();
    assert_eq!(again, def);
    // Every optional field set round-trips too (written bare).
    let mut full = def.clone();
    full.start = Some("rest".into());
    full.layout = vec![("hop".into(), (10.0, 20.0))];
    if let StateDef::Blend1D {
        wrap, speed_param, ..
    } = &mut full.states[0].1
    {
        *wrap = Some(360.0);
        *speed_param = Some("speed".into());
    }
    if let StateDef::Clip { looping, .. } = &mut full.states[1].1 {
        *looping = Some(true);
    }
    let text = full.to_ron();
    assert!(
        text.contains("start: \"rest\"") && !text.contains("Some("),
        "{text}"
    );
    assert_eq!(AnimGraphDef::from_ron(&text).unwrap(), full);
    let names: Vec<&str> = def.states.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["locomotion", "hop", "rest"]);
    assert_eq!(def.transitions[2].from, From::Any);
    // "*" means any state too.
    let star = GRAPH.replace("from: Any", "from: \"*\"");
    assert_eq!(AnimGraphDef::from_ron(&star).unwrap(), def);
    // Unknown fields are errors, not silently dropped on the next save.
    let typo = GRAPH.replace("blend: 0.08", "blnd: 0.08");
    assert!(AnimGraphDef::from_ron(&typo).is_err());
    // A state named twice is an error.
    let twice = GRAPH.replace("\"rest\": Clip", "\"hop\": Clip");
    assert!(
        AnimGraphDef::from_ron(&twice)
            .unwrap_err()
            .contains("written twice")
    );
}

#[test]
fn check_names_what_is_wrong() {
    let (def, _) = graph();
    assert!(anim::check(&def, Some(&clips()), &|_| true).is_empty());
    let mut bad = def.clone();
    bad.states.push((
        "lost".into(),
        StateDef::Clip {
            clip: "swim".into(),
            looping: Some(false),
            speed: 1.0,
            speed_param: Some("grounded".into()),
            root_motion: true,
            events: vec![(3.0, "late".into()), (0.0, "sfx:nope".into())],
        },
    ));
    if let StateDef::Blend1D { points, .. } = &mut bad.states[0].1 {
        points.swap(0, 1);
    }
    bad.transitions.push(anim::TransitionDef {
        from: From::State("ghost".into()),
        to: "hop".into(),
        when: CondDef::Greater("hop".into(), 1.0),
        blend: 0.1,
    });
    bad.transitions.push(anim::TransitionDef {
        from: From::State("hop".into()),
        to: "rest".into(),
        when: CondDef::Trigger("speed".into()),
        blend: 0.1,
    });
    let issues = anim::check(&bad, Some(&clips()), &|s| s != "nope");
    let text: Vec<String> = issues.iter().map(|i| i.to_string()).collect();
    let has = |s: &str| text.iter().any(|t| t.contains(s));
    assert!(
        has("state \"lost\": clip `swim` is not in models/strider"),
        "{text:#?}"
    );
    assert!(has("speed_param `grounded` must be a Float parameter"));
    assert!(has("event `sfx:nope`: no sound `sounds/nope`"));
    assert!(has("points must be in increasing order"));
    assert!(has("transition 4: from `ghost`: no such state"));
    assert!(has("`hop` is a Trigger, use Trigger(..)"));
    assert!(has("Trigger(`speed`) needs a Trigger parameter"));
    assert!(has("warning: state \"lost\": unreachable"));
    // A one-shot state without an exit holds its last frame forever.
    let mut stuck = def.clone();
    stuck.transitions.truncate(1);
    let w = anim::check(&stuck, Some(&clips()), &|_| true);
    assert!(
        w.iter()
            .any(|i| i.level == Level::Warning && i.message.contains("no exit")),
        "{w:?}"
    );
    // Events past a clip's end, against the clip's real length.
    let mut late = def.clone();
    if let StateDef::Clip { events, .. } = &mut late.states[1].1 {
        events.push((1.5, "late".into()));
    }
    let e = anim::to_tang(&late, &clips(), &|_| true).unwrap_err();
    assert!(e.contains("past the end of clip `hop` (1.000 s)"), "{e}");
}

#[test]
fn a_trigger_fires_one_transition_once() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let hop = g.param_named("hop").unwrap();
    let late = g.param_named("late").unwrap();
    let mut i = AnimGraphInstance::new(&g);
    i.update(&g, 1.0 / 60.0);
    assert_eq!(i.state(), 0);
    // A trigger nothing listens to is gone after one update.
    i.set_trigger(late);
    i.update(&g, 1.0 / 60.0);
    assert_eq!(i.param(late), 0.0);
    i.set_trigger(hop);
    i.update(&g, 1.0 / 60.0);
    assert_eq!(i.state(), 1, "hop");
    assert_eq!(i.param(hop), 0.0, "consumed");
    assert_eq!(i.transitions, 1);
    // Back after the one-shot clip ends, and a stale trigger can't re-fire.
    run(&mut i, &g, 1.1);
    assert_eq!(i.state(), 0);
    assert_eq!(i.transitions, 2);
    run(&mut i, &g, 0.5);
    assert_eq!(i.transitions, 2);
    // Not grounded: the hop is refused (And), and the Any transition cuts
    // to rest; grounded again after 0.25 s back to locomotion.
    let grounded = g.param_named("grounded").unwrap();
    i.set_bool(grounded, false);
    i.set_trigger(hop);
    i.update(&g, 1.0 / 60.0);
    assert_eq!(i.state(), 2, "rest");
    assert!(i.transition().is_none(), "blend 0 cuts");
    i.set_bool(grounded, true);
    run(&mut i, &g, 0.2);
    assert_eq!(i.state(), 2, "not before 0.25 s");
    run(&mut i, &g, 0.1);
    assert_eq!(i.state(), 0);
}

fn weights(i: &AnimGraphInstance, g: &AnimGraph) -> Vec<Contribution> {
    let mut c = [Contribution {
        clip: 0,
        time: 0.0,
        weight: 0.0,
    }; 4];
    let n = i.contributions(g, &mut c);
    c[..n].to_vec()
}

/// Each clip's weight in an `AnimRef`, as the pose mixes it.
fn ref_weights(a: AnimRef) -> Vec<(ClipId, f32)> {
    let mut out = vec![(a.clip, 1.0)];
    if let Some(b) = a.blend {
        out[0].1 = 1.0 - b.weight;
        out.push((b.clip, b.weight));
    }
    let o = a.over;
    if o.weight > 0.0 {
        for w in out.iter_mut() {
            w.1 *= 1.0 - o.weight;
        }
        out.push((o.clip, o.weight));
    }
    out
}

#[test]
fn blend_weights_sum_to_one_and_the_pose_mixes_them() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let speed = g.param_named("speed").unwrap();
    let mut i = AnimGraphInstance::new(&g);
    for k in 0..=50 {
        let v = k as f32 * 0.05 - 0.2;
        i.set_float(speed, v);
        i.update(&g, 1.0 / 60.0);
        let w = weights(&i, &g);
        let sum: f32 = w.iter().map(|c| c.weight).sum();
        assert!((sum - 1.0).abs() < 1e-5, "speed {v}: {w:?}");
        assert!(w.len() <= 2);
        let pose: f32 = ref_weights(i.anim(&g)).iter().map(|x| x.1).sum();
        assert!((pose - 1.0).abs() < 1e-5);
    }
    // At 1.4 m/s: halfway between walk (0.8) and run (2.0).
    i.set_float(speed, 1.4);
    i.update(&g, 1.0 / 60.0);
    let w = weights(&i, &g);
    assert_eq!(w.len(), 2);
    assert!((w[1].weight - 0.5).abs() < 1e-5);
    // Synchronized: both clips at the same fraction of their cycles.
    let (dw, dr) = (1.0, 0.6);
    assert!((w[0].time / dw - w[1].time / dr).abs() < 1e-4, "{w:?}");
    // Mid-transition out of the blend into the hop: three clips, still 1.
    i.set_trigger(g.param_named("hop").unwrap());
    i.update(&g, 1.0 / 60.0);
    i.update(&g, 0.02);
    let w = weights(&i, &g);
    assert_eq!(w.len(), 3, "{w:?}");
    let sum: f32 = w.iter().map(|c| c.weight).sum();
    assert!((sum - 1.0).abs() < 1e-5);
    let a = i.anim(&g);
    assert!(a.over.weight > 0.0);
    let mixed = ref_weights(a);
    for c in &w {
        let id = g.clip(c.clip).id;
        let m = mixed.iter().find(|x| x.0 == id).unwrap().1;
        assert!((m - c.weight).abs() < 1e-5, "{mixed:?} vs {w:?}");
    }
}

#[test]
fn events_fire_at_their_times_from_the_heavier_clip() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let speed = g.param_named("speed").unwrap();
    let mut i = AnimGraphInstance::new(&g);
    i.set_float(speed, 0.8); // walk only
    let mut log = Vec::new();
    for u in 0..120 {
        i.update(&g, 1.0 / 60.0);
        for e in i.events().iter() {
            log.push((u, e.id, e.time));
        }
    }
    // Two seconds of a 1 s walk: steps at 0, 0.5, 1 and 1.5 s (the 30th
    // update ends at 0.5 s; the clock's sum of 1/60s may land a hair
    // short, then the step is one update later).
    assert_eq!(log.len(), 4, "{log:?}");
    for (k, (u, id, t)) in log.iter().enumerate() {
        let ideal = if k == 0 { 0 } else { 30 * k - 1 };
        assert!((*u as usize).abs_diff(ideal) <= 1, "{log:?}");
        let (name, at) = if k % 2 == 0 {
            ("foot_l", 0.0)
        } else {
            ("foot_r", 0.5)
        };
        assert_eq!((*id, *t), (EventId::from_name(name), at));
    }
    // Blended walk and run: one clip steps, never both.
    i.set_float(speed, 1.2);
    let mut both = 0;
    for _ in 0..240 {
        i.update(&g, 1.0 / 60.0);
        both = both.max(i.events().len());
    }
    assert_eq!(both, 1);
    // A hop: its state event (crouch) and the clip's (takeoff), in order.
    i.set_trigger(g.param_named("hop").unwrap());
    i.update(&g, 1.0 / 60.0);
    let mut hop = Vec::new();
    for _ in 0..55 {
        i.update(&g, 1.0 / 60.0);
        hop.extend(i.events().iter().map(|e| e.id));
    }
    assert_eq!(
        hop,
        [EventId::from_name("crouch"), EventId::from_name("takeoff")]
    );
}

#[test]
fn root_motion_adds_up_and_a_state_can_turn_it_off() {
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let mut i = AnimGraphInstance::new(&g);
    i.set_float(g.param_named("speed").unwrap(), 0.8);
    let mut z = 0.0;
    for _ in 0..180 {
        i.update(&g, 1.0 / 60.0);
        z += i.root_motion().translation.z;
    }
    // Three cycles of 0.8 m.
    assert!((z - 2.4).abs() < 1e-3, "{z}");
    // The hop says root_motion: false: once its fade is over, nothing.
    i.set_trigger(g.param_named("hop").unwrap());
    i.update(&g, 1.0 / 60.0);
    run(&mut i, &g, 0.1);
    i.update(&g, 1.0 / 60.0);
    assert_eq!(i.root_motion().translation.z, 0.0);
}

#[test]
fn a_clip_state_plays_at_the_games_own_clock() {
    // The showcase's robots: one clip at rate 1 gives exactly the time the
    // game used to pass (the same additions), so its frames stay identical.
    let (_, tang) = graph();
    let g = AnimGraph::parse(&tang).unwrap();
    let mut i = AnimGraphInstance::new_in(&g, 2);
    // Not grounded: rest never leaves.
    i.set_bool(g.param_named("grounded").unwrap(), false);
    let mut time = 0.0f32;
    for k in 0..500 {
        let dt = if k % 7 == 0 {
            0.0
        } else {
            1.0 / 60.0 + k as f32 * 1e-6
        };
        time += dt;
        i.update(&g, dt);
        let a = i.anim(&g);
        assert_eq!(a, AnimRef::new(ClipId::from_name("idle"), time));
    }
}

#[test]
fn the_same_updates_give_the_same_digest() {
    let (def, tang) = graph();
    let script = ScriptDef::from_ron(
        r#"Script(duration: 4.0, steps: [
            (at: 0.5, set: {"speed": 0.6}),
            (at: 1.5, set: {"speed": 1.8}),
            (at: 2.0, trigger: ["hop"]),
            (at: 3.2, set: {"grounded": 0.0}),
            (at: 3.5, set: {"grounded": 1.0, "nope": 2.0}),
        ])"#,
    )
    .unwrap();
    let names: Vec<String> = ["idle", "walk", "run", "hop"].map(String::from).to_vec();
    let a = anim::simulate(&def, &tang, &names, &script, 30).unwrap();
    let b = anim::simulate(&def, &tang, &names, &script, 30).unwrap();
    assert_eq!(a, b);
    let path: Vec<&str> = a.transitions.iter().map(|t| t.to.as_str()).collect();
    assert_eq!(
        path,
        ["hop", "locomotion", "rest", "locomotion"],
        "{:#?}",
        a.transitions
    );
    assert_eq!(a.unknown, ["nope"]);
    assert!(a.events.iter().any(|e| e.name == "crouch"));
    assert!(a.root.1 > 1.0, "{:?}", a.root);
    assert_eq!(a.segments.first().unwrap().state, "locomotion");
    assert_eq!(a.segments.last().unwrap().to, 4.0);
}

#[test]
fn broken_blobs_are_refused_never_played() {
    let (_, tang) = graph();
    assert!(AnimGraph::parse(&tang).is_ok());
    assert_eq!(AnimGraph::parse(b"NOPE").unwrap_err(), GraphError::BadMagic);
    assert_eq!(
        AnimGraph::parse(&tang[..20]).unwrap_err(),
        GraphError::Truncated
    );
    let mut v2 = tang.clone();
    v2[4] = 2;
    assert_eq!(AnimGraph::parse(&v2).unwrap_err(), GraphError::Version(2));
    // Every truncation and every single-byte change either parses (and then
    // plays without a panic) or is refused.
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
                    i.set_float(p, 1.0);
                }
                for _ in 0..30 {
                    i.update(&g, 1.0 / 30.0);
                    let _ = i.anim(&g);
                }
            }
        }
    }
}
