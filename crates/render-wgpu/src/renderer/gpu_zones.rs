//! GPU zones for the timeline trace (issue #62): a timestamp at each pass
//! boundary of the main screen's frame (shadow map, scene, AO, upscale, 2D,
//! the N64 VI pass, the color-vision view), read back a frame or two later
//! and recorded on the trace's `gpu` track ([`trino_core::trace::record_zone`]).
//! Only while a trace records (`trace::is_on`), only where the adapter has
//! `TIMESTAMP_QUERY` (not WebGL2): otherwise nothing is created. A mark is an
//! empty compute pass that writes one timestamp, as [`super::gpu_timer`]
//! does for dynamic resolution, so no render pass descriptor changes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use trino_core::trace::{self, track};

const FREE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;
/// Timestamps per frame: one per pass boundary plus the end.
pub(super) const MAX_MARKS: u32 = 16;

struct Slot {
    buf: wgpu::Buffer,
    state: Arc<AtomicU8>,
    labels: Vec<&'static str>,
    /// The trace clock when the frame was submitted: the GPU work starts
    /// at or after it.
    cpu_us: u32,
}

pub(super) struct GpuZones {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: [Slot; 3],
    current: Option<usize>,
    labels: Vec<&'static str>,
    period_ns: f32,
    /// The GPU clock → the trace clock (µs), see [`place`].
    offset: Option<i64>,
}

impl GpuZones {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuZones> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let bytes = MAX_MARKS as u64 * 8;
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("trino-trace-timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: MAX_MARKS,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-trace-resolve"),
            size: bytes,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slot = || Slot {
            buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-trace-read"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(FREE)),
            labels: Vec::new(),
            cpu_us: 0,
        };
        Some(GpuZones {
            set,
            resolve,
            slots: [slot(), slot(), slot()],
            current: None,
            labels: Vec::new(),
            period_ns: queue.get_timestamp_period(),
            offset: None,
        })
    }

    fn write(&self, encoder: &mut wgpu::CommandEncoder, index: u32) {
        let _ = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trino-trace-mark"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.set,
                beginning_of_pass_write_index: Some(index),
                end_of_pass_write_index: None,
            }),
        });
    }

    /// The frame's GPU work starts: its first zone is `label`.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder, label: &'static str) {
        self.current = self
            .slots
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE);
        self.labels.clear();
        if self.current.is_some() {
            self.mark(encoder, label);
        }
    }

    /// The previous zone ends and `label` starts.
    pub fn mark(&mut self, encoder: &mut wgpu::CommandEncoder, label: &'static str) {
        if self.current.is_none() || self.labels.len() as u32 >= MAX_MARKS - 1 {
            return;
        }
        // Two marks in a row: the first zone was empty, rename it.
        self.write(encoder, self.labels.len() as u32);
        self.labels.push(label);
    }

    /// The frame's work is encoded: the last timestamp, then copy them out.
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.current else {
            return;
        };
        let n = self.labels.len() as u32;
        self.write(encoder, n);
        encoder.resolve_query_set(&self.set, 0..n + 1, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.slots[i].buf, 0, (n as u64 + 1) * 8);
        self.slots[i].labels = std::mem::take(&mut self.labels);
    }

    /// After the submit: map the frame's slot.
    pub fn submitted(&mut self) {
        let Some(i) = self.current.take() else {
            return;
        };
        let slot = &mut self.slots[i];
        slot.cpu_us = trace::now();
        slot.state.store(MAPPING, Ordering::Release);
        let state = slot.state.clone();
        slot.buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            state.store(if r.is_ok() { MAPPED } else { FREE }, Ordering::Release);
        });
    }

    /// Record the zones of every frame read back since the last call (polls
    /// the device without waiting). Returns how many frames.
    pub fn read(&mut self, device: &wgpu::Device) -> u32 {
        let _ = device.poll(wgpu::PollType::Poll);
        let mut frames = 0;
        for slot in &mut self.slots {
            if slot.state.load(Ordering::Acquire) != MAPPED {
                continue;
            }
            {
                let data = slot.buf.slice(..).get_mapped_range();
                let ts: Vec<u64> = data
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .take(slot.labels.len() + 1)
                    .map(|c| u64::from_le_bytes(*c))
                    .collect();
                for (name, depth, start, dur) in place(
                    &slot.labels,
                    &ts,
                    self.period_ns,
                    slot.cpu_us,
                    &mut self.offset,
                ) {
                    trace::record_zone(name, track::GPU, depth, start, dur);
                }
                #[cfg(all(feature = "tracy", not(target_arch = "wasm32")))]
                crate::tracy::gpu_frame(&slot.labels, &ts, self.period_ns);
            }
            slot.buf.unmap();
            slot.state.store(FREE, Ordering::Release);
            frames += 1;
        }
        frames
    }
}

/// One frame's GPU zones from its timestamps (`labels.len() + 1` of them, in
/// ticks of `period_ns`): `gpu frame` around them all at depth 0, each pass at
/// depth 1, as `(name, depth, start, duration)` on the trace's clock (µs).
///
/// The GPU clock is not the trace's: `offset` maps one onto the other. It is
/// fixed at the first frame (its start = the submit, `cpu_us`) and moved only
/// when a frame would start before its own submit, so frames keep the GPU's
/// own spacing and never overlap (a queued frame starts after the previous
/// one ends), and a pass's start and end come from the same mapping, so it
/// always lies inside its frame.
pub(super) fn place(
    labels: &[&'static str],
    ts: &[u64],
    period_ns: f32,
    cpu_us: u32,
    offset: &mut Option<i64>,
) -> Vec<(&'static str, u8, u32, u32)> {
    let us = |t: u64| (t as f64 * period_ns as f64 / 1000.0) as i64;
    let (Some(&t0), Some(&last)) = (ts.first(), ts.last()) else {
        return Vec::new();
    };
    if last < t0 || ts.windows(2).any(|w| w[1] < w[0]) {
        return Vec::new();
    }
    let mut off = offset.unwrap_or(cpu_us as i64 - us(t0));
    if us(t0) + off < cpu_us as i64 {
        off = cpu_us as i64 - us(t0);
    }
    *offset = Some(off);
    let at = |t: u64| (us(t) + off) as u32;
    let mut out = vec![("gpu frame", 0, at(t0), at(last) - at(t0))];
    for (i, label) in labels.iter().enumerate() {
        let (Some(&a), Some(&b)) = (ts.get(i), ts.get(i + 1)) else {
            break;
        };
        out.push((label, 1, at(a), at(b) - at(a)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_nest_in_their_frame_and_queued_frames_never_overlap() {
        let mut off = None;
        // 1 tick = 1 µs. Frame 1 submitted at 1 000 µs: anchored there.
        let a = place(
            &["scene", "2d"],
            &[50_000, 50_600, 50_900],
            1000.0,
            1_000,
            &mut off,
        );
        assert_eq!(
            a,
            [
                ("gpu frame", 0, 1_000, 900),
                ("scene", 1, 1_000, 600),
                ("2d", 1, 1_600, 300)
            ]
        );
        // Frame 2 submitted at 1 200 µs (before frame 1's GPU work ended),
        // its GPU work at 51 000: placed by the GPU's own spacing, after it.
        let b = place(&["scene"], &[51_000, 51_400], 1000.0, 1_200, &mut off);
        assert_eq!(b, [("gpu frame", 0, 2_000, 400), ("scene", 1, 2_000, 400)]);
        // A frame that would start before its submit moves the mapping on.
        let c = place(&["scene"], &[51_500, 51_700], 1000.0, 9_000, &mut off);
        assert_eq!(c[0], ("gpu frame", 0, 9_000, 200));
        // Rounding: fractional ticks still end every pass inside the frame.
        let mut off = None;
        let d = place(&["x", "y"], &[3, 7, 11], 333.3, 0, &mut off);
        let end = |z: &(&str, u8, u32, u32)| z.2 + z.3;
        assert!(d[1..].iter().all(|z| z.2 >= d[0].2 && end(z) <= end(&d[0])));
        assert_eq!(end(&d[2]), end(&d[0]));
        // Out-of-order timestamps (a driver hiccup) record nothing.
        assert!(place(&["x"], &[5, 3], 1.0, 0, &mut None).is_empty());
    }
}
