//! N64 audio: libdragon's `wav64` container and VADPCM, the ADPCM codec
//! its `audioconv64` writes by default (`--wav-compress 1`). The tools read
//! what the ROM will hold, decode it exactly as the console does, and —
//! where the N64 toolchain is missing — encode VADPCM themselves so the PC
//! can play what the console will sound like.
//!
//! Layout at the pinned libdragon (`2c3d5db6`, `wav64_internal.h`), all
//! big-endian. Header, 24 bytes: `WV64`, version 2, format (0 raw,
//! 1 VADPCM, 3 Opus), channels, bits, rate, frames, loop length, data
//! offset (`i32`s from byte 8). VADPCM continues with predictors (4),
//! order (2), 6 pad bytes, loop and play state (4 × 8 `i16`, zero) and
//! `predictors × order × channels` codebook vectors of 8 `i16`: data at
//! 96 + 128 × channels. Its data is 9-byte frames, one per channel in turn,
//! zero-padded to a multiple of 32 samples (the RSP decodes frame pairs);
//! the header's frame count is the unpadded one. Raw data is `i16` (or
//! `i8`) interleaved. Opus adds frame size, max packet and bitrate (3 ×
//! `u32`), then `u16`-length-prefixed CELT packets at 48 kHz.
//!
//! The console decodes VADPCM on the RSP (`rsp_mixer.S`,
//! `VADPCM_Decompress`): residuals scaled in 16-bit lanes (saturating),
//! accumulated in 32 bits, `>> 11`, clamped. [`decode`] does the same math;
//! for `audioconv64` output (scale ≤ 12) that equals libdragon's C
//! reference decoder, which the tests pin against.
//!
//! Fixtures (`tests/fixtures/audio`): `n64_tone.wav` is a script-made
//! 440 Hz tone + 200–3000 Hz chirp (8017 frames, 22050 Hz, mono); the
//! wav64s are the pinned `audioconv64`'s (`trino-n64` image, `out/` must
//! exist), renamed `n64_tone_vadpcm.wav64` / `n64_tone_raw.wav64`:
//! `docker run --rm -u $(id -u):$(id -g) -v $PWD:/w -w /w trino-n64
//! audioconv64 -o out --wav-compress 1 n64_tone.wav` (and `0`).

/// Codec of a wav64 file (`--wav-compress`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wav64Format {
    /// `0`: PCM, 8 or 16 bits.
    Raw,
    /// `1`, `audioconv64`'s default: 9 bytes per 16 samples, decoded on
    /// the RSP.
    Vadpcm,
    /// `3`: CELT at 48 kHz; needs `wav64_init_compression(3)` on the
    /// console.
    Opus,
}

/// A wav64 header.
#[derive(Clone, Debug, PartialEq)]
pub struct Wav64Info {
    /// Codec of the data.
    pub format: Wav64Format,
    /// Interleaved channels: 1 or 2.
    pub channels: u8,
    /// Bits per sample: 8 or 16 (always 16 for VADPCM and Opus).
    pub bits: u8,
    /// Playback rate in Hz (Opus is always resampled to 48000).
    pub sample_rate: u32,
    /// Sample frames played; VADPCM data is padded past it to 32.
    pub frames: u32,
    /// Loop length back from the end, 0 = no loop.
    pub loop_len: u32,
    /// Everything before the samples (header, codec header, codebook).
    pub header_bytes: usize,
    /// Everything from there to the end of the file.
    pub data_bytes: usize,
}

/// A VADPCM stream: the codebook and the 9-byte frames of 16 samples (mono).
#[derive(Clone, Debug, PartialEq)]
pub struct Vadpcm {
    /// Predictor order (2; the RSP decodes no other).
    pub order: usize,
    /// Predictors in the codebook (a frame's low nibble picks one).
    pub predictors: usize,
    /// `predictors × order` vectors, predictor-major: vector `k` of
    /// predictor `p` at `p * order + k`. Vector `k` weighs history sample
    /// `8 - order + k` of the previous 8 into each of the next 8 samples
    /// (Q11); the last one also spreads each residual forward.
    pub codebook: Vec<[i16; 8]>,
    /// Frames: control byte (scale << 4 | predictor), then 16 residual
    /// nibbles, high nibble first.
    pub frames: Vec<u8>,
    /// Samples the frames hold before the zero padding.
    pub samples: usize,
}

/// Samples per VADPCM frame (two vectors of 8).
pub const SAMPLES_PER_FRAME: usize = 16;
/// Bytes per VADPCM frame: control byte + 16 nibbles.
pub const BYTES_PER_FRAME: usize = 9;

const HEADER_BYTES: usize = 24;
/// Predictors, order, padding, loop state and state.
const VADPCM_EXT_BYTES: usize = 72;
const OPUS_EXT_BYTES: usize = 12;
/// `audioconv64`'s codebook: 4 predictors of order 2 per channel, which is
/// also what one channel's slot in the RSP's codebook buffer holds.
const PREDICTORS: usize = 4;
const ORDER: usize = 2;
/// Largest scale `audioconv64` writes; above it the RSP's 16-bit residual
/// lanes saturate.
const MAX_SCALE: u32 = 12;
/// `audioconv64` pads VADPCM to frame pairs.
const PAD_SAMPLES: usize = 2 * SAMPLES_PER_FRAME;

fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Parse a wav64 header (the pinned libdragon's layout).
pub fn parse_wav64(bytes: &[u8]) -> Result<Wav64Info, String> {
    let head = bytes
        .get(..HEADER_BYTES)
        .ok_or("wav64: shorter than its 24-byte header")?;
    if &head[..4] != b"WV64" {
        return Err(if &head[..4] == b"RIFF" {
            "wav64: this is a RIFF WAV, not a wav64".into()
        } else {
            "wav64: missing WV64 magic".into()
        });
    }
    if head[4] != 2 {
        return Err(format!("wav64: version {} (expected 2)", head[4]));
    }
    let format = match head[5] {
        0 => Wav64Format::Raw,
        1 => Wav64Format::Vadpcm,
        3 => Wav64Format::Opus,
        f => return Err(format!("wav64: unknown format {f}")),
    };
    let channels = head[6];
    if !(1..=2).contains(&channels) {
        return Err(format!("wav64: {channels} channels (1 or 2)"));
    }
    let bits = head[7];
    if !(bits == 16 || bits == 8 && format == Wav64Format::Raw) {
        return Err(format!("wav64: {bits}-bit {format:?}"));
    }
    let [sample_rate, frames, loop_len, start] = [8, 12, 16, 20].map(|at| be_u32(head, at));
    if sample_rate == 0
        || [sample_rate, frames, loop_len, start]
            .iter()
            .any(|&v| v > i32::MAX as u32)
    {
        return Err("wav64: rate, length or offset out of range".into());
    }
    if loop_len > frames {
        return Err(format!("wav64: loop of {loop_len} in {frames} frames"));
    }
    let ch = u64::from(channels);
    let (min_start, need) = match format {
        Wav64Format::Raw => (HEADER_BYTES, u64::from(frames) * ch * u64::from(bits / 8)),
        Wav64Format::Vadpcm => {
            let ext = bytes
                .get(HEADER_BYTES..HEADER_BYTES + VADPCM_EXT_BYTES)
                .ok_or("wav64: truncated VADPCM header")?;
            let (predictors, order) = (usize::from(ext[0]), usize::from(ext[1]));
            if order != ORDER {
                return Err(format!(
                    "wav64: VADPCM order {order} (the RSP decodes order 2)"
                ));
            }
            // The RSP keeps 8 codebook vectors per channel; stereo's second
            // channel sits at the 9th.
            if !(1..=PREDICTORS).contains(&predictors) || channels == 2 && predictors != PREDICTORS
            {
                return Err(format!(
                    "wav64: {predictors} VADPCM predictors with {channels} channels"
                ));
            }
            let book = predictors * order * usize::from(channels) * 16;
            let frames_needed = u64::from(frames).div_ceil(SAMPLES_PER_FRAME as u64);
            (
                HEADER_BYTES + VADPCM_EXT_BYTES + book,
                frames_needed * ch * BYTES_PER_FRAME as u64,
            )
        }
        Wav64Format::Opus => (HEADER_BYTES + OPUS_EXT_BYTES, 0),
    };
    let start = start as usize;
    if start < min_start || start > bytes.len() {
        return Err(format!(
            "wav64: data offset {start} outside {min_start}..={}",
            bytes.len()
        ));
    }
    let data_bytes = bytes.len() - start;
    if (data_bytes as u64) < need {
        return Err(format!("wav64: {data_bytes} data bytes, {need} needed"));
    }
    Ok(Wav64Info {
        format,
        channels,
        bits,
        sample_rate,
        frames,
        loop_len,
        header_bytes: start,
        data_bytes,
    })
}

/// Decode a raw or VADPCM wav64 to interleaved i16 (Opus → Err, "Opus
/// wav64 can't be decoded on the host"). 8-bit raw becomes `s << 8`.
pub fn decode_wav64(bytes: &[u8]) -> Result<(Wav64Info, Vec<i16>), String> {
    let info = parse_wav64(bytes)?;
    let data = &bytes[info.header_bytes..];
    let ch = usize::from(info.channels);
    let n = info.frames as usize * ch;
    let pcm = match info.format {
        Wav64Format::Opus => return Err("Opus wav64 can't be decoded on the host".into()),
        Wav64Format::Raw if info.bits == 8 => {
            data[..n].iter().map(|&b| i16::from(b as i8) << 8).collect()
        }
        Wav64Format::Raw => data[..2 * n]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i16::from_be_bytes(b))
            .collect(),
        Wav64Format::Vadpcm => {
            let predictors = usize::from(bytes[HEADER_BYTES]);
            let book: Vec<[i16; 8]> = bytes[HEADER_BYTES + VADPCM_EXT_BYTES..info.header_bytes]
                .as_chunks::<16>()
                .0
                .iter()
                .map(|v| std::array::from_fn(|i| i16::from_be_bytes([v[2 * i], v[2 * i + 1]])))
                .collect();
            let frames = (info.frames as usize).div_ceil(SAMPLES_PER_FRAME);
            let all = &data[..frames * ch * BYTES_PER_FRAME]
                .as_chunks::<BYTES_PER_FRAME>()
                .0;
            let mut pcm = vec![0; n];
            for c in 0..ch {
                let mut stream = Vec::with_capacity(frames * BYTES_PER_FRAME);
                for (i, f) in all.iter().skip(c).step_by(ch).enumerate() {
                    if usize::from(f[0] & 15) >= predictors {
                        return Err(format!(
                            "wav64: VADPCM frame {i} uses predictor {}",
                            f[0] & 15
                        ));
                    }
                    stream.extend_from_slice(f);
                }
                let v = Vadpcm {
                    order: ORDER,
                    predictors,
                    codebook: book[c * predictors * ORDER..(c + 1) * predictors * ORDER].to_vec(),
                    frames: stream,
                    samples: info.frames as usize,
                };
                for (dst, s) in pcm.iter_mut().skip(c).step_by(ch).zip(decode(&v)) {
                    *dst = s;
                }
            }
            pcm
        }
    };
    Ok((info, pcm))
}

fn clamp16(x: i32) -> i32 {
    x.clamp(i32::from(i16::MIN), i32::from(i16::MAX))
}

/// Sign-extended nibble.
fn nibble(x: u8) -> i32 {
    i32::from((x << 4) as i8 >> 4)
}

/// Decode one vector of 8 samples the RSP's way: prediction from the last
/// `book.len()` history samples, plus each residual `<< 11` at its own
/// sample and through the last vector into the ones after it; `>> 11`,
/// clamped. `residuals` are already scaled.
fn decode_vector(book: &[[i16; 8]], history: &[i16; 8], residuals: &[i32; 8]) -> [i16; 8] {
    let mut acc = [0i32; 8];
    let tail = &history[8 - book.len()..];
    for (coef, &h) in book.iter().zip(tail) {
        for (a, &c) in acc.iter_mut().zip(coef) {
            *a = a.wrapping_add(i32::from(h) * i32::from(c));
        }
    }
    let last = book.last().copied().unwrap_or_default();
    for (k, &r) in residuals.iter().enumerate() {
        acc[k] = acc[k].wrapping_add(r << 11);
        for (a, &c) in acc[k + 1..].iter_mut().zip(&last) {
            *a = a.wrapping_add(r * i32::from(c));
        }
    }
    acc.map(|a| clamp16(a >> 11) as i16)
}

/// Decode (bit-exact to the console runtime for the same codebook/frames).
/// A frame naming a predictor past the codebook predicts from zeros.
pub fn decode(v: &Vadpcm) -> Vec<i16> {
    let order = v.order.min(8);
    let mut out = Vec::with_capacity(v.frames.len() / BYTES_PER_FRAME * SAMPLES_PER_FRAME);
    let mut history = [0i16; 8];
    let zeros = vec![[0i16; 8]; order];
    for f in v.frames.as_chunks::<BYTES_PER_FRAME>().0 {
        let p = usize::from(f[0] & 15);
        let book = match v.codebook.get(p * order..(p + 1) * order) {
            Some(b) if p < v.predictors => b,
            _ => &zeros,
        };
        // The RSP multiplies by `1 << scale` in a 16-bit lane: scale 15
        // wraps to -32768, and the product saturates.
        let scale = i32::from((1u16 << (f[0] >> 4)) as i16);
        for half in f[1..].as_chunks::<4>().0 {
            let mut res = [0i32; 8];
            for (pair, &b) in res.as_chunks_mut::<2>().0.iter_mut().zip(half) {
                *pair = [
                    clamp16(nibble(b >> 4) * scale),
                    clamp16(nibble(b & 15) * scale),
                ];
            }
            history = decode_vector(book, &history, &res);
            out.extend_from_slice(&history);
        }
    }
    out.truncate(v.samples);
    out
}

/// Bytes of a VADPCM wav64 for `frames` mono sample frames as audioconv64
/// writes it (header + codebook + data padded to 32 samples), for size
/// estimates without Docker; exact when it matches the fixture.
pub fn wav64_vadpcm_bytes(frames: usize) -> usize {
    let data = frames.div_ceil(PAD_SAMPLES) * PAD_SAMPLES / SAMPLES_PER_FRAME * BYTES_PER_FRAME;
    HEADER_BYTES + VADPCM_EXT_BYTES + PREDICTORS * ORDER * 16 + data
}

/// A mono VADPCM wav64 of `v` at `rate`, as the pinned libdragon reads it
/// (the layout above): what the N64 bake writes with [`encode`] instead of
/// `audioconv64`'s encoder. `looped` loops the whole sound — libdragon
/// loops VADPCM only from sample 0 and asserts a loop length that is a
/// multiple of 16 (the bake trims loops to that).
pub fn write_wav64(v: &Vadpcm, rate: u32, looped: bool) -> Vec<u8> {
    assert_eq!(v.order, ORDER, "the RSP decodes order 2");
    assert_eq!(v.codebook.len(), v.predictors * v.order, "a mono codebook");
    let book = v.codebook.len() * 16;
    let start = HEADER_BYTES + VADPCM_EXT_BYTES + book;
    let mut b = wav64_header(Wav64Format::Vadpcm, rate, v.samples, looped, start);
    b.push(v.predictors as u8);
    b.push(v.order as u8);
    // Padding, then the loop and play states (zero: decoding starts from
    // silence, and a whole-sound loop restarts from it).
    b.resize(HEADER_BYTES + VADPCM_EXT_BYTES, 0);
    for vector in &v.codebook {
        for c in vector {
            b.extend_from_slice(&c.to_be_bytes());
        }
    }
    b.extend_from_slice(&v.frames);
    // The RSP decodes frame pairs: zero frames up to 32 samples.
    let frames = v.samples.div_ceil(PAD_SAMPLES) * PAD_SAMPLES / SAMPLES_PER_FRAME;
    b.resize(start + frames * BYTES_PER_FRAME, 0);
    b
}

/// A mono 16-bit raw wav64 (`--wav-compress 0`): the header and big-endian
/// samples.
pub fn write_wav64_raw(pcm: &[i16], rate: u32, looped: bool) -> Vec<u8> {
    let mut b = wav64_header(Wav64Format::Raw, rate, pcm.len(), looped, HEADER_BYTES);
    for s in pcm {
        b.extend_from_slice(&s.to_be_bytes());
    }
    b
}

fn wav64_header(
    format: Wav64Format,
    rate: u32,
    frames: usize,
    looped: bool,
    start: usize,
) -> Vec<u8> {
    let mut b = Vec::with_capacity(start);
    b.extend_from_slice(b"WV64");
    b.push(2);
    b.push(match format {
        Wav64Format::Raw => 0,
        Wav64Format::Vadpcm => 1,
        Wav64Format::Opus => 3,
    });
    b.push(1);
    b.push(16);
    let loop_len = if looped { frames } else { 0 };
    for v in [rate as usize, frames, loop_len, start] {
        b.extend_from_slice(&(v as u32).to_be_bytes());
    }
    b
}

/// Signal-to-noise ratio of `decoded` against `reference` in dB (missing
/// samples count as silence; identical → infinity).
pub fn snr_db(reference: &[i16], decoded: &[i16]) -> f64 {
    let (mut signal, mut noise) = (0f64, 0f64);
    for (i, &r) in reference.iter().enumerate() {
        let r = f64::from(r);
        let d = r - f64::from(decoded.get(i).copied().unwrap_or(0));
        signal += r * r;
        noise += d * d;
    }
    if noise == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (signal / noise).log10()
    }
}

// ---------------------------------------------------------------- encoder

/// Second-order statistics of a frame for the predictor `x[n] ≈ a0 x[n-1] +
/// a1 x[n-2]`: its squared error is `e0 - 2 a·c + aᵀ R a`.
#[derive(Clone, Copy, Default)]
struct Stats {
    e0: f64,
    c: [f64; 2],
    r: [f64; 3], // R00, R01, R11
}

impl Stats {
    /// The frame starting at `at` (up to the end of `x`); its first two
    /// samples look back into the previous frame.
    fn of(x: &[i16], at: usize) -> Stats {
        let s = |n: usize, back: usize| n.checked_sub(back).map_or(0, |i| i64::from(x[i]));
        let (mut e0, mut c0, mut c1, mut r00, mut r01, mut r11) =
            (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
        for n in at..(at + SAMPLES_PER_FRAME).min(x.len()) {
            let (x0, x1, x2) = (s(n, 0), s(n, 1), s(n, 2));
            e0 += x0 * x0;
            c0 += x0 * x1;
            c1 += x0 * x2;
            r00 += x1 * x1;
            r01 += x1 * x2;
            r11 += x2 * x2;
        }
        Stats {
            e0: e0 as f64,
            c: [c0 as f64, c1 as f64],
            r: [r00 as f64, r01 as f64, r11 as f64],
        }
    }

    fn add(&mut self, o: &Stats) {
        self.e0 += o.e0;
        self.c = [self.c[0] + o.c[0], self.c[1] + o.c[1]];
        self.r = [self.r[0] + o.r[0], self.r[1] + o.r[1], self.r[2] + o.r[2]];
    }

    fn error(&self, a: [f64; 2]) -> f64 {
        let [r00, r01, r11] = self.r;
        self.e0 - 2.0 * (a[0] * self.c[0] + a[1] * self.c[1])
            + a[0] * a[0] * r00
            + 2.0 * a[0] * a[1] * r01
            + a[1] * a[1] * r11
    }

    /// Least-squares predictor (lightly regularised), kept stable.
    fn solve(&self) -> [f64; 2] {
        let ridge = 1e-6 * (self.r[0] + self.r[2]) + 1e-3;
        let (r00, r01, r11) = (self.r[0] + ridge, self.r[1], self.r[2] + ridge);
        let det = r00 * r11 - r01 * r01;
        if det <= 0.0 {
            return [0.0; 2];
        }
        stable([
            (self.c[0] * r11 - self.c[1] * r01) / det,
            (self.c[1] * r00 - self.c[0] * r01) / det,
        ])
    }
}

/// Clamp into the AR(2) stability triangle (|a1| < 1, |a0| < 1 - a1), a
/// little inside, which also keeps the Q11 codebook within `i16`.
fn stable(a: [f64; 2]) -> [f64; 2] {
    const EDGE: f64 = 0.998;
    let a1 = a[1].clamp(-EDGE, EDGE);
    let lim = EDGE * (1.0 - a1);
    [a[0].clamp(-lim, lim), a1]
}

/// `k` predictors minimising the summed prediction error: seeded one at a
/// time with the frame that gains most from its own predictor, then Lloyd
/// iterations.
fn design(stats: &[Stats], k: usize) -> Vec<[f64; 2]> {
    let mut total = Stats::default();
    stats.iter().for_each(|s| total.add(s));
    let own: Vec<[f64; 2]> = stats.iter().map(Stats::solve).collect();
    let best = |s: &Stats, centers: &[[f64; 2]]| {
        let mut b = (0, f64::INFINITY);
        for (i, &c) in centers.iter().enumerate() {
            let e = s.error(c);
            if e < b.1 {
                b = (i, e);
            }
        }
        b
    };
    let mut centers = vec![total.solve()];
    while centers.len() < k {
        let mut pick = (f64::NEG_INFINITY, [0.0; 2]);
        for (s, &a) in stats.iter().zip(&own) {
            let gain = best(s, &centers).1 - s.error(a);
            if gain > pick.0 {
                pick = (gain, a);
            }
        }
        centers.push(pick.1);
    }
    let mut assign = vec![usize::MAX; stats.len()];
    for _ in 0..24 {
        let mut changed = false;
        let mut sums = vec![Stats::default(); k];
        for (s, slot) in stats.iter().zip(assign.iter_mut()) {
            let c = best(s, &centers).0;
            changed |= *slot != c;
            *slot = c;
            sums[c].add(s);
        }
        if !changed {
            break;
        }
        for (c, sum) in centers.iter_mut().zip(&sums) {
            if sum.e0 > 0.0 {
                *c = sum.solve();
            }
        }
    }
    centers
}

/// The codebook vectors of `x[n] = a0 x[n-1] + a1 x[n-2]`: the response
/// of the next 8 samples to `x[-2] = 1` and to `x[-1] = 1`, Q11.
fn expand(a: [f64; 2]) -> [[i16; 8]; 2] {
    let mut out = [[0i16; 8]; 2];
    for (vector, start) in out.iter_mut().zip([[1.0, 0.0], [0.0, 1.0]]) {
        let [mut x2, mut x1] = start;
        for v in vector.iter_mut() {
            let x = a[0] * x1 + a[1] * x2;
            *v = (x * 2048.0).round().clamp(-32768.0, 32767.0) as i16;
            (x2, x1) = (x1, x);
        }
    }
    out
}

/// One closed-loop try of a frame with a predictor and scale: each residual
/// is the nearest the decoder can reach given the ones before it; samples
/// from `valid` on are padding (residual 0, no error). `None` once the
/// error reaches `bound`.
fn trial(
    x: &[i16; 16],
    valid: usize,
    history: &[i16; 8],
    book: &[[i16; 8]],
    scale: u32,
    bound: u64,
) -> Option<(u64, [u8; 16], [i16; 8])> {
    let mut history = *history;
    let mut nibbles = [0u8; 16];
    let mut err = 0u64;
    let step = 2048i64 << scale;
    for (half, (xs, ns)) in x
        .as_chunks::<8>()
        .0
        .iter()
        .zip(nibbles.as_chunks_mut::<8>().0)
        .enumerate()
    {
        let mut acc = [0i32; 8];
        for (coef, &h) in book.iter().zip(&history[8 - book.len()..]) {
            for (a, &c) in acc.iter_mut().zip(coef) {
                *a = a.wrapping_add(i32::from(h) * i32::from(c));
            }
        }
        let last = book[book.len() - 1];
        let mut out = [0i16; 8];
        for k in 0..8 {
            let target = i64::from(xs[k]);
            // Aim at the middle of the output's step (the decoder floors);
            // a tie goes down, the side that lands on it.
            let q = if half * 8 + k < valid {
                ((target << 11) + 1024 - i64::from(acc[k]) + step / 2 - 1)
                    .div_euclid(step)
                    .clamp(-8, 7) as i32
            } else {
                0
            };
            let r = q << scale;
            acc[k] = acc[k].wrapping_add(r << 11);
            for (a, &c) in acc[k + 1..].iter_mut().zip(&last) {
                *a = a.wrapping_add(r * i32::from(c));
            }
            let o = clamp16(acc[k] >> 11) as i16;
            if half * 8 + k < valid {
                let d = (target - i64::from(o)).unsigned_abs();
                err += d * d;
            }
            out[k] = o;
            ns[k] = (q & 15) as u8;
        }
        if err >= bound {
            return None;
        }
        history = out;
    }
    Some((err, nibbles, history))
}

/// Smallest scale whose residual range covers the open-loop residuals of
/// `x` under predictor `book` (history from the decoder).
fn open_loop_scale(x: &[i16], history: &[i16; 8], book: &[[i16; 8]]) -> u32 {
    let (a0, a1) = (i64::from(book[1][0]), i64::from(book[0][0]));
    let (mut p2, mut p1) = (i64::from(history[6]), i64::from(history[7]));
    let (mut lo, mut hi) = (0i64, 0i64);
    for &s in x {
        let s = i64::from(s);
        let e = s - ((a0 * p1 + a1 * p2 + 1024) >> 11);
        lo = lo.min(e);
        hi = hi.max(e);
        (p2, p1) = (p1, s);
    }
    let mut scale = 0;
    while scale < MAX_SCALE && (lo < -8 << scale || hi > 7 << scale) {
        scale += 1;
    }
    scale
}

/// Encode mono i16 PCM the way audioconv64 does in spirit (order 2, same
/// predictor count as the pinned audioconv64's default), deterministic.
/// The last frame is padded to 16 samples; `samples` keeps the real length.
pub fn encode(pcm: &[i16]) -> Vadpcm {
    let frames = pcm.len().div_ceil(SAMPLES_PER_FRAME);
    let mut x = pcm.to_vec();
    x.resize(frames * SAMPLES_PER_FRAME, 0);
    let stats: Vec<Stats> = (0..frames)
        .map(|f| Stats::of(pcm, f * SAMPLES_PER_FRAME))
        .collect();
    let codebook: Vec<[i16; 8]> = design(&stats, PREDICTORS)
        .into_iter()
        .flat_map(expand)
        .collect();
    let mut out = Vec::with_capacity(frames * BYTES_PER_FRAME);
    let mut history = [0i16; 8];
    for (f, block) in x.as_chunks::<SAMPLES_PER_FRAME>().0.iter().enumerate() {
        let valid = (pcm.len() - f * SAMPLES_PER_FRAME).min(SAMPLES_PER_FRAME);
        let mut best = (u64::MAX, 0u8, [0u8; 16], [0i16; 8]);
        for (p, book) in codebook.as_chunks::<ORDER>().0.iter().enumerate() {
            let s0 = open_loop_scale(&block[..valid], &history, book);
            for scale in s0.saturating_sub(1)..=(s0 + 1).min(MAX_SCALE) {
                if let Some((err, nibbles, h)) = trial(block, valid, &history, book, scale, best.0)
                {
                    best = (err, (scale as u8) << 4 | p as u8, nibbles, h);
                }
            }
        }
        let (_, control, nibbles, h) = best;
        out.push(control);
        out.extend(nibbles.as_chunks::<2>().0.iter().map(|&[a, b]| a << 4 | b));
        history = h;
    }
    Vadpcm {
        order: ORDER,
        predictors: PREDICTORS,
        codebook,
        frames: out,
        samples: pcm.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/audio/");
    const RATE: f64 = 22050.0;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{DIR}{name}")).unwrap()
    }

    /// Samples of a plain 16-bit PCM RIFF WAV.
    fn wav_samples(b: &[u8]) -> (u32, u16, Vec<i16>) {
        assert_eq!((&b[..4], &b[8..12]), (&b"RIFF"[..], &b"WAVE"[..]));
        let (mut at, mut fmt, mut data) = (12, None, Vec::new());
        while at + 8 <= b.len() {
            let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &b[at + 8..at + 8 + len];
            match &b[at..at + 4] {
                b"fmt " => {
                    let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                    assert_eq!((u16_at(0), u16_at(14)), (1, 16), "16-bit PCM");
                    fmt = Some((
                        u32::from_le_bytes(body[4..8].try_into().unwrap()),
                        u16_at(2),
                    ));
                }
                b"data" => {
                    data = body
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&s| i16::from_le_bytes(s))
                        .collect()
                }
                _ => {}
            }
            at += 8 + len + (len & 1);
        }
        let (rate, channels) = fmt.unwrap();
        (rate, channels, data)
    }

    fn fnv1a(samples: &[i16]) -> u64 {
        samples
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .fold(0xcbf2_9ce4_8422_2325, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            })
    }

    fn tone() -> Vec<i16> {
        (0..22050)
            .map(|i| (12000.0 * (std::f64::consts::TAU * 440.0 * f64::from(i) / RATE).sin()) as i16)
            .collect()
    }

    /// 50 Hz → 8 kHz linear sweep over a second.
    fn chirp() -> Vec<i16> {
        (0..22050)
            .map(|i| {
                let t = f64::from(i) / RATE;
                (16000.0 * (std::f64::consts::TAU * (50.0 * t + 3975.0 * t * t)).sin()) as i16
            })
            .collect()
    }

    /// Karplus–Strong: a burst of LCG noise, lowpassed round a 100-sample loop.
    fn pluck() -> Vec<i16> {
        let mut seed = 0x1234_5678u32;
        let mut line: Vec<f64> = (0..100)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f64::from((seed >> 16) as i16) * 0.6
            })
            .collect();
        let mut out = Vec::with_capacity(22050);
        for i in 0..22050 {
            let j = i % 100;
            let next = line[(j + 1) % 100];
            out.push(line[j] as i16);
            line[j] = 0.996 * 0.5 * (line[j] + next);
        }
        out
    }

    fn square() -> Vec<i16> {
        (0..22050)
            .map(|i| if i / 50 % 2 == 0 { 12000 } else { -12000 })
            .collect()
    }

    #[test]
    fn parses_the_fixtures() {
        let (rate, channels, src) = wav_samples(&fixture("n64_tone.wav"));
        assert_eq!((rate, channels, src.len()), (22050, 1, 8017));
        for (name, format) in [
            ("n64_tone_vadpcm.wav64", Wav64Format::Vadpcm),
            ("n64_tone_raw.wav64", Wav64Format::Raw),
        ] {
            let bytes = fixture(name);
            let info = parse_wav64(&bytes).unwrap();
            // The header keeps the WAV's length; only VADPCM data is padded
            // (to 8032 samples, 502 frames).
            assert_eq!(
                (
                    info.format,
                    info.channels,
                    info.bits,
                    info.sample_rate,
                    info.frames,
                    info.loop_len
                ),
                (format, 1, 16, 22050, 8017, 0)
            );
            assert_eq!(info.header_bytes + info.data_bytes, bytes.len());
            match format {
                Wav64Format::Vadpcm => {
                    assert_eq!((info.header_bytes, info.data_bytes), (224, 502 * 9))
                }
                _ => assert_eq!((info.header_bytes, info.data_bytes), (24, 8017 * 2)),
            }
        }
    }

    #[test]
    fn raw_decodes_to_the_source() {
        let (_, _, src) = wav_samples(&fixture("n64_tone.wav"));
        let (info, pcm) = decode_wav64(&fixture("n64_tone_raw.wav64")).unwrap();
        assert_eq!(info.format, Wav64Format::Raw);
        assert_eq!(pcm, src);
    }

    /// The hash is of libdragon's C reference decoder (`vadpcm_decode` in
    /// `wav64_vadpcm.c`, `VADPCM_REFERENCE_DECODER`) run on the fixture; it
    /// also agrees with `audioconv64`'s `vadpcm/decode.c` and with an
    /// instruction-level model of the RSP ucode.
    #[test]
    fn vadpcm_decodes_like_libdragon() {
        let (_, _, src) = wav_samples(&fixture("n64_tone.wav"));
        let (info, pcm) = decode_wav64(&fixture("n64_tone_vadpcm.wav64")).unwrap();
        assert_eq!(info.format, Wav64Format::Vadpcm);
        assert_eq!(pcm.len(), 8017);
        assert_eq!((pcm[0], pcm[8016]), (0, -945));
        assert_eq!(fnv1a(&pcm), 0x03b6_14dc_a687_a1f2);
        let snr = snr_db(&src, &pcm);
        println!("audioconv64 VADPCM: {snr:.2} dB");
        assert!(snr >= 25.0, "{snr}");
    }

    #[test]
    fn encoder_quality() {
        let (_, _, fixture_src) = wav_samples(&fixture("n64_tone.wav"));
        for (name, pcm) in [
            ("tone", tone()),
            ("chirp", chirp()),
            ("pluck", pluck()),
            ("square", square()),
            ("fixture", fixture_src),
        ] {
            let v = encode(&pcm);
            assert_eq!((v.order, v.predictors, v.codebook.len()), (2, 4, 8));
            let snr = snr_db(&pcm, &decode(&v));
            println!("{name}: {snr:.2} dB");
            assert!(snr >= 25.0, "{name}: {snr}");
        }
    }

    #[test]
    fn encoder_edges() {
        let v = encode(&[0; 1000]);
        assert!(decode(&v).iter().all(|&s| s == 0));
        assert!(v.frames.iter().all(|&b| b == 0));
        let src = tone();
        for n in [0, 1, 15, 16, 17] {
            let v = encode(&src[..n]);
            assert_eq!(
                (v.samples, v.frames.len()),
                (n, n.div_ceil(16) * BYTES_PER_FRAME)
            );
            let pcm = decode(&v);
            assert_eq!(pcm.len(), n);
            assert!(
                pcm.iter()
                    .zip(&src)
                    .all(|(&a, &b)| (i32::from(a) - i32::from(b)).abs() < 512),
                "{n}: {pcm:?}"
            );
        }
        let full = encode(&src);
        assert_eq!(full, encode(&src));
        assert_eq!(full.frames.len(), 22050usize.div_ceil(16) * 9);
        // Clipping-level input stays in range and decodes sanely.
        let loud: Vec<i16> = (0..4000)
            .map(|i| if i % 7 < 3 { i16::MAX } else { i16::MIN })
            .collect();
        assert!(snr_db(&loud, &decode(&encode(&loud))) > 10.0);
    }

    #[test]
    fn decode_follows_the_rsp_past_scale_12() {
        // `audioconv64` stops at scale 12; past it the RSP's 16-bit residual
        // lane saturates (13, 14) and the multiplier wraps (15), where C's
        // `r << scale` would not.
        let v = Vadpcm {
            order: 2,
            predictors: 1,
            codebook: vec![[0; 8], [1024, 0, 0, 0, 0, 0, 0, 0]],
            frames: vec![
                0xd0, 0x70, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x1f, 0, 0, 0, 0, 0, 0, 0,
            ],
            samples: 32,
        };
        let pcm = decode(&v);
        // 7 << 13 saturates to 32767, half of which spreads on (C: 28672).
        assert_eq!(&pcm[..3], &[32767, 16383, 0]);
        // Scale 15: +1 → -32768, -1 → +32767 (C: 32767, -16384).
        assert_eq!(&pcm[16..18], &[-32768, 16383]);
        // A predictor past the codebook predicts from zeros, never panics.
        let bad = Vadpcm {
            frames: vec![0x05, 0x10, 0, 0, 0, 0, 0, 0, 0],
            samples: 16,
            ..v
        };
        assert_eq!(decode(&bad)[..2], [1, 0]);
    }

    /// Our wav64s read back through the parser and decode to what the
    /// encoder produced; the sizes are audioconv64's.
    #[test]
    fn written_wav64s_round_trip() {
        let (rate, _, src) = wav_samples(&fixture("n64_tone.wav"));
        let v = encode(&src);
        let bytes = write_wav64(&v, rate, false);
        assert_eq!(bytes.len(), wav64_vadpcm_bytes(src.len()));
        let (info, pcm) = decode_wav64(&bytes).unwrap();
        assert_eq!(info.format, Wav64Format::Vadpcm);
        assert_eq!(
            (info.sample_rate, info.frames, info.loop_len),
            (rate, 8017, 0)
        );
        assert_eq!((info.channels, info.header_bytes), (1, 224));
        assert_eq!(pcm, decode(&v));
        // The header and codebook match audioconv64's layout byte for byte
        // where they don't depend on the encoder.
        let theirs = fixture("n64_tone_vadpcm.wav64");
        assert_eq!(bytes[..12], theirs[..12]);
        assert_eq!(bytes[16..26], theirs[16..26]);
        let looped = write_wav64(&encode(&src[..8000]), rate, true);
        assert_eq!(parse_wav64(&looped).unwrap().loop_len, 8000);

        let raw = write_wav64_raw(&src, rate, true);
        let (info, pcm) = decode_wav64(&raw).unwrap();
        assert_eq!((info.format, info.loop_len), (Wav64Format::Raw, 8017));
        assert_eq!(pcm, src);
        // audioconv64's raw file for the same tone: identical but the loop.
        let mut theirs = fixture("n64_tone_raw.wav64");
        theirs[16..20].copy_from_slice(&8017u32.to_be_bytes());
        assert_eq!(raw, theirs);
        // Empty input still makes a valid (silent) file.
        let silent = decode_wav64(&write_wav64(&encode(&[]), 8000, false)).unwrap();
        assert!(silent.1.is_empty());
    }

    #[test]
    fn vadpcm_size_matches_audioconv64() {
        let bytes = fixture("n64_tone_vadpcm.wav64");
        assert_eq!(wav64_vadpcm_bytes(8017), bytes.len());
        assert_eq!(wav64_vadpcm_bytes(32), 224 + 18);
        assert_eq!(wav64_vadpcm_bytes(33), 224 + 36);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_wav64(&[]).is_err());
        assert!(parse_wav64(b"WV64").is_err());
        assert!(
            parse_wav64(&fixture("n64_tone.wav"))
                .unwrap_err()
                .contains("RIFF")
        );
        let mut seed = 7u32;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 16) as u8
            })
            .collect();
        assert!(decode_wav64(&noise).is_err());
        for name in ["n64_tone_vadpcm.wav64", "n64_tone_raw.wav64"] {
            let bytes = fixture(name);
            // Every truncation fails cleanly.
            for len in 0..bytes.len() {
                assert!(decode_wav64(&bytes[..len]).is_err(), "{name} cut at {len}");
            }
            // Every header byte corrupted: an error or a decode, never a panic.
            for at in 0..240.min(bytes.len()) {
                for v in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                    let mut b = bytes.clone();
                    b[at] = v;
                    let _ = decode_wav64(&b);
                }
            }
        }
        // A frame naming a predictor the codebook lacks.
        let mut b = fixture("n64_tone_vadpcm.wav64");
        b[224] = 0x0c;
        assert!(decode_wav64(&b).unwrap_err().contains("predictor"));
        // Opus is parsed but not decoded.
        let mut opus = b[..36].to_vec();
        opus[5] = 3;
        opus[20..24].copy_from_slice(&36u32.to_be_bytes());
        assert_eq!(parse_wav64(&opus).unwrap().format, Wav64Format::Opus);
        assert_eq!(
            decode_wav64(&opus).unwrap_err(),
            "Opus wav64 can't be decoded on the host"
        );
    }
}
