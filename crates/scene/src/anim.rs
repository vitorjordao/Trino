//! Animation graphs as text (issue #65): `assets/anim/<name>.anim.ron`,
//! the file the editor's Animation tab and an agent both write.
//!
//! ```ron
//! AnimGraph(
//!     model: "models/strider",
//!     params: { "speed": Float(0.0), "grounded": Bool(true), "hop": Trigger },
//!     states: {
//!         "locomotion": Blend1D(param: "speed", points: [(0.0, "idle"), (0.8, "walk"), (2.0, "run")]),
//!         "hop": Clip(clip: "hop", events: [(0.1, "crouch")]),
//!     },
//!     transitions: [
//!         (from: "locomotion", to: "hop", when: Trigger("hop"), blend: 0.08),
//!         (from: "hop", to: "locomotion", when: Finished, blend: 0.15),
//!         (from: "*", to: "locomotion", when: Not(Param("grounded")), blend: 0.1),
//!     ],
//! )
//! ```
//!
//! States and parameters keep the order they are written in (the first
//! state is the start unless `start` names one). `from: "*"` (or `Any`)
//! leaves any state. [`check`] is `cargo xtask anim check`; [`to_tang`]
//! bakes the `TANG` blob `trino_core::anim::AnimGraph` reads (with the
//! clip facts the model's bake knows: durations, loops, events, root
//! tracks); [`simulate`] runs a script of parameter changes through the
//! runtime itself (`cargo xtask anim simulate`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use trino_core::anim::graph::{self as g, op};
use trino_core::anim::{AnimGraph, AnimGraphInstance, ClipMeta, EventId};
use trino_core::{ClipId, Mesh};

/// 2: layers, `Blend2D`, `mode: Inertial` (issue #112); a version-1 file
/// reads unchanged.
pub const GRAPH_VERSION: u32 = 3;

/// Where graphs live, under the project root.
pub const DIR: &str = "assets/anim";

/// The blob a graph bakes to: `anim/<name>`.
pub fn blob_path(name: &str) -> String {
    format!("anim/{name}")
}

/// `assets/anim/<name>.anim.ron` under `root`.
pub fn graph_file(root: &Path, name: &str) -> PathBuf {
    root.join(DIR).join(format!("{name}.anim.ron"))
}

/// Every graph of a project: (name, file), sorted.
pub fn graph_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(dir) = std::fs::read_dir(root.join(DIR)) {
        for e in dir.flatten() {
            let p = e.path();
            if let Some(name) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".anim.ron"))
            {
                out.push((name.to_string(), p.clone()));
            }
        }
    }
    out.sort();
    out
}

/// A map written `{ "a": x, "b": y }` that keeps its order.
mod ordered {
    use serde::de::{MapAccess, Visitor};
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(
        v: &[(String, T)],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(v.len()))?;
        for (k, x) in v {
            m.serialize_entry(k, x)?;
        }
        m.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Vec<(String, T)>, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Vec<(String, T)>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of names")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                let mut out: Vec<(String, T)> = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, T>()? {
                    if out.iter().any(|(n, _)| *n == k) {
                        return Err(serde::de::Error::custom(format!("`{k}` is written twice")));
                    }
                    out.push((k, v));
                }
                Ok(out)
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

/// An optional field written bare (`start: "idle"`, not `Some("idle")`):
/// read and written so (with `skip_serializing_if = "Option::is_none"`).
pub(crate) fn bare<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

pub(crate) fn bare_ser<S: serde::Serializer, T: Serialize>(
    v: &Option<T>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => x.serialize(s),
        None => s.serialize_none(),
    }
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn version_one() -> u32 {
    1
}

fn default_blend() -> f32 {
    0.1
}

/// A parameter and its default.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ParamDef {
    Float(f32),
    Bool(bool),
    /// Set by the game for one update.
    Trigger,
}

/// A state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum StateDef {
    /// One clip of the model.
    Clip {
        clip: String,
        /// Override the clip's baked loop flag.
        #[serde(
            default,
            deserialize_with = "bare",
            serialize_with = "bare_ser",
            rename = "loop",
            skip_serializing_if = "Option::is_none"
        )]
        looping: Option<bool>,
        #[serde(default = "one")]
        speed: f32,
        /// A float parameter that multiplies `speed` (a stride rate).
        #[serde(
            default,
            deserialize_with = "bare",
            serialize_with = "bare_ser",
            skip_serializing_if = "Option::is_none"
        )]
        speed_param: Option<String>,
        /// Move the entity by the clip's root track (when it has one).
        #[serde(default = "yes")]
        root_motion: bool,
        /// Events of this state on top of the clip's: (seconds, name).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        events: Vec<(f32, String)>,
    },
    /// A blend space over one float parameter: between the two points
    /// around its value, synchronized by normalized time.
    Blend1D {
        param: String,
        points: Vec<(f32, String)>,
        /// The parameter is circular with this period (an angle: 360).
        #[serde(
            default,
            deserialize_with = "bare",
            serialize_with = "bare_ser",
            skip_serializing_if = "Option::is_none"
        )]
        wrap: Option<f32>,
        #[serde(default = "one")]
        speed: f32,
        #[serde(
            default,
            deserialize_with = "bare",
            serialize_with = "bare_ser",
            skip_serializing_if = "Option::is_none"
        )]
        speed_param: Option<String>,
        #[serde(default = "yes")]
        root_motion: bool,
    },
    /// A blend space over two float parameters (direction × speed, aim
    /// pitch × yaw): the clips at the corners of the triangle around the
    /// value (a Delaunay triangulation baked from the points), weighted
    /// by its barycentric coordinates, synchronized by normalized time.
    /// Outside the points' hull, the nearest edge.
    Blend2D {
        x: String,
        y: String,
        /// (x, y, clip).
        points: Vec<(f32, f32, String)>,
        #[serde(default = "one")]
        speed: f32,
        #[serde(
            default,
            deserialize_with = "bare",
            serialize_with = "bare_ser",
            skip_serializing_if = "Option::is_none"
        )]
        speed_param: Option<String>,
        #[serde(default = "yes")]
        root_motion: bool,
    },
}

impl StateDef {
    /// Every clip it plays.
    pub fn clips(&self) -> Vec<&str> {
        match self {
            StateDef::Clip { clip, .. } => vec![clip.as_str()],
            StateDef::Blend1D { points, .. } => points.iter().map(|(_, c)| c.as_str()).collect(),
            StateDef::Blend2D { points, .. } => points.iter().map(|(_, _, c)| c.as_str()).collect(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            StateDef::Clip { .. } => "Clip",
            StateDef::Blend1D { .. } => "Blend1D",
            StateDef::Blend2D { .. } => "Blend2D",
        }
    }

    /// The most clips it plays at once (1, 2 or 3).
    pub fn width(&self) -> usize {
        match self {
            StateDef::Clip { .. } => 1,
            StateDef::Blend1D { points, .. } => points.len().clamp(1, 2),
            StateDef::Blend2D { points, .. } => points.len().clamp(1, 3),
        }
    }
}

/// When a transition fires.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum CondDef {
    #[default]
    Always,
    /// A trigger was set (it is consumed).
    Trigger(String),
    /// The state's clip ended (one-shot) or looped once.
    Finished,
    /// A bool is true (a float is not 0).
    Param(String),
    Not(Box<CondDef>),
    And(Vec<CondDef>),
    Or(Vec<CondDef>),
    Greater(String, f32),
    Less(String, f32),
    /// Seconds in the state.
    After(f32),
}

impl CondDef {
    fn visit<'a>(&'a self, f: &mut dyn FnMut(&'a CondDef)) {
        f(self);
        match self {
            CondDef::Not(c) => c.visit(f),
            CondDef::And(v) | CondDef::Or(v) => v.iter().for_each(|c| c.visit(f)),
            _ => {}
        }
    }

    fn depth(&self) -> usize {
        match self {
            CondDef::Not(c) => 1 + c.depth(),
            CondDef::And(v) | CondDef::Or(v) => 1 + v.iter().map(|c| c.depth()).max().unwrap_or(0),
            _ => 0,
        }
    }
}

/// Where a transition leaves from: a state, or any (`"*"` / `Any`).
#[derive(Clone, Debug, PartialEq)]
pub enum From {
    Any,
    State(String),
}

impl Serialize for From {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            From::Any => s.serialize_str("*"),
            From::State(n) => s.serialize_str(n),
        }
    }
}

impl<'de> Deserialize<'de> for From {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = From;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a state name, \"*\" or Any")
            }
            // RON reads a bare `Any` as a unit value.
            fn visit_unit<E: serde::de::Error>(self) -> Result<From, E> {
                Ok(From::Any)
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<From, E> {
                Ok(if v == "*" || v == "Any" {
                    From::Any
                } else {
                    From::State(v.to_string())
                })
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransitionDef {
    pub from: From,
    pub to: String,
    #[serde(default)]
    pub when: CondDef,
    /// Crossfade seconds (0 = cut); an inertialized transition's decay.
    #[serde(default = "default_blend")]
    pub blend: f32,
    /// `Crossfade` (both states play, weighted) or `Inertial` (the
    /// destination plays alone from the first frame, plus the offset from
    /// where the source was, decaying over `blend`: no extra clip played).
    #[serde(default, skip_serializing_if = "is_default")]
    pub mode: TransitionMode,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// How a transition blends (issue #112).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransitionMode {
    #[default]
    Crossfade,
    Inertial,
}

/// How a layer combines with the ones under it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LayerModeDef {
    /// Its masked joints take its pose, by its weight.
    #[default]
    Override,
    /// Its clips' motion from their first frame is added on top.
    Additive,
}

/// A layer's weight: a number, or a Float parameter's name (its value,
/// clamped to 0..1).
#[derive(Clone, Debug, PartialEq)]
pub enum WeightDef {
    Value(f32),
    Param(String),
}

impl Default for WeightDef {
    fn default() -> Self {
        WeightDef::Value(1.0)
    }
}

impl Serialize for WeightDef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            WeightDef::Value(v) => s.serialize_f32(*v),
            WeightDef::Param(p) => s.serialize_str(p),
        }
    }
}

impl<'de> Deserialize<'de> for WeightDef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = WeightDef;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a weight (0..1) or a Float parameter's name")
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<WeightDef, E> {
                Ok(WeightDef::Value(v as f32))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<WeightDef, E> {
                Ok(WeightDef::Value(v as f32))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<WeightDef, E> {
                Ok(WeightDef::Value(v as f32))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<WeightDef, E> {
                Ok(WeightDef::Param(v.to_string()))
            }
        }
        d.deserialize_any(V)
    }
}

/// A layer over the base state machine (issue #112): its own states and
/// transitions, playing on the joints of its mask.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayerDef {
    pub name: String,
    /// The joints it moves, by name (the model's glTF nodes); `"torso/*"`
    /// is a joint and every joint under it (issue #136); empty: all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mask: Vec<String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub mode: LayerModeDef,
    #[serde(default)]
    pub weight: WeightDef,
    /// Its start state (default: its first).
    #[serde(
        default,
        deserialize_with = "bare",
        serialize_with = "bare_ser",
        skip_serializing_if = "Option::is_none"
    )]
    pub start: Option<String>,
    #[serde(with = "ordered")]
    pub states: Vec<(String, StateDef)>,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
    /// The editor's canvas for this layer.
    #[serde(default, with = "ordered", skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<(String, (f32, f32))>,
    /// Its looping states follow the base's cycle (v3, issue #138): every
    /// update lands them at the fraction of a cycle the base's looping
    /// state is at (arms that carry in step with the legs' walk).
    #[serde(default, skip_serializing_if = "is_default")]
    pub sync: bool,
}

impl LayerDef {
    pub fn state(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }

    pub fn start_index(&self) -> usize {
        self.start
            .as_ref()
            .and_then(|s| self.states.iter().position(|(n, _)| n == s))
            .unwrap_or(0)
    }

    /// A starter layer: one clip state.
    pub fn starter(name: &str, clip: &str) -> LayerDef {
        LayerDef {
            name: name.to_string(),
            mask: Vec::new(),
            mode: LayerModeDef::Override,
            weight: WeightDef::Value(1.0),
            start: None,
            states: vec![(
                clip.to_string(),
                StateDef::Clip {
                    clip: clip.to_string(),
                    looping: None,
                    speed: 1.0,
                    speed_param: None,
                    root_motion: false,
                    events: Vec::new(),
                },
            )],
            transitions: Vec::new(),
            layout: Vec::new(),
            sync: false,
        }
    }
}

/// `assets/anim/<name>.anim.ron`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename = "AnimGraph", deny_unknown_fields)]
pub struct AnimGraphDef {
    #[serde(default = "version_one")]
    pub version: u32,
    /// The model whose clips it plays (`models/strider`).
    pub model: String,
    #[serde(default, with = "ordered")]
    pub params: Vec<(String, ParamDef)>,
    /// The start state (default: the first).
    #[serde(
        default,
        deserialize_with = "bare",
        serialize_with = "bare_ser",
        skip_serializing_if = "Option::is_none"
    )]
    pub start: Option<String>,
    #[serde(with = "ordered")]
    pub states: Vec<(String, StateDef)>,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
    /// Where the editor's graph view draws each state (its own layout).
    #[serde(default, with = "ordered", skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<(String, (f32, f32))>,
    /// Layers over the base (issue #112), bottom to top.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerDef>,
    /// Other models the graph serves (v3, issue #138): rigs with the same
    /// clips whose joints differ (a prop, a tool). The bake resolves each
    /// layer's mask by joint name against each one; a game picks one with
    /// `AnimGraph::for_model`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

impl AnimGraphDef {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let def: AnimGraphDef = ron::from_str(text).map_err(|e| e.to_string())?;
        if def.version > GRAPH_VERSION {
            return Err(format!(
                "graph version {} is newer than this build reads ({GRAPH_VERSION})",
                def.version
            ));
        }
        Ok(def)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn to_ron(&self) -> String {
        let cfg = ron::ser::PrettyConfig::new()
            .depth_limit(3)
            .struct_names(true)
            .separate_tuple_members(false);
        // A file using layers, 2D spaces or inertia says it is version 2.
        let mut def = self.clone();
        if def.uses_v2() {
            def.version = def.version.max(2);
        }
        ron::ser::to_string_pretty(&def, cfg).expect("graph serializes")
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, self.to_ron()).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn state(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }

    pub fn param(&self, name: &str) -> Option<&ParamDef> {
        self.params.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }

    /// The start state's index.
    pub fn start_index(&self) -> usize {
        self.start
            .as_ref()
            .and_then(|s| self.states.iter().position(|(n, _)| n == s))
            .unwrap_or(0)
    }

    /// The model's logical path (`models/x`).
    pub fn model_path(&self) -> String {
        if self.model.starts_with("models/") {
            self.model.clone()
        } else {
            format!("models/{}", self.model)
        }
    }

    /// A starter graph for a model's clips: one state per clip, the first
    /// the start, no transitions (the editor's New graph).
    pub fn starter(model: &str, clips: &[String]) -> AnimGraphDef {
        AnimGraphDef {
            version: GRAPH_VERSION,
            model: model.to_string(),
            params: Vec::new(),
            start: None,
            states: clips
                .iter()
                .map(|c| {
                    (
                        c.clone(),
                        StateDef::Clip {
                            clip: c.clone(),
                            looping: None,
                            speed: 1.0,
                            speed_param: None,
                            root_motion: true,
                            events: Vec::new(),
                        },
                    )
                })
                .collect(),
            transitions: Vec::new(),
            layout: Vec::new(),
            layers: Vec::new(),
            models: Vec::new(),
        }
    }

    /// Whether it needs `TANG` v2: layers, a 2D blend space or an
    /// inertialized transition (else it bakes to v1, byte for byte).
    /// Whether it needs `TANG` v3: a synced layer, or other models it
    /// serves (their masks per layer; with no layers, only which models: a
    /// graph without layers used to drop `models` and serve its own model
    /// alone, so `AnimGraph::for_model` failed for the rest).
    pub fn uses_v3(&self) -> bool {
        !self.models.is_empty() || self.layers.iter().any(|l| l.sync)
    }

    /// Other models' paths (`models/x`).
    pub fn model_paths(&self) -> Vec<String> {
        self.models
            .iter()
            .map(|m| {
                if m.starts_with("models/") {
                    m.clone()
                } else {
                    format!("models/{m}")
                }
            })
            .collect()
    }

    pub fn uses_v2(&self) -> bool {
        !self.layers.is_empty()
            || self
                .states
                .iter()
                .any(|(_, s)| matches!(s, StateDef::Blend2D { .. }))
            || self
                .transitions
                .iter()
                .any(|t| t.mode == TransitionMode::Inertial)
    }

    /// Every state machine: the base (`None`) then each layer.
    pub fn machines(&self) -> Vec<Machine<'_>> {
        let mut out = vec![Machine {
            layer: None,
            states: &self.states,
            transitions: &self.transitions,
            start: self.start_index(),
        }];
        for l in &self.layers {
            out.push(Machine {
                layer: Some(l),
                states: &l.states,
                transitions: &l.transitions,
                start: l.start_index(),
            });
        }
        out
    }
}

/// One state machine of a graph: the base, or a layer's.
#[derive(Clone, Copy, Debug)]
pub struct Machine<'a> {
    pub layer: Option<&'a LayerDef>,
    pub states: &'a [(String, StateDef)],
    pub transitions: &'a [TransitionDef],
    pub start: usize,
}

impl Machine<'_> {
    /// `state "x"` or `layer "upper" state "x"`.
    pub fn at(&self, what: &str) -> String {
        match self.layer {
            Some(l) => format!("layer \"{}\" {what}", l.name),
            None => what.to_string(),
        }
    }

    pub fn state(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }
}

/// What the model's bake says about one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipFacts {
    pub id: ClipId,
    pub duration: f32,
    pub looping: bool,
    /// The clip's `AEVT` track (`u32 n` + records), if any.
    pub events: Option<Vec<u8>>,
    /// Its `ROOT` track (`f32 fps, u32 frames` + records), if any.
    pub root: Option<Vec<u8>>,
    /// The IK feet over the clip (`f32 fps, u32 frames` + 2 × xyz per frame),
    /// when the model has IK chains.
    pub feet: Option<Vec<u8>>,
}

/// The clips of a baked model (TMDL), by id.
pub fn clip_facts(tmdl: &[u8]) -> Result<Vec<ClipFacts>, String> {
    let mesh = Mesh::from_tmdl(tmdl).map_err(|e| format!("model: {e:?}"))?;
    ClipMeta::from_tmdl(tmdl).ok_or("model: malformed ROOT/AEVT section")?;
    let raw = |body: &[u8], entry_len: fn(&[u8]) -> Option<usize>, id: ClipId| -> Option<Vec<u8>> {
        let n = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
        let mut at = 4;
        for _ in 0..n {
            let cid = u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?);
            let len = entry_len(body.get(at + 4..)?)?;
            if cid == id.0 {
                return Some(body[at + 4..at + 4 + len].to_vec());
            }
            at += 4 + len;
        }
        None
    };
    let events_len = |b: &[u8]| trino_core::anim::EventTrack::parse(b).map(|t| t.1);
    let root_len = |b: &[u8]| trino_core::anim::RootTrack::parse(b).map(|t| t.1);
    let aevt = trino_core::render3d::tmdl_section(tmdl, b"AEVT").unwrap_or(&[]);
    let rootb = trino_core::render3d::tmdl_section(tmdl, b"ROOT").unwrap_or(&[]);
    // The ankles per frame, posed by the runtime itself (compressed clips
    // decode as they will on the target).
    let chains = mesh.ik_chains();
    let feet = |c: &trino_core::ClipInfo| -> Option<Vec<u8>> {
        let ch = chains?;
        let mut b = c.fps.to_le_bytes().to_vec();
        b.extend_from_slice(&(c.frame_count as u32).to_le_bytes());
        let mut world = [trino_core::Mat34::IDENTITY; trino_core::MAX_JOINTS];
        for f in 0..c.frame_count {
            let t = if c.fps > 0.0 { f as f32 / c.fps } else { 0.0 };
            mesh.pose_world(Some(trino_core::AnimRef::new(c.id, t)), &mut world);
            for leg in 0..2 {
                let j = ch.chain[leg.min(ch.legs as usize - 1)][2] as usize;
                for r in 0..3 {
                    b.extend_from_slice(&world[j].m[r][3].to_le_bytes());
                }
            }
        }
        Some(b)
    };
    Ok((0..mesh.clip_count())
        .filter_map(|n| mesh.clip(n))
        .map(|c| ClipFacts {
            id: c.id,
            duration: c.duration(),
            looping: c.looping,
            events: raw(aevt, events_len, c.id),
            root: raw(rootb, root_len, c.id),
            feet: feet(&c),
        })
        .collect())
}

pub use crate::ui::Level;

/// One problem `anim check` (and the editor) reports.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Issue {
    pub level: Level,
    /// What it is about: `state "hop"`, `transition 2`, `param "speed"`.
    pub at: String,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: {}: {}", self.at, self.message)
    }
}

/// Validate a graph: names (states, params, clips), parameter kinds per
/// use, limits, blend points, events past their clip's end, unreachable
/// states and dead ends, in the base and in every layer. `clips` = the
/// model's (`clip_facts`), `None` when the model is unknown (then clip
/// checks are skipped and reported). `sound` says whether `sounds/<x>`
/// exists for `sfx:<x>` events.
pub fn check(
    def: &AnimGraphDef,
    clips: Option<&[ClipFacts]>,
    sound: &dyn Fn(&str) -> bool,
) -> Vec<Issue> {
    check_with(def, clips, None, sound)
}

/// A model's joint as layer masks see it: its glTF node name and its
/// parent (an earlier joint: TMDL order, parents first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Joint {
    pub name: String,
    pub parent: Option<usize>,
}

impl Joint {
    /// Joints from names and parents (`parents[j]`, `None` for a root).
    pub fn list(names: &[&str], parents: &[Option<usize>]) -> Vec<Joint> {
        names
            .iter()
            .enumerate()
            .map(|(j, n)| Joint {
                name: n.to_string(),
                parent: parents.get(j).copied().flatten().filter(|&p| p < j),
            })
            .collect()
    }
}

/// [`check`] knowing the model's joints (`joints`, TMDL order): layer
/// masks are checked against them.
pub fn check_with(
    def: &AnimGraphDef,
    clips: Option<&[ClipFacts]>,
    joints: Option<&[Joint]>,
    sound: &dyn Fn(&str) -> bool,
) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut err = |at: String, m: String| {
        out.push(Issue {
            level: Level::Error,
            at,
            message: m,
        })
    };
    if def.states.is_empty() {
        err("graph".into(), "has no states".into());
    }
    let total: usize = def.machines().iter().map(|m| m.states.len()).sum();
    if total > g::MAX_STATES {
        err(
            "graph".into(),
            format!("{total} states (at most {})", g::MAX_STATES),
        );
    }
    if def.params.len() > g::MAX_PARAMS {
        err(
            "graph".into(),
            format!(
                "{} parameters (at most {})",
                def.params.len(),
                g::MAX_PARAMS
            ),
        );
    }
    if def.layers.len() > g::MAX_LAYERS {
        err(
            "graph".into(),
            format!("{} layers (at most {})", def.layers.len(), g::MAX_LAYERS),
        );
    }
    if let Some(s) = &def.start
        && def.state(s).is_none()
    {
        err("start".into(), format!("names no state (`{s}`)"));
    }
    if clips.is_none() {
        err(
            "model".into(),
            format!(
                "`{}` is not a baked animated model: clips not checked",
                def.model
            ),
        );
    }
    let kind = |p: &str| def.param(p);
    let float_param =
        |at: &str, p: &str, what: &str, err: &mut dyn FnMut(String, String)| match kind(p) {
            Some(ParamDef::Float(_)) => {}
            Some(_) => err(at.into(), format!("{what} `{p}` must be a Float parameter")),
            None => err(at.into(), format!("{what} `{p}` is not a parameter")),
        };
    // Layers: names, masks, weights.
    let mut masks: Vec<(String, u32)> = Vec::new();
    for (i, l) in def.layers.iter().enumerate() {
        let at = format!("layer \"{}\"", l.name);
        if def.layers[..i].iter().any(|o| o.name == l.name) || l.name.is_empty() {
            err(
                at.clone(),
                "layer names must be unique and not empty".into(),
            );
        }
        if l.states.is_empty() {
            err(at.clone(), "a layer needs states".into());
        }
        if let Some(s) = &l.start
            && l.state(s).is_none()
        {
            err(at.clone(), format!("start names no state (`{s}`)"));
        }
        match &l.weight {
            WeightDef::Value(v) if !(0.0..=1.0).contains(v) => {
                err(at.clone(), format!("weight {v} is outside 0..1"))
            }
            WeightDef::Param(p) => float_param(&at, p, "weight", &mut err),
            _ => {}
        }
        match (joints, mask_bits(&l.mask, joints.unwrap_or(&[]))) {
            (_, Ok(bits)) if l.mode == LayerModeDef::Override => masks.push((l.name.clone(), bits)),
            (_, Ok(_)) => {}
            (None, Err(_)) => err(
                at.clone(),
                "its mask names joints, but the model's joints are unknown".into(),
            ),
            (Some(_), Err(e)) => err(at.clone(), e),
        }
    }
    // Two override layers on the same joint: the upper one hides the lower.
    let mut overlaps = Vec::new();
    for (i, (a, ma)) in masks.iter().enumerate() {
        for (b, mb) in &masks[i + 1..] {
            if ma & mb != 0 {
                overlaps.push(Issue {
                    level: Level::Warning,
                    at: format!("layer \"{b}\""),
                    message: format!(
                        "overrides {} joint(s) layer `{a}` also overrides: at full weight it hides them",
                        (ma & mb).count_ones()
                    ),
                });
            }
        }
    }
    for m in def.machines() {
        check_machine(def, &m, clips, sound, &mut out);
    }
    out.extend(overlaps);
    out
}

/// A layer mask's bits from joint names (empty: every joint); `"x/*"` is
/// joint `x` and everything under it.
pub fn mask_bits(names: &[String], joints: &[Joint]) -> Result<u32, String> {
    if names.is_empty() {
        return Ok(u32::MAX);
    }
    let mut bits = 0u32;
    for n in names {
        bits |= entry_bits(n, joints)?;
    }
    Ok(bits)
}

/// One mask entry's bits: a joint, or (`"x/*"`) a joint and its subtree.
fn entry_bits(entry: &str, joints: &[Joint]) -> Result<u32, String> {
    let (name, subtree) = match entry.strip_suffix("/*") {
        Some(root) => (root, true),
        None => (entry, false),
    };
    if name.is_empty() || name.contains('*') {
        return Err(format!(
            "mask: `{entry}` is not a joint name or `<joint>/*` (a joint and every joint under it)"
        ));
    }
    let Some(root) = joints.iter().position(|j| j.name == name) else {
        let names: Vec<&str> = joints.iter().map(|j| j.name.as_str()).collect();
        return Err(format!(
            "mask: no joint `{name}` in the model (joints: {})",
            names.join(", ")
        ));
    };
    let mut set = vec![false; joints.len()];
    set[root] = true;
    if subtree {
        // Parents come first: one pass finds every descendant.
        for j in root + 1..joints.len() {
            if let Some(p) = joints[j].parent
                && p < j
                && set[p]
            {
                set[j] = true;
            }
        }
    }
    let mut bits = 0u32;
    for (j, _) in set.iter().enumerate().filter(|(_, on)| **on) {
        if j >= 32 {
            return Err(format!(
                "mask: joint `{}` (in `{entry}`) is past the 32 a mask holds",
                joints[j].name
            ));
        }
        bits |= 1 << j;
    }
    Ok(bits)
}

/// Turn joint `j` on or off in a layer mask as the editor's picker does,
/// keeping the entries as written where it can: `subtree` toggles `"x/*"`
/// (the joint and everything under it). A joint covered only by a subtree
/// entry it is turned off from splits that entry into the joints it held.
/// An empty mask (every joint) becomes explicit first; a mask that comes
/// to cover every joint becomes empty again.
pub fn mask_toggle(mask: &mut Vec<String>, joints: &[Joint], j: usize, subtree: bool) {
    let Some(name) = joints.get(j).map(|x| x.name.clone()) else {
        return;
    };
    let n = joints.len().min(32);
    let full = if n == 32 { u32::MAX } else { (1u32 << n) - 1 };
    let bits = |m: &[String]| mask_bits(m, joints).unwrap_or(0) & full;
    if mask.is_empty() {
        *mask = joints.iter().take(32).map(|x| x.name.clone()).collect();
    }
    let target = if subtree {
        entry_bits(&format!("{name}/*"), joints).unwrap_or(0)
    } else {
        1 << j.min(31)
    };
    let on = bits(mask) & target == target;
    if on {
        // Off: drop entries inside the target, split those that cover it.
        let mut out = Vec::new();
        for e in mask.iter() {
            let eb = entry_bits(e, joints).unwrap_or(0);
            if eb & !target == 0 {
                continue;
            }
            if eb & target == 0 {
                out.push(e.clone());
                continue;
            }
            for (k, x) in joints.iter().enumerate().take(32) {
                if eb & !target & (1 << k) != 0 {
                    out.push(x.name.clone());
                }
            }
        }
        out.dedup();
        *mask = out;
    } else {
        mask.retain(|e| entry_bits(e, joints).is_ok_and(|eb| eb & !target != 0));
        mask.push(if subtree { format!("{name}/*") } else { name });
    }
    // Joint order (subtree entries at their root), as the file reads best.
    let key = |e: &String| {
        let root = e.strip_suffix("/*").unwrap_or(e);
        joints
            .iter()
            .position(|x| x.name == root)
            .unwrap_or(usize::MAX)
    };
    mask.sort_by_key(key);
    mask.dedup();
    if bits(mask) == full {
        mask.clear();
    }
}

/// One machine's states and transitions (`check_with`).
fn check_machine(
    def: &AnimGraphDef,
    m: &Machine,
    clips: Option<&[ClipFacts]>,
    sound: &dyn Fn(&str) -> bool,
    out: &mut Vec<Issue>,
) {
    let mut err = |at: String, msg: String| {
        out.push(Issue {
            level: Level::Error,
            at,
            message: msg,
        })
    };
    let state_at = |n: &str| m.at(&format!("state \"{n}\""));
    let kind = |p: &str| def.param(p);
    let clip =
        |name: &str| clips.and_then(|cs| cs.iter().find(|c| c.id == ClipId::from_name(name)));
    let float_param =
        |at: &str, p: &str, what: &str, err: &mut dyn FnMut(String, String)| match kind(p) {
            Some(ParamDef::Float(_)) => {}
            Some(_) => err(at.into(), format!("{what} `{p}` must be a Float parameter")),
            None => err(at.into(), format!("{what} `{p}` is not a parameter")),
        };
    for (name, s) in m.states {
        let at = state_at(name);
        for c in s.clips() {
            if clips.is_some() && clip(c).is_none() {
                err(at.clone(), format!("clip `{c}` is not in {}", def.model));
            }
        }
        match s {
            StateDef::Clip {
                clip: c,
                speed_param,
                events,
                looping,
                ..
            } => {
                if let Some(p) = speed_param {
                    float_param(&at, p, "speed_param", &mut err);
                }
                if let Some(f) = clip(c) {
                    let looping = looping.unwrap_or(f.looping);
                    for (t, e) in events {
                        if !(*t >= 0.0 && *t <= f.duration + 1e-4) {
                            err(
                                at.clone(),
                                format!(
                                    "event `{e}` at {t} s is past the end of clip `{c}` ({:.3} s)",
                                    f.duration
                                ),
                            );
                        } else if looping && *t >= f.duration - 1e-4 && *t > 0.0 {
                            err(
                                at.clone(),
                                format!(
                                    "event `{e}` at the end of a loop: write it at 0 (the same instant)"
                                ),
                            );
                        }
                    }
                }
                for (_, e) in events {
                    if let Some(snd) = e.strip_prefix("sfx:")
                        && !sound(snd)
                    {
                        err(at.clone(), format!("event `{e}`: no sound `sounds/{snd}`"));
                    }
                }
            }
            StateDef::Blend1D {
                param,
                points,
                wrap,
                speed_param,
                ..
            } => {
                float_param(&at, param, "param", &mut err);
                if let Some(p) = speed_param {
                    float_param(&at, p, "speed_param", &mut err);
                }
                if points.is_empty() {
                    err(at.clone(), "a blend space needs points".into());
                }
                if points.len() > g::MAX_POINTS {
                    err(
                        at.clone(),
                        format!("{} points (at most {})", points.len(), g::MAX_POINTS),
                    );
                }
                if points
                    .windows(2)
                    .any(|w| w[0].0 >= w[1].0 || w[0].0.is_nan() || w[1].0.is_nan())
                {
                    err(
                        at.clone(),
                        "points must be in increasing order of value".into(),
                    );
                }
                if let Some(w) = wrap
                    && let (Some(a), Some(b)) = (points.first(), points.last())
                    && (*w <= b.0 - a.0 || w.is_nan())
                {
                    err(
                        at.clone(),
                        format!("wrap {w} must exceed the points' span ({})", b.0 - a.0),
                    );
                }
            }
            StateDef::Blend2D {
                x,
                y,
                points,
                speed_param,
                ..
            } => {
                float_param(&at, x, "x", &mut err);
                float_param(&at, y, "y", &mut err);
                if x == y {
                    err(at.clone(), "x and y must be two parameters".into());
                }
                if let Some(p) = speed_param {
                    float_param(&at, p, "speed_param", &mut err);
                }
                if points.len() > g::MAX_POINTS {
                    err(
                        at.clone(),
                        format!("{} points (at most {})", points.len(), g::MAX_POINTS),
                    );
                }
                match triangulate(points) {
                    Ok(t) if t.len() > g::MAX_TRIANGLES => err(
                        at.clone(),
                        format!("{} triangles (at most {})", t.len(), g::MAX_TRIANGLES),
                    ),
                    Ok(_) => {}
                    Err(e) => err(at.clone(), e),
                }
            }
        }
    }
    let state_names: BTreeSet<&str> = m.states.iter().map(|(n, _)| n.as_str()).collect();
    for (i, t) in m.transitions.iter().enumerate() {
        let at = m.at(&format!("transition {i}"));
        if let From::State(f) = &t.from
            && !state_names.contains(f.as_str())
        {
            err(at.clone(), format!("from `{f}`: no such state"));
        }
        if !state_names.contains(t.to.as_str()) {
            err(at.clone(), format!("to `{}`: no such state", t.to));
        }
        if t.blend < 0.0 || t.blend.is_nan() {
            err(at.clone(), "blend must be 0 or more seconds".into());
        }
        if t.mode == TransitionMode::Inertial && t.blend <= 0.0 {
            err(
                at.clone(),
                "an Inertial transition needs a blend (its decay) above 0".into(),
            );
        }
        if t.when.depth() > g::MAX_COND_DEPTH {
            err(
                at.clone(),
                format!("condition nested deeper than {}", g::MAX_COND_DEPTH),
            );
        }
        t.when.visit(&mut |c| {
            let p = match c {
                CondDef::Trigger(p) => Some((p, "Trigger")),
                CondDef::Param(p) => Some((p, "Param")),
                CondDef::Greater(p, _) | CondDef::Less(p, _) => Some((p, "Greater/Less")),
                _ => None,
            };
            if let Some((p, what)) = p {
                match (kind(p), c) {
                    (None, _) => err(at.clone(), format!("`{p}` is not a parameter")),
                    (Some(ParamDef::Trigger), CondDef::Trigger(_)) => {}
                    (Some(_), CondDef::Trigger(_)) => err(
                        at.clone(),
                        format!("Trigger(`{p}`) needs a Trigger parameter"),
                    ),
                    (Some(ParamDef::Trigger), _) => err(
                        at.clone(),
                        format!("{what}(`{p}`): `{p}` is a Trigger, use Trigger(..)"),
                    ),
                    (Some(ParamDef::Bool(_)), CondDef::Greater(..) | CondDef::Less(..)) => err(
                        at.clone(),
                        format!("{what}(`{p}`) compares a Bool; use Param"),
                    ),
                    _ => {}
                }
            }
        });
    }
    // Reachability from the start.
    if !m.states.is_empty() {
        let start = m.states[m.start.min(m.states.len() - 1)].0.as_str();
        let mut seen = BTreeSet::from([start]);
        let any: Vec<&str> = m
            .transitions
            .iter()
            .filter(|t| t.from == From::Any)
            .map(|t| t.to.as_str())
            .collect();
        seen.extend(any.iter().copied());
        loop {
            let before = seen.len();
            for t in m.transitions {
                if let From::State(f) = &t.from
                    && seen.contains(f.as_str())
                {
                    seen.insert(t.to.as_str());
                }
            }
            if seen.len() == before {
                break;
            }
        }
        for (name, _) in m.states {
            if !seen.contains(name.as_str()) {
                out.push(Issue {
                    level: Level::Warning,
                    at: state_at(name),
                    message: "unreachable: no transition from the start leads here".into(),
                });
            }
        }
        // A one-shot state no transition leaves holds its last frame
        // forever.
        for (name, s) in m.states {
            let leaves = m.transitions.iter().any(|t| match &t.from {
                From::Any => t.to != *name,
                From::State(f) => f == name,
            });
            let once = match s {
                StateDef::Clip {
                    clip: c, looping, ..
                } => clip(c).is_some_and(|f| !looping.unwrap_or(f.looping)),
                _ => false,
            };
            if once && !leaves {
                out.push(Issue {
                    level: Level::Warning,
                    at: state_at(name),
                    message: "a one-shot clip with no exit: it holds its last frame forever (add a Finished transition)".into(),
                });
            }
        }
    }
}

/// The Delaunay triangulation of a 2D blend space's points (point
/// indices, counter-clockwise, in a deterministic order): every triangle
/// whose circumcircle holds no other point, overlapping ones (cocircular
/// points) dropped. Errors: fewer than three points, duplicates, all on a
/// line.
pub fn triangulate(points: &[(f32, f32, String)]) -> Result<Vec<[u8; 3]>, String> {
    if points.len() < 3 {
        return Err("a 2D blend space needs at least three points".into());
    }
    if points.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
        return Err("points must be finite".into());
    }
    let p: Vec<(f64, f64)> = points.iter().map(|q| (q.0 as f64, q.1 as f64)).collect();
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            if (p[i].0 - p[j].0).abs() < 1e-6 && (p[i].1 - p[j].1).abs() < 1e-6 {
                return Err(format!(
                    "points {i} and {j} are at the same place ({}, {})",
                    points[i].0, points[i].1
                ));
            }
        }
    }
    let cross = |a: usize, b: usize, c: usize| {
        (p[b].0 - p[a].0) * (p[c].1 - p[a].1) - (p[b].1 - p[a].1) * (p[c].0 - p[a].0)
    };
    let span = p
        .iter()
        .flat_map(|q| [q.0.abs(), q.1.abs()])
        .fold(1e-9, f64::max);
    let eps = 1e-9 * span * span;
    let mut tris: Vec<[usize; 3]> = Vec::new();
    for a in 0..p.len() {
        for b in a + 1..p.len() {
            for c in b + 1..p.len() {
                let d = cross(a, b, c);
                if d.abs() <= eps {
                    continue;
                }
                let t = if d > 0.0 { [a, b, c] } else { [a, c, b] };
                // Strictly inside the circumcircle (counter-clockwise t).
                let inside = |k: usize| {
                    let [i, j, l] = t;
                    let (ax, ay) = (p[i].0 - p[k].0, p[i].1 - p[k].1);
                    let (bx, by) = (p[j].0 - p[k].0, p[j].1 - p[k].1);
                    let (cx, cy) = (p[l].0 - p[k].0, p[l].1 - p[k].1);
                    let det = (ax * ax + ay * ay) * (bx * cy - cx * by)
                        - (bx * bx + by * by) * (ax * cy - cx * ay)
                        + (cx * cx + cy * cy) * (ax * by - bx * ay);
                    det > eps * span * span
                };
                if !(0..p.len()).any(|k| k != a && k != b && k != c && inside(k)) {
                    tris.push(t);
                }
            }
        }
    }
    if tris.is_empty() {
        return Err("the points are all on one line: nothing to triangulate".into());
    }
    // Cocircular points give crossing triangles: keep the first of each
    // overlapping pair.
    let centroid = |t: &[usize; 3]| {
        (
            (p[t[0]].0 + p[t[1]].0 + p[t[2]].0) / 3.0,
            (p[t[0]].1 + p[t[1]].1 + p[t[2]].1) / 3.0,
        )
    };
    let holds = |t: &[usize; 3], q: (f64, f64)| {
        (0..3).all(|e| {
            let (a, b) = (p[t[e]], p[t[(e + 1) % 3]]);
            (b.0 - a.0) * (q.1 - a.1) - (b.1 - a.1) * (q.0 - a.0) > eps
        })
    };
    let crosses = |s: &[usize; 3], t: &[usize; 3]| {
        let seg = |a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)| {
            let o = |p: (f64, f64), q: (f64, f64), r: (f64, f64)| {
                (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
            };
            o(a, b, c) * o(a, b, d) < -eps && o(c, d, a) * o(c, d, b) < -eps
        };
        (0..3).any(|i| (0..3).any(|j| seg(p[s[i]], p[s[(i + 1) % 3]], p[t[j]], p[t[(j + 1) % 3]])))
            || holds(s, centroid(t))
            || holds(t, centroid(s))
    };
    let mut kept: Vec<[usize; 3]> = Vec::new();
    for t in tris {
        if !kept.iter().any(|k| crosses(k, &t)) {
            kept.push(t);
        }
    }
    Ok(kept.into_iter().map(|t| t.map(|i| i as u8)).collect())
}

/// Bake a graph to `TANG` against its model's clips. Errors are
/// [`check`]'s errors, joined. A graph with no layers, 2D spaces or
/// inertialized transitions bakes to `TANG` v1 (the bytes #65 baked).
pub fn to_tang(
    def: &AnimGraphDef,
    clips: &[ClipFacts],
    sound: &dyn Fn(&str) -> bool,
) -> Result<Vec<u8>, String> {
    to_tang_with(def, clips, None, sound)
}

/// [`to_tang`] with the model's joint names, which layer masks need.
pub fn to_tang_with(
    def: &AnimGraphDef,
    clips: &[ClipFacts],
    joints: Option<&[Joint]>,
    sound: &dyn Fn(&str) -> bool,
) -> Result<Vec<u8>, String> {
    to_tang_bound(def, clips, joints, &[], sound)
}

/// One more model a graph serves ([`AnimGraphDef::models`]): its path, its
/// clips and its joints.
pub struct BoundModel<'a> {
    pub path: String,
    pub clips: &'a [ClipFacts],
    pub joints: Option<&'a [Joint]>,
}

/// Each bound model has every clip the graph plays, as long, and each
/// layer mask's joints.
pub fn check_binds(
    def: &AnimGraphDef,
    clips: &[ClipFacts],
    binds: &[BoundModel],
) -> Result<(), String> {
    let used: Vec<&str> = def
        .machines()
        .iter()
        .flat_map(|m| m.states.iter().flat_map(|(_, s)| s.clips()))
        .collect();
    for b in binds {
        for c in &used {
            let id = ClipId::from_name(c);
            let own = clips.iter().find(|f| f.id == id);
            match b.clips.iter().find(|f| f.id == id) {
                None => return Err(format!("models: `{}` has no clip `{c}`", b.path)),
                Some(f) if own.is_some_and(|o| (o.duration - f.duration).abs() > 1e-4) => {
                    return Err(format!(
                        "models: `{}`'s clip `{c}` lasts {:.3} s, the graph's model's {:.3} s \
                         (a graph keeps one duration per clip)",
                        b.path,
                        f.duration,
                        own.map_or(0.0, |o| o.duration)
                    ));
                }
                Some(_) => {}
            }
        }
        for l in &def.layers {
            mask_bits(&l.mask, b.joints.unwrap_or(&[]))
                .map_err(|e| format!("models: `{}`, layer `{}`: {e}", b.path, l.name))?;
        }
    }
    Ok(())
}

/// [`to_tang_with`] for a graph that serves more models (v3 binds).
pub fn to_tang_bound(
    def: &AnimGraphDef,
    clips: &[ClipFacts],
    joints: Option<&[Joint]>,
    binds: &[BoundModel],
    sound: &dyn Fn(&str) -> bool,
) -> Result<Vec<u8>, String> {
    check_binds(def, clips, binds)?;
    let errors: Vec<String> = check_with(def, Some(clips), joints, sound)
        .into_iter()
        .filter(|i| i.level == Level::Error)
        .map(|i| i.to_string())
        .collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let v2 = def.uses_v2();
    let v3 = def.uses_v3();
    let machines = def.machines();
    // Each machine's first state and transition in the shared tables.
    let mut s0 = Vec::new();
    let mut t0 = Vec::new();
    let (mut ns, mut nt) = (0usize, 0usize);
    for m in &machines {
        s0.push(ns);
        t0.push(nt);
        ns += m.states.len();
        nt += m.transitions.len();
    }
    let param_index = |p: &str| def.params.iter().position(|(n, _)| n == p).unwrap() as u16;
    // The clips the graph plays, in first-use order.
    let mut used: Vec<&str> = Vec::new();
    for m in &machines {
        for (_, s) in m.states {
            for c in s.clips() {
                if !used.contains(&c) {
                    used.push(c);
                }
            }
        }
    }
    let clip_index = |c: &str| used.iter().position(|u| *u == c).unwrap() as u16;
    let facts = |c: &str| clips.iter().find(|f| f.id == ClipId::from_name(c)).unwrap();
    // `feet()` reads the base's clips only: a clip only layers play needs
    // no feet track (the console's RAM), nor root motion.
    let base_clips: Vec<&str> = def.states.iter().flat_map(|(_, s)| s.clips()).collect();

    let head = g::HEADER_BYTES
        + def.params.len() * g::PARAM_BYTES
        + used.len() * g::CLIP_BYTES
        + ns * g::STATE_BYTES
        + nt * g::TRANSITION_BYTES;
    let mut data: Vec<u8> = Vec::new();
    let mut put = |bytes: &[u8]| -> u32 {
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let at = (head + data.len()) as u32;
        data.extend_from_slice(bytes);
        at
    };
    let u32le = |v: u32| v.to_le_bytes();
    let mut table = Vec::with_capacity(head);
    table.extend_from_slice(g::TANG_MAGIC);
    table.extend_from_slice(&u32le(if v3 {
        3
    } else if v2 {
        2
    } else {
        1
    }));
    table.extend_from_slice(&u32le(trino_core::asset_id(&def.model_path())));
    for n in [
        def.params.len(),
        used.len(),
        ns,
        nt,
        def.start_index(),
        if v2 { def.layers.len() } else { 0 },
    ] {
        table.extend_from_slice(&(n as u16).to_le_bytes());
    }
    // The layers' offset (patched below), the base's share.
    let layers_at_field = table.len();
    table.extend_from_slice(&[0; 4]);
    if v2 {
        table.extend_from_slice(&(def.states.len() as u16).to_le_bytes());
        table.extend_from_slice(&(def.transitions.len() as u16).to_le_bytes());
    } else {
        table.extend_from_slice(&[0; 4]);
    }
    for (name, p) in &def.params {
        table.extend_from_slice(&u32le(trino_core::asset_id(name)));
        let (kind, v) = match p {
            ParamDef::Float(v) => (0u8, *v),
            ParamDef::Bool(b) => (1, if *b { 1.0 } else { 0.0 }),
            ParamDef::Trigger => (2, 0.0),
        };
        table.extend_from_slice(&[kind, 0, 0, 0]);
        table.extend_from_slice(&v.to_le_bytes());
    }
    for c in &used {
        let f = facts(c);
        table.extend_from_slice(&u32le(f.id.0));
        table.extend_from_slice(&f.duration.to_le_bytes());
        table.extend_from_slice(&[f.looping as u8, 0, 0, 0]);
        let base = base_clips.contains(c);
        let ev = f.events.as_ref().map_or(g::NONE32, |b| put(b));
        let root = f
            .root
            .as_ref()
            .filter(|_| base)
            .map_or(g::NONE32, |b| put(b));
        let feet = f
            .feet
            .as_ref()
            .filter(|_| base)
            .map_or(g::NONE32, |b| put(b));
        table.extend_from_slice(&u32le(ev));
        table.extend_from_slice(&u32le(root));
        table.extend_from_slice(&u32le(feet));
    }
    for m in &machines {
        for (name, s) in m.states {
            let StateRow {
                kind,
                flags,
                points,
                speed,
                speed_param,
                param,
                wrap,
                events,
            } = match s {
                StateDef::Clip {
                    clip,
                    looping,
                    speed,
                    speed_param,
                    root_motion,
                    events,
                } => {
                    let mut flags = if *root_motion {
                        g::STATE_ROOT_MOTION
                    } else {
                        0
                    };
                    if let Some(l) = looping {
                        flags |= g::STATE_LOOP_SET | if *l { g::STATE_LOOP_ON } else { 0 };
                    }
                    StateRow {
                        kind: 0,
                        flags,
                        points: vec![(0.0, 0.0, clip_index(clip))],
                        speed: *speed,
                        speed_param: speed_param.as_ref(),
                        param: g::NONE16,
                        wrap: 0.0f32.to_le_bytes(),
                        events,
                    }
                }
                StateDef::Blend1D {
                    param,
                    points,
                    wrap,
                    speed,
                    speed_param,
                    root_motion,
                } => StateRow {
                    kind: 1,
                    flags: if *root_motion {
                        g::STATE_ROOT_MOTION
                    } else {
                        0
                    },
                    points: points
                        .iter()
                        .map(|(v, c)| (*v, 0.0, clip_index(c)))
                        .collect(),
                    speed: *speed,
                    speed_param: speed_param.as_ref(),
                    param: param_index(param),
                    wrap: wrap.unwrap_or(0.0).to_le_bytes(),
                    events: &[],
                },
                StateDef::Blend2D {
                    x,
                    y,
                    points,
                    speed,
                    speed_param,
                    root_motion,
                } => {
                    let tris = triangulate(points).expect("checked");
                    let mut w = param_index(y).to_le_bytes().to_vec();
                    w.extend_from_slice(&(tris.len() as u16).to_le_bytes());
                    StateRow {
                        kind: 2,
                        flags: if *root_motion {
                            g::STATE_ROOT_MOTION
                        } else {
                            0
                        },
                        points: points
                            .iter()
                            .map(|(px, py, c)| (*px, *py, clip_index(c)))
                            .collect(),
                        speed: *speed,
                        speed_param: speed_param.as_ref(),
                        param: param_index(x),
                        wrap: [w[0], w[1], w[2], w[3]],
                        events: &[],
                    }
                }
            };
            let mut pts = Vec::new();
            for (vx, vy, c) in &points {
                pts.extend_from_slice(&vx.to_le_bytes());
                if kind == 2 {
                    pts.extend_from_slice(&vy.to_le_bytes());
                }
                pts.extend_from_slice(&c.to_le_bytes());
                pts.extend_from_slice(&[0, 0]);
            }
            let points_at = put(&pts);
            let tris_at = if let StateDef::Blend2D { points, .. } = s {
                let tris = triangulate(points).expect("checked");
                put(&tris.concat())
            } else {
                0
            };
            let events_at = if events.is_empty() {
                g::NONE32
            } else {
                let mut list: Vec<&(f32, String)> = events.iter().collect();
                list.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut b = (list.len() as u32).to_le_bytes().to_vec();
                for (t, n) in list {
                    b.extend_from_slice(&t.to_le_bytes());
                    b.extend_from_slice(&EventId::from_name(n).0.to_le_bytes());
                    let snd = n
                        .strip_prefix("sfx:")
                        .map_or(0, |s| trino_core::asset_id(&format!("sounds/{s}")));
                    b.extend_from_slice(&snd.to_le_bytes());
                }
                put(&b)
            };
            table.extend_from_slice(&u32le(trino_core::asset_id(name)));
            table.extend_from_slice(&[kind, flags]);
            table.extend_from_slice(&(points.len() as u16).to_le_bytes());
            table.extend_from_slice(&speed.to_le_bytes());
            table.extend_from_slice(
                &speed_param
                    .map_or(g::NONE16, |p| param_index(p))
                    .to_le_bytes(),
            );
            table.extend_from_slice(&param.to_le_bytes());
            table.extend_from_slice(&wrap);
            table.extend_from_slice(&u32le(points_at));
            table.extend_from_slice(&u32le(events_at));
            table.extend_from_slice(&u32le(tris_at));
        }
    }
    for (mi, m) in machines.iter().enumerate() {
        let state_index =
            |n: &str| (s0[mi] + m.states.iter().position(|(x, _)| x == n).unwrap()) as u16;
        for t in m.transitions {
            let from = match &t.from {
                From::Any => g::NONE16,
                From::State(f) => state_index(f),
            };
            let mut to = state_index(&t.to);
            if t.mode == TransitionMode::Inertial {
                to |= g::INERTIAL;
            }
            let mut code = Vec::new();
            encode_cond(&t.when, &param_index, &mut code);
            let cond_at = put(&code);
            table.extend_from_slice(&from.to_le_bytes());
            table.extend_from_slice(&to.to_le_bytes());
            table.extend_from_slice(&t.blend.to_le_bytes());
            table.extend_from_slice(&u32le(cond_at));
        }
    }
    if v3 || (v2 && !def.layers.is_empty()) {
        let mut rec = Vec::new();
        for (i, l) in def.layers.iter().enumerate() {
            let mi = i + 1;
            let mask = mask_bits(&l.mask, joints.unwrap_or(&[]))?;
            let (wp, w) = match &l.weight {
                WeightDef::Value(v) => (g::NONE16, *v),
                WeightDef::Param(p) => (param_index(p), 1.0),
            };
            rec.extend_from_slice(&u32le(trino_core::asset_id(&l.name)));
            rec.extend_from_slice(&u32le(mask));
            let flags = if l.sync { g::LAYER_SYNC } else { 0 };
            rec.extend_from_slice(&[(l.mode == LayerModeDef::Additive) as u8, flags]);
            rec.extend_from_slice(&wp.to_le_bytes());
            rec.extend_from_slice(&w.to_le_bytes());
            for v in [
                s0[mi],
                l.states.len(),
                t0[mi],
                l.transitions.len(),
                s0[mi] + l.start_index(),
                0,
            ] {
                rec.extend_from_slice(&(v as u16).to_le_bytes());
            }
        }
        if v3 {
            // The binds: each other model's id and its mask per layer.
            rec.extend_from_slice(&(binds.len() as u16).to_le_bytes());
            rec.extend_from_slice(&[0, 0]);
            for b in binds {
                rec.extend_from_slice(&u32le(trino_core::asset_id(&b.path)));
                for l in &def.layers {
                    rec.extend_from_slice(&u32le(mask_bits(&l.mask, b.joints.unwrap_or(&[]))?));
                }
            }
        }
        let at = put(&rec);
        table[layers_at_field..layers_at_field + 4].copy_from_slice(&u32le(at));
    }
    debug_assert_eq!(table.len(), head);
    table.extend_from_slice(&data);
    // Round trip through the runtime's own validation.
    AnimGraph::parse(&table).map_err(|e| format!("baked graph does not parse: {e:?}"))?;
    Ok(table)
}

/// One state's record before it is written (`to_tang`).
struct StateRow<'a> {
    kind: u8,
    flags: u8,
    /// (x, y, clip); y only for 2D.
    points: Vec<(f32, f32, u16)>,
    speed: f32,
    speed_param: Option<&'a String>,
    param: u16,
    /// The 1D wrap, or a 2D state's y param and triangle count.
    wrap: [u8; 4],
    events: &'a [(f32, String)],
}

fn encode_cond(c: &CondDef, param: &dyn Fn(&str) -> u16, out: &mut Vec<u8>) {
    match c {
        CondDef::Always => out.push(op::TRUE),
        CondDef::Finished => out.push(op::FINISHED),
        CondDef::Trigger(p) | CondDef::Param(p) => {
            out.push(if matches!(c, CondDef::Trigger(_)) {
                op::TRIGGER
            } else {
                op::PARAM
            });
            out.extend_from_slice(&param(p).to_le_bytes());
        }
        CondDef::Greater(p, v) | CondDef::Less(p, v) => {
            out.push(if matches!(c, CondDef::Greater(..)) {
                op::GREATER
            } else {
                op::LESS
            });
            out.extend_from_slice(&param(p).to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        CondDef::After(s) => {
            out.push(op::AFTER);
            out.extend_from_slice(&s.to_le_bytes());
        }
        CondDef::Not(x) => {
            out.push(op::NOT);
            encode_cond(x, param, out);
        }
        CondDef::And(v) | CondDef::Or(v) => {
            out.push(if matches!(c, CondDef::And(_)) {
                op::AND
            } else {
                op::OR
            });
            out.push(v.len().min(255) as u8);
            for x in v.iter().take(255) {
                encode_cond(x, param, out);
            }
        }
    }
}

/// What a tier folds of a graph (issue #112, `anim check --platform`): the
/// same rules [`trino_core::anim::AnimPose::fold`] applies at run time,
/// worked out for the graph's worst case, so the fallback a console plays
/// is written down, never a silent difference.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FoldNote {
    /// What it is about: `layer "upper"`, `transition 2`, `pose`.
    pub at: String,
    pub message: String,
}

/// Fold notes for a tier's `caps` (empty: the graph plays whole there).
pub fn fold_report(def: &AnimGraphDef, caps: trino_core::anim::AnimLayers) -> Vec<FoldNote> {
    let mut out = Vec::new();
    let keep = (caps.layers.max(1) as usize).min(def.layers.len() + 1);
    let aref = caps.layers <= 1 && caps.clips <= 3;
    for l in def.layers.iter().skip(keep - 1) {
        out.push(FoldNote {
            at: format!("layer \"{}\"", l.name),
            message: format!(
                "dropped: this tier plays {} layer(s); the base's pose draws",
                caps.layers.max(1)
            ),
        });
    }
    // The worst case per machine: a crossfade between its two widest
    // states, or an inertialized one (its pose plus three frozen clips
    // each way).
    let mut worst = 0usize;
    let mut inertial_notes = Vec::new();
    for (mi, m) in def.machines().into_iter().take(keep).enumerate() {
        let width = |n: &str| m.state(n).map_or(1, StateDef::width);
        let widest = m.states.iter().map(|(_, s)| s.width()).max().unwrap_or(1);
        let mut w = widest;
        for (i, t) in m.transitions.iter().enumerate() {
            let from = match &t.from {
                From::State(f) => width(f),
                From::Any => widest,
            };
            let to = width(&t.to);
            let n = match t.mode {
                TransitionMode::Crossfade if t.blend > 0.0 => from + to,
                TransitionMode::Inertial => {
                    inertial_notes.push((mi, i, from.min(3), to));
                    to + from.min(3) + to
                }
                _ => to,
            };
            w = w.max(n);
        }
        worst += w;
    }
    for (mi, i, from, to) in inertial_notes {
        let m = &def.machines()[mi];
        if aref || worst > caps.clips as usize {
            out.push(FoldNote {
                at: m.at(&format!("transition {i}")),
                message: format!(
                    "inertialized -> a crossfade from the frozen source ({from} + {to} clips): no offsets on this tier"
                ),
            });
        }
    }
    if worst > caps.clips as usize {
        out.push(FoldNote {
            at: "pose".into(),
            message: format!(
                "up to {worst} clips at once fold to {}: the lightest go, each layer renormalized (a 2D space toward its nearest edge)",
                caps.clips
            ),
        });
    }
    out
}

/// A script for [`simulate`]: parameter changes at times.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename = "Script", deny_unknown_fields)]
pub struct ScriptDef {
    /// Seconds per update (default 1/60).
    #[serde(default = "sixtieth")]
    pub dt: f32,
    /// Seconds to run.
    pub duration: f32,
    #[serde(default)]
    pub steps: Vec<StepDef>,
}

fn sixtieth() -> f32 {
    1.0 / 60.0
}

/// At `at` seconds, set parameters (floats; bools 0/1) and fire triggers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StepDef {
    pub at: f32,
    #[serde(default, with = "ordered", skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<(String, f32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger: Vec<String>,
}

impl ScriptDef {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|e| e.to_string())
    }
}

/// One stretch of time in a state.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Segment {
    /// The layer (`None`: the base).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    pub state: String,
    pub from: f32,
    pub to: f32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TraceTransition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    pub at: f32,
    pub from: String,
    pub to: String,
    /// An inertialized transition.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inertial: bool,
}

/// A layer's share of a pose sample.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LayerSample {
    pub layer: String,
    pub state: String,
    /// Its weight now (0: left out of the pose).
    pub weight: f32,
    pub clips: Vec<(String, f32, f32)>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TraceEvent {
    pub at: f32,
    pub name: String,
    pub clip: String,
    /// The clip's time when it fired.
    pub time: f32,
    pub weight: f32,
}

/// A sample of the pose (every `sample_every` updates).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TraceSample {
    pub at: f32,
    pub state: String,
    /// (clip, time, weight) of each clip in the base's pose.
    pub clips: Vec<(String, f32, f32)>,
    /// Each layer's (issue #112).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerSample>,
    /// What's left of the base's inertialized offset (1 → 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inertia: Option<f32>,
    /// `AnimPose::digest` of the whole pose.
    pub pose: u32,
}

/// What a script did to a graph (`anim simulate`'s JSON).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Trace {
    pub graph: String,
    pub updates: usize,
    pub segments: Vec<Segment>,
    pub transitions: Vec<TraceTransition>,
    pub events: Vec<TraceEvent>,
    /// Root motion summed over the run, in the start's frame (x, z, yaw).
    pub root: (f32, f32, f32),
    pub samples: Vec<TraceSample>,
    /// The runtime's digest after the last update (`AnimGraphInstance::digest`).
    pub digest: u32,
    /// Parameters a step named that the graph lacks.
    pub unknown: Vec<String>,
}

/// Run `script` through the runtime on the baked `tang` (names from `def`
/// and the model's clip names). Samples the pose every `sample_every`
/// updates.
pub fn simulate(
    def: &AnimGraphDef,
    tang: &[u8],
    clip_names: &[String],
    script: &ScriptDef,
    sample_every: usize,
) -> Result<Trace, String> {
    let graph = AnimGraph::parse(tang).map_err(|e| format!("graph: {e:?}"))?;
    let mut inst = AnimGraphInstance::new(&graph);
    // State names by global index (the base's, then each layer's).
    let all_states: Vec<String> = def
        .machines()
        .iter()
        .flat_map(|m| m.states.iter().map(|(n, _)| n.clone()))
        .collect();
    let state_name = |i: usize| all_states.get(i).cloned().unwrap_or_else(|| "?".into());
    let layer_name = |l: usize| (l > 0).then(|| def.layers[l - 1].name.clone());
    let clip_name = |id: ClipId| {
        clip_names
            .iter()
            .find(|n| ClipId::from_name(n) == id)
            .cloned()
            .unwrap_or_else(|| format!("{:#010x}", id.0))
    };
    let mut event_names: Vec<String> = Vec::new();
    for m in def.machines() {
        for (_, s) in m.states {
            if let StateDef::Clip { events, .. } = s {
                event_names.extend(events.iter().map(|(_, n)| n.clone()));
            }
        }
    }
    let machines = 1 + inst.layer_count();
    let dt = if script.dt > 0.0 {
        script.dt
    } else {
        sixtieth()
    };
    let updates = (script.duration / dt).round().max(0.0) as usize;
    let mut steps: Vec<&StepDef> = script.steps.iter().collect();
    steps.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut next = 0;
    let mut trace = Trace {
        graph: def.model_path(),
        updates,
        segments: (0..machines)
            .map(|l| Segment {
                layer: layer_name(l),
                state: state_name(inst.layer_state(l)),
                from: 0.0,
                to: 0.0,
            })
            .collect(),
        transitions: Vec::new(),
        events: Vec::new(),
        root: (0.0, 0.0, 0.0),
        samples: Vec::new(),
        digest: 0,
        unknown: Vec::new(),
    };
    let mut pose = trino_core::anim::RootPose::IDENTITY;
    for u in 0..updates {
        let t = u as f32 * dt;
        while next < steps.len() && steps[next].at <= t + dt * 0.5 {
            for (p, v) in &steps[next].set {
                if !inst.set(&graph, p, *v) && !trace.unknown.contains(p) {
                    trace.unknown.push(p.clone());
                }
            }
            for p in &steps[next].trigger {
                if !inst.set(&graph, p, 1.0) && !trace.unknown.contains(p) {
                    trace.unknown.push(p.clone());
                }
            }
            next += 1;
        }
        let before: Vec<usize> = (0..machines).map(|l| inst.layer_state(l)).collect();
        inst.update(&graph, dt);
        let now = t + dt;
        for e in inst.events().iter() {
            let name = event_names
                .iter()
                .find(|n| EventId::from_name(n) == e.id)
                .cloned()
                .unwrap_or_else(|| format!("{:#010x}", e.id.0));
            trace.events.push(TraceEvent {
                at: now,
                name,
                clip: clip_name(e.clip),
                time: e.time,
                weight: e.weight,
            });
        }
        let d = inst.root_motion();
        pose = pose.then(trino_core::anim::RootPose {
            x: d.translation.x,
            z: d.translation.z,
            yaw: d.yaw,
        });
        for (l, &was) in before.iter().enumerate() {
            let is = inst.layer_state(l);
            if is == was {
                continue;
            }
            let name = layer_name(l);
            if let Some(seg) = trace.segments.iter_mut().rev().find(|s| s.layer == name) {
                seg.to = now;
            }
            trace.transitions.push(TraceTransition {
                layer: name.clone(),
                at: now,
                from: state_name(was),
                to: state_name(is),
                inertial: inst.inertia(l).is_some_and(|d| d > 0.99),
            });
            trace.segments.push(Segment {
                layer: name,
                state: state_name(is),
                from: now,
                to: now,
            });
        }
        if sample_every > 0 && u % sample_every == 0 {
            let clips_of = |l: usize| {
                let mut c = [trino_core::anim::Contribution::NONE; g::MAX_CONTRIBUTIONS];
                let n = inst.layer_contributions(&graph, l, &mut c);
                c[..n]
                    .iter()
                    .map(|x| (clip_name(graph.clip(x.clip).id), x.time, x.weight))
                    .collect::<Vec<_>>()
            };
            let pose = inst.pose(&graph);
            trace.samples.push(TraceSample {
                at: now,
                state: state_name(inst.state()),
                clips: clips_of(0),
                layers: (1..machines)
                    .map(|l| {
                        let lw = graph.layer(l);
                        let w = lw.weight
                            * lw.weight_param
                                .map_or(1.0, |p| inst.param(p).clamp(0.0, 1.0));
                        LayerSample {
                            layer: def.layers[l - 1].name.clone(),
                            state: state_name(inst.layer_state(l)),
                            weight: w,
                            clips: clips_of(l),
                        }
                    })
                    .collect(),
                inertia: inst.inertia(0),
                pose: pose.digest(),
            });
        }
    }
    // Each machine's last segment runs to the end.
    for l in 0..machines {
        let name = layer_name(l);
        if let Some(seg) = trace.segments.iter_mut().rev().find(|s| s.layer == name) {
            seg.to = updates as f32 * dt;
        }
    }
    trace.root = (pose.x, pose.z, pose.yaw);
    trace.digest = inst.digest();
    Ok(trace)
}

/// Event names a trace can name: the graph's state events plus names a
/// caller knows (the manifest's clip events).
pub fn name_events(trace: &mut Trace, names: &[String]) {
    for e in &mut trace.events {
        if e.name.starts_with("0x")
            && let Some(n) = names
                .iter()
                .find(|n| format!("{:#010x}", EventId::from_name(n).0) == e.name)
        {
            e.name = n.clone();
        }
    }
}

/// What `scene check` says about a resolved scene's `anim_graph`s (project
/// at `root`): the graph file exists and parses, it animates the entity's
/// own model, and its start state and parameters exist.
pub fn scene_problems(root: &Path, items: &[crate::DrawItem]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        let Some(c) = &item.anim_graph else {
            continue;
        };
        let name = c.graph.strip_prefix("anim/").unwrap_or(&c.graph);
        let file = graph_file(root, name);
        let def = match AnimGraphDef::load(&file) {
            Ok(d) => d,
            Err(e) => {
                out.push(format!("{}: anim_graph `{}`: {e}", item.name, c.graph));
                continue;
            }
        };
        if let Some((_, model)) = &item.model
            && *model != def.model_path()
        {
            out.push(format!(
                "{}: anim_graph `{}` animates {}, the entity draws {model}",
                item.name,
                c.graph,
                def.model_path()
            ));
        }
        if let Some(s) = &c.state
            && def.state(s).is_none()
        {
            out.push(format!(
                "{}: anim_graph `{}` has no state `{s}`",
                item.name, c.graph
            ));
        }
        for p in c.params.keys() {
            if def.param(p).is_none() {
                out.push(format!(
                    "{}: anim_graph `{}` has no parameter `{p}`",
                    item.name, c.graph
                ));
            }
        }
    }
    out
}

/// Play `tang` through its blob and through its decoded tables (issue
/// #138) with a pseudo-random parameter script of `updates` steps from
/// `seed`: every update's digest, pose, root motion and events must agree,
/// and so must a crowd-sized instance (no events) over the tables. `Err`
/// names the first update that differs.
pub fn compare_tables(tang: &[u8], seed: u32, updates: usize) -> Result<(), String> {
    use trino_core::anim::{GraphInstance, GraphTables, ParamKind};
    let g = AnimGraph::parse(tang).map_err(|e| format!("graph: {e:?}"))?;
    let mut memory = vec![std::mem::MaybeUninit::<u8>::uninit(); GraphTables::bytes_for(&g)];
    let arena = trino_core::mem::Arena::new(&mut memory);
    let t = GraphTables::decode_in(&g, &arena).ok_or("the tables outgrew bytes_for")?;
    let mut a = AnimGraphInstance::new(&g);
    let mut b = AnimGraphInstance::new(&t);
    let mut c = GraphInstance::<4, 4, 0>::new(&t);
    let mut rng = seed | 1;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng
    };
    for step in 0..updates {
        for p in 0..g.param_count() {
            let r = next();
            let v = match g.param_info(p).1 {
                ParamKind::Float if r % 6 == 0 => (next() % 1000) as f32 / 100.0 - 1.0,
                ParamKind::Bool if r % 9 == 0 => (next() % 2) as f32,
                ParamKind::Trigger if r % 17 == 0 => 1.0,
                _ => continue,
            };
            a.set_float(p, v);
            b.set_float(p, v);
            c.set_float(p, v);
        }
        let dt = match next() % 20 {
            0 => 0.0,
            1 => 0.25,
            _ => 1.0 / 60.0,
        };
        a.update(&g, dt);
        b.update(&t, dt);
        c.update(&t, dt);
        let fail = |what: &str| Err(format!("update {step}: {what} differs"));
        if a.digest() != b.digest() || a.digest() != c.digest() {
            return fail("the digest");
        }
        let pa = a.pose(&g).digest();
        if pa != b.pose(&t).digest() || pa != c.pose(&t).digest() {
            return fail("the pose");
        }
        if a.anim(&g) != b.anim(&t) {
            return fail("the AnimRef");
        }
        if a.root_motion() != b.root_motion() || a.root_motion() != c.root_motion() {
            return fail("the root motion");
        }
        if a.events().as_slice() != b.events().as_slice() {
            return fail("the events");
        }
        if a.feet(&g) != b.feet(&t) {
            return fail("the feet");
        }
        if (a.state(), a.transitions) != (b.state(), b.transitions) {
            return fail("the state");
        }
    }
    Ok(())
}

/// A baked graph's decoded tables as static Rust data (issue #138, `cargo
/// xtask anim export <name> --rust <out.rs>`): one `pub static <NAME>:
/// GraphTables<'static>` a game `include!`s. A build that plays only
/// exported tables never links the blob parser (`AnimGraph::parse`).
pub fn tables_to_rust(tang: &[u8], name: &str, source: &str) -> Result<String, String> {
    use std::fmt::Write;
    use trino_core::anim::{GraphTables, LayerMode, ParamKind};
    let g = AnimGraph::parse(tang).map_err(|e| format!("graph: {e:?}"))?;
    let mut memory = vec![std::mem::MaybeUninit::<u8>::uninit(); GraphTables::bytes_for(&g)];
    let arena = trino_core::mem::Arena::new(&mut memory);
    let t = GraphTables::decode_in(&g, &arena).ok_or("the tables outgrew bytes_for")?;
    let f = |v: f32| format!("f32::from_bits({:#010x})", v.to_bits());
    let bytes = |b: &[u8]| {
        let mut s = String::from("&[");
        for (i, x) in b.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "{x}");
        }
        s.push(']');
        s
    };
    let opt = |o: Option<usize>| o.map_or("None".to_string(), |v| format!("Some({v})"));
    const A: &str = "trino_core::anim";
    let mut o = String::new();
    let _ = writeln!(
        o,
        "// Generated by `cargo xtask anim export {source} --rust`: the graph's\n\
         // decoded tables (issue #138). Do not edit; export again after a\n\
         // change to the graph. `include!` it: playing these tables links no\n\
         // blob parser.\n\
         #[rustfmt::skip]\n\
         pub static {name}: {A}::GraphTables<'static> = {A}::GraphTables {{"
    );
    let _ = writeln!(o, "    model: {:#010x},", t.model);
    o.push_str("    params: &[\n");
    for (id, kind, d) in t.params {
        let k = match kind {
            ParamKind::Float => "Float",
            ParamKind::Bool => "Bool",
            ParamKind::Trigger => "Trigger",
        };
        let _ = writeln!(
            o,
            "        ({A}::ParamId({:#010x}), {A}::ParamKind::{k}, {}),",
            id.0,
            f(*d)
        );
    }
    o.push_str("    ],\n    clips: &[\n");
    for c in t.clips {
        let _ = writeln!(
            o,
            "        {A}::tables::ClipRec {{ id: trino_core::ClipId({:#010x}), duration: {}, looping: {}, \
             events: {A}::EventTrack::from_records({}), root: {A}::RootTrack::from_parts({}, {}, {}), \
             feet: {A}::FeetTrack::from_parts({}, {}, {}) }},",
            c.id.0,
            f(c.duration),
            c.looping,
            bytes(c.events.records()),
            f(c.root.fps()),
            c.root.frames(),
            bytes(c.root.records()),
            f(c.feet.fps()),
            c.feet.frames(),
            bytes(c.feet.records()),
        );
    }
    o.push_str("    ],\n    states: &[\n");
    for s in t.states {
        let _ = writeln!(
            o,
            "        {A}::GraphState {{ id: {A}::StateId({:#010x}), param: {}, param_y: {}, triangles: {}, \
             tris_at: {}, points: {}, speed: {}, speed_param: {}, wrap: {}, root_motion: {}, looping: {:?}, \
             points_at: {}, events: {A}::EventTrack::from_records({}) }},",
            s.id.0,
            opt(s.param),
            opt(s.param_y),
            s.triangles,
            s.tris_at,
            s.points,
            f(s.speed),
            opt(s.speed_param),
            f(s.wrap),
            s.root_motion,
            s.looping,
            s.points_at,
            bytes(s.events.records()),
        );
    }
    o.push_str("    ],\n    points: &[\n");
    for p in t.points {
        let _ = writeln!(
            o,
            "        {A}::tables::Point {{ x: {}, y: {}, clip: {} }},",
            f(p.x),
            f(p.y),
            p.clip
        );
    }
    o.push_str("    ],\n    triangles: &[\n");
    for tr in t.triangles {
        let _ = writeln!(o, "        {tr:?},");
    }
    o.push_str("    ],\n    edges: &[\n");
    for e in t.edges {
        let _ = writeln!(
            o,
            "        {A}::tables::Edge {{ from: {}, to: {}, inertial: {}, blend: {}, cond: {} }},",
            e.from,
            e.to,
            e.inertial,
            f(e.blend),
            e.cond
        );
    }
    o.push_str("    ],\n    conds: &[\n");
    for c in t.conds {
        let _ = writeln!(
            o,
            "        {A}::tables::CondNode {{ op: {}, param: {}, children: {}, trigger: {}, end: {}, value: {} }},",
            c.op,
            c.param,
            c.children,
            c.trigger,
            c.end,
            f(c.value)
        );
    }
    o.push_str("    ],\n    layers: &[\n");
    for l in t.layers {
        let mode = match l.mode {
            LayerMode::Override => "Override",
            LayerMode::Additive => "Additive",
        };
        let _ = writeln!(
            o,
            "        {A}::GraphLayer {{ id: {:#010x}, mask: {A}::JointMask({:#010x}), mode: {A}::LayerMode::{mode}, \
             weight_param: {}, weight: {}, states: {:?}, transitions: {:?}, start: {}, sync: {} }},",
            l.id,
            l.mask.0,
            opt(l.weight_param),
            f(l.weight),
            l.states,
            l.transitions,
            l.start,
            l.sync
        );
    }
    let _ = writeln!(
        o,
        "    ],\n    start: {},\n    base_states: {},\n    base_transitions: {},\n    triggers: {:#06x},\n}};",
        t.start, t.base_states, t.base_transitions, t.triggers
    );
    Ok(o)
}

/// What a baked graph costs at run time (issue #138, `anim check`): its
/// decoded tables' bytes and an instance's (`AnimGraphInstance`, and a
/// crowd's `GraphInstance` sized to its layers without inertia or events),
/// as this host lays them out (a console's are a little smaller).
pub fn runtime_bytes(tang: &[u8]) -> Result<(usize, usize, usize), String> {
    use std::mem::size_of;
    use trino_core::anim::{GraphInstance, GraphTables};
    let g = AnimGraph::parse(tang).map_err(|e| format!("graph: {e:?}"))?;
    let mut memory = vec![std::mem::MaybeUninit::<u8>::uninit(); GraphTables::bytes_for(&g)];
    let arena = trino_core::mem::Arena::new(&mut memory);
    let t = GraphTables::decode_in(&g, &arena).ok_or("the tables outgrew bytes_for")?;
    let crowd = match g.layer_count() {
        0 => size_of::<GraphInstance<1, 0, 0>>(),
        1 => size_of::<GraphInstance<2, 0, 0>>(),
        2 => size_of::<GraphInstance<3, 0, 0>>(),
        _ => size_of::<GraphInstance<4, 0, 0>>(),
    };
    Ok((t.bytes(), size_of::<AnimGraphInstance>(), crowd))
}
