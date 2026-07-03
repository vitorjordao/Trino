//! Live-reload watching: masters change on disk → debounced rebake →
//! callback with the handles whose baked output actually changed.
//!
//! The debounce window absorbs the write+rename dance editors and Windows
//! do on save. Rebakes run on the watcher thread; callers receive changed
//! handle IDs and decide how to swap content in (the PC app re-uploads
//! textures/sounds under the same IDs).

use std::path::PathBuf;
use std::time::Duration;

use notify_debouncer_full::notify::RecursiveMode;
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

    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) if !events.is_empty() => {
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

    Ok(debouncer)
}
