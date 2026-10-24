//! Every render pipeline goes through [`render`] (issue #101): it hands the
//! driver the renderer's persistent [`wgpu::PipelineCache`] (Vulkan, when the
//! adapter has `PIPELINE_CACHE`) and counts creations, apart for the
//! precompile workers and the render thread, so a test can prove a frame
//! compiled nothing (`TRINO_FRAME`'s `pipes=`).
//!
//! The ~20 pipeline builders keep their `&wgpu::Device` signatures: the
//! renderer opens a [`Scope`] on the thread around the code that may build
//! pipelines (and each worker opens one for its jobs), and [`render`] reads
//! it. A device can't be the key: `wgpu::Device` compares by an id that
//! repeats across instances, so two renderers alive at once would share
//! an entry and one device would get the other's cache.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// One renderer's cache and counters.
#[derive(Debug, Default)]
pub(crate) struct DeviceEntry {
    pub cache: Option<wgpu::PipelineCache>,
    /// Pipelines created on the render thread (startup, lazy first uses).
    pub render_thread: AtomicU32,
    /// Pipelines created by precompile workers (or the Web's time slices).
    pub warm: AtomicU32,
}

impl DeviceEntry {
    // The Web's wgpu handles are not `Send`: there is one thread there.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub fn new(cache: Option<wgpu::PipelineCache>) -> Arc<DeviceEntry> {
        Arc::new(DeviceEntry {
            cache,
            ..DeviceEntry::default()
        })
    }
}

/// Pipelines built outside any [`Scope`], process-wide: always 0 unless
/// a lazy site lost its scope (`precompile.rs` asserts it stays put).
static UNSCOPED: AtomicU32 = AtomicU32::new(0);

pub(crate) fn unscoped() -> u32 {
    UNSCOPED.load(Ordering::Relaxed)
}

thread_local! {
    static CURRENT: RefCell<Option<(Arc<DeviceEntry>, bool)>> = const { RefCell::new(None) };
}

/// While alive, this thread's pipelines count for `entry` and use its
/// cache. Scopes nest (the previous one comes back on drop).
pub(crate) struct Scope {
    prev: Option<(Arc<DeviceEntry>, bool)>,
}

fn enter(entry: &Arc<DeviceEntry>, warm: bool) -> Scope {
    let prev = CURRENT.with(|c| c.replace(Some((entry.clone(), warm))));
    Scope { prev }
}

/// The render thread's scope.
pub(crate) fn scope(entry: &Arc<DeviceEntry>) -> Scope {
    enter(entry, false)
}

/// A precompile job's scope: its creations count as `warm`.
pub(crate) fn warm_scope(entry: &Arc<DeviceEntry>) -> Scope {
    enter(entry, true)
}

impl Drop for Scope {
    fn drop(&mut self) {
        let prev = self.prev.take();
        CURRENT.with(|c| *c.borrow_mut() = prev);
    }
}

/// `device.create_render_pipeline(&desc)` with the scope's cache, counted.
pub(crate) fn render(
    device: &wgpu::Device,
    desc: wgpu::RenderPipelineDescriptor<'_>,
) -> wgpu::RenderPipeline {
    let Some((e, warm)) = CURRENT.with(|c| c.borrow().clone()) else {
        UNSCOPED.fetch_add(1, Ordering::Relaxed);
        return device.create_render_pipeline(&desc);
    };
    let counter = if warm { &e.warm } else { &e.render_thread };
    counter.fetch_add(1, Ordering::Relaxed);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        cache: e.cache.as_ref(),
        ..desc
    })
}
