//! The main screen's GPU time per frame, from timestamp queries (issue #55:
//! what dynamic resolution holds). Two empty compute passes bracket the
//! frame's work, the pair is resolved into one of three read-back buffers,
//! and a buffer is read when its map completes, a frame or two later.
//! Adapters without `TIMESTAMP_QUERY` (WebGL2, some drivers) have none: the
//! host's frame intervals stand in ([`super::PcRenderer::report_frame_time`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

const FREE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;

struct Slot {
    buf: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

pub(super) struct GpuTimer {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: [Slot; 3],
    /// The slot this frame's pair goes to (`None`: every slot is busy,
    /// this frame isn't timed).
    current: Option<usize>,
    period_ns: f32,
}

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuTimer> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("trino-frame-timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-timestamps-resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slot = || Slot {
            buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-timestamps-read"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            state: Arc::new(AtomicU8::new(FREE)),
        };
        Some(GpuTimer {
            set,
            resolve,
            slots: [slot(), slot(), slot()],
            current: None,
            period_ns: queue.get_timestamp_period(),
        })
    }

    /// The frame starts: an empty pass writes the first timestamp.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.current = self
            .slots
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE);
        if self.current.is_none() {
            return;
        }
        let _ = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trino-ts-begin"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.set,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: None,
            }),
        });
    }

    /// The frame's work is encoded: the second timestamp, then the pair is
    /// copied out for reading.
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.current else {
            return;
        };
        let _ = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trino-ts-end"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.set,
                beginning_of_pass_write_index: None,
                end_of_pass_write_index: Some(1),
            }),
        });
        encoder.resolve_query_set(&self.set, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.slots[i].buf, 0, 16);
    }

    /// After the submit: start mapping this frame's slot.
    pub fn submitted(&mut self) {
        let Some(i) = self.current.take() else {
            return;
        };
        let slot = &self.slots[i];
        slot.state.store(MAPPING, Ordering::Release);
        let state = slot.state.clone();
        slot.buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            state.store(if r.is_ok() { MAPPED } else { FREE }, Ordering::Release);
        });
    }

    /// The newest GPU time read back since the last call, µs (polls the
    /// device without waiting).
    pub fn read(&mut self, device: &wgpu::Device) -> Option<u32> {
        let _ = device.poll(wgpu::PollType::Poll);
        let mut newest = None;
        for slot in &self.slots {
            if slot.state.load(Ordering::Acquire) != MAPPED {
                continue;
            }
            {
                let data = slot.buf.slice(..).get_mapped_range();
                let t0 = u64::from_le_bytes(data[0..8].try_into().unwrap());
                let t1 = u64::from_le_bytes(data[8..16].try_into().unwrap());
                if t1 > t0 {
                    let us = (t1 - t0) as f64 * self.period_ns as f64 / 1000.0;
                    newest = Some(us.min(u32::MAX as f64) as u32);
                }
            }
            slot.buf.unmap();
            slot.state.store(FREE, Ordering::Release);
        }
        newest
    }
}
