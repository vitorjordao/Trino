//! The UI tab's state (issue #58): the project's screens (`ui/*.ui.ron`),
//! the open one, and what the tab previews it on — a target (its profile
//! and picture size) and a language. The preview is the PC renderer at
//! the target's profile drawing the screen with the runtime's own [`Ui`]
//! (what `cargo xtask ui shot` renders), so the tab shows what the target
//! shows; the issues are `cargo xtask ui check`'s for that target and
//! language. Edits land in `def` first and are written once the user
//! settles (`App::flush_pending`), the file's leading comments kept; a file
//! edited outside (an agent, a text editor) reloads here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui;
use eframe::egui_wgpu::RenderState;
use trino_asset_pipeline as pipeline;
use trino_core::ui::{NONE, UiScreen, flags};
use trino_core::{Button, Color, Family, InputState, Renderer, Screen, Ui, UiEnv, UiStats};
use trino_platform_pc::{PcRenderer, SimProfile};
use trino_scene::ui::{
    Align, Anchor, Dir, Flat, Fonts, Issue, Justify, Kind, Langs, NodeDef, ScreenDef, Size, Target,
    apply_preview, audit, screen_name, validate,
};

use crate::project::Project;

/// A target the tab previews on.
pub struct UiTarget {
    /// Its name in `TRINO_EDITOR_UI` (`pause,3ds`).
    pub key: &'static str,
    pub label: &'static str,
    pub family: Family,
    /// The picture on the PC and the Web (the consoles' are fixed).
    pub size: Option<(u32, u32)>,
}

pub const TARGETS: [UiTarget; 6] = [
    UiTarget {
        key: "n64",
        label: "N64",
        family: Family::N64,
        size: None,
    },
    UiTarget {
        key: "3ds",
        label: "3DS",
        family: Family::N3ds,
        size: None,
    },
    UiTarget {
        key: "pc",
        label: "PC 1080p",
        family: Family::Pc,
        size: Some((1920, 1080)),
    },
    UiTarget {
        key: "4k",
        label: "PC 4K",
        family: Family::Pc,
        size: Some((3840, 2160)),
    },
    UiTarget {
        key: "wide",
        label: "PC 21:9",
        family: Family::Pc,
        size: Some((2560, 1080)),
    },
    UiTarget {
        key: "web",
        label: "Web",
        family: Family::Web,
        size: Some((1280, 720)),
    },
];

/// What the preview clears to (the starter screens' backdrop).
const CLEAR: Color = Color::rgb(24, 26, 40);

/// The target named `key` (`n64`, `3ds`, `pc`, `4k`, `wide`, `web`; `1080p`
/// and `21:9` too).
pub fn target_index(key: &str) -> Option<usize> {
    let key = match key.trim().to_ascii_lowercase().as_str() {
        "1080p" => "pc".to_string(),
        "21:9" | "ultrawide" => "wide".to_string(),
        k => k.to_string(),
    };
    TARGETS.iter().position(|t| t.key == key)
}

fn profile_of(family: Family) -> SimProfile {
    match family {
        Family::N64 => SimProfile::N64,
        Family::N3ds => SimProfile::N3ds,
        Family::Pc => SimProfile::Native,
        Family::Web => SimProfile::Web,
    }
}

/// What a preview and the issues were made from: the edit, the target,
/// the language, the assets, the string tables.
type Key = (
    u64,
    usize,
    Option<String>,
    u64,
    u64,
    trino_core::AccessSettings,
);

pub struct UiScreens {
    /// The project's screens, sorted.
    pub files: Vec<PathBuf>,
    /// The open screen's file.
    pub path: Option<PathBuf>,
    /// The open screen (edits land here first).
    pub def: Option<ScreenDef>,
    /// The file's leading comment lines, written back above the screen.
    header: String,
    /// The file names itself (else its stem does, and saving keeps it so).
    named: bool,
    /// Why the file on disk doesn't parse (the last good screen is kept).
    pub error: Option<String>,
    /// Edited and not written yet (written once the user settles).
    pub dirty: bool,
    /// Bumped on every change: the preview and the issues follow.
    pub rev: u64,
    /// The selected node's path in `def.root`.
    pub selected: Option<Vec<usize>>,
    /// Index into [`TARGETS`].
    pub target: usize,
    /// The language previewed (`None`: the nodes' default texts).
    pub lang: Option<String>,
    /// The project's string tables (`assets/i18n/*.ron`).
    pub langs: Langs,
    pub langs_error: Option<String>,
    langs_rev: u64,
    /// The Accessibility tab's simulation (issue #60): the preview's text
    /// size, high contrast, color-vision icons and simulation view.
    pub access: trino_core::AccessSettings,
    /// Overlays: the pad's focus order, overflowing texts.
    pub show_focus: bool,
    pub show_overflow: bool,
    /// The "New screen" form (`Some`: open, with the name typed so far).
    pub new_name: Option<String>,
    pub preview: Option<UiPreview>,
    /// `validate` and `audit` for the target and the language.
    pub issues: Vec<Issue>,
    /// What the audit counted (nodes, draws, overflowing texts).
    pub stats: UiStats,
    issues_key: Option<Key>,
}

impl UiScreens {
    /// The screens under `root`, the first one open.
    pub fn load(root: &Path) -> UiScreens {
        let mut s = UiScreens {
            files: Vec::new(),
            path: None,
            def: None,
            header: String::new(),
            named: false,
            error: None,
            dirty: false,
            rev: 0,
            selected: None,
            target: 0,
            lang: None,
            langs: Langs::default(),
            langs_error: None,
            langs_rev: 0,
            access: trino_core::AccessSettings::DEFAULT,
            show_focus: true,
            show_overflow: true,
            new_name: None,
            preview: None,
            issues: Vec::new(),
            stats: UiStats::default(),
            issues_key: None,
        };
        s.reload_langs(root);
        s.refresh_files(root);
        if let Some(first) = s.files.first().cloned() {
            s.open(first);
        }
        s
    }

    /// [`load`](Self::load), then the dev hook
    /// `TRINO_EDITOR_UI=<screen>[,<platform>[,<lang>[,<node id>]]]`.
    pub fn from_env(root: &Path) -> UiScreens {
        let mut s = UiScreens::load(root);
        if let Ok(v) = std::env::var("TRINO_EDITOR_UI") {
            s.apply_hook(&v);
        }
        s
    }

    /// `pause,3ds,pt-BR,resume`: that screen on that target, in that
    /// language (empty or `default`: the default texts), that node
    /// selected.
    pub fn apply_hook(&mut self, hook: &str) {
        let mut parts = hook.split(',').map(str::trim);
        if let Some(name) = parts.next().filter(|n| !n.is_empty()) {
            self.open_name(name);
        }
        if let Some(t) = parts.next().and_then(target_index) {
            self.target = t;
        }
        if let Some(l) = parts.next() {
            self.lang = (!l.is_empty() && l != "default").then(|| l.to_string());
        }
        if let Some(id) = parts.next().filter(|i| !i.is_empty())
            && let Some(def) = &self.def
        {
            self.selected = def
                .nodes()
                .into_iter()
                .find(|(_, n)| n.id == id)
                .map(|(p, _)| p);
        }
    }

    pub fn refresh_files(&mut self, root: &Path) {
        self.files = pipeline::ui::screen_files(root);
    }

    pub fn reload_langs(&mut self, root: &Path) {
        match pipeline::ui::langs(root) {
            Ok(l) => {
                self.langs = l;
                self.langs_error = None;
            }
            Err(e) => self.langs_error = Some(e),
        }
        self.langs_rev += 1;
    }

    /// The screen's name (its file's stem when it doesn't name itself).
    pub fn name(&self) -> String {
        match (&self.def, &self.path) {
            (Some(d), _) if !d.name.is_empty() => d.name.clone(),
            (_, Some(p)) => screen_name(p),
            _ => String::new(),
        }
    }

    /// Open the screen in `path` (the selection and the error cleared).
    pub fn open(&mut self, path: PathBuf) {
        self.path = Some(path.clone());
        self.def = None;
        self.selected = None;
        self.dirty = false;
        match std::fs::read_to_string(&path) {
            Ok(text) => self.reload(&text),
            Err(e) => {
                self.error = Some(format!("cannot read {}: {e}", path.display()));
                self.rev += 1;
            }
        }
    }

    /// Open the screen named `name` (`pause`: `ui/pause.ui.ron`, or the
    /// file whose screen says `name: "pause"`). False when none is.
    pub fn open_name(&mut self, name: &str) -> bool {
        let found = self.files.iter().find(|f| screen_name(f) == name).cloned();
        let found = found.or_else(|| {
            self.files
                .iter()
                .find(|f| ScreenDef::load(f).is_ok_and(|d| d.name == name))
                .cloned()
        });
        match found {
            Some(f) => {
                self.open(f);
                true
            }
            None => false,
        }
    }

    /// The open file's new text: parsed (the selection kept where it still
    /// is), or the last good screen kept with the error.
    fn reload(&mut self, text: &str) {
        self.rev += 1;
        self.dirty = false;
        match ScreenDef::from_ron(text) {
            Ok(mut def) => {
                self.named = !def.name.is_empty();
                if !self.named
                    && let Some(p) = &self.path
                {
                    def.name = screen_name(p);
                }
                if let Some(sel) = &self.selected
                    && def.root.at(sel).is_none()
                {
                    self.selected = None;
                }
                self.header = leading_comments(text);
                self.def = Some(def);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// A screen file changed on disk (`text`: `None` when it is gone).
    /// What to tell the user, if anything.
    pub fn file_changed(
        &mut self,
        root: &Path,
        path: &Path,
        text: Option<&str>,
    ) -> Option<Result<String, String>> {
        self.refresh_files(root);
        if self.path.as_deref() != Some(path) {
            if self.path.is_none()
                && let Some(first) = self.files.first().cloned()
            {
                self.open(first);
            }
            return None;
        }
        let Some(text) = text else {
            // The open screen was deleted: the next one opens.
            self.path = None;
            self.def = None;
            self.selected = None;
            self.dirty = false;
            self.rev += 1;
            if let Some(first) = self.files.first().cloned() {
                self.open(first);
            }
            return Some(Ok(format!("{} deleted", path.display())));
        };
        self.reload(text);
        Some(match &self.error {
            Some(e) => Err(format!("{}: {e}", path.display())),
            None => Ok(format!("{} reloaded", path.display())),
        })
    }

    /// The open screen changed (an edit in the tab).
    pub fn changed(&mut self) {
        self.dirty = true;
        self.rev += 1;
    }

    /// The file to write for the open screen, and its text.
    pub fn to_text(&self) -> Option<(PathBuf, String)> {
        let (path, def) = (self.path.as_ref()?, self.def.as_ref()?);
        let text = if !self.named && def.name == screen_name(path) {
            // The stem names it: keep the file without a name.
            ScreenDef {
                name: String::new(),
                ..def.clone()
            }
            .to_ron()
        } else {
            def.to_ron()
        };
        Some((path.clone(), format!("{}{text}", self.header)))
    }

    /// A new screen `ui/<name>.ui.ron` (a backdrop, a title and a Back
    /// button), opened; written by the next flush.
    pub fn create(&mut self, root: &Path, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "\"{name}\": a screen name is letters, digits, - and _"
            ));
        }
        let path = root
            .join(pipeline::ui::UI_DIR)
            .join(format!("{name}.ui.ron"));
        if path.exists() || self.files.iter().any(|f| screen_name(f) == name) {
            return Err(format!("{} exists", path.display()));
        }
        self.files.push(path.clone());
        self.files.sort();
        self.path = Some(path);
        self.def = Some(starter(name));
        self.named = true;
        self.header = format!(
            "// The {name} screen (Trino UI system: docs/ASSETS.md \"UI screens\"), made in\n\
             // the editor's UI tab. Edit it there, by hand or with an agent.\n"
        );
        self.error = None;
        self.selected = None;
        self.changed();
        Ok(())
    }

    /// The selected node's depth-first index (the flat list's, the
    /// issues').
    pub fn selected_index(&self) -> Option<usize> {
        let (def, sel) = (self.def.as_ref()?, self.selected.as_ref()?);
        def.nodes().iter().position(|(p, _)| p == sel)
    }

    /// Select the node with depth-first index `i`.
    pub fn select_index(&mut self, i: usize) {
        if let Some(def) = &self.def {
            self.selected = def.nodes().get(i).map(|(p, _)| p.clone());
        }
    }

    /// Edit the open screen's tree: `f` gets the root and the selection
    /// and returns the new selection (`None`: nothing changed).
    pub fn edit_tree(
        &mut self,
        f: impl FnOnce(&mut ScreenDef, Option<&[usize]>) -> Option<Option<Vec<usize>>>,
    ) {
        let Some(def) = &mut self.def else {
            return;
        };
        if let Some(sel) = f(def, self.selected.as_deref()) {
            self.selected = sel;
            self.changed();
        }
    }

    /// Lay the screen out on the target (the issues), and render it when
    /// anything it depends on changed.
    pub fn refresh(&mut self, ctx: &egui::Context, rs: &RenderState, project: &Project) {
        let Some(def) = &self.def else {
            self.preview = None;
            self.issues.clear();
            return;
        };
        let key: Key = (
            self.rev,
            self.target,
            self.lang.clone(),
            project.generation,
            self.langs_rev,
            self.access,
        );
        let target = &TARGETS[self.target];
        if self.issues_key.as_ref() != Some(&key) {
            let mut caps = target.family.caps();
            if let Some((w, h)) = target.size {
                caps.screen_width = w as u16;
                caps.screen_height = h as u16;
            }
            let sizes = sprite_sizes(project);
            let flat = Flat::new(def, &sizes);
            let report = audit(
                &flat,
                Target {
                    caps,
                    lang: self.lang.as_deref(),
                    interface_scale: 1.0,
                    access: ui_access(&project.root, &self.access),
                },
                &fonts(project),
                &self.langs,
            );
            self.issues = validate(def);
            self.issues.extend(report.issues);
            self.stats = report.stats;
            self.issues_key = Some(key.clone());
        }
        let profile = profile_of(target.family);
        if self.preview.as_ref().is_some_and(|p| p.profile != profile) {
            self.preview = None;
        }
        let p = self
            .preview
            .get_or_insert_with(|| UiPreview::new(rs.clone(), profile));
        if p.key.as_ref() != Some(&key) {
            p.access = ui_access(&project.root, &self.access);
            p.simulate = self.access.cvd;
            p.render(ctx, def, project, &self.langs, self.lang.as_deref(), target);
            p.key = Some(key);
        }
    }

    /// Depth-first indices of the nodes whose text overflows in the
    /// preview.
    pub fn overflowing(&self) -> Vec<usize> {
        let Some(p) = &self.preview else {
            return Vec::new();
        };
        let mut out: Vec<usize> =
            p.ui.slots()
                .iter()
                .filter(|s| s.has(flags::OVERFLOW))
                .map(|s| s.node as usize)
                .collect();
        out.dedup();
        out
    }

    /// Where the preview shows the node at `path` (its first copy), in
    /// points: what a click there selects.
    #[cfg(test)]
    pub fn node_point(&self, path: &[usize]) -> Option<egui::Pos2> {
        let p = self.preview.as_ref()?;
        let node = p.paths.iter().position(|q| q == path)?;
        let i = (0..p.ui.slots().len()).find(|&i| p.ui.slots()[i].node as usize == node)?;
        Some(p.slot_rect(i)?.center())
    }
}

/// The comment (and blank) lines a file starts with.
fn leading_comments(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with("//") && !t.is_empty() {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn sprite_sizes(project: &Project) -> impl Fn(&str) -> Option<(u16, u16)> + '_ {
    |s: &str| {
        project.sprites.get(s).map(|a| {
            (
                a.width.min(u16::MAX as u32) as u16,
                a.height.min(u16::MAX as u32) as u16,
            )
        })
    }
}

/// The project's fonts as the PC bakes them (what the preview measures
/// with).
fn fonts(project: &Project) -> Fonts {
    let mut f = Fonts::default();
    for (logical, (_, table)) in &project.fonts {
        f.insert(logical, table.clone());
    }
    f
}

/// A new screen: a backdrop, the name as a title and a Back button.
fn starter(name: &str) -> ScreenDef {
    let title = NodeDef {
        text: name.to_string(),
        text_scale: 2,
        ..NodeDef::new(Kind::Text)
    };
    let back = NodeDef {
        id: "back".into(),
        text: "Back".into(),
        on_click: "back".into(),
        size: (Size::Px(112), Size::Px(18)),
        ..NodeDef::new(Kind::Button)
    };
    ScreenDef {
        name: name.to_string(),
        on_back: "back".into(),
        root: NodeDef {
            children: vec![
                NodeDef {
                    anchor: Anchor::Fill,
                    bleed: true,
                    color: Some([24, 26, 40, 255]),
                    ..NodeDef::new(Kind::Rect {
                        border: 0,
                        border_color: None,
                    })
                },
                NodeDef {
                    anchor: Anchor::Center,
                    children: vec![title, back],
                    ..NodeDef::new(Kind::Stack {
                        dir: Dir::Vertical,
                        gap: 6,
                        align: Align::Center,
                        justify: Justify::Start,
                    })
                },
            ],
            ..NodeDef::default()
        },
        ..ScreenDef::default()
    }
}

// ---------------------------------------------------------------- tree --

/// Where a dragged node lands.
#[derive(Clone, Debug, PartialEq)]
pub enum Drop {
    Before(Vec<usize>),
    After(Vec<usize>),
    /// The container's last child.
    Into(Vec<usize>),
}

/// `base`, or `base2`, `base3`... whichever no node of `def` has as id.
pub fn unique_id(def: &ScreenDef, base: &str) -> String {
    let ids: HashSet<&str> = def
        .nodes()
        .into_iter()
        .map(|(_, n)| n.id.as_str())
        .collect();
    if !ids.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}{n}"))
        .find(|c| !ids.contains(c.as_str()))
        .unwrap()
}

/// A node of `kind` ready to show: a text, a control with an id, its
/// action or its value.
pub fn new_node(def: &ScreenDef, kind: Kind) -> NodeDef {
    let mut n = NodeDef::new(kind);
    match &n.kind {
        Kind::Text => n.text = "Text".into(),
        Kind::Button => {
            n.id = unique_id(def, "button");
            n.on_click = n.id.clone();
            n.text = "Button".into();
        }
        Kind::Toggle | Kind::Slider { .. } => {
            let base = n.kind.name().to_lowercase();
            n.id = unique_id(def, &base);
            n.bind = n.id.clone();
            n.text = n.kind.name().into();
        }
        Kind::List { .. } => {
            n.bind = "items".into();
            n.children.push(NodeDef {
                bind: "item".into(),
                ..NodeDef::new(Kind::Text)
            });
        }
        Kind::Rect { .. } => n.size = (Size::Px(64), Size::Px(32)),
        _ => {}
    }
    n
}

/// Add `node` as the selected container's last child, else after the
/// selected node (the root's last child without a selection). Its path.
pub fn add(def: &mut ScreenDef, sel: Option<&[usize]>, node: NodeDef) -> Option<Vec<usize>> {
    let (parent, index) = match sel {
        Some(p) if def.root.at(p).is_some_and(|n| n.kind.is_container()) => {
            (p.to_vec(), def.root.at(p)?.children.len())
        }
        Some(p) if !p.is_empty() => {
            let (i, parent) = p.split_last()?;
            (parent.to_vec(), i + 1)
        }
        _ => (Vec::new(), def.root.children.len()),
    };
    let at = def.root.at_mut(&parent)?;
    let index = index.min(at.children.len());
    at.children.insert(index, node);
    let mut out = parent;
    out.push(index);
    Some(out)
}

/// Remove the node at `path` (never the root). The node to select next:
/// the one that took its place, else the one before, else the parent.
pub fn delete(def: &mut ScreenDef, path: &[usize]) -> Option<Vec<usize>> {
    let (i, parent) = path.split_last()?;
    let at = def.root.at_mut(parent)?;
    if *i >= at.children.len() {
        return None;
    }
    at.children.remove(*i);
    let mut next = parent.to_vec();
    if *i < at.children.len() {
        next.push(*i);
    } else if *i > 0 {
        next.push(i - 1);
    }
    Some(next)
}

/// A copy of the node at `path` after it, its ids made unique. The copy's
/// path.
pub fn duplicate(def: &mut ScreenDef, path: &[usize]) -> Option<Vec<usize>> {
    let (i, parent) = path.split_last()?;
    let mut copy = def.root.at(path)?.clone();
    fn renumber(def: &ScreenDef, taken: &mut HashSet<String>, n: &mut NodeDef) {
        if !n.id.is_empty() {
            let mut id = unique_id(def, &n.id);
            let mut k = 2;
            while taken.contains(&id) {
                id = unique_id(def, &format!("{}{k}", n.id));
                k += 1;
            }
            if n.on_click == n.id {
                n.on_click = id.clone();
            }
            taken.insert(id.clone());
            n.id = id;
        }
        for c in &mut n.children {
            renumber(def, taken, c);
        }
    }
    renumber(def, &mut HashSet::new(), &mut copy);
    def.root.at_mut(parent)?.children.insert(i + 1, copy);
    let mut out = parent.to_vec();
    out.push(i + 1);
    Some(out)
}

/// Swap the node at `path` with its sibling `by` places away (−1 up, +1
/// down). Its new path.
pub fn shift(def: &mut ScreenDef, path: &[usize], by: isize) -> Option<Vec<usize>> {
    let (i, parent) = path.split_last()?;
    let at = def.root.at_mut(parent)?;
    let j = i
        .checked_add_signed(by)
        .filter(|j| *j < at.children.len())?;
    at.children.swap(*i, j);
    let mut out = parent.to_vec();
    out.push(j);
    Some(out)
}

/// Move the node at `from` to `to` (not into itself). Its new path.
pub fn move_node(def: &mut ScreenDef, from: &[usize], to: &Drop) -> Option<Vec<usize>> {
    let target = match to {
        Drop::Before(p) | Drop::After(p) | Drop::Into(p) => p,
    };
    if from.is_empty() || target.starts_with(from) {
        return None;
    }
    let (mut parent, mut index) = match to {
        Drop::Into(p) => {
            let n = def.root.at(p)?;
            if !n.kind.is_container() {
                return None;
            }
            (p.clone(), n.children.len())
        }
        Drop::Before(p) | Drop::After(p) => {
            let (i, parent) = p.split_last()?;
            (
                parent.to_vec(),
                i + usize::from(matches!(to, Drop::After(_))),
            )
        }
    };
    // Taking `from` out moves its later siblings (and paths through them)
    // up one.
    let (fi, fparent) = from.split_last()?;
    if parent.len() > fparent.len() && parent.starts_with(fparent) && parent[fparent.len()] > *fi {
        parent[fparent.len()] -= 1;
    }
    if parent == fparent && index > *fi {
        index -= 1;
    }
    let (i, p) = from.split_last()?;
    let node = def.root.at_mut(p)?.children.remove(*i);
    let at = def.root.at_mut(&parent)?;
    let index = index.min(at.children.len());
    at.children.insert(index, node);
    parent.push(index);
    Some(parent)
}

// ------------------------------------------------------------- preview --

/// Where a frame shows the screens, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Both screens (the 3DS's bezel).
    pub all: egui::Rect,
    pub top: egui::Rect,
    /// The 3DS's touch screen, centered under the top one.
    pub bottom: Option<egui::Rect>,
    /// Physical pixels per picture pixel.
    pub scale: f32,
}

/// Fit the screens (`top`, and `bottom` under it) in `area` (points, at
/// `ppp` pixels per point): whole-pixel scales when they fit at 1:1 or
/// more.
pub fn place(area: egui::Rect, ppp: f32, top: (u32, u32), bottom: Option<(u32, u32)>) -> Placed {
    let (bw, bh) = bottom.unwrap_or((0, 0));
    let (w, h) = (top.0.max(bw) as f32, (top.1 + bh) as f32);
    let mut scale = (area.width() * ppp / w)
        .min(area.height() * ppp / h)
        .max(0.01);
    if scale >= 1.0 {
        scale = scale.floor();
    }
    let k = scale / ppp;
    let all = egui::Rect::from_center_size(area.center(), egui::vec2(w * k, h * k));
    let at = |y: f32, (sw, sh): (u32, u32)| {
        egui::Rect::from_min_size(
            egui::pos2(all.center().x - sw as f32 * k / 2.0, y),
            egui::vec2(sw as f32 * k, sh as f32 * k),
        )
    };
    let top_rect = at(all.min.y, top);
    Placed {
        all,
        top: top_rect,
        bottom: bottom.map(|b| at(top_rect.max.y, b)),
        scale,
    }
}

/// The screen drawn by the target's renderer, and the runtime's state of
/// that frame (its slots and their rects, the focus): what the overlays,
/// the clicks and the tests read.
pub struct UiPreview {
    render_state: RenderState,
    renderer: PcRenderer,
    /// The picture sampled whole-pixel and smoothed (shown smaller than it
    /// is).
    textures: [egui::TextureId; 2],
    /// The 3DS touch screen, read back from the renderer (the same two
    /// ways).
    bottom: Option<[egui::TextureHandle; 2]>,
    profile: SimProfile,
    /// The top screen's picture size (what the texture was registered at).
    pub size: (u32, u32),
    pub bottom_size: Option<(u32, u32)>,
    /// The last render's layout.
    pub ui: Box<Ui<1024>>,
    /// Each node's path (`Flat::paths`) at the last render.
    pub paths: Vec<Vec<usize>>,
    /// The controls the pad visits from the first focus, in order (slots).
    pub order: Vec<usize>,
    /// What the renderer counted.
    pub stats: UiStats,
    key: Option<Key>,
    uploaded: Option<u64>,
    /// Where the last frame put the screens.
    pub placed: Option<Placed>,
    /// What the player's settings change in the screen, and the color
    /// vision the picture is shown through.
    access: trino_core::ui::UiAccess,
    simulate: trino_core::CvdMode,
}

impl UiPreview {
    fn new(render_state: RenderState, profile: SimProfile) -> UiPreview {
        let device = wgpu::Device::clone(&render_state.device);
        let queue = wgpu::Queue::clone(&render_state.queue);
        let renderer = PcRenderer::with_device(device, queue, profile);
        let textures = register(&render_state, &renderer);
        let size = renderer.internal_size();
        UiPreview {
            render_state,
            renderer,
            textures,
            bottom: None,
            profile,
            size,
            bottom_size: None,
            ui: Box::new(Ui::new()),
            paths: Vec::new(),
            order: Vec::new(),
            stats: UiStats::default(),
            key: None,
            uploaded: None,
            placed: None,
            access: trino_core::ui::UiAccess::NONE,
            simulate: trino_core::CvdMode::Off,
        }
    }

    /// The top screen's picture, `smooth` for a picture shown smaller
    /// than it is.
    pub fn texture(&self, smooth: bool) -> egui::TextureId {
        self.textures[usize::from(smooth)]
    }

    pub fn bottom_texture(&self, smooth: bool) -> Option<egui::TextureId> {
        self.bottom.as_ref().map(|t| t[usize::from(smooth)].id())
    }

    /// Draw `def` as `target` shows it: the screen's preview values, the
    /// first control focused (two frames, as a game's first update does).
    fn render(
        &mut self,
        ctx: &egui::Context,
        def: &ScreenDef,
        project: &Project,
        langs: &Langs,
        lang: Option<&str>,
        target: &UiTarget,
    ) {
        let r = &mut self.renderer;
        if let Some((w, h)) = target.size {
            r.set_internal_size(w, h);
        }
        if self.uploaded != Some(project.generation) {
            project.upload_all(r);
            project.upload_platform_sprites(r, crate::viewport::target_of(self.profile));
            self.uploaded = Some(project.generation);
        }
        let sizes = sprite_sizes(project);
        let flat = Flat::new(def, &sizes);
        let screen = flat.screen();
        let strings = lang.map(|l| langs.strings(l));
        let ui = &mut *self.ui;
        // A fresh state each time: the screen's own first focus, its
        // preview values.
        *ui = Ui::new();
        apply_preview(ui, def);
        let idle = InputState::default();
        r.set_cvd_view(self.simulate);
        for frame in 0..2 {
            let env = UiEnv {
                access: self.access,
                ..UiEnv::of(&*r)
            };
            let env = match &strings {
                Some(s) => env.with_strings(s),
                None => env,
            };
            if frame == 0 {
                // Explicitly: an edited screen can reuse the old one's
                // node buffer, which the draw's own check would miss.
                let measure: &dyn Renderer = &*r;
                ui.layout(&screen, &env, measure);
            }
            r.begin_frame(CLEAR);
            ui.draw(r, &screen, &env);
            r.end_frame();
            if frame == 0 {
                ui.update(&screen, &idle, &idle, 0.0);
            }
        }
        self.stats = r.last_ui_stats().unwrap_or_default();
        self.order = pad_order(ui, &screen);
        self.paths = flat.paths.clone();
        self.bottom_size = r.second_screen().map(|s| (s.x as u32, s.y as u32));
        self.bottom = match self.bottom_size {
            Some((w, h)) => {
                let mut px = r.read_second_screen();
                for a in px.iter_mut().skip(3).step_by(4) {
                    *a = 255;
                }
                let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                let options = [egui::TextureOptions::NEAREST, egui::TextureOptions::LINEAR];
                match self.bottom.take() {
                    Some(mut t) => {
                        for (t, o) in t.iter_mut().zip(options) {
                            t.set(image.clone(), o);
                        }
                        Some(t)
                    }
                    None => Some(
                        options.map(|o| ctx.load_texture("ui-preview-bottom", image.clone(), o)),
                    ),
                }
            }
            None => None,
        };
        let size = r.internal_size();
        if size != self.size {
            self.size = size;
            self.free();
            self.textures = register(&self.render_state, &self.renderer);
        }
    }

    fn free(&self) {
        let mut r = self.render_state.renderer.write();
        for t in &self.textures {
            r.free_texture(t);
        }
    }

    /// Slot `i` of the last render, where the last frame showed it.
    pub fn slot_rect(&self, i: usize) -> Option<egui::Rect> {
        let placed = self.placed?;
        let s = self.ui.slots().get(i)?;
        let (image, px) = match s.on {
            Screen::Top => (placed.top, self.size),
            Screen::Bottom => (placed.bottom?, self.bottom_size?),
        };
        let k = image.width() / px.0.max(1) as f32;
        let r = self.ui.rect(i);
        Some(egui::Rect::from_min_size(
            image.min + egui::vec2(r.pos.x * k, r.pos.y * k),
            egui::vec2(r.size.x * k, r.size.y * k),
        ))
    }

    /// Not drawn: clipped by a scroll box, or inside one that is.
    pub fn hidden(&self, i: usize) -> bool {
        let slots = self.ui.slots();
        let mut at = i;
        loop {
            let s = slots[at];
            if s.has(flags::CLIPPED) {
                return true;
            }
            if s.parent == NONE {
                return false;
            }
            at = s.parent as usize;
        }
    }

    /// The topmost shown slot under `pos` (points).
    pub fn slot_at(&self, pos: egui::Pos2) -> Option<usize> {
        let placed = self.placed?;
        let (on, image, px) = if placed.top.contains(pos) {
            (Screen::Top, placed.top, self.size)
        } else {
            let b = placed.bottom.filter(|b| b.contains(pos))?;
            (Screen::Bottom, b, self.bottom_size?)
        };
        let k = px.0 as f32 / image.width().max(1.0);
        let p = trino_core::Vec2::new((pos.x - image.min.x) * k, (pos.y - image.min.y) * k);
        let slots = self.ui.slots();
        (0..slots.len())
            .rev()
            .find(|&i| slots[i].on == on && !self.hidden(i) && self.ui.rect(i).contains(p))
    }
}

impl std::ops::Drop for UiPreview {
    fn drop(&mut self) {
        self.free();
    }
}

/// The renderer's picture as egui textures: sampled whole-pixel, smoothed.
fn register(rs: &RenderState, renderer: &PcRenderer) -> [egui::TextureId; 2] {
    let mut r = rs.renderer.write();
    [wgpu::FilterMode::Nearest, wgpu::FilterMode::Linear]
        .map(|f| r.register_native_texture(&rs.device, renderer.offscreen_view(), f))
}

/// The controls Down presses visit from the first focus (Right where Down
/// stops: a row), in order — the numbers the focus overlay shows.
fn pad_order<const N: usize>(ui: &Ui<N>, screen: &UiScreen<'_>) -> Vec<usize> {
    let mut sim = Box::new(ui.clone());
    let idle = InputState::default();
    let mut order = Vec::new();
    while let Some(at) = sim.focus_slot() {
        if order.contains(&at) {
            break;
        }
        order.push(at);
        let mut moved = false;
        for button in [Button::DpadDown, Button::DpadRight] {
            let mut held = InputState::default();
            held.set(button, true);
            sim.update(screen, &held, &idle, 0.0);
            sim.update(screen, &idle, &held, 0.0);
            if sim.focus_slot() != Some(at) {
                moved = true;
                break;
            }
        }
        if !moved {
            break;
        }
    }
    order
}

/// The UI's accessibility for a preview: the simulated settings and the
/// project's own high-contrast theme (`ui/themes/high_contrast.ron`).
fn ui_access(root: &Path, s: &trino_core::AccessSettings) -> trino_core::ui::UiAccess {
    let contrast = if s.high_contrast {
        trino_scene::access::load_theme(root)
            .ok()
            .flatten()
            .map(|t| trino_scene::access::contrast_theme(&t))
    } else {
        None
    };
    trino_core::ui::UiAccess {
        text_scale: s.text_factor(),
        high_contrast: s.high_contrast,
        contrast,
        cvd: s.cvd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pause() -> ScreenDef {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../templates/standalone/ui/pause.ui.ron"),
        )
        .unwrap();
        ScreenDef::from_ron(&text).unwrap()
    }

    fn ids(def: &ScreenDef, path: &[usize]) -> Vec<String> {
        def.root
            .at(path)
            .unwrap()
            .children
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }

    /// Add, duplicate, shift, drag and delete keep ids unique and return
    /// where the node went.
    #[test]
    fn tree_edits_return_the_new_paths() {
        let mut def = pause();
        let stack = vec![1, 0];
        assert_eq!(ids(&def, &stack), ["", "resume", "options", "quit"]);
        // Into the selected container, after a selected leaf.
        let b = new_node(&def, Kind::Button);
        assert_eq!(b.id, "button");
        assert_eq!(add(&mut def, Some(&stack), b), Some(vec![1, 0, 4]));
        let t = new_node(&def, Kind::Text);
        assert_eq!(add(&mut def, Some(&[1, 0, 1]), t), Some(vec![1, 0, 2]));
        assert_eq!(new_node(&def, Kind::Button).id, "button2");
        // A copy after the node, its id (and its action) renumbered.
        assert_eq!(duplicate(&mut def, &[1, 0, 1]), Some(vec![1, 0, 2]));
        let copy = def.root.at(&[1, 0, 2]).unwrap();
        assert_eq!(
            (copy.id.as_str(), copy.on_click.as_str()),
            ("resume2", "resume2")
        );
        assert_eq!(shift(&mut def, &[1, 0, 2], -1), Some(vec![1, 0, 1]));
        assert_eq!(shift(&mut def, &[1, 0, 0], -1), None);
        assert_eq!(
            ids(&def, &stack),
            ["", "resume2", "resume", "", "options", "quit", "button"]
        );
        // Dragged before a later sibling, after an earlier one, into the
        // panel; never into itself.
        let quit = vec![1, 0, 5];
        assert_eq!(
            move_node(&mut def, &quit, &Drop::Before(vec![1, 0, 1])),
            Some(vec![1, 0, 1])
        );
        assert_eq!(
            move_node(&mut def, &[1, 0, 1], &Drop::After(vec![1, 0, 3])),
            Some(vec![1, 0, 3])
        );
        assert_eq!(
            ids(&def, &stack),
            ["", "resume2", "resume", "quit", "", "options", "button"]
        );
        assert_eq!(
            move_node(&mut def, &[1, 0, 6], &Drop::Into(vec![1])),
            Some(vec![1, 1])
        );
        assert_eq!(move_node(&mut def, &[1], &Drop::Into(vec![1, 0])), None);
        assert_eq!(
            move_node(&mut def, &[1, 0, 0], &Drop::Into(vec![1, 0, 1])),
            None
        );
        // A node moved out of an earlier sibling's subtree lands where the
        // drop pointed.
        assert_eq!(
            move_node(&mut def, &[0], &Drop::After(vec![1, 0, 0])),
            Some(vec![0, 0, 1])
        );
        assert_eq!(delete(&mut def, &[0, 0, 1]), Some(vec![0, 0, 1]));
        assert_eq!(delete(&mut def, &[]), None);
        assert!(
            trino_scene::ui::validate(&def)
                .iter()
                .all(|i| !i.message.contains("duplicate"))
        );
    }

    /// Saving keeps the file's leading comments and leaves a name the
    /// stem gives unwritten; a broken file keeps the last good screen.
    #[test]
    fn files_keep_their_comments_and_bad_text_keeps_the_last_screen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let file = root.join("ui/menu.ui.ron");
        let text = "// The menu.\n// Two lines.\n\n(version: 1, root: (children: [(kind: Text, text: \"Hi\")]))\n";
        std::fs::write(&file, text).unwrap();
        let mut s = UiScreens::load(root);
        assert_eq!(s.files, std::slice::from_ref(&file));
        assert_eq!(s.name(), "menu");
        s.def.as_mut().unwrap().root.children[0].text = "Hello".into();
        s.changed();
        let (path, out) = s.to_text().unwrap();
        assert_eq!(path, file);
        assert!(out.starts_with("// The menu.\n// Two lines.\n\n("), "{out}");
        assert!(out.contains("\"Hello\"") && !out.contains("name:"), "{out}");
        let rev = s.rev;
        let r = s.file_changed(root, &file, Some("(version: 1, root: (kind: Nope))"));
        assert!(matches!(r, Some(Err(_))), "{r:?}");
        assert_eq!(s.def.as_ref().unwrap().root.children[0].text, "Hello");
        assert!(s.rev > rev && !s.dirty);
        s.create(root, "shop").unwrap();
        assert!(s.create(root, "shop").is_err() && s.create(root, "a b").is_err());
        let (path, out) = s.to_text().unwrap();
        assert_eq!(path, root.join("ui/shop.ui.ron"));
        let back = ScreenDef::from_ron(&out).unwrap();
        assert_eq!(back.name, "shop");
        assert!(trino_scene::ui::validate(&back).is_empty());
    }

    #[test]
    fn hook_picks_screen_target_language_and_node() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("ui")).unwrap();
        std::fs::write(root.join("ui/pause.ui.ron"), pause().to_ron()).unwrap();
        std::fs::write(root.join("ui/a.ui.ron"), starter("a").to_ron()).unwrap();
        let mut s = UiScreens::load(root);
        assert_eq!(s.name(), "a");
        s.apply_hook("pause,3ds,pt-BR,quit");
        assert_eq!(s.name(), "pause");
        assert_eq!(TARGETS[s.target].key, "3ds");
        assert_eq!(s.lang.as_deref(), Some("pt-BR"));
        assert_eq!(s.selected, Some(vec![1, 0, 3]));
        s.apply_hook("pause,21:9,default");
        assert_eq!((TARGETS[s.target].key, s.lang.as_deref()), ("wide", None));
        assert_eq!(target_index("4K"), Some(3));
    }

    #[test]
    fn screens_fit_whole_pixels_and_stack_on_the_3ds() {
        let area = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(700.0, 500.0));
        let n64 = place(area, 1.0, (320, 240), None);
        assert_eq!(n64.top.size(), egui::vec2(640.0, 480.0), "2×");
        assert_eq!(n64.bottom, None);
        let ds = place(area, 1.0, (400, 240), Some((320, 240)));
        let b = ds.bottom.unwrap();
        assert_eq!(ds.top.size(), egui::vec2(400.0, 240.0));
        assert_eq!((b.min.y, b.width()), (ds.top.max.y, 320.0));
        assert_eq!(b.center().x, ds.top.center().x);
        let pc = place(area, 1.0, (1920, 1080), None);
        assert!((pc.top.width() - 700.0).abs() < 0.01, "{:?}", pc.top);
        assert!(pc.scale < 1.0);
        // Whole physical pixels on a 2× display: 1:1 is half a point.
        let hidpi = place(area, 2.0, (320, 240), None);
        assert_eq!((hidpi.scale, hidpi.top.width()), (4.0, 640.0));
    }
}
