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

pub const GRAPH_VERSION: u32 = 1;

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
    GRAPH_VERSION
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
}

impl StateDef {
    /// Every clip it plays.
    pub fn clips(&self) -> Vec<&str> {
        match self {
            StateDef::Clip { clip, .. } => vec![clip.as_str()],
            StateDef::Blend1D { points, .. } => points.iter().map(|(_, c)| c.as_str()).collect(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            StateDef::Clip { .. } => "Clip",
            StateDef::Blend1D { .. } => "Blend1D",
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
    /// Crossfade seconds (0 = cut).
    #[serde(default = "default_blend")]
    pub blend: f32,
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
        ron::ser::to_string_pretty(self, cfg).expect("graph serializes")
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
        }
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
/// states and dead ends. `clips` = the model's (`clip_facts`), `None` when
/// the model is unknown (then clip checks are skipped and reported).
/// `sound` says whether `sounds/<x>` exists for `sfx:<x>` events.
pub fn check(
    def: &AnimGraphDef,
    clips: Option<&[ClipFacts]>,
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
    let state_at = |n: &str| format!("state \"{n}\"");
    if def.states.is_empty() {
        err("graph".into(), "has no states".into());
    }
    if def.states.len() > g::MAX_STATES {
        err(
            "graph".into(),
            format!("{} states (at most {})", def.states.len(), g::MAX_STATES),
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
    if let Some(s) = &def.start
        && def.state(s).is_none()
    {
        err("start".into(), format!("names no state (`{s}`)"));
    }
    let kind = |p: &str| def.param(p);
    let clip =
        |name: &str| clips.and_then(|cs| cs.iter().find(|c| c.id == ClipId::from_name(name)));
    if clips.is_none() {
        err(
            "model".into(),
            format!(
                "`{}` is not a baked animated model: clips not checked",
                def.model
            ),
        );
    }
    let float_param =
        |at: &str, p: &str, what: &str, err: &mut dyn FnMut(String, String)| match kind(p) {
            Some(ParamDef::Float(_)) => {}
            Some(_) => err(at.into(), format!("{what} `{p}` must be a Float parameter")),
            None => err(at.into(), format!("{what} `{p}` is not a parameter")),
        };
    for (name, s) in &def.states {
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
        }
    }
    let state_names: BTreeSet<&str> = def.states.iter().map(|(n, _)| n.as_str()).collect();
    for (i, t) in def.transitions.iter().enumerate() {
        let at = format!("transition {i}");
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
    if !def.states.is_empty() {
        let start = def.states[def.start_index()].0.as_str();
        let mut seen = BTreeSet::from([start]);
        let any: Vec<&str> = def
            .transitions
            .iter()
            .filter(|t| t.from == From::Any)
            .map(|t| t.to.as_str())
            .collect();
        seen.extend(any.iter().copied());
        loop {
            let before = seen.len();
            for t in &def.transitions {
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
        for (name, _) in &def.states {
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
        for (name, s) in &def.states {
            let leaves = def.transitions.iter().any(|t| match &t.from {
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
    out
}

/// Bake a graph to `TANG` against its model's clips. Errors are
/// [`check`]'s errors, joined.
pub fn to_tang(
    def: &AnimGraphDef,
    clips: &[ClipFacts],
    sound: &dyn Fn(&str) -> bool,
) -> Result<Vec<u8>, String> {
    let errors: Vec<String> = check(def, Some(clips), sound)
        .into_iter()
        .filter(|i| i.level == Level::Error)
        .map(|i| i.to_string())
        .collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let param_index = |p: &str| def.params.iter().position(|(n, _)| n == p).unwrap() as u16;
    let state_index = |s: &str| def.states.iter().position(|(n, _)| n == s).unwrap() as u16;
    // The clips the graph plays, in first-use order.
    let mut used: Vec<&str> = Vec::new();
    for (_, s) in &def.states {
        for c in s.clips() {
            if !used.contains(&c) {
                used.push(c);
            }
        }
    }
    let clip_index = |c: &str| used.iter().position(|u| *u == c).unwrap() as u16;
    let facts = |c: &str| clips.iter().find(|f| f.id == ClipId::from_name(c)).unwrap();

    let head = g::HEADER_BYTES
        + def.params.len() * g::PARAM_BYTES
        + used.len() * g::CLIP_BYTES
        + def.states.len() * g::STATE_BYTES
        + def.transitions.len() * g::TRANSITION_BYTES;
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
    table.extend_from_slice(&u32le(g::TANG_VERSION));
    table.extend_from_slice(&u32le(trino_core::asset_id(&def.model_path())));
    for n in [
        def.params.len(),
        used.len(),
        def.states.len(),
        def.transitions.len(),
        def.start_index(),
        0,
    ] {
        table.extend_from_slice(&(n as u16).to_le_bytes());
    }
    table.extend_from_slice(&[0; 8]);
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
        let ev = f.events.as_ref().map_or(g::NONE32, |b| put(b));
        let root = f.root.as_ref().map_or(g::NONE32, |b| put(b));
        let feet = f.feet.as_ref().map_or(g::NONE32, |b| put(b));
        table.extend_from_slice(&u32le(ev));
        table.extend_from_slice(&u32le(root));
        table.extend_from_slice(&u32le(feet));
    }
    for (name, s) in &def.states {
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
                    points: vec![(0.0, clip_index(clip))],
                    speed: *speed,
                    speed_param: speed_param.as_ref(),
                    param: g::NONE16,
                    wrap: 0.0,
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
                points: points.iter().map(|(v, c)| (*v, clip_index(c))).collect(),
                speed: *speed,
                speed_param: speed_param.as_ref(),
                param: param_index(param),
                wrap: wrap.unwrap_or(0.0),
                events: &[],
            },
        };
        let mut pts = Vec::new();
        for (v, c) in &points {
            pts.extend_from_slice(&v.to_le_bytes());
            pts.extend_from_slice(&c.to_le_bytes());
            pts.extend_from_slice(&[0, 0]);
        }
        let points_at = put(&pts);
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
        table.extend_from_slice(&wrap.to_le_bytes());
        table.extend_from_slice(&u32le(points_at));
        table.extend_from_slice(&u32le(events_at));
        table.extend_from_slice(&[0; 4]);
    }
    for t in &def.transitions {
        let from = match &t.from {
            From::Any => g::NONE16,
            From::State(f) => state_index(f),
        };
        let mut code = Vec::new();
        encode_cond(&t.when, &param_index, &mut code);
        let cond_at = put(&code);
        table.extend_from_slice(&from.to_le_bytes());
        table.extend_from_slice(&state_index(&t.to).to_le_bytes());
        table.extend_from_slice(&t.blend.to_le_bytes());
        table.extend_from_slice(&u32le(cond_at));
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
    points: Vec<(f32, u16)>,
    speed: f32,
    speed_param: Option<&'a String>,
    param: u16,
    wrap: f32,
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
    pub state: String,
    pub from: f32,
    pub to: f32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TraceTransition {
    pub at: f32,
    pub from: String,
    pub to: String,
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
    /// (clip, time, weight) of each clip in the pose.
    pub clips: Vec<(String, f32, f32)>,
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
    let state_name = |i: usize| def.states.get(i).map_or("?".into(), |(n, _)| n.clone());
    let clip_name = |id: ClipId| {
        clip_names
            .iter()
            .find(|n| ClipId::from_name(n) == id)
            .cloned()
            .unwrap_or_else(|| format!("{:#010x}", id.0))
    };
    let mut event_names: Vec<String> = Vec::new();
    for (_, s) in &def.states {
        if let StateDef::Clip { events, .. } = s {
            event_names.extend(events.iter().map(|(_, n)| n.clone()));
        }
    }
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
        segments: vec![Segment {
            state: state_name(inst.state()),
            from: 0.0,
            to: 0.0,
        }],
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
        let before = inst.state();
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
        if inst.state() != before {
            trace.segments.last_mut().unwrap().to = now;
            trace.transitions.push(TraceTransition {
                at: now,
                from: state_name(before),
                to: state_name(inst.state()),
            });
            trace.segments.push(Segment {
                state: state_name(inst.state()),
                from: now,
                to: now,
            });
        }
        if sample_every > 0 && u % sample_every == 0 {
            let mut c = [trino_core::anim::Contribution {
                clip: 0,
                time: 0.0,
                weight: 0.0,
            }; 4];
            let n = inst.contributions(&graph, &mut c);
            trace.samples.push(TraceSample {
                at: now,
                state: state_name(inst.state()),
                clips: c[..n]
                    .iter()
                    .map(|x| (clip_name(graph.clip(x.clip).id), x.time, x.weight))
                    .collect(),
            });
        }
    }
    if let Some(s) = trace.segments.last_mut() {
        s.to = updates as f32 * dt;
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
