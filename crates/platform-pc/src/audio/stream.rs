//! Streamed playback (issue #53): a decoder per stream fills a lock-free
//! ring on a worker thread; the output callback drains it. Memory stays at
//! the encoded file plus one ring (half a second of samples) per playing
//! stream, however long the track.
//!
//! [`Job::fill`] is the whole producer side, so an offline [`super::PcAudio`]
//! (tests, tools) runs the same code inline before each render.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;

/// A single-producer single-consumer ring of mono samples. No locks: the
/// output callback never waits for the decoder.
pub struct Ring {
    data: Box<[AtomicU32]>,
    /// Samples ever written / read (wrapping); the index is mod capacity.
    head: AtomicUsize,
    tail: AtomicUsize,
    /// The producer reached the end (a stream that doesn't loop).
    finished: AtomicBool,
    /// The consumer let go (voice stopped): the producer drops the job.
    closed: AtomicBool,
}

impl Ring {
    pub fn new(capacity: usize) -> Arc<Ring> {
        Arc::new(Ring {
            data: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            finished: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// Samples waiting to be read.
    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Acquire))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes this ring holds.
    pub fn bytes(&self) -> usize {
        self.data.len() * 4
    }

    /// Producer: write as much of `src` as fits; returns how much.
    pub fn push(&self, src: &[f32]) -> usize {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        let free = self.capacity() - h.wrapping_sub(t);
        let n = free.min(src.len());
        for (i, s) in src[..n].iter().enumerate() {
            self.data[h.wrapping_add(i) % self.capacity()].store(s.to_bits(), Ordering::Relaxed);
        }
        self.head.store(h.wrapping_add(n), Ordering::Release);
        n
    }

    /// Consumer: read up to `dst.len()` samples; returns how many.
    pub fn pop(&self, dst: &mut [f32]) -> usize {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        let n = h.wrapping_sub(t).min(dst.len());
        for (i, d) in dst[..n].iter_mut().enumerate() {
            *d = f32::from_bits(
                self.data[t.wrapping_add(i) % self.capacity()].load(Ordering::Relaxed),
            );
        }
        self.tail.store(t.wrapping_add(n), Ordering::Release);
        n
    }

    pub fn finish(&self) {
        self.finished.store(true, Ordering::Release);
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Played out: finished and drained.
    pub fn done(&self) -> bool {
        self.finished() && self.is_empty()
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// Where a stream's samples come from.
#[derive(Clone)]
pub enum StreamSource {
    /// An Ogg Vorbis file in memory and its length in samples (lewton
    /// decodes the padded last block; the loop seam must not hear it).
    Vorbis { bytes: Arc<[u8]>, frames: usize },
    /// Mono PCM at `rate`.
    Pcm { rate: u32, data: Arc<Vec<f32>> },
}

impl StreamSource {
    /// Bytes it holds in memory.
    pub fn bytes(&self) -> usize {
        match self {
            StreamSource::Vorbis { bytes, .. } => bytes.len(),
            StreamSource::Pcm { data, .. } => data.len() * 4,
        }
    }

    fn open(&self) -> Result<Box<dyn Decode>, String> {
        Ok(match self {
            StreamSource::Vorbis { bytes, frames } => {
                Box::new(VorbisDecode::open(bytes.clone(), *frames)?)
            }
            StreamSource::Pcm { rate, data } => Box::new(PcmDecode {
                rate: *rate,
                data: data.clone(),
                at: 0,
            }),
        })
    }
}

/// A decoder a stream pulls from.
trait Decode: Send {
    fn rate(&self) -> u32;
    /// Append the next block of mono samples to `out`; false at the end.
    fn next(&mut self, out: &mut Vec<f32>) -> bool;
    /// Back to the start (loops); false if that failed.
    fn rewind(&mut self) -> bool;
}

struct VorbisDecode {
    bytes: Arc<[u8]>,
    reader: OggStreamReader<Cursor<Arc<[u8]>>>,
    /// Samples left before the end.
    left: usize,
    frames: usize,
}

impl VorbisDecode {
    fn open(bytes: Arc<[u8]>, frames: usize) -> Result<Self, String> {
        // lewton reserves what a codebook's header says first (#121).
        trino_core::vorbis::check(&bytes).map_err(|e| format!("Vorbis stream: {e:?}"))?;
        let reader = OggStreamReader::new(Cursor::new(bytes.clone()))
            .map_err(|e| format!("Vorbis stream: {e}"))?;
        Ok(VorbisDecode {
            bytes,
            reader,
            left: frames,
            frames,
        })
    }
}

impl Decode for VorbisDecode {
    fn rate(&self) -> u32 {
        self.reader.ident_hdr.audio_sample_rate
    }

    fn next(&mut self, out: &mut Vec<f32>) -> bool {
        // An empty packet (the first audio packet of a stream) is not the end.
        let start = out.len();
        loop {
            if self.left == 0 {
                return false;
            }
            match self
                .reader
                .read_dec_packet_generic::<InterleavedSamples<f32>>()
            {
                Ok(Some(p)) if p.samples.is_empty() => continue,
                Ok(Some(p)) => {
                    if p.channel_count <= 1 {
                        out.extend_from_slice(&p.samples);
                    } else {
                        let c = p.channel_count;
                        out.extend(
                            p.samples
                                .chunks(c)
                                .map(|f| f.iter().sum::<f32>() / c as f32),
                        );
                    }
                    let n = (out.len() - start).min(self.left);
                    out.truncate(start + n);
                    self.left -= n;
                    return true;
                }
                // A damaged packet ends the stream rather than the program.
                Ok(None) | Err(_) => return false,
            }
        }
    }

    fn rewind(&mut self) -> bool {
        match Self::open(self.bytes.clone(), self.frames) {
            Ok(fresh) => {
                *self = fresh;
                true
            }
            Err(_) => false,
        }
    }
}

struct PcmDecode {
    rate: u32,
    data: Arc<Vec<f32>>,
    at: usize,
}

impl Decode for PcmDecode {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn next(&mut self, out: &mut Vec<f32>) -> bool {
        let end = (self.at + 4096).min(self.data.len());
        out.extend_from_slice(&self.data[self.at..end]);
        let more = end > self.at;
        self.at = end;
        more
    }

    fn rewind(&mut self) -> bool {
        self.at = 0;
        true
    }
}

/// Linear resampler that carries its phase across blocks (and across a
/// loop's seam).
struct Linear {
    /// Source samples per output sample.
    step: f64,
    /// Position in `buf`, in source samples.
    pos: f64,
    buf: Vec<f32>,
}

impl Linear {
    fn new(from: u32, to: u32) -> Self {
        Linear {
            step: from.max(1) as f64 / to.max(1) as f64,
            pos: 0.0,
            buf: Vec::new(),
        }
    }

    fn run(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        while self.pos + 1.0 < self.buf.len() as f64 {
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            out.push(self.buf[i] + (self.buf[i + 1] - self.buf[i]) * f);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.buf.len().saturating_sub(1));
        self.buf.drain(..used);
        self.pos -= used as f64;
    }

    /// The last sample, held back for interpolation, at the very end.
    fn flush(&mut self, out: &mut Vec<f32>) {
        if self.pos < self.buf.len() as f64 {
            out.push(self.buf[self.buf.len() - 1]);
        }
        self.buf.clear();
        self.pos = 0.0;
    }
}

/// One stream's producer: decode, resample to the output rate, push.
pub struct Job {
    ring: Arc<Ring>,
    dec: Box<dyn Decode>,
    looped: bool,
    rs: Linear,
    pending: Vec<f32>,
    at: usize,
    block: Vec<f32>,
    ending: bool,
}

impl Job {
    /// A job for `source` played at `out_rate`, and the ring it fills
    /// (`capacity` samples).
    pub fn start(
        source: &StreamSource,
        out_rate: u32,
        looped: bool,
        capacity: usize,
    ) -> Result<(Job, Arc<Ring>), String> {
        let dec = source.open()?;
        let ring = Ring::new(capacity);
        let rs = Linear::new(dec.rate(), out_rate);
        let job = Job {
            ring: ring.clone(),
            dec,
            looped,
            rs,
            pending: Vec::new(),
            at: 0,
            block: Vec::new(),
            ending: false,
        };
        Ok((job, ring))
    }

    /// Fill the ring as far as it goes. False when the job is over: the
    /// stream ended (the ring is marked finished) or the voice let go.
    pub fn fill(&mut self) -> bool {
        let mut empty_rewinds = 0;
        loop {
            if self.ring.closed() {
                return false;
            }
            if self.at < self.pending.len() {
                self.at += self.ring.push(&self.pending[self.at..]);
                if self.at < self.pending.len() {
                    return true; // full: come back later
                }
            }
            self.pending.clear();
            self.at = 0;
            if self.ending {
                self.ring.finish();
                return false;
            }
            self.block.clear();
            if self.dec.next(&mut self.block) {
                empty_rewinds = 0;
            } else if self.looped && empty_rewinds == 0 && self.dec.rewind() {
                empty_rewinds += 1;
                continue;
            } else {
                self.rs.flush(&mut self.pending);
                self.ending = true;
                continue;
            }
            self.rs.run(&self.block, &mut self.pending);
        }
    }
}

/// The worker thread that fills every playing stream's ring.
pub struct Streamer {
    tx: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Streamer {
    pub fn spawn() -> Streamer {
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("trino-audio-stream".into())
            .spawn(move || {
                // The fills on the trace's audio track (issue #130).
                let _lane = trino_core::trace::bind_thread(trino_core::trace::track::AUDIO);
                let mut jobs: Vec<Job> = Vec::new();
                loop {
                    let next = if jobs.is_empty() {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    } else {
                        // A ring holds half a second: 5 ms of latency is safe.
                        rx.recv_timeout(Duration::from_millis(5))
                    };
                    match next {
                        Ok(job) => {
                            jobs.push(job);
                            jobs.extend(rx.try_iter());
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                    trino_core::zone!("stream fill");
                    jobs.retain_mut(|j| {
                        trino_core::zone!("stream job");
                        j.fill()
                    });
                }
            })
            .ok();
        Streamer {
            tx: thread.as_ref().map(|_| tx),
            thread,
        }
    }

    /// Hand a (prefilled) job to the thread; gives it back if there is none.
    pub fn send(&self, job: Job) -> Result<(), Box<Job>> {
        match &self.tx {
            Some(tx) => tx.send(job).map_err(|e| Box::new(e.0)),
            None => Err(Box::new(job)),
        }
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        self.tx = None; // disconnects: the thread returns
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_wraps_and_reports() {
        let r = Ring::new(4);
        assert_eq!(r.push(&[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0; 2];
        assert_eq!(r.pop(&mut out), 2);
        assert_eq!(out, [1.0, 2.0]);
        assert_eq!(r.push(&[4.0, 5.0, 6.0, 7.0]), 3, "one slot was taken");
        let mut out = [0.0; 8];
        assert_eq!(r.pop(&mut out), 4);
        assert_eq!(out[..4], [3.0, 4.0, 5.0, 6.0]);
        assert!(r.is_empty() && !r.done());
        r.finish();
        assert!(r.done());
    }

    #[test]
    fn a_pcm_job_streams_resamples_and_loops() {
        let data: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let src = StreamSource::Pcm {
            rate: 100,
            data: Arc::new(data.clone()),
        };
        // Same rate, no loop: the samples, then finished.
        let (mut job, ring) = Job::start(&src, 100, false, 64).unwrap();
        assert!(!job.fill(), "short stream: over at once");
        let mut out = [0.0; 64];
        let n = ring.pop(&mut out);
        assert_eq!(out[..n], data[..]);
        assert!(ring.done());
        // Looped at half the rate: every other sample, across the seam.
        let (mut job, ring) = Job::start(&src, 50, true, 12).unwrap();
        assert!(job.fill(), "a loop never ends");
        let n = ring.pop(&mut out);
        assert_eq!(
            out[..n],
            [0.0, 2.0, 4.0, 6.0, 8.0, 0.0, 2.0, 4.0, 6.0, 8.0, 0.0, 2.0]
        );
        // The voice let go: the job ends.
        ring.close();
        assert!(!job.fill());
    }

    /// The worker thread keeps a ring topped up as the output drains it,
    /// ends the job at the end of the stream, and stops when dropped.
    #[test]
    fn the_worker_refills_rings_and_finishes() {
        let data: Vec<f32> = (0..10_000).map(|i| (i % 100) as f32).collect();
        let src = StreamSource::Pcm {
            rate: 1000,
            data: Arc::new(data.clone()),
        };
        let (mut job, ring) = Job::start(&src, 1000, false, 256).unwrap();
        assert!(job.fill(), "prefilled, not over");
        assert_eq!(ring.len(), 256);
        let streamer = Streamer::spawn();
        assert!(streamer.send(job).is_ok());
        let (mut got, mut out) = (Vec::new(), [0.0f32; 64]);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !ring.done() {
            let n = ring.pop(&mut out);
            got.extend_from_slice(&out[..n]);
            assert!(std::time::Instant::now() < deadline, "the worker stalled");
            std::thread::yield_now();
        }
        let n = ring.pop(&mut out);
        got.extend_from_slice(&out[..n]);
        assert_eq!(got, data, "every sample, in order, through the ring");
        drop(streamer); // joins the thread
    }
}
