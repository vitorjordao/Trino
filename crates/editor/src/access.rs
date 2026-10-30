//! What the Accessibility tab edits and shows (issue #60, ADR 0027):
//! `assets/palettes.ron` (the semantic colors and their color-vision
//! alternatives — the same file an agent writes by hand), the simulation
//! the Scene, Game and UI views draw through (a color-vision mode, the UI's
//! text size and high contrast), and the checklist `cargo xtask access
//! check` prints, from the same code.

use std::path::{Path, PathBuf};

use trino_asset_pipeline::access::AccessReport;
use trino_core::AccessSettings;
use trino_scene::access::{PalettesDef, palettes_path};

pub struct AccessState {
    /// What the tab shows (edits land here first).
    pub palettes: PalettesDef,
    /// The palettes file exists (else the first edit creates it).
    pub exists: bool,
    /// Why the file on disk isn't valid (the tab keeps the last good one).
    pub error: Option<String>,
    /// Edited and not written yet.
    pub dirty: bool,
    /// Bumped on every change (the checklist reruns).
    pub rev: u64,
    /// The views' simulation: the color vision they show, the UI tab's text
    /// size and high contrast. Not the game's settings: a review tool.
    pub preview: AccessSettings,
    /// Show the views as the previewed viewer sees them (the simulation);
    /// off, they show the picture as drawn (the correction filter alone,
    /// `preview.cvd_filter`, issue #64).
    pub simulate: bool,
    /// The last checklist and the revision it ran at.
    pub report: Option<Result<AccessReport, String>>,
    pub report_rev: u64,
    /// The audio inspector's caption preview (issue #96).
    pub caption: CaptionPreview,
}

impl AccessState {
    /// What the views draw with (issue #96): the palettes, and the
    /// simulated settings while the simulation is on (the defaults
    /// otherwise: semantic slots at their base, no outlines).
    pub fn view(&self) -> (&PalettesDef, AccessSettings) {
        (
            &self.palettes,
            if self.simulate {
                self.preview
            } else {
                AccessSettings::default()
            },
        )
    }

    pub fn load(root: &Path) -> AccessState {
        let mut s = AccessState {
            palettes: PalettesDef::default(),
            exists: false,
            error: None,
            dirty: false,
            rev: 1,
            preview: AccessSettings::DEFAULT,
            simulate: true,
            report: None,
            report_rev: 0,
            caption: CaptionPreview::default(),
        };
        let text = std::fs::read_to_string(palettes_path(root)).ok();
        s.reload(text.as_deref());
        // Dev hook: TRINO_EDITOR_ACCESS=cvd=deuteranopia,text=200,contrast=on
        // starts the views with that simulation (screenshots, tests).
        if let Ok(v) = std::env::var("TRINO_EDITOR_ACCESS") {
            s.preview = AccessSettings::parse_text(AccessSettings::DEFAULT, &v);
        }
        s
    }

    pub fn path(root: &Path) -> PathBuf {
        palettes_path(root)
    }

    /// The file's new content (`None`: deleted). A broken file keeps the
    /// last good palettes and says why.
    pub fn reload(&mut self, text: Option<&str>) {
        self.exists = text.is_some();
        self.dirty = false;
        self.rev += 1;
        match text.map(PalettesDef::from_ron) {
            None => {
                self.palettes = PalettesDef::default();
                self.error = None;
            }
            Some(Ok(p)) => {
                self.palettes = p;
                self.error = None;
            }
            Some(Err(e)) => self.error = Some(e),
        }
    }

    /// Something the checklist reads changed (the manifest, a string
    /// table, a screen, the theme): run it again.
    pub fn stale(&mut self) {
        self.rev += 1;
    }

    /// A tab edit: written once, the checklist reruns.
    pub fn edited(&mut self) {
        self.dirty = true;
        self.rev += 1;
    }

    pub fn to_text(&self) -> String {
        self.palettes.to_ron()
    }

    /// The checklist, run again when anything it reads changed since.
    pub fn refresh_report(&mut self, root: &Path) {
        if self.report.is_some() && self.report_rev == self.rev {
            return;
        }
        self.report_rev = self.rev;
        let manifest = trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml"));
        self.report = Some(manifest.and_then(|m| {
            trino_asset_pipeline::access::check_project(
                root,
                &m,
                &[
                    trino_core::Family::N64,
                    trino_core::Family::N3ds,
                    trino_core::Family::Pc,
                    trino_core::Family::Web,
                ],
            )
        }));
    }
}

/// The audio inspector's caption Preview (issue #96): a caption queued in
/// the Game view as a player with the Accessibility tab's simulated text
/// size and contrast sees it, where the chosen style puts it.
pub struct CaptionPreview {
    pub queue: trino_core::access::Captions,
    pub place: trino_core::access::CaptionPlace,
    last: Option<std::time::Instant>,
}

impl Default for CaptionPreview {
    fn default() -> Self {
        CaptionPreview {
            queue: trino_core::access::Captions::new(),
            place: trino_core::access::CaptionPlace::Bottom,
            last: None,
        }
    }
}

impl CaptionPreview {
    /// Queue `key`'s caption with the sound's settings. The preview shows
    /// it even when the simulated caption mode is off (it was asked for).
    pub fn show(
        &mut self,
        key: &str,
        keys: &crate::audio::CaptionKeys,
        place: trino_core::access::CaptionPlace,
        settings: &AccessSettings,
    ) {
        // A `'static` key, as a game's caption table's are: one small
        // string per click.
        let key: &'static str = Box::leak(key.to_owned().into_boxed_str());
        let mut s = *settings;
        s.captions = trino_core::access::CaptionMode::All;
        self.queue.configure(None, &s);
        self.place = place;
        let important = keys.important.unwrap_or(false);
        self.queue.push(
            trino_core::access::CaptionInfo {
                key,
                priority: keys.priority.unwrap_or(if important { 100 } else { 0 }),
                important,
                direction: keys.direction.unwrap_or(false),
                duration_ms: keys.seconds.map_or(0, |t| (t * 1000.0) as u16),
            },
            keys.direction.unwrap_or(false).then_some(0.8),
        );
        self.last = Some(std::time::Instant::now());
    }

    /// Age the queue by the time since the last call; `true` while it
    /// shows something (the views repaint).
    pub fn tick(&mut self) -> bool {
        if let Some(t) = self.last {
            let now = std::time::Instant::now();
            self.queue.update(now.duration_since(t).as_secs_f32());
            self.last = Some(now);
        }
        !self.queue.is_empty()
    }
}
