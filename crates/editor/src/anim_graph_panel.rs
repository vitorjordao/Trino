//! The Animation tab's **Graph** mode (issue #65): author and drive an
//! animation graph — `assets/anim/<name>.anim.ron`, the file `cargo xtask
//! anim check|simulate` reads and an agent writes.
//!
//! - a canvas of states (dragged around: the file's `layout`) and the
//!   transitions between them, the live state lit and the fade drawn;
//! - the parameters, driving a live `AnimGraphInstance` the preview poses
//!   (and carries by its root motion);
//! - the selected state (its clip, or a blend space's points with the
//!   weights now), the selected transition (from, to, blend, its condition
//!   as RON), add / remove / rename, the start state;
//! - the graph's `check` issues against the model's clips.
//!
//! Every edit saves the file at once; an outside edit to it (an agent, a
//! text editor) reloads here. The preview runs the same runtime the
//! consoles do (`trino_core::anim`), so what plays here plays there.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use trino_core::AnimRef;
use trino_core::anim::{AnimGraph, AnimGraphInstance, RootPose};
use trino_scene::anim::{
    self as sa, AnimGraphDef, ClipFacts, CondDef, From, Issue, Level, ParamDef, StateDef,
    TransitionDef,
};

use crate::theme::{self, c, icon};

/// The base's (`layer` 0) or a layer's states.
pub fn states_in(def: &AnimGraphDef, layer: usize) -> &Vec<(String, StateDef)> {
    match layer.checked_sub(1).and_then(|l| def.layers.get(l)) {
        Some(l) => &l.states,
        None => &def.states,
    }
}

pub fn states_in_mut(def: &mut AnimGraphDef, layer: usize) -> &mut Vec<(String, StateDef)> {
    match layer.checked_sub(1).filter(|&l| l < def.layers.len()) {
        Some(l) => &mut def.layers[l].states,
        None => &mut def.states,
    }
}

pub fn transitions_in(def: &AnimGraphDef, layer: usize) -> &Vec<TransitionDef> {
    match layer.checked_sub(1).and_then(|l| def.layers.get(l)) {
        Some(l) => &l.transitions,
        None => &def.transitions,
    }
}

pub fn transitions_in_mut(def: &mut AnimGraphDef, layer: usize) -> &mut Vec<TransitionDef> {
    match layer.checked_sub(1).filter(|&l| l < def.layers.len()) {
        Some(l) => &mut def.layers[l].transitions,
        None => &mut def.transitions,
    }
}

fn layout_in(def: &AnimGraphDef, layer: usize) -> &Vec<(String, (f32, f32))> {
    match layer.checked_sub(1).and_then(|l| def.layers.get(l)) {
        Some(l) => &l.layout,
        None => &def.layout,
    }
}

fn layout_in_mut(def: &mut AnimGraphDef, layer: usize) -> &mut Vec<(String, (f32, f32))> {
    match layer.checked_sub(1).filter(|&l| l < def.layers.len()) {
        Some(l) => &mut def.layers[l].layout,
        None => &mut def.layout,
    }
}

fn start_in_mut(def: &mut AnimGraphDef, layer: usize) -> &mut Option<String> {
    match layer.checked_sub(1).filter(|&l| l < def.layers.len()) {
        Some(l) => &mut def.layers[l].start,
        None => &mut def.start,
    }
}

fn start_index_in(def: &AnimGraphDef, layer: usize) -> usize {
    match layer.checked_sub(1).and_then(|l| def.layers.get(l)) {
        Some(l) => l.start_index(),
        None => def.start_index(),
    }
}

/// What is selected on the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sel {
    State(usize),
    Transition(usize),
}

/// A graph open for editing and its live instance.
pub struct GraphEdit {
    pub name: String,
    pub file: PathBuf,
    pub def: AnimGraphDef,
    /// The text last written or read (an outside edit differs from it).
    saved: String,
    checked: Instant,
    pub issues: Vec<Issue>,
    /// The baked graph and its instance (`None` while the graph has errors).
    tang: Option<Vec<u8>>,
    pub inst: Option<AnimGraphInstance>,
    /// Root motion the preview has been carried by (wrapped to stay in view).
    pub carried: RootPose,
    pub selected: Option<Sel>,
    /// The machine the canvas shows: 0 the base, `k` the graph's layer
    /// `k - 1` (issue #112).
    pub layer: usize,
    /// The model's joint names (layer masks), and its rest-pose joints
    /// (the mask picker's skeleton): set by the Animation tab.
    pub joints: Vec<String>,
    pub rest: Vec<(trino_core::Vec3, Option<usize>)>,
    /// The mask is picked in the 3D preview (issue #136) instead of on the
    /// 2D skeleton; the preview's click lands in `mask_pick` (joint,
    /// subtree).
    pub mask_in_3d: bool,
    pub mask_pick: Option<(usize, bool)>,
    /// Where the preview drew each joint last frame (panel points; `None`
    /// behind the camera): tests and hooks click them.
    pub mask_points: Vec<Option<egui::Pos2>>,
    /// The 2D blend space editor's dragged point.
    drag2d: Option<usize>,
    /// The fold report of the previewed tier.
    pub folds: Vec<sa::FoldNote>,
    cond_text: String,
    cond_error: Option<String>,
    cond_for: Option<usize>,
    /// Name the next added state gets.
    pub new_name: String,
    new_param: String,
    new_kind: usize,
    /// Rebake and reset the instance at the next frame.
    dirty: bool,
    /// The clip facts the last bake used (model generation).
    baked_for: Option<u64>,
}

impl GraphEdit {
    pub fn open(root: &std::path::Path, name: &str) -> Result<GraphEdit, String> {
        let file = sa::graph_file(root, name);
        let saved =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let def = AnimGraphDef::from_ron(&saved).map_err(|e| format!("{}: {e}", file.display()))?;
        Ok(GraphEdit {
            name: name.to_string(),
            file,
            def,
            saved,
            checked: Instant::now(),
            issues: Vec::new(),
            tang: None,
            inst: None,
            carried: RootPose::IDENTITY,
            selected: None,
            layer: 0,
            joints: Vec::new(),
            rest: Vec::new(),
            mask_in_3d: false,
            mask_pick: None,
            mask_points: Vec::new(),
            drag2d: None,
            folds: Vec::new(),
            cond_text: String::new(),
            cond_error: None,
            cond_for: None,
            new_name: "state".into(),
            new_param: "param".into(),
            new_kind: 0,
            dirty: true,
            baked_for: None,
        })
    }

    /// Write the file (and remember it, so the reload check ignores it).
    pub fn save(&mut self) -> Result<(), String> {
        let text = self.def.to_ron();
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.file, &text).map_err(|e| format!("{}: {e}", self.file.display()))?;
        self.saved = text;
        self.dirty = true;
        Ok(())
    }

    /// Reload an outside edit (checked twice a second). `true` when it did.
    pub fn poll_file(&mut self) -> bool {
        if self.checked.elapsed().as_secs_f32() < 0.5 {
            return false;
        }
        self.checked = Instant::now();
        let Ok(text) = std::fs::read_to_string(&self.file) else {
            return false;
        };
        if text == self.saved {
            return false;
        }
        self.saved = text.clone();
        match AnimGraphDef::from_ron(&text) {
            Ok(def) => {
                self.def = def;
                self.dirty = true;
                if let Some(Sel::State(i)) = self.selected
                    && i >= states_in(&self.def, self.layer).len()
                {
                    self.selected = None;
                }
                if let Some(Sel::Transition(i)) = self.selected
                    && i >= transitions_in(&self.def, self.layer).len()
                {
                    self.selected = None;
                }
                self.cond_for = None;
                true
            }
            Err(e) => {
                self.issues = vec![Issue {
                    level: Level::Error,
                    at: "file".into(),
                    message: e,
                }];
                self.tang = None;
                self.inst = None;
                false
            }
        }
    }

    /// The model's joints as masks resolve them (names and parents, from
    /// the rest skeleton).
    pub fn joint_list(&self) -> Vec<sa::Joint> {
        self.joints
            .iter()
            .enumerate()
            .map(|(j, n)| sa::Joint {
                name: n.clone(),
                parent: self.rest.get(j).and_then(|r| r.1).filter(|&p| p < j),
            })
            .collect()
    }

    /// Check and bake against the model's clips; a new instance keeps the
    /// parameters it can.
    pub fn rebake(&mut self, clips: Option<&[ClipFacts]>, sounds: &[String], generation: u64) {
        if !self.dirty && self.baked_for == Some(generation) {
            return;
        }
        self.dirty = false;
        self.baked_for = Some(generation);
        let has = |s: &str| sounds.iter().any(|x| x == &format!("sounds/{s}"));
        let list = self.joint_list();
        let joints = (!list.is_empty()).then_some(list.as_slice());
        self.issues = sa::check_with(&self.def, clips, joints, &has);
        let old = self.inst.take().zip(self.tang.take());
        self.tang = clips.and_then(|c| sa::to_tang_with(&self.def, c, joints, &has).ok());
        if let Some(g) = self.tang.as_deref().and_then(|b| AnimGraph::parse(b).ok()) {
            let mut inst = AnimGraphInstance::new(&g);
            // Keep the values the panel had set.
            if let Some((prev, bytes)) = old
                && let Ok(pg) = AnimGraph::parse(&bytes)
            {
                for i in 0..pg.param_count() {
                    let (id, kind, _) = pg.param_info(i);
                    if kind != trino_core::anim::ParamKind::Trigger
                        && let Some(j) = g.param(id)
                    {
                        inst.set_float(j, prev.param(i));
                    }
                }
            }
            self.inst = Some(inst);
        }
    }

    pub fn graph(&self) -> Option<AnimGraph<'_>> {
        self.tang.as_deref().and_then(|b| AnimGraph::parse(b).ok())
    }

    /// Advance the live instance (`dt` seconds) and the carried root.
    pub fn tick(&mut self, dt: f32) {
        let Some(bytes) = self.tang.clone() else {
            return;
        };
        let Ok(g) = AnimGraph::parse(&bytes) else {
            return;
        };
        if let Some(inst) = &mut self.inst {
            inst.update(&g, dt);
            let d = inst.root_motion();
            self.carried = self.carried.then(RootPose {
                x: d.translation.x,
                z: d.translation.z,
                yaw: d.yaw,
            });
            // Keep the preview in view: back to the start past 4 m.
            if self.carried.x * self.carried.x + self.carried.z * self.carried.z > 16.0 {
                self.carried = RootPose::IDENTITY;
            }
        }
    }

    /// The pose to draw now.
    pub fn pose(&self) -> Option<AnimRef> {
        let g = self.graph()?;
        Some(self.inst.as_ref()?.anim(&g))
    }

    /// The whole layered pose (issue #112) for a graph that has layers, a
    /// 2D space or inertia: the preview pushes it into its renderer's pose
    /// table, which folds it to the previewed look's tier. `None`: the
    /// `AnimRef` is the pose.
    pub fn layered_pose(&self) -> Option<trino_core::anim::AnimPose> {
        if !self.def.uses_v2() {
            return None;
        }
        let g = self.graph()?;
        Some(self.inst.as_ref()?.pose(&g))
    }

    /// Set a parameter of the live instance by name.
    pub fn set_param(&mut self, name: &str, v: f32) -> bool {
        let Some(bytes) = self.tang.clone() else {
            return false;
        };
        let (Ok(g), Some(inst)) = (AnimGraph::parse(&bytes), self.inst.as_mut()) else {
            return false;
        };
        inst.set(&g, name, v)
    }

    /// A live parameter's value by name.
    pub fn param(&self, name: &str) -> Option<f32> {
        let g = self.graph()?;
        Some(self.inst.as_ref()?.param(g.param_named(name)?))
    }

    /// The live state's index in the shown machine (base or layer).
    pub fn live_state(&self) -> Option<usize> {
        let g = self.graph()?;
        let lo = g.states_of(self.layer).0;
        self.inst
            .as_ref()
            .and_then(|i| i.layer_state(self.layer).checked_sub(lo))
    }

    /// The shown machine mid-crossfade: (from, progress), local indices.
    pub fn live_fade(&self) -> Option<(usize, f32)> {
        let g = self.graph()?;
        let lo = g.states_of(self.layer).0;
        let (f, k) = self.inst.as_ref()?.layer_transition(self.layer)?;
        Some((f.checked_sub(lo)?, k))
    }

    /// Where state `i` is drawn (its `layout` entry, else a grid).
    pub fn pos(&self, i: usize) -> Pos2 {
        let name = &states_in(&self.def, self.layer)[i].0;
        match layout_in(&self.def, self.layer)
            .iter()
            .find(|(n, _)| n == name)
        {
            Some((_, (x, y))) => Pos2::new(*x, *y),
            None => Pos2::new(24.0 + (i % 3) as f32 * 210.0, 36.0 + (i / 3) as f32 * 96.0),
        }
    }

    fn set_pos(&mut self, i: usize, p: Pos2) {
        let name = states_in(&self.def, self.layer)[i].0.clone();
        let v = (p.x.round(), p.y.round());
        match layout_in_mut(&mut self.def, self.layer)
            .iter_mut()
            .find(|(n, _)| *n == name)
        {
            Some(e) => e.1 = v,
            None => layout_in_mut(&mut self.def, self.layer).push((name, v)),
        }
    }

    /// Rename state `i` everywhere (transitions, start, layout).
    pub fn rename_state(&mut self, i: usize, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty()
            || states_in(&self.def, self.layer)
                .iter()
                .any(|(n, _)| n == to)
        {
            return Err(format!("`{to}`: empty or already a state"));
        }
        let from = std::mem::replace(
            &mut states_in_mut(&mut self.def, self.layer)[i].0,
            to.to_string(),
        );
        for t in transitions_in_mut(&mut self.def, self.layer) {
            if t.from == From::State(from.clone()) {
                t.from = From::State(to.to_string());
            }
            if t.to == from {
                t.to = to.to_string();
            }
        }
        if start_in_mut(&mut self.def, self.layer).as_deref() == Some(from.as_str()) {
            *start_in_mut(&mut self.def, self.layer) = Some(to.to_string());
        }
        for (n, _) in layout_in_mut(&mut self.def, self.layer) {
            if *n == from {
                *n = to.to_string();
            }
        }
        Ok(())
    }

    /// Remove state `i` and every transition to or from it.
    pub fn remove_state(&mut self, i: usize) {
        let (name, _) = states_in_mut(&mut self.def, self.layer).remove(i);
        self.def
            .transitions
            .retain(|t| t.to != name && t.from != From::State(name.clone()));
        layout_in_mut(&mut self.def, self.layer).retain(|(n, _)| *n != name);
        if start_in_mut(&mut self.def, self.layer).as_deref() == Some(name.as_str()) {
            *start_in_mut(&mut self.def, self.layer) = None;
        }
        self.selected = None;
    }

    /// Add a state (a clip, or a blend space over the first float
    /// parameter); returns its index.
    pub fn add_state(&mut self, blend: bool, clips: &[String]) -> usize {
        self.add_state_kind(if blend { 1 } else { 0 }, clips)
    }

    /// A float parameter named `name`, made when missing.
    fn float_param(&mut self, name: &str) -> String {
        if self.def.param(name).is_none() {
            self.def.params.push((name.into(), ParamDef::Float(0.0)));
        }
        name.to_string()
    }

    /// Mark the graph for a rebake (the model's joints changed).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Add a state of `kind` (0 clip, 1 a 1D blend space, 2 a 2D one over
    /// `x` and `y`: three clips in a triangle); returns its index.
    pub fn add_state_kind(&mut self, kind: usize, clips: &[String]) -> usize {
        let mut name = self.new_name.trim().to_string();
        if name.is_empty() {
            name = "state".into();
        }
        let base = name.clone();
        let mut k = 2;
        while states_in(&self.def, self.layer)
            .iter()
            .any(|(n, _)| *n == name)
        {
            name = format!("{base}-{k}");
            k += 1;
        }
        let clip = clips.first().cloned().unwrap_or_default();
        let blend = kind == 1;
        let state = if kind == 2 {
            let (x, y) = (self.float_param("x"), self.float_param("y"));
            let at = |i: usize| clips.get(i).or(clips.first()).cloned().unwrap_or_default();
            StateDef::Blend2D {
                x,
                y,
                points: vec![(0.0, 0.0, at(0)), (1.0, 0.0, at(1)), (0.0, 1.0, at(2))],
                speed: 1.0,
                speed_param: None,
                root_motion: true,
            }
        } else if blend {
            let param = self
                .def
                .params
                .iter()
                .find(|(_, p)| matches!(p, ParamDef::Float(_)))
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| {
                    self.def.params.push(("speed".into(), ParamDef::Float(0.0)));
                    "speed".into()
                });
            StateDef::Blend1D {
                param,
                points: clips
                    .iter()
                    .take(2)
                    .enumerate()
                    .map(|(i, c)| (i as f32, c.clone()))
                    .collect(),
                wrap: None,
                speed: 1.0,
                speed_param: None,
                root_motion: true,
            }
        } else {
            StateDef::Clip {
                clip,
                looping: None,
                speed: 1.0,
                speed_param: None,
                root_motion: true,
                events: Vec::new(),
            }
        };
        states_in_mut(&mut self.def, self.layer).push((name, state));
        let i = states_in(&self.def, self.layer).len() - 1;
        self.selected = Some(Sel::State(i));
        i
    }

    /// Add a transition from state `from` to the next state, on `Finished`.
    pub fn add_transition(&mut self, from: usize) -> usize {
        let n = states_in(&self.def, self.layer).len();
        let to = (from + 1) % n.max(1);
        let names = states_in(&self.def, self.layer);
        let (from, to) = (names[from].0.clone(), names[to].0.clone());
        transitions_in_mut(&mut self.def, self.layer).push(TransitionDef {
            from: From::State(from),
            to,
            when: CondDef::Finished,
            blend: 0.15,
            mode: Default::default(),
        });
        let i = transitions_in(&self.def, self.layer).len() - 1;
        self.selected = Some(Sel::Transition(i));
        i
    }
}

/// A short label for a condition (the canvas's arrows).
pub fn cond_label(c: &CondDef) -> String {
    match c {
        CondDef::Always => "always".into(),
        CondDef::Trigger(p) => format!("{p}!"),
        CondDef::Finished => "finished".into(),
        CondDef::Param(p) => p.clone(),
        CondDef::Not(x) => format!("not {}", cond_label(x)),
        CondDef::And(v) => v.iter().map(cond_label).collect::<Vec<_>>().join(" & "),
        CondDef::Or(v) => v.iter().map(cond_label).collect::<Vec<_>>().join(" | "),
        CondDef::Greater(p, v) => format!("{p} > {v}"),
        CondDef::Less(p, v) => format!("{p} < {v}"),
        CondDef::After(s) => format!("after {s} s"),
    }
}

/// A condition as the RON the transition editor shows.
fn cond_ron(c: &CondDef) -> String {
    ron::to_string(c).unwrap_or_default()
}

const BOX: Vec2 = Vec2::new(132.0, 38.0);

/// The canvas: states, transitions, the live state and the fade; drag a
/// state to move it, click to select. Returns whether the layout moved.
pub fn canvas(ui: &mut egui::Ui, rect: Rect, g: &mut GraphEdit) -> bool {
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, c::BG0);
    // A faint grid.
    let mut x = rect.min.x;
    while x < rect.max.x {
        p.line_segment(
            [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)],
            (1.0, c::BG1),
        );
        x += 24.0;
    }
    let origin = rect.min.to_vec2();
    let n = states_in(&g.def, g.layer).len();
    let boxes: Vec<Rect> = (0..n)
        .map(|i| Rect::from_min_size(g.pos(i) + origin, BOX))
        .collect();
    let any = Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::new(44.0, 20.0));
    let live = g.live_state();
    let fading = g.live_fade();
    // Transitions: a line between box edges, an arrowhead, its condition.
    let mut hit: Option<Sel> = None;
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    for (ti, t) in transitions_in(&g.def, g.layer).iter().enumerate() {
        let Some(to) = states_in(&g.def, g.layer)
            .iter()
            .position(|(nm, _)| *nm == t.to)
        else {
            continue;
        };
        let a = match &t.from {
            From::Any => any.center(),
            From::State(f) => match states_in(&g.def, g.layer)
                .iter()
                .position(|(nm, _)| nm == f)
            {
                Some(i) => boxes[i].center(),
                None => continue,
            },
        };
        let b = boxes[to].center();
        // Two transitions between the same states sit apart.
        let d = (b - a).normalized();
        let side = Vec2::new(-d.y, d.x) * 7.0;
        let (a, b) = (a + side, b + side);
        let end = edge(boxes[to], a, b);
        let selected = g.selected == Some(Sel::Transition(ti));
        let active = fading.is_some_and(|(from, _)| {
            matches!(&t.from, From::State(f) if states_in(&g.def, g.layer).get(from).is_some_and(|s| s.0 == *f))
                && Some(to) == live
        });
        let col = if selected {
            c::ACCENT_TEXT
        } else if active {
            c::PLAY
        } else {
            c::TEXT_FAINT
        };
        p.line_segment([a, end], Stroke::new(if selected { 2.5 } else { 1.5 }, col));
        let head = end - d * 9.0;
        let n2 = Vec2::new(-d.y, d.x) * 4.5;
        p.add(egui::Shape::convex_polygon(
            vec![end, head + n2, head - n2],
            col,
            Stroke::NONE,
        ));
        let mid = a + (end - a) * 0.5;
        p.text(
            mid,
            egui::Align2::CENTER_CENTER,
            cond_label(&t.when),
            egui::FontId::proportional(10.5),
            col,
        );
        if let Some(pt) = pointer
            && hit.is_none()
            && seg_dist(pt, a, end) < 6.0
        {
            hit = Some(Sel::Transition(ti));
        }
    }
    if transitions_in(&g.def, g.layer)
        .iter()
        .any(|t| t.from == From::Any)
    {
        p.rect_filled(any, 10.0, c::BG3);
        p.text(
            any.center(),
            egui::Align2::CENTER_CENTER,
            "any",
            egui::FontId::proportional(11.0),
            c::TEXT_WEAK,
        );
    }
    let start = start_index_in(&g.def, g.layer);
    for (i, r) in boxes.iter().enumerate() {
        let (name, s) = &states_in(&g.def, g.layer)[i];
        let on = live == Some(i);
        let sel = g.selected == Some(Sel::State(i));
        p.rect_filled(*r, 7.0, if on { c::ACCENT_SOFT } else { c::BG2 });
        p.rect_stroke(
            *r,
            7.0,
            Stroke::new(
                if sel { 2.0 } else { 1.0 },
                if sel { c::ACCENT_TEXT } else { c::BORDER },
            ),
            egui::StrokeKind::Inside,
        );
        let glyph = match s {
            StateDef::Clip { .. } => icon::FILM_STRIP,
            StateDef::Blend1D { .. } => icon::SLIDERS_HORIZONTAL,
            StateDef::Blend2D { .. } => icon::GRID_FOUR,
        };
        // The start state carries a flag (the editor's fonts have no arrows).
        let star = if i == start {
            format!("  {}", icon::FLAG_CHECKERED)
        } else {
            String::new()
        };
        p.text(
            r.left_center() + Vec2::new(8.0, -6.0),
            egui::Align2::LEFT_CENTER,
            format!("{glyph} {name}{star}"),
            egui::FontId::proportional(12.5),
            if on { c::TEXT } else { c::TEXT_WEAK },
        );
        let sub = match s {
            StateDef::Clip { clip, .. } => clip.clone(),
            StateDef::Blend1D { param, points, .. } => format!("{param}: {} clips", points.len()),
            StateDef::Blend2D { x, y, points, .. } => {
                format!("{x} × {y}: {} clips", points.len())
            }
        };
        p.text(
            r.left_center() + Vec2::new(8.0, 8.0),
            egui::Align2::LEFT_CENTER,
            sub,
            egui::FontId::proportional(10.5),
            c::TEXT_FAINT,
        );
        if on && let Some(inst) = &g.inst {
            // The state's clock as a bar under it.
            let k = (inst.clock() / 1.0).fract().abs();
            let bar = Rect::from_min_size(
                r.left_bottom() + Vec2::new(4.0, -4.0),
                Vec2::new((r.width() - 8.0) * k, 2.0),
            );
            p.rect_filled(bar, 1.0, c::PLAY);
        }
        if let Some(pt) = pointer
            && r.contains(pt)
        {
            hit = Some(Sel::State(i));
        }
    }
    let mut moved = false;
    if resp.drag_started() {
        g.selected = hit;
    }
    if resp.dragged()
        && let Some(Sel::State(i)) = g.selected
    {
        let np = g.pos(i) + resp.drag_delta();
        let np = Pos2::new(
            np.x.clamp(0.0, (rect.width() - BOX.x).max(0.0)),
            np.y.clamp(0.0, (rect.height() - BOX.y).max(0.0)),
        );
        g.set_pos(i, np);
    }
    if resp.drag_stopped() && matches!(g.selected, Some(Sel::State(_))) {
        moved = true;
    }
    if resp.clicked() {
        g.selected = hit;
    }
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Other,
            true,
            format!("Graph canvas: {n} states"),
        )
    });
    moved
}

/// Where the segment a→b enters `r` (the arrow's tip).
fn edge(r: Rect, a: Pos2, b: Pos2) -> Pos2 {
    let mut t = 1.0f32;
    let d = b - a;
    for (lo, hi, o, dd) in [(r.min.x, r.max.x, a.x, d.x), (r.min.y, r.max.y, a.y, d.y)] {
        if dd.abs() > 1e-3 {
            for edge in [lo, hi] {
                let k = (edge - o) / dd;
                if k > 0.0 && k < t {
                    let q = a + d * k;
                    if q.x >= r.min.x - 0.5
                        && q.x <= r.max.x + 0.5
                        && q.y >= r.min.y - 0.5
                        && q.y <= r.max.y + 0.5
                    {
                        t = k;
                    }
                }
            }
        }
    }
    a + d * t
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let k = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * k - p).length()
}

/// A combo box as wide as what is left of the row (at most 200 px) over
/// `items`; returns the picked index.
fn pick(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    current: &str,
    items: &[String],
) -> Option<usize> {
    let w = ui.available_width().min(200.0);
    pick_w(ui, id, label, current, items, w)
}

fn pick_w(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    current: &str,
    items: &[String],
    w: f32,
) -> Option<usize> {
    let mut out = None;
    let w = w.max(30.0);
    theme::fixed_width(ui, w, |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(current)
            .width(w)
            .truncate()
            .show_ui(ui, |ui| {
                for (i, it) in items.iter().enumerate() {
                    if ui.selectable_label(it == current, it).clicked() {
                        out = Some(i);
                    }
                }
            })
            .response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, label));
    });
    out
}

/// The side panel: parameters, the selection, adding, the issues. Returns
/// whether the graph changed (save it).
pub fn controls(ui: &mut egui::Ui, g: &mut GraphEdit, clips: &[String]) -> bool {
    let mut changed = layers_ui(ui, g, clips);
    // ---- parameters: live values (and the list itself) ----
    theme::component(
        ui,
        "graph-params",
        icon::SLIDERS,
        c::ACCENT,
        "Parameters",
        |ui| {
            let params = g.def.params.clone();
            let mut remove = None;
            for (pi, (name, kind)) in params.iter().enumerate() {
                ui.horizontal(|ui| {
                    match kind {
                        ParamDef::Float(_) => {
                            let mut v = g.param(name).unwrap_or(0.0);
                            let (lo, hi) = float_range(&g.def, name);
                            ui.spacing_mut().slider_width =
                                (ui.available_width() - 150.0).clamp(30.0, 140.0);
                            ui.label(RichText::new(name).color(c::TEXT_WEAK));
                            if ui.add(egui::Slider::new(&mut v, lo..=hi)).changed() {
                                g.set_param(name, v);
                            }
                        }
                        ParamDef::Bool(_) => {
                            let mut on = g.param(name).unwrap_or(0.0) != 0.0;
                            if ui.checkbox(&mut on, name.as_str()).changed() {
                                g.set_param(name, on as u8 as f32);
                            }
                        }
                        ParamDef::Trigger => {
                            let r = ui
                                .button(format!("{} {name}", icon::LIGHTNING))
                                .on_hover_text("Fire the trigger");
                            let label = format!("Fire {name}");
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label)
                            });
                            if r.clicked() {
                                g.set_param(name, 1.0);
                            }
                        }
                    }
                    if theme::named_icon_button(
                        ui,
                        icon::TRASH,
                        &format!("Remove parameter {name}"),
                    )
                    .clicked()
                    {
                        remove = Some(pi);
                    }
                });
            }
            if let Some(pi) = remove {
                g.def.params.remove(pi);
                changed = true;
            }
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 170.0).max(40.0);
                ui.add(egui::TextEdit::singleline(&mut g.new_param).desired_width(w));
                let kinds = ["Float".to_string(), "Bool".into(), "Trigger".into()];
                if let Some(k) = pick_w(
                    ui,
                    "graph-param-kind",
                    "Parameter kind",
                    &kinds[g.new_kind].clone(),
                    &kinds,
                    80.0,
                ) {
                    g.new_kind = k;
                }
                if ui
                    .button(format!("{} Add", icon::PLUS))
                    .on_hover_text("Add a parameter")
                    .clicked()
                {
                    let n = g.new_param.trim().to_string();
                    if !n.is_empty() && g.def.param(&n).is_none() {
                        let p = match g.new_kind {
                            1 => ParamDef::Bool(false),
                            2 => ParamDef::Trigger,
                            _ => ParamDef::Float(0.0),
                        };
                        g.def.params.push((n, p));
                        changed = true;
                    }
                }
            });
        },
    );

    // ---- the states as a list (the canvas's twin: keyboard, screen readers, tests) ----
    theme::component(
        ui,
        "graph-states",
        icon::TREE_STRUCTURE,
        c::TEXT,
        "States",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for i in 0..states_in(&g.def, g.layer).len() {
                    let name = states_in(&g.def, g.layer)[i].0.clone();
                    let on = g.selected == Some(Sel::State(i));
                    let live = g.live_state() == Some(i);
                    let text = if live {
                        RichText::new(&name).color(c::PLAY)
                    } else {
                        RichText::new(&name)
                    };
                    if ui
                        .add(
                            egui::Button::new(text)
                                .selected(on)
                                .frame_when_inactive(false),
                        )
                        .clicked()
                    {
                        g.selected = Some(Sel::State(i));
                    }
                }
            });
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 190.0).max(40.0);
                ui.add(egui::TextEdit::singleline(&mut g.new_name).desired_width(w));
                let named = |ui: &mut egui::Ui, text: String, name: &str| {
                    let r = ui.button(text).on_hover_text(name);
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name)
                    });
                    r.clicked()
                };
                if named(ui, format!("{} Clip", icon::PLUS), "Add a clip state") {
                    g.add_state(false, clips);
                    changed = true;
                }
                if named(ui, format!("{} Blend", icon::PLUS), "Add a blend space") {
                    g.add_state(true, clips);
                    changed = true;
                }
                if named(ui, format!("{} 2D", icon::PLUS), "Add a 2D blend space") {
                    g.add_state_kind(2, clips);
                    changed = true;
                }
            });
        },
    );

    // ---- the selection ----
    match g.selected {
        Some(Sel::State(i)) if i < states_in(&g.def, g.layer).len() => {
            changed |= state_ui(ui, g, i, clips)
        }
        Some(Sel::Transition(i)) if i < transitions_in(&g.def, g.layer).len() => {
            changed |= transition_ui(ui, g, i)
        }
        _ => {}
    }

    // ---- issues ----
    let errors = g.issues.iter().filter(|i| i.level == Level::Error).count();
    let title = if g.issues.is_empty() {
        "Check: ok".to_string()
    } else {
        format!(
            "Check: {errors} error(s), {} warning(s)",
            g.issues.len() - errors
        )
    };
    theme::component(
        ui,
        "graph-issues",
        icon::CHECK_CIRCLE,
        if errors > 0 { c::ERROR } else { c::OK },
        &title,
        |ui| {
            if g.issues.is_empty() {
                ui.label(
                    RichText::new(format!(
                        "{} names, kinds, clips and reachability",
                        icon::CHECK_CIRCLE
                    ))
                    .color(c::OK),
                );
            }
            for i in &g.issues {
                let col = if i.level == Level::Error {
                    c::ERROR
                } else {
                    c::WARN
                };
                ui.add(
                    egui::Label::new(RichText::new(format!("{}: {}", i.at, i.message)).color(col))
                        .wrap(),
                );
            }
            ui.label(
                RichText::new(format!("Same checks: cargo xtask anim check {}", g.name))
                    .size(11.0)
                    .color(c::TEXT_FAINT),
            );
        },
    );
    // ---- what the previewed look's tier folds (issue #112) ----
    if !g.folds.is_empty() {
        let n = g.folds.len();
        theme::component(
            ui,
            "graph-folds",
            icon::STACK,
            c::WARN,
            &format!("This look folds the pose ({n})"),
            |ui| {
                for f in &g.folds {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}: {}", f.at, f.message)).color(c::TEXT_WEAK),
                        )
                        .wrap(),
                    );
                }
                ui.label(
                    RichText::new(format!(
                        "Every tier: cargo xtask anim check {} --platform n64",
                        g.name
                    ))
                    .size(11.0)
                    .color(c::TEXT_FAINT),
                );
            },
        );
    }
    changed
}

/// The machines (the base and each layer), the selected layer's settings
/// and its joint mask. Returns whether the graph changed.
fn layers_ui(ui: &mut egui::Ui, g: &mut GraphEdit, clips: &[String]) -> bool {
    let mut changed = false;
    theme::component(ui, "graph-layers", icon::STACK, c::ACCENT, "Layers", |ui| {
        ui.horizontal_wrapped(|ui| {
            let mut pick = None;
            let names: Vec<String> = std::iter::once("Base".to_string())
                .chain(g.def.layers.iter().map(|l| l.name.clone()))
                .collect();
            for (k, n) in names.iter().enumerate() {
                let r = ui.add(
                    egui::Button::new(n.as_str())
                        .selected(g.layer == k)
                        .frame_when_inactive(false),
                );
                let label = format!("Layer {n}");
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                if r.clicked() {
                    pick = Some(k);
                }
            }
            if let Some(k) = pick {
                g.layer = k;
                g.selected = None;
                g.cond_for = None;
            }
            let r = ui.button(format!("{} Layer", icon::PLUS)).on_hover_text(
                "A layer over the others: its own states, on the joints of its mask",
            );
            r.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add a layer")
            });
            if r.clicked() && g.def.layers.len() < trino_core::anim::graph::MAX_LAYERS {
                let mut name = "layer".to_string();
                let mut k = 2;
                while g.def.layers.iter().any(|l| l.name == name) {
                    name = format!("layer-{k}");
                    k += 1;
                }
                let clip = clips.first().cloned().unwrap_or_default();
                g.def.layers.push(sa::LayerDef::starter(&name, &clip));
                g.layer = g.def.layers.len();
                g.selected = None;
                changed = true;
            }
        });
        if g.layer == 0 {
            ui.label(
                RichText::new("The base moves every joint; layers above it play on their masks.")
                    .size(11.0)
                    .color(c::TEXT_FAINT),
            );
            return;
        }
        let li = g.layer - 1;
        let floats: Vec<String> = g
            .def
            .params
            .iter()
            .filter(|(_, p)| matches!(p, ParamDef::Float(_)))
            .map(|(n, _)| n.clone())
            .collect();
        let mut remove = false;
        {
            let l = &mut g.def.layers[li];
            theme::prop(ui, "Name", |ui| {
                let mut text = l.name.clone();
                let r = ui
                    .add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()));
                if r.lost_focus() && !text.trim().is_empty() && text != l.name {
                    l.name = text.trim().to_string();
                    changed = true;
                }
            });
            let modes = ["Override".to_string(), "Additive".into()];
            let cur = (l.mode == sa::LayerModeDef::Additive) as usize;
            theme::prop(ui, "Mode", |ui| {
                if let Some(k) = pick(
                    ui,
                    "graph-layer-mode",
                    "Layer mode",
                    &modes[cur].clone(),
                    &modes,
                ) {
                    l.mode = if k == 1 {
                        sa::LayerModeDef::Additive
                    } else {
                        sa::LayerModeDef::Override
                    };
                    changed = true;
                }
            });
            let mut opts = vec!["value".to_string()];
            opts.extend(floats.iter().cloned());
            let cur = match &l.weight {
                sa::WeightDef::Param(p) => p.clone(),
                sa::WeightDef::Value(_) => "value".into(),
            };
            theme::prop(ui, "Weight", |ui| {
                if let Some(k) = pick(ui, "graph-layer-weight", "Layer weight", &cur, &opts) {
                    l.weight = if k == 0 {
                        sa::WeightDef::Value(1.0)
                    } else {
                        sa::WeightDef::Param(opts[k].clone())
                    };
                    changed = true;
                }
            });
            if let sa::WeightDef::Value(v) = &mut l.weight {
                theme::prop(ui, "Value", |ui| {
                    changed |= ui.add(egui::Slider::new(v, 0.0..=1.0)).changed();
                });
            }
        }
        // The mask: click the skeleton's joints, or tick them.
        let joints = g.joints.clone();
        if joints.is_empty() {
            ui.label(
                RichText::new("The model's joint names are unknown: the mask can't be edited here")
                    .size(11.0)
                    .color(c::TEXT_FAINT),
            );
        } else {
            let list = g.joint_list();
            let mask = g.def.layers[li].mask.clone();
            let all = mask.is_empty();
            let bits = sa::mask_bits(&mask, &list).unwrap_or(0);
            ui.label(
                RichText::new(if all {
                    "Mask: every joint (click joints to pick some; Shift: a joint and all under it)"
                        .to_string()
                } else {
                    format!(
                        "Mask: {} of {} joints ({})",
                        bits.count_ones().min(joints.len() as u32),
                        joints.len(),
                        mask.join(", ")
                    )
                })
                .color(c::TEXT_WEAK),
            );
            ui.horizontal(|ui| {
                ui.selectable_value(&mut g.mask_in_3d, false, "Skeleton")
                    .on_hover_text("The rest skeleton seen from the front");
                ui.selectable_value(&mut g.mask_in_3d, true, "In the preview")
                    .on_hover_text(
                        "Click joints on the model in the 3D preview (Shift: a subtree)",
                    );
            });
            let picked = if g.mask_in_3d {
                ui.label(
                    RichText::new(
                        "Click a joint in the preview to toggle it; Shift+click toggles it and every joint under it.",
                    )
                    .size(11.0)
                    .color(c::TEXT_FAINT),
                );
                g.mask_pick.take()
            } else {
                mask_view(ui, &g.rest, &joints, bits)
            };
            if let Some((j, subtree)) = picked {
                sa::mask_toggle(&mut g.def.layers[li].mask, &list, j, subtree);
                changed = true;
            }
            ui.horizontal_wrapped(|ui| {
                for (j, n) in joints.iter().enumerate() {
                    let mut on = j < 32 && bits & (1 << j) != 0;
                    if ui.checkbox(&mut on, n.as_str()).changed() {
                        sa::mask_toggle(&mut g.def.layers[li].mask, &list, j, false);
                        changed = true;
                    }
                }
            });
        }
        if ui
            .button(format!("{} Delete layer", icon::TRASH))
            .on_hover_text("Remove the layer, its states and transitions")
            .clicked()
        {
            remove = true;
        }
        if remove {
            g.def.layers.remove(li);
            g.layer = 0;
            g.selected = None;
            changed = true;
        }
    });
    changed
}

/// The model's rest skeleton seen from the front: a dot per joint (lit
/// when in the mask), a line to its parent. Returns the clicked joint.
fn mask_view(
    ui: &mut egui::Ui,
    rest: &[(trino_core::Vec3, Option<usize>)],
    joints: &[String],
    bits: u32,
) -> Option<(usize, bool)> {
    let w = (ui.available_width() - 4.0).max(80.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 150.0), Sense::click());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, c::BG0);
    if rest.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (v, _) in rest {
        lo = lo.min(Vec2::new(v.x, v.y));
        hi = hi.max(Vec2::new(v.x, v.y));
    }
    let span = (hi - lo).max(Vec2::splat(1e-3));
    // A diagram, not a drawing: each axis fills the box (a biped is tall
    // and narrow; uniform scale piles its legs' labels on each other).
    let kx = (rect.width() - 140.0).max(40.0) / span.x;
    let ky = (rect.height() - 24.0) / span.y;
    let mid = rect.center();
    let c0 = (lo + hi) * 0.5;
    // Front view: the model faces +Z, its left (+X) on the screen's right.
    let at = |v: trino_core::Vec3| Pos2::new(mid.x + (v.x - c0.x) * kx, mid.y - (v.y - c0.y) * ky);
    let on = |j: usize| j < 32 && bits & (1 << j) != 0;
    for (j, (v, parent)) in rest.iter().enumerate() {
        if let Some(pj) = parent {
            p.line_segment([at(*v), at(rest[*pj].0)], (1.5, c::BORDER));
        }
        let _ = j;
    }
    let mut clicked = None;
    let pointer = resp.interact_pointer_pos();
    for (j, (v, _)) in rest.iter().enumerate() {
        let q = at(*v);
        p.circle_filled(q, 5.0, if on(j) { c::ACCENT } else { c::BG3 });
        if let Some(name) = joints.get(j) {
            // Labels away from the middle: the left side's to the left.
            let left = q.x < mid.x - 1.0;
            p.text(
                q + Vec2::new(if left { -7.0 } else { 7.0 }, 0.0),
                if left {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::LEFT_CENTER
                },
                name,
                egui::FontId::proportional(9.5),
                c::TEXT_FAINT,
            );
        }
        if resp.clicked()
            && let Some(pt) = pointer
            && (pt - q).length() < 8.0
        {
            clicked = Some((j, ui.input(|i| i.modifiers.shift)));
        }
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Layer mask"));
    clicked
}

/// A float parameter's slider range: around a blend space's points, else 0..5.
fn float_range(def: &AnimGraphDef, name: &str) -> (f32, f32) {
    let mut lo: f32 = 0.0;
    let mut hi: f32 = 5.0;
    for (_, s) in def.machines().iter().flat_map(|m| m.states.iter()) {
        if let StateDef::Blend2D { x, y, points, .. } = s
            && (x == name || y == name)
        {
            let v = points.iter().map(|p| if x == name { p.0 } else { p.1 });
            lo = v.clone().fold(lo, f32::min) * 1.25;
            hi = v.fold(f32::MIN, f32::max).max(0.1) * 1.25;
        }
        if let StateDef::Blend1D {
            param,
            points,
            wrap,
            ..
        } = s
            && param == name
            && let (Some(a), Some(b)) = (points.first(), points.last())
        {
            lo = a.0.min(0.0);
            hi = match wrap {
                Some(w) => a.0 + w,
                None => b.0 + (b.0 - a.0).max(1.0) * 0.25,
            };
        }
    }
    (lo, hi.max(lo + 0.1))
}

fn state_ui(ui: &mut egui::Ui, g: &mut GraphEdit, i: usize, clips: &[String]) -> bool {
    let mut changed = false;
    let name = states_in(&g.def, g.layer)[i].0.clone();
    let floats: Vec<String> = g
        .def
        .params
        .iter()
        .filter(|(_, p)| matches!(p, ParamDef::Float(_)))
        .map(|(n, _)| n.clone())
        .collect();
    let layer = g.layer;
    let weights = g.graph().zip(g.inst.as_ref()).map(|(gr, inst)| {
        let mut cbuf = [trino_core::anim::Contribution::NONE; 6];
        let n = inst.layer_contributions(&gr, layer, &mut cbuf);
        cbuf[..n]
            .iter()
            .map(|x| (gr.clip(x.clip).id, x.weight))
            .collect::<Vec<_>>()
    });
    let live = g.live_state() == Some(i);
    let blend_value = match &states_in(&g.def, g.layer)[i].1 {
        StateDef::Blend1D { param, .. } => g.param(param),
        _ => None,
    };
    let blend_xy = match &states_in(&g.def, g.layer)[i].1 {
        StateDef::Blend2D { x, y, .. } => Some((
            x.clone(),
            y.clone(),
            g.param(x).unwrap_or(0.0),
            g.param(y).unwrap_or(0.0),
        )),
        _ => None,
    };
    let mut set_xy: Option<(f32, f32)> = None;
    let mut drag2d = g.drag2d;
    let mut rename: Option<String> = None;
    theme::component(
        ui,
        "graph-state",
        icon::FILM_STRIP,
        c::MODEL,
        &format!("State {name}"),
        |ui| {
            let mut text = name.clone();
            theme::prop(ui, "Name", |ui| {
                let r = ui
                    .add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width()));
                if r.lost_focus() && text != name {
                    rename = Some(text.clone());
                }
            });
            let (_, s) = &mut states_in_mut(&mut g.def, g.layer)[i];
            match s {
                StateDef::Clip {
                    clip,
                    looping,
                    speed,
                    speed_param,
                    root_motion,
                    events,
                } => {
                    theme::prop(ui, "Clip", |ui| {
                        if let Some(k) = pick(ui, "graph-clip", "State clip", &clip.clone(), clips)
                        {
                            *clip = clips[k].clone();
                            changed = true;
                        }
                    });
                    let loops = ["clip's".to_string(), "loop".into(), "once".into()];
                    let cur = match looping {
                        None => 0,
                        Some(true) => 1,
                        Some(false) => 2,
                    };
                    theme::prop(ui, "Loop", |ui| {
                        if let Some(k) = pick(
                            ui,
                            "graph-loop",
                            "Loop override",
                            &loops[cur].clone(),
                            &loops,
                        ) {
                            *looping = [None, Some(true), Some(false)][k];
                            changed = true;
                        }
                    });
                    theme::prop(ui, "Speed", |ui| {
                        changed |= ui
                            .add(egui::DragValue::new(speed).range(0.0..=10.0).speed(0.01))
                            .changed();
                    });
                    let mut opts = vec!["none".to_string()];
                    opts.extend(floats.iter().cloned());
                    theme::prop(ui, "× param", |ui| {
                        let cur = speed_param.clone().unwrap_or_else(|| "none".into());
                        if let Some(k) =
                            pick(ui, "graph-speed-param", "Speed parameter", &cur, &opts)
                        {
                            *speed_param = (k > 0).then(|| opts[k].clone());
                            changed = true;
                        }
                    });
                    changed |= ui.checkbox(root_motion, "Root motion").changed();
                    let mut remove = None;
                    for (k, (t, n)) in events.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(t)
                                        .range(0.0..=60.0)
                                        .speed(0.01)
                                        .suffix(" s"),
                                )
                                .changed();
                            let w = (ui.available_width() - 30.0).max(40.0);
                            changed |= ui
                                .add(egui::TextEdit::singleline(n).desired_width(w))
                                .lost_focus();
                            if theme::named_icon_button(ui, icon::TRASH, "Remove state event")
                                .clicked()
                            {
                                remove = Some(k);
                            }
                        });
                    }
                    if let Some(k) = remove {
                        events.remove(k);
                        changed = true;
                    }
                    if ui
                        .button(format!("{} State event", icon::FLAG))
                        .on_hover_text("An event of this state (the clip's own are in Clips mode)")
                        .clicked()
                    {
                        events.push((0.0, "event".into()));
                        changed = true;
                    }
                }
                StateDef::Blend1D {
                    param,
                    points,
                    wrap,
                    speed,
                    speed_param,
                    root_motion,
                } => {
                    theme::prop(ui, "Param", |ui| {
                        if let Some(k) = pick(
                            ui,
                            "graph-blend-param",
                            "Blend parameter",
                            &param.clone(),
                            &floats,
                        ) {
                            *param = floats[k].clone();
                            changed = true;
                        }
                    });
                    // The blend space: its points on a line, the value now, the weights.
                    blend_bar(ui, points, *wrap, blend_value.filter(|_| live));
                    if let Some(w) = weights.as_ref().filter(|_| live) {
                        let text: Vec<String> = w
                            .iter()
                            .map(|(id, k)| {
                                let n = clips
                                    .iter()
                                    .find(|c| trino_core::ClipId::from_name(c) == *id)
                                    .cloned()
                                    .unwrap_or_default();
                                format!("{n} {:.0}%", k * 100.0)
                            })
                            .collect();
                        ui.label(
                            RichText::new(text.join(" · "))
                                .monospace()
                                .color(c::TEXT_WEAK),
                        );
                    }
                    let mut remove = None;
                    for (k, (val, clip)) in points.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            changed |= ui.add(egui::DragValue::new(val).speed(0.01)).changed();
                            let w = ui.available_width() - 36.0;
                            if let Some(j) = pick_w(
                                ui,
                                &format!("graph-point-{k}"),
                                "Point clip",
                                &clip.clone(),
                                clips,
                                w,
                            ) {
                                *clip = clips[j].clone();
                                changed = true;
                            }
                            if theme::named_icon_button(ui, icon::TRASH, "Remove point").clicked() {
                                remove = Some(k);
                            }
                        });
                    }
                    if let Some(k) = remove {
                        points.remove(k);
                        changed = true;
                    }
                    if ui.button(format!("{} Point", icon::PLUS)).clicked() {
                        let v = points.last().map_or(0.0, |p| p.0 + 1.0);
                        points.push((v, clips.first().cloned().unwrap_or_default()));
                        changed = true;
                    }
                    let mut w = wrap.unwrap_or(0.0);
                    theme::prop(ui, "Wrap", |ui| {
                        if ui
                            .add(egui::DragValue::new(&mut w).range(0.0..=720.0))
                            .on_hover_text(
                                "A circular parameter's period (360 for an angle); 0 = none",
                            )
                            .changed()
                        {
                            *wrap = (w > 0.0).then_some(w);
                            changed = true;
                        }
                    });
                    theme::prop(ui, "Speed", |ui| {
                        changed |= ui
                            .add(egui::DragValue::new(speed).range(0.0..=10.0).speed(0.01))
                            .changed();
                    });
                    let mut opts = vec!["none".to_string()];
                    opts.extend(floats.iter().cloned());
                    theme::prop(ui, "× param", |ui| {
                        let cur = speed_param.clone().unwrap_or_else(|| "none".into());
                        if let Some(k) = pick(
                            ui,
                            "graph-blend-speed-param",
                            "Speed parameter",
                            &cur,
                            &opts,
                        ) {
                            *speed_param = (k > 0).then(|| opts[k].clone());
                            changed = true;
                        }
                    });
                    changed |= ui.checkbox(root_motion, "Root motion").changed();
                }
                StateDef::Blend2D {
                    x,
                    y,
                    points,
                    speed,
                    speed_param,
                    root_motion,
                } => {
                    theme::prop(ui, "X", |ui| {
                        if let Some(k) =
                            pick(ui, "graph-2d-x", "Blend x parameter", &x.clone(), &floats)
                        {
                            *x = floats[k].clone();
                            changed = true;
                        }
                    });
                    theme::prop(ui, "Y", |ui| {
                        if let Some(k) =
                            pick(ui, "graph-2d-y", "Blend y parameter", &y.clone(), &floats)
                        {
                            *y = floats[k].clone();
                            changed = true;
                        }
                    });
                    // The plane: points (drag them), the baked triangles,
                    // the parameters' dot (drag it to drive the preview).
                    let value = blend_xy.as_ref().map(|b| (b.2, b.3));
                    let w = weights.as_ref().filter(|_| live);
                    match blend_2d(ui, points, value, w.map(|v| v.as_slice()), &mut drag2d) {
                        Blend2DEdit::None => {}
                        Blend2DEdit::Moved => changed = true,
                        Blend2DEdit::Value(vx, vy) => set_xy = Some((vx, vy)),
                    }
                    let mut remove = None;
                    for (k, (px, py, clip)) in points.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            changed |= ui
                                .add(egui::DragValue::new(px).speed(0.01).prefix("x "))
                                .changed();
                            changed |= ui
                                .add(egui::DragValue::new(py).speed(0.01).prefix("y "))
                                .changed();
                            let w = ui.available_width() - 36.0;
                            if let Some(j) = pick_w(
                                ui,
                                &format!("graph-point2-{k}"),
                                "Point clip",
                                &clip.clone(),
                                clips,
                                w,
                            ) {
                                *clip = clips[j].clone();
                                changed = true;
                            }
                            if theme::named_icon_button(ui, icon::TRASH, "Remove point").clicked() {
                                remove = Some(k);
                            }
                        });
                    }
                    if let Some(k) = remove {
                        points.remove(k);
                        changed = true;
                    }
                    if ui.button(format!("{} Point", icon::PLUS)).clicked() {
                        let n = points.len() as f32;
                        points.push((n.cos(), n.sin(), clips.first().cloned().unwrap_or_default()));
                        changed = true;
                    }
                    theme::prop(ui, "Speed", |ui| {
                        changed |= ui
                            .add(egui::DragValue::new(speed).range(0.0..=10.0).speed(0.01))
                            .changed();
                    });
                    let mut opts = vec!["none".to_string()];
                    opts.extend(floats.iter().cloned());
                    theme::prop(ui, "× param", |ui| {
                        let cur = speed_param.clone().unwrap_or_else(|| "none".into());
                        if let Some(k) =
                            pick(ui, "graph-2d-speed-param", "Speed parameter", &cur, &opts)
                        {
                            *speed_param = (k > 0).then(|| opts[k].clone());
                            changed = true;
                        }
                    });
                    changed |= ui.checkbox(root_motion, "Root motion").changed();
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} Start here", icon::FLAG_CHECKERED))
                    .on_hover_text("The graph's start state")
                    .clicked()
                {
                    *start_in_mut(&mut g.def, g.layer) = Some(name.clone());
                    changed = true;
                }
                if ui
                    .button(format!("{} Transition", icon::ARROW_RIGHT))
                    .on_hover_text("A transition from this state")
                    .clicked()
                {
                    g.add_transition(i);
                    changed = true;
                }
                if ui
                    .button(format!("{} Delete", icon::TRASH))
                    .on_hover_text("Remove the state and its transitions")
                    .clicked()
                {
                    g.remove_state(i);
                    changed = true;
                }
            });
        },
    );
    g.drag2d = drag2d;
    if let (Some((vx, vy)), Some((x, y, ..))) = (set_xy, blend_xy) {
        g.set_param(&x, vx);
        g.set_param(&y, vy);
    }
    if let Some(to) = rename
        && g.rename_state(i, &to).is_ok()
    {
        changed = true;
    }
    changed
}

/// What the 2D blend space view did.
enum Blend2DEdit {
    None,
    /// A point was dragged (save the graph).
    Moved,
    /// The parameters' dot was dragged: drive the preview there.
    Value(f32, f32),
}

/// A 2D blend space: its points on a plane (drag one to move it), the
/// Delaunay triangles the bake will use, the parameters' value (drag it to
/// set them) and the live weights by each point.
fn blend_2d(
    ui: &mut egui::Ui,
    points: &mut [(f32, f32, String)],
    value: Option<(f32, f32)>,
    weights: Option<&[(trino_core::ClipId, f32)]>,
    drag: &mut Option<usize>,
) -> Blend2DEdit {
    let w = (ui.available_width() - 4.0).max(120.0);
    let side = w.min(220.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, side), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 6.0, c::BG0);
    let mut span = 1.0f32;
    for q in points.iter() {
        span = span.max(q.0.abs()).max(q.1.abs());
    }
    let span = span * 1.25;
    let mid = rect.center();
    let k = (side * 0.5 - 8.0) / span;
    let at = |x: f32, y: f32| Pos2::new(mid.x + x * k, mid.y - y * k);
    let back = |q: Pos2| ((q.x - mid.x) / k, (mid.y - q.y) / k);
    p.line_segment([at(-span, 0.0), at(span, 0.0)], (1.0, c::BG2));
    p.line_segment([at(0.0, -span), at(0.0, span)], (1.0, c::BG2));
    if let Ok(tris) = sa::triangulate(points) {
        for t in tris {
            let v = t.map(|i| at(points[i as usize].0, points[i as usize].1));
            for e in 0..3 {
                p.line_segment([v[e], v[(e + 1) % 3]], (1.0, c::BORDER));
            }
        }
    }
    let mut out = Blend2DEdit::None;
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started()
        && let Some(pt) = pointer
    {
        *drag = points
            .iter()
            .position(|q| (at(q.0, q.1) - pt).length() < 8.0);
    }
    if resp.dragged()
        && let Some(pt) = pointer
    {
        match *drag {
            Some(i) if i < points.len() => {
                let (x, y) = back(pt);
                points[i].0 = (x * 100.0).round() / 100.0;
                points[i].1 = (y * 100.0).round() / 100.0;
            }
            _ => {
                let (x, y) = back(pt);
                out = Blend2DEdit::Value(x, y);
            }
        }
    }
    if resp.drag_stopped() {
        if drag.is_some() {
            out = Blend2DEdit::Moved;
        }
        *drag = None;
    }
    if resp.clicked()
        && let Some(pt) = pointer
    {
        let (x, y) = back(pt);
        out = Blend2DEdit::Value(x, y);
    }
    for q in points.iter() {
        let c0 = at(q.0, q.1);
        let wt = weights
            .and_then(|w| {
                w.iter()
                    .find(|x| x.0 == trino_core::ClipId::from_name(&q.2))
            })
            .map_or(0.0, |x| x.1);
        p.circle_filled(
            c0,
            4.0 + 5.0 * wt,
            if wt > 0.0 { c::PLAY } else { c::ACCENT },
        );
        p.text(
            c0 + Vec2::new(7.0, -7.0),
            egui::Align2::LEFT_BOTTOM,
            if wt > 0.0 {
                format!("{} {:.0}%", q.2, wt * 100.0)
            } else {
                q.2.clone()
            },
            egui::FontId::proportional(10.0),
            c::TEXT_WEAK,
        );
    }
    if let Some((x, y)) = value {
        let q = at(x.clamp(-span, span), y.clamp(-span, span));
        p.circle_stroke(q, 6.0, Stroke::new(2.0, c::PLAY));
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "2D blend space"));
    out
}

/// A blend space's points on a line, the parameter's value marked.
fn blend_bar(ui: &mut egui::Ui, points: &[(f32, String)], wrap: Option<f32>, value: Option<f32>) {
    let w = (ui.available_width() - 4.0).max(60.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::hover());
    let p = ui.painter_at(rect);
    let (Some(a), Some(b)) = (points.first(), points.last()) else {
        return;
    };
    let lo = a.0;
    let hi = match wrap {
        Some(w) => a.0 + w,
        None => b.0.max(a.0 + 1e-3),
    };
    let x_of =
        |v: f32| rect.min.x + 10.0 + (rect.width() - 20.0) * ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    let y = rect.min.y + 12.0;
    p.line_segment(
        [
            Pos2::new(rect.min.x + 10.0, y),
            Pos2::new(rect.max.x - 10.0, y),
        ],
        (2.0, c::BG3),
    );
    for (v, clip) in points {
        let x = x_of(*v);
        p.circle_filled(Pos2::new(x, y), 4.0, c::ACCENT);
        p.text(
            Pos2::new(x, y + 12.0),
            egui::Align2::CENTER_CENTER,
            clip,
            egui::FontId::proportional(10.0),
            c::TEXT_WEAK,
        );
    }
    if let Some(v) = value {
        let x = x_of(v);
        p.line_segment(
            [Pos2::new(x, rect.min.y + 2.0), Pos2::new(x, y + 6.0)],
            (2.0, c::PLAY),
        );
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Blend space"));
}

fn transition_ui(ui: &mut egui::Ui, g: &mut GraphEdit, i: usize) -> bool {
    let mut changed = false;
    if g.cond_for != Some(i) {
        g.cond_for = Some(i);
        g.cond_text = cond_ron(&transitions_in(&g.def, g.layer)[i].when);
        g.cond_error = None;
    }
    let names: Vec<String> = states_in(&g.def, g.layer)
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    let mut froms = vec!["any".to_string()];
    froms.extend(names.iter().cloned());
    let mut remove = false;
    theme::component(
        ui,
        "graph-transition",
        icon::ARROW_RIGHT,
        c::PREFAB,
        &format!("Transition {i}"),
        |ui| {
            let t = &mut transitions_in_mut(&mut g.def, g.layer)[i];
            let cur = match &t.from {
                From::Any => "any".to_string(),
                From::State(s) => s.clone(),
            };
            theme::prop(ui, "From", |ui| {
                if let Some(k) = pick(ui, "graph-from", "Transition from", &cur, &froms) {
                    t.from = if k == 0 {
                        From::Any
                    } else {
                        From::State(froms[k].clone())
                    };
                    changed = true;
                }
            });
            theme::prop(ui, "To", |ui| {
                if let Some(k) = pick(ui, "graph-to", "Transition to", &t.to.clone(), &names) {
                    t.to = names[k].clone();
                    changed = true;
                }
            });
            theme::prop(ui, "Blend", |ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut t.blend)
                            .range(0.0..=5.0)
                            .speed(0.01)
                            .suffix(" s"),
                    )
                    .changed();
            });
            let modes = ["Crossfade".to_string(), "Inertial".into()];
            let cur = (t.mode == sa::TransitionMode::Inertial) as usize;
            theme::prop(ui, "Mode", |ui| {
                if let Some(k) = pick(
                    ui,
                    "graph-mode",
                    "Transition mode",
                    &modes[cur].clone(),
                    &modes,
                )
                .map(|k| k == 1)
                {
                    t.mode = if k {
                        sa::TransitionMode::Inertial
                    } else {
                        sa::TransitionMode::Crossfade
                    };
                    changed = true;
                }
            });
            ui.label(RichText::new("When (RON): Trigger(\"x\"), Finished, Param(\"x\"), Not(..), And([..]), Greater(\"x\", 1.0), After(0.5)").size(11.0).color(c::TEXT_FAINT));
            let r = ui.add(
                egui::TextEdit::singleline(&mut g.cond_text)
                    .desired_width(ui.available_width())
                    .font(egui::TextStyle::Monospace),
            );
            if r.lost_focus() || r.changed() {
                match ron::from_str::<CondDef>(&g.cond_text) {
                    Ok(cond) => {
                        g.cond_error = None;
                        if cond != t.when && r.lost_focus() {
                            t.when = cond;
                            changed = true;
                        }
                    }
                    Err(e) => g.cond_error = Some(e.to_string()),
                }
            }
            if let Some(e) = &g.cond_error {
                ui.label(RichText::new(e).color(c::ERROR).size(11.0));
            }
            if ui.button(format!("{} Delete", icon::TRASH)).clicked() {
                remove = true;
            }
        },
    );
    if remove {
        transitions_in_mut(&mut g.def, g.layer).remove(i);
        g.selected = None;
        g.cond_for = None;
        changed = true;
    }
    changed
}

/// Graphs whose model is `model` (or all, when none is).
pub fn graphs_for(root: &std::path::Path, model: &str) -> Vec<String> {
    let all = sa::graph_files(root);
    let mine: Vec<String> = all
        .iter()
        .filter(|(_, p)| AnimGraphDef::load(p).is_ok_and(|d| d.model_path() == model))
        .map(|(n, _)| n.clone())
        .collect();
    mine
}

/// The canvas moved a state: the layout is written when the drag ends.
pub fn layout_moved(g: &mut GraphEdit) -> Result<(), String> {
    g.save()
}

/// For tests and hooks: select a state or a transition by spec
/// (`state:<name>` / `transition:<i>`).
pub fn select_spec(g: &mut GraphEdit, spec: &str) {
    // Several, comma-separated: `layer:upper,state:aim`.
    if spec.contains(',') {
        for part in spec.split(',') {
            select_spec(g, part.trim());
        }
        return;
    }
    // The mask picked in the 3D preview (issue #136).
    if spec == "mask3d" {
        g.mask_in_3d = true;
        return;
    }
    if let Some(n) = spec.strip_prefix("layer:") {
        g.layer = g
            .def
            .layers
            .iter()
            .position(|l| l.name == n)
            .map_or(0, |i| i + 1);
        g.selected = None;
        return;
    }
    if let Some(n) = spec.strip_prefix("state:") {
        g.selected = g
            .def
            .states
            .iter()
            .position(|(s, _)| s == n)
            .map(Sel::State);
    } else if let Some(i) = spec
        .strip_prefix("transition:")
        .and_then(|i| i.parse().ok())
    {
        g.selected = (i < transitions_in(&g.def, g.layer).len()).then_some(Sel::Transition(i));
    }
}
