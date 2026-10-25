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
    /// The condition being edited (RON) and why it does not parse.
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
                    && i >= self.def.states.len()
                {
                    self.selected = None;
                }
                if let Some(Sel::Transition(i)) = self.selected
                    && i >= self.def.transitions.len()
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

    /// Check and bake against the model's clips; a new instance keeps the
    /// parameters it can.
    pub fn rebake(&mut self, clips: Option<&[ClipFacts]>, sounds: &[String], generation: u64) {
        if !self.dirty && self.baked_for == Some(generation) {
            return;
        }
        self.dirty = false;
        self.baked_for = Some(generation);
        let has = |s: &str| sounds.iter().any(|x| x == &format!("sounds/{s}"));
        self.issues = sa::check(&self.def, clips, &has);
        let old = self.inst.take().zip(self.tang.take());
        self.tang = clips.and_then(|c| sa::to_tang(&self.def, c, &has).ok());
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

    /// The live state's index.
    pub fn live_state(&self) -> Option<usize> {
        self.inst.as_ref().map(|i| i.state())
    }

    /// Where state `i` is drawn (its `layout` entry, else a grid).
    pub fn pos(&self, i: usize) -> Pos2 {
        let name = &self.def.states[i].0;
        match self.def.layout.iter().find(|(n, _)| n == name) {
            Some((_, (x, y))) => Pos2::new(*x, *y),
            None => Pos2::new(24.0 + (i % 3) as f32 * 210.0, 36.0 + (i / 3) as f32 * 96.0),
        }
    }

    fn set_pos(&mut self, i: usize, p: Pos2) {
        let name = self.def.states[i].0.clone();
        let v = (p.x.round(), p.y.round());
        match self.def.layout.iter_mut().find(|(n, _)| *n == name) {
            Some(e) => e.1 = v,
            None => self.def.layout.push((name, v)),
        }
    }

    /// Rename state `i` everywhere (transitions, start, layout).
    pub fn rename_state(&mut self, i: usize, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() || self.def.states.iter().any(|(n, _)| n == to) {
            return Err(format!("`{to}`: empty or already a state"));
        }
        let from = std::mem::replace(&mut self.def.states[i].0, to.to_string());
        for t in &mut self.def.transitions {
            if t.from == From::State(from.clone()) {
                t.from = From::State(to.to_string());
            }
            if t.to == from {
                t.to = to.to_string();
            }
        }
        if self.def.start.as_deref() == Some(from.as_str()) {
            self.def.start = Some(to.to_string());
        }
        for (n, _) in &mut self.def.layout {
            if *n == from {
                *n = to.to_string();
            }
        }
        Ok(())
    }

    /// Remove state `i` and every transition to or from it.
    pub fn remove_state(&mut self, i: usize) {
        let (name, _) = self.def.states.remove(i);
        self.def
            .transitions
            .retain(|t| t.to != name && t.from != From::State(name.clone()));
        self.def.layout.retain(|(n, _)| *n != name);
        if self.def.start.as_deref() == Some(name.as_str()) {
            self.def.start = None;
        }
        self.selected = None;
    }

    /// Add a state (a clip, or a blend space over the first float
    /// parameter); returns its index.
    pub fn add_state(&mut self, blend: bool, clips: &[String]) -> usize {
        let mut name = self.new_name.trim().to_string();
        if name.is_empty() {
            name = "state".into();
        }
        let base = name.clone();
        let mut k = 2;
        while self.def.states.iter().any(|(n, _)| *n == name) {
            name = format!("{base}-{k}");
            k += 1;
        }
        let clip = clips.first().cloned().unwrap_or_default();
        let state = if blend {
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
        self.def.states.push((name, state));
        let i = self.def.states.len() - 1;
        self.selected = Some(Sel::State(i));
        i
    }

    /// Add a transition from state `from` to the next state, on `Finished`.
    pub fn add_transition(&mut self, from: usize) -> usize {
        let n = self.def.states.len();
        let to = (from + 1) % n.max(1);
        self.def.transitions.push(TransitionDef {
            from: From::State(self.def.states[from].0.clone()),
            to: self.def.states[to].0.clone(),
            when: CondDef::Finished,
            blend: 0.15,
        });
        let i = self.def.transitions.len() - 1;
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
    let n = g.def.states.len();
    let boxes: Vec<Rect> = (0..n)
        .map(|i| Rect::from_min_size(g.pos(i) + origin, BOX))
        .collect();
    let any = Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::new(44.0, 20.0));
    let live = g.live_state();
    let fading = g.inst.as_ref().and_then(|i| i.transition());
    // Transitions: a line between box edges, an arrowhead, its condition.
    let mut hit: Option<Sel> = None;
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    for (ti, t) in g.def.transitions.iter().enumerate() {
        let Some(to) = g.def.states.iter().position(|(nm, _)| *nm == t.to) else {
            continue;
        };
        let a = match &t.from {
            From::Any => any.center(),
            From::State(f) => match g.def.states.iter().position(|(nm, _)| nm == f) {
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
            matches!(&t.from, From::State(f) if g.def.states.get(from).is_some_and(|s| s.0 == *f))
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
    if g.def.transitions.iter().any(|t| t.from == From::Any) {
        p.rect_filled(any, 10.0, c::BG3);
        p.text(
            any.center(),
            egui::Align2::CENTER_CENTER,
            "any",
            egui::FontId::proportional(11.0),
            c::TEXT_WEAK,
        );
    }
    let start = g.def.start_index();
    for (i, r) in boxes.iter().enumerate() {
        let (name, s) = &g.def.states[i];
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
    let mut changed = false;
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
                for i in 0..g.def.states.len() {
                    let name = g.def.states[i].0.clone();
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
            });
        },
    );

    // ---- the selection ----
    match g.selected {
        Some(Sel::State(i)) if i < g.def.states.len() => changed |= state_ui(ui, g, i, clips),
        Some(Sel::Transition(i)) if i < g.def.transitions.len() => {
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
    changed
}

/// A float parameter's slider range: around a blend space's points, else 0..5.
fn float_range(def: &AnimGraphDef, name: &str) -> (f32, f32) {
    let mut lo: f32 = 0.0;
    let mut hi: f32 = 5.0;
    for (_, s) in &def.states {
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
    let name = g.def.states[i].0.clone();
    let floats: Vec<String> = g
        .def
        .params
        .iter()
        .filter(|(_, p)| matches!(p, ParamDef::Float(_)))
        .map(|(n, _)| n.clone())
        .collect();
    let weights = g.graph().zip(g.inst.as_ref()).map(|(gr, inst)| {
        let mut cbuf = [trino_core::anim::Contribution {
            clip: 0,
            time: 0.0,
            weight: 0.0,
        }; 4];
        let n = inst.contributions(&gr, &mut cbuf);
        cbuf[..n]
            .iter()
            .map(|x| (gr.clip(x.clip).id, x.weight))
            .collect::<Vec<_>>()
    });
    let live = g.live_state() == Some(i);
    let blend_value = match &g.def.states[i].1 {
        StateDef::Blend1D { param, .. } => g.param(param),
        _ => None,
    };
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
            let (_, s) = &mut g.def.states[i];
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
            }
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} Start here", icon::FLAG_CHECKERED))
                    .on_hover_text("The graph's start state")
                    .clicked()
                {
                    g.def.start = Some(name.clone());
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
    if let Some(to) = rename
        && g.rename_state(i, &to).is_ok()
    {
        changed = true;
    }
    changed
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
        g.cond_text = cond_ron(&g.def.transitions[i].when);
        g.cond_error = None;
    }
    let names: Vec<String> = g.def.states.iter().map(|(n, _)| n.clone()).collect();
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
            let t = &mut g.def.transitions[i];
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
        g.def.transitions.remove(i);
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
        g.selected = (i < g.def.transitions.len()).then_some(Sel::Transition(i));
    }
}
