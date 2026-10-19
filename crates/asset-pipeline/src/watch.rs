//! Live-reload watching: masters change on disk → debounced rebake →
//! callback with the handles whose baked output actually changed.
//!
//! The debounce window absorbs the write+rename dance editors and Windows
//! do on save. Rebakes run on the watcher thread; callers receive changed
//! handle IDs and decide how to swap content in (the PC app re-uploads
//! textures/sounds under the same IDs).

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};

use crate::bake::bake_all;
use crate::manifest::Platform;

/// Watch `assets_root`, rebaking into `out_dir` on change. Returns a guard —
/// dropping it stops watching. `on_change` runs on the watcher thread with
/// the IDs whose baked bytes changed (no-op rebakes are not reported).
pub fn watch(
    assets_root: PathBuf,
    platform: Platform,
    out_dir: PathBuf,
    on_change: impl Fn(Vec<u32>) + Send + 'static,
) -> Result<impl Sized, String> {
    let watched_root = assets_root.clone();
    // Event paths arrive absolute: compare against the absolute directory.
    let abs_root = std::fs::canonicalize(&assets_root).unwrap_or_else(|_| assets_root.clone());
    let materials_dir = abs_root.join("materials");
    // Baked scenes ([scenes.*]) also depend on scenes/ and prefabs/ (next
    // to assets/) and on materials.
    let project = abs_root.parent().map(Path::to_path_buf);
    let has_scenes = {
        let root = assets_root.clone();
        move || {
            crate::Manifest::load(&root.join("manifest.toml"))
                .map(|m| !m.scenes.is_empty())
                .unwrap_or(false)
        }
    };

    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        None,
        move |result: DebounceEventResult| match result {
            // Only real changes rebake. Reading a master emits inotify
            // *access* events — reacting to those made every bake trigger the
            // next one (a full rebake every ~150 ms, forever, while idle).
            // `assets/materials/` holds scene-layer RON, not bake inputs.
            Ok(events)
                if events.iter().any(|e| {
                    !matches!(e.event.kind, EventKind::Access(_))
                        && e.event.paths.iter().any(|p| {
                            let scene_input = p.starts_with(&materials_dir)
                                || project.as_ref().is_some_and(|r| {
                                    p.starts_with(r.join("scenes"))
                                        || p.starts_with(r.join("prefabs"))
                                });
                            !scene_input || has_scenes()
                        })
                }) =>
            {
                match bake_all(&assets_root, platform, &out_dir) {
                    Ok(report) => {
                        let changed = report.changed_ids();
                        if !changed.is_empty() {
                            on_change(changed);
                        }
                    }
                    // A half-saved file can fail a bake; report and keep
                    // watching — the next save usually fixes it.
                    Err(e) => eprintln!("trino-assets: rebake failed:\n{e}"),
                }
            }
            Ok(_) => {}
            Err(errors) => {
                for e in errors {
                    eprintln!("trino-assets: watch error: {e}");
                }
            }
        },
    )
    .map_err(|e| format!("failed to start watcher: {e}"))?;

    debouncer
        .watch(&watched_root, RecursiveMode::Recursive)
        .map_err(|e| format!("failed to watch {}: {e}", watched_root.display()))?;
    if let Some(project) = watched_root.parent() {
        for dir in ["scenes", "prefabs", crate::ui::UI_DIR] {
            let d = project.join(dir);
            if d.is_dir() {
                debouncer
                    .watch(&d, RecursiveMode::Recursive)
                    .map_err(|e| format!("failed to watch {}: {e}", d.display()))?;
            }
        }
    }

    Ok(debouncer)
}
