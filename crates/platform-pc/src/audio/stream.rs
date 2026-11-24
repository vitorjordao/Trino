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

    /// Its length in samples at `out_rate` (`None` when the header doesn't
    /// say).
    pub fn output_frames(&self, out_rate: u32) -> Option<u64> {
        let (frames, rate) = match self {
            StreamSource::Vorbis { bytes, frames } => {
                let rate =
                    lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(&bytes[..]))
                        .ok()?
                        .ident_hdr
                        .audio_sample_rate;
                (*frames as u64, rate)
            }
            StreamSource::Pcm { rate, data } => (data.len() as u64, *rate),
        };
        (rate > 0).then(|| frames * out_rate as u64 / rate as u64)
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
    /// Go to sample `frame` and append the samples from there to the end of
    /// the block it falls in (a synced crossfade, Bongo-Kart #177); false
    /// past the end or on a damaged stream.
    fn seek(&mut self, frame: usize, out: &mut Vec<f32>) -> bool;
}

/// How far before a seek's target lewton's page seek lands: it seeks with a
/// page's granularity and the first packet after it only primes the
/// decoder, so the target must lie past both.
const SEEK_MARGIN: u64 = 8192;

/// Mono samples of a decoded packet.
fn mono(p: &InterleavedSamples<f32>, out: &mut Vec<f32>) {
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
                    mono(&p, out);
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

    fn seek(&mut self, frame: usize, out: &mut Vec<f32>) -> bool {
        if frame >= self.frames || !self.rewind() {
            return false;
        }
        // A page seek lands at most a page early. Near the start it may land
        // on the header pages (decoding fails), near the end in the last
        // page (its padded packet hides where its samples start): then a
        // page further back, else from the top.
        let mut back = (frame as u64).saturating_sub(SEEK_MARGIN);
        for _ in 0..2 {
            if back == 0 {
                break;
            }
            if self.reader.seek_absgp_pg(back).is_ok() && self.drop_to(None, frame, out) {
                return true;
            }
            back = back.saturating_sub(1 << 16);
            if !self.rewind() {
                return false;
            }
        }
        self.rewind() && self.drop_to(Some(0), frame, out)
    }
}

impl VorbisDecode {
    /// Decode and drop up to sample `frame`, then append the rest of its
    /// packet. `known` is the position now when it is known (the stream's
    /// start); after a page seek it is learned once a packet ends a page
    /// (lewton's last granule: where that packet's samples end).
    fn drop_to(&mut self, mut known: Option<usize>, frame: usize, out: &mut Vec<f32>) -> bool {
        let mut buf = Vec::new();
        loop {
            let p = match self
                .reader
                .read_dec_packet_generic::<InterleavedSamples<f32>>()
            {
                Ok(Some(p)) => p,
                Ok(None) | Err(_) => return false,
            };
            mono(&p, &mut buf);
            let end = match known {
                Some(at) => at + buf.len(),
                None => match self.reader.get_last_absgp() {
                    // The stream's last packet: untrimmed, it can't place
                    // its samples.
                    Some(g) if g as usize >= self.frames => return false,
                    Some(g) => g as usize,
                    None => continue,
                },
            };
            if end > frame {
                // A seek that landed past the target starts where it can
                // (within a packet), never earlier than asked.
                let start = end.saturating_sub(buf.len());
                let from = frame.saturating_sub(start).min(buf.len());
                let to = buf.len().min(self.frames.saturating_sub(start));
                out.extend_from_slice(&buf[from..to.max(from)]);
                self.left = self.frames.saturating_sub(start + to.max(from));
                return true;
            }
            known = Some(end);
            buf.clear();
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

    fn seek(&mut self, frame: usize, out: &mut Vec<f32>) -> bool {
        if frame >= self.data.len() {
            return false;
        }
        self.at = frame;
        self.next(out)
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

    /// [`Job::start`] from output sample `skip` on (a synced crossfade):
    /// the decoder seeks there (a page seek, then a few packets decoded and
    /// dropped), so the ring's first sample is the one asked for. Past the
    /// end, or where the seek fails, it starts from the top.
    pub fn start_at(
        source: &StreamSource,
        out_rate: u32,
        looped: bool,
        capacity: usize,
        skip: u64,
    ) -> Result<(Job, Arc<Ring>), String> {
        let (mut job, ring) = Job::start(source, out_rate, looped, capacity)?;
        if skip > 0 {
            let rate = job.dec.rate().max(1) as u64;
            let frame = (skip * rate / out_rate.max(1) as u64) as usize;
            let mut block = Vec::new();
            if job.dec.seek(frame, &mut block) {
                job.rs.run(&block, &mut job.pending);
            } else {
                job.dec.rewind();
            }
        }
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

    /// A Vorbis `TSN2` payload of `secs` seconds of a two-partial melody at
    /// 16 kHz, and its frame count.
    fn vorbis(secs: u32) -> (Arc<[u8]>, usize) {
        use trino_asset_pipeline::audio::{AudioSettings, Codec, Master};
        let rate = 16_000u32;
        let n = (rate * secs) as usize;
        let samples = (0..n)
            .map(|i| {
                let f = 220.0 * 2f32.powf(((i / 4000) % 12) as f32 / 12.0);
                let t = i as f32 / rate as f32;
                0.3 * (t * f * std::f32::consts::TAU).sin()
                    + 0.1 * (t * 3.0 * f * std::f32::consts::TAU).sin()
            })
            .collect();
        let settings = AudioSettings {
            codec: Codec::Vorbis,
            auto: false,
            quality: 4.0,
            sample_rate: None,
            stream: true,
            looped: false,
            bus: trino_core::Bus::Music,
        };
        let master = Master { rate, samples };
        let enc = trino_asset_pipeline::audio::encode(
            trino_asset_pipeline::Platform::Pc,
            &settings,
            &master,
            1,
        )
        .unwrap();
        let blob = enc.blob.unwrap();
        let b = trino_core::SoundBlob::parse(&blob).unwrap();
        (b.payload.into(), b.frames as usize)
    }

    /// Every sample of a job, filled to its end.
    fn drain(mut job: Job, ring: &Ring, limit: usize) -> Vec<f32> {
        let (mut got, mut out) = (Vec::new(), vec![0.0f32; 4096]);
        loop {
            let more = job.fill();
            let n = ring.pop(&mut out);
            got.extend_from_slice(&out[..n]);
            if (!more && ring.is_empty()) || got.len() >= limit {
                return got;
            }
        }
    }

    /// A synced crossfade starts a stream mid-way (Bongo-Kart #177): the
    /// page seek plus the packets decoded and dropped land on the sample
    /// asked for, so the job plays exactly what the whole decode plays from
    /// there (no offset, no priming glitch).
    #[test]
    fn a_vorbis_job_started_mid_way_plays_the_whole_decodes_samples_from_there() {
        let (bytes, frames) = vorbis(12);
        let src = StreamSource::Vorbis { bytes, frames };
        assert_eq!(src.output_frames(16_000), Some(frames as u64));
        assert_eq!(src.output_frames(48_000), Some(frames as u64 * 3));
        let (job, ring) = Job::start(&src, 16_000, false, 1 << 20).unwrap();
        let whole = drain(job, &ring, usize::MAX);
        assert_eq!(whole.len(), frames);
        for skip in [1usize, 777, 8_192, 8_193, 50_000, 123_457, frames - 300] {
            let (job, ring) = Job::start_at(&src, 16_000, false, 1 << 16, skip as u64).unwrap();
            let got = drain(job, &ring, 4096);
            let want = &whole[skip..(skip + 4096).min(frames)];
            assert_eq!(
                got.len().min(want.len()),
                want.len(),
                "skip {skip}: cut short"
            );
            let worst = got
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-5, "skip {skip}: off by {worst}");
        }
        // Past the end: from the top.
        let (job, ring) = Job::start_at(&src, 16_000, false, 1 << 16, frames as u64 + 5).unwrap();
        let got = drain(job, &ring, 256);
        assert_eq!(got[..256], whole[..256]);
    }

    #[test]
    fn a_pcm_job_started_mid_way_starts_there() {
        let data: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        let src = StreamSource::Pcm {
            rate: 100,
            data: Arc::new(data.clone()),
        };
        let (job, ring) = Job::start_at(&src, 100, false, 2048, 640).unwrap();
        assert_eq!(drain(job, &ring, usize::MAX), data[640..]);
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
