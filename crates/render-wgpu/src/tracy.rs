//! Tracy on the PC (issue #62, feature `tracy`): the same zones the trace
//! records ([`trino_core::trace`]) as Tracy spans while they happen, counters
//! as plots, frame marks as Tracy frames, and the GPU passes' timestamps
//! (`renderer::gpu_zones`) as a Tracy GPU context. On-demand: a run without a
//! connected Tracy viewer sends nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use tracy_client::{Client, GpuContext, GpuContextType, PlotName, Span};
use trino_core::trace::{self, Hooks};

thread_local! {
    /// Open spans of this thread, innermost last (zones nest per thread).
    static SPANS: RefCell<Vec<Span>> = const { RefCell::new(Vec::new()) };
}

static PLOTS: Mutex<Option<HashMap<&'static str, PlotName>>> = Mutex::new(None);
static GPU: Mutex<Option<GpuContext>> = Mutex::new(None);

/// Start the Tracy client and route the engine's zones to it. Works with or
/// without a trace ring installed; returns `false` if already started.
pub fn start() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    // Our own flag: the Tracy client itself is already running from the
    // process's start (its C++ static initializer), hooks or not.
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return false;
    }
    let _ = Client::start();
    trace::install_hooks_clock(crate::trace_host::clock);
    trace::set_hooks(Some(Hooks {
        begin,
        end,
        counter,
        frame,
    }));
    true
}

fn begin(_track: u8, name: &'static str) {
    if let Some(c) = Client::running() {
        let span = c.span_alloc(Some(name), name, "trino", 0, 0);
        SPANS.with(|s| s.borrow_mut().push(span));
    }
}

fn end(_track: u8, _name: &'static str, _start: u32, _end: u32) {
    SPANS.with(|s| drop(s.borrow_mut().pop()));
}

fn counter(name: &'static str, value: u32) {
    let Some(c) = Client::running() else {
        return;
    };
    let mut plots = PLOTS.lock().unwrap_or_else(|e| e.into_inner());
    let plot = *plots
        .get_or_insert_with(HashMap::new)
        .entry(name)
        .or_insert_with(|| PlotName::new_leak(name.to_string()));
    c.plot(plot, value as f64);
}

fn frame(_n: u32) {
    if let Some(c) = Client::running() {
        c.frame_mark();
    }
}

/// One frame's GPU passes (`labels.len() + 1` raw timestamps in ticks of
/// `period_ns`) as Tracy GPU spans, uploaded in increasing order.
pub(crate) fn gpu_frame(labels: &[&'static str], ts: &[u64], period_ns: f32) {
    let Some(c) = Client::running() else {
        return;
    };
    let (Some(&t0), Some(&last)) = (ts.first(), ts.last()) else {
        return;
    };
    let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    if gpu.is_none() {
        *gpu = c
            .new_gpu_context(Some("wgpu"), GpuContextType::Invalid, t0 as i64, period_ns)
            .ok();
    }
    let Some(ctx) = gpu.as_ref() else {
        return;
    };
    let Ok(mut outer) = ctx.span_alloc("gpu frame", "gpu", "trino", 0) else {
        return;
    };
    outer.upload_timestamp_start(t0 as i64);
    for (i, label) in labels.iter().enumerate() {
        let (Some(&a), Some(&b)) = (ts.get(i), ts.get(i + 1)) else {
            break;
        };
        if let Ok(mut span) = ctx.span_alloc(label, "gpu", "trino", 0) {
            span.upload_timestamp_start(a as i64);
            span.end_zone();
            span.upload_timestamp_end(b.max(a) as i64);
        }
    }
    outer.end_zone();
    outer.upload_timestamp_end(last as i64);
}
