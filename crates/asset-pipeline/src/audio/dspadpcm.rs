//! DSP-ADPCM (the 3DS DSP's native ADPCM, also GameCube/Wii): 8-byte
//! frames of 14 mono samples, 4.57 bits per sample.
//!
//! - **Frame**: byte 0 is the header — high nibble the predictor index
//!   (the DSP reads 3 bits: 0..=7), low nibble the scale shift (0..=15; the
//!   encoder uses 0..=12); bytes 1..8 hold 14 signed nibbles, high first.
//! - **Predictors**: 8 pairs `(c1, c2)` in Q11, `coefs[2p]` and
//!   `coefs[2p + 1]`, one table per sound (`ndspChnSetAdpcmCoefs`).
//! - **Decoding** (bit-exact with the DSP): `s = ((n << shift) << 11) + 1024
//!   + c1·h1 + c2·h2`, sample = `clamp_i16(s >> 11)`, then `h2 = h1`,
//!   `h1 = sample`. The history runs across frames, so a wave buffer that
//!   starts mid-stream (a streaming chunk, a loop restart) needs the
//!   [`Context`] of its first frame.
//!
//! The encoder designs the predictors from the signal: a generalized Lloyd
//! (k-means) over the frames' second-order statistics, clustering frames by
//! the prediction error each predictor would leave, with entry 0 pinned to
//! `(0, 0)` for transients. Each frame then tries every predictor × shift
//! (0..=12) against an exact simulation of the decoder (clamping and running
//! history included) and keeps the lowest squared error; nibbles are chosen
//! against the *decoded* history, so error never accumulates. Deterministic.

/// Samples per frame.
pub const SAMPLES_PER_FRAME: usize = 14;
/// Bytes per frame: the header and 14 nibbles.
pub const BYTES_PER_FRAME: usize = 8;

/// Predictor pairs per sound.
const PREDICTORS: usize = 8;
/// Largest shift the encoder uses: 8 · 2¹² already spans the i16 range.
const MAX_SHIFT: u32 = 12;
/// 1.0 in Q11.
const ONE: f64 = 2048.0;
/// Lloyd passes after each new predictor, and once the table is full.
const PASSES: usize = 6;
const FINAL_PASSES: usize = 24;

/// Decoder state at a frame boundary: what `ndspAdpcmData` takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Context {
    /// Header byte of the frame (`ndspAdpcmData::index`).
    pub header: u8,
    /// Last decoded sample (`history0`).
    pub hist1: i16,
    /// The one before it (`history1`).
    pub hist2: i16,
}

/// A DSP-ADPCM sound.
#[derive(Clone, PartialEq, Debug)]
pub struct Encoded {
    /// Predictor pairs `(c1, c2)` in Q11, flattened.
    pub coefs: [i16; 16],
    /// Whole frames; the last one padded with silence.
    pub data: Vec<u8>,
    /// Real sample count (≤ frames × 14).
    pub samples: usize,
}

impl Encoded {
    fn frames(&self) -> usize {
        self.data.len() / BYTES_PER_FRAME
    }
}

/// Bytes of DSP-ADPCM data for `samples` samples (whole frames).
pub fn data_bytes(samples: usize) -> usize {
    samples.div_ceil(SAMPLES_PER_FRAME) * BYTES_PER_FRAME
}

/// Encode 16-bit mono PCM.
pub fn encode(pcm: &[i16]) -> Encoded {
    encode_with_output(pcm).0
}

/// Decode `samples` samples from `data` with `coefs`, starting from silence (hist = 0).
///
/// Panics if `data` is shorter than [`data_bytes`]`(samples)`.
pub fn decode(coefs: &[i16; 16], data: &[u8], samples: usize) -> Vec<i16> {
    decode_from(coefs, data, samples, Context::default())
}

/// Decode starting from `ctx` (history only; the header comes from each
/// frame) — what the DSP does for a wave buffer that starts mid-stream.
///
/// Panics if `data` is shorter than [`data_bytes`]`(samples)`.
pub fn decode_from(coefs: &[i16; 16], data: &[u8], samples: usize, ctx: Context) -> Vec<i16> {
    assert!(
        data.len() >= data_bytes(samples),
        "DSP-ADPCM: {} bytes cannot hold {samples} samples",
        data.len()
    );
    let mut out = Vec::with_capacity(samples);
    // i64: the DSP's accumulator is wider than 32 bits, so extreme
    // coefficients must not wrap here either.
    let (mut h1, mut h2) = (i64::from(ctx.hist1), i64::from(ctx.hist2));
    let (frames, _) = data.as_chunks::<BYTES_PER_FRAME>();
    for (f, frame) in frames.iter().enumerate() {
        let len = (samples - f * SAMPLES_PER_FRAME).min(SAMPLES_PER_FRAME);
        let p = usize::from((frame[0] >> 4) & 7);
        let shift = u32::from(frame[0] & 15);
        let (c1, c2) = (i64::from(coefs[2 * p]), i64::from(coefs[2 * p + 1]));
        for i in 0..len {
            let byte = frame[1 + i / 2];
            let n = i64::from(sign_extend(if i % 2 == 0 { byte >> 4 } else { byte }));
            let s = ((((n << shift) << 11) + 1024 + c1 * h1 + c2 * h2) >> 11).clamp(-32768, 32767);
            (h2, h1) = (h1, s);
            out.push(s as i16);
        }
        if out.len() == samples {
            break;
        }
    }
    out
}

/// Decoder context at the start of frame `frame` (header = that frame's
/// first byte, histories = the last two decoded samples before it; zeros at
/// frame 0). At `frame` == frame count the header is 0 and the histories
/// are the decoder's state after the whole data, padding included.
///
/// Panics if `frame` is past the frame count.
pub fn context_at(enc: &Encoded, frame: usize) -> Context {
    assert!(
        frame <= enc.frames(),
        "DSP-ADPCM: frame {frame} past the end ({} frames)",
        enc.frames()
    );
    let out = decode(&enc.coefs, &enc.data, frame * SAMPLES_PER_FRAME);
    context_in(enc, &out, frame)
}

/// Contexts at every `every`-th frame (0, every, 2·every, …): the per-chunk
/// table a streaming player hands the DSP.
///
/// Panics if `every` is 0.
pub fn contexts(enc: &Encoded, every: usize) -> Vec<Context> {
    assert!(every > 0, "DSP-ADPCM: contexts every 0 frames");
    let out = decode(&enc.coefs, &enc.data, enc.frames() * SAMPLES_PER_FRAME);
    (0..enc.frames())
        .step_by(every)
        .map(|f| context_in(enc, &out, f))
        .collect()
}

/// Signal-to-noise ratio in dB of `decoded` against `reference` (same
/// length; equal → `f64::INFINITY`).
///
/// Panics if the lengths differ.
pub fn snr_db(reference: &[i16], decoded: &[i16]) -> f64 {
    assert_eq!(reference.len(), decoded.len(), "snr_db: length mismatch");
    let (mut signal, mut noise) = (0.0, 0.0);
    for (&r, &d) in reference.iter().zip(decoded) {
        let (r, e) = (f64::from(r), f64::from(i32::from(r) - i32::from(d)));
        signal += r * r;
        noise += e * e;
    }
    if noise == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (signal / noise).log10()
    }
}

/// [`Context`] at frame `f`, given `out` decoded from frame 0 through at
/// least `f`.
fn context_in(enc: &Encoded, out: &[i16], f: usize) -> Context {
    let end = f * SAMPLES_PER_FRAME;
    Context {
        header: enc.data.get(f * BYTES_PER_FRAME).copied().unwrap_or(0),
        hist1: if end >= 1 { out[end - 1] } else { 0 },
        hist2: if end >= 2 { out[end - 2] } else { 0 },
    }
}

/// A 4-bit two's-complement nibble (the low 4 bits of `nib`) as −8..=7.
fn sign_extend(nib: u8) -> i8 {
    ((nib << 4) as i8) >> 4
}

/// The frames of `pcm` with their real lengths, the last one zero-padded.
fn frames(pcm: &[i16]) -> impl Iterator<Item = ([i16; SAMPLES_PER_FRAME], usize)> + '_ {
    let (whole, rest) = pcm.as_chunks::<SAMPLES_PER_FRAME>();
    let last = (!rest.is_empty()).then(|| {
        let mut frame = [0; SAMPLES_PER_FRAME];
        frame[..rest.len()].copy_from_slice(rest);
        (frame, rest.len())
    });
    whole.iter().map(|f| (*f, SAMPLES_PER_FRAME)).chain(last)
}

/// [`encode`], plus the samples the decoder will produce.
fn encode_with_output(pcm: &[i16]) -> (Encoded, Vec<i16>) {
    let table = design(pcm);
    let coefs = std::array::from_fn(|i| table[i / 2][i % 2]);
    let table = table.map(|pair| pair.map(i32::from));
    let mut data = Vec::with_capacity(data_bytes(pcm.len()));
    let mut out = Vec::with_capacity(pcm.len());
    let mut hist = (0, 0);
    for (target, real) in frames(pcm) {
        let (header, nibbles, decoded) = encode_frame(&table, &target, real, hist);
        data.push(header);
        let (pairs, _) = nibbles.as_chunks::<2>();
        data.extend(pairs.iter().map(|&[hi, lo]| (hi << 4) | lo));
        out.extend_from_slice(&decoded[..real]);
        hist = (i32::from(decoded[13]), i32::from(decoded[12]));
    }
    let enc = Encoded {
        coefs,
        data,
        samples: pcm.len(),
    };
    (enc, out)
}

/// The best header, nibbles (low 4 bits) and decoded samples for one
/// frame: every predictor × shift, lowest squared error over the `real`
/// samples (the padding follows along, quantized towards 0).
fn encode_frame(
    table: &[[i32; 2]; PREDICTORS],
    target: &[i16; SAMPLES_PER_FRAME],
    real: usize,
    hist: (i32, i32),
) -> (u8, [u8; SAMPLES_PER_FRAME], [i16; SAMPLES_PER_FRAME]) {
    let mut best = (u64::MAX, 0, 0);
    for (p, &pair) in table.iter().enumerate() {
        for shift in 0..=MAX_SHIFT {
            let mut err = 0;
            let mut beaten = false;
            simulate(pair, shift, target, hist, |i, t, y, _| {
                if i < real {
                    let e = i64::from(t - y);
                    err += e.unsigned_abs() * e.unsigned_abs();
                    beaten = err >= best.0;
                }
                !beaten
            });
            if !beaten {
                best = (err, p, shift);
            }
        }
    }
    let (_, p, shift) = best;
    let mut nibbles = [0; SAMPLES_PER_FRAME];
    let mut decoded = [0; SAMPLES_PER_FRAME];
    simulate(table[p], shift, target, hist, |i, _, y, n| {
        nibbles[i] = n as u8 & 15;
        decoded[i] = y as i16;
        true
    });
    (((p as u8) << 4) | shift as u8, nibbles, decoded)
}

/// Runs the decoder over a frame with predictor `(c1, c2)` and `shift`,
/// choosing each nibble that lands closest to its target; calls
/// `step(i, target, decoded, nibble)` per sample and stops when it returns
/// false. Same arithmetic as [`decode_from`] (i32 suffices: the encoder's
/// coefficients stay within ±4096).
fn simulate(
    [c1, c2]: [i32; 2],
    shift: u32,
    target: &[i16; SAMPLES_PER_FRAME],
    (mut h1, mut h2): (i32, i32),
    mut step: impl FnMut(usize, i32, i32, i32) -> bool,
) {
    for (i, &t) in target.iter().enumerate() {
        let t = i32::from(t);
        // s >> 11 == (n << shift) + (prediction >> 11): the nibble's term is
        // a multiple of 2048.
        let pred = (1024 + c1 * h1 + c2 * h2) >> 11;
        // The decoded value is monotonic in the nibble, so the closest is
        // floor((t − pred) / step) or the one above.
        let lo = ((t - pred) >> shift).clamp(-8, 7);
        let hi = (lo + 1).min(7);
        let y_lo = ((lo << shift) + pred).clamp(-32768, 32767);
        let y_hi = ((hi << shift) + pred).clamp(-32768, 32767);
        let (n, y) = if t - y_lo <= y_hi - t {
            (lo, y_lo)
        } else {
            (hi, y_hi)
        };
        if !step(i, t, y, n) {
            return;
        }
        (h2, h1) = (h1, y);
    }
}

/// Second-order statistics of a frame: Σ x[n−i]·x[n−j] over its real
/// samples for (i, j) = 00, 01, 02, 11, 12, 22 — enough for the squared
/// prediction error of any predictor ([`residual`]).
type Cov = [f64; 6];

/// Squared prediction error over the frames summed in `r` with predictor
/// `(c1, c2)`.
fn residual(r: &Cov, [c1, c2]: [f64; 2]) -> f64 {
    r[0] - 2.0 * (c1 * r[1] + c2 * r[2]) + c1 * c1 * r[3] + 2.0 * c1 * c2 * r[4] + c2 * c2 * r[5]
}

fn to_f64(pair: [i16; 2]) -> [f64; 2] {
    pair.map(|c| f64::from(c) / ONE)
}

/// The predictor minimizing [`residual`] (lightly ridge-regularized),
/// quantized to Q11 strictly inside the stability triangle
/// (|c2| < 1, |c1| < 1 − c2).
fn optimal(r: &Cov) -> [i16; 2] {
    let ridge = 1e-6 * (r[3] + r[5]) + 1e-3;
    let (a, b, d) = (r[3] + ridge, r[4], r[5] + ridge);
    let det = a * d - b * b;
    let (c1, c2) = if det > 0.0 {
        ((r[1] * d - r[2] * b) / det, (r[2] * a - r[1] * b) / det)
    } else {
        (0.0, 0.0)
    };
    let c2 = (c2 * ONE).round().clamp(-ONE + 2.0, ONE - 2.0);
    let lim = ONE - 2.0 - c2;
    let c1 = (c1 * ONE).round().clamp(-lim, lim);
    [c1 as i16, c2 as i16]
}

/// The predictor table for `pcm`: entry 0 is `(0, 0)`; the others grow one
/// at a time, each seeded with the own predictor of the frame that would
/// gain the most from it, and are refined by Lloyd passes ([`lloyd`]).
fn design(pcm: &[i16]) -> [[i16; 2]; PREDICTORS] {
    let covs = frame_covs(pcm);
    let own: Vec<([i16; 2], f64)> = covs
        .iter()
        .map(|r| {
            let c = optimal(r);
            (c, residual(r, to_f64(c)))
        })
        .collect();
    let mut table = vec![[0, 0]];
    while table.len() < PREDICTORS {
        let errs: Vec<f64> = covs
            .iter()
            .map(|r| {
                table
                    .iter()
                    .map(|&c| residual(r, to_f64(c)))
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        let Some(seed) = most_gain(&errs, &own) else {
            break;
        };
        table.push(own[seed].0);
        let passes = if table.len() == PREDICTORS {
            FINAL_PASSES
        } else {
            PASSES
        };
        lloyd(&covs, &own, &mut table, passes);
    }
    std::array::from_fn(|k| table.get(k).copied().unwrap_or([0, 0]))
}

/// The frame whose own predictor would cut its error (`errs`, under the
/// current table) the most, if any would.
fn most_gain(errs: &[f64], own: &[([i16; 2], f64)]) -> Option<usize> {
    let gains = errs.iter().zip(own).map(|(e, (_, o))| e - o);
    let (f, gain) = gains.enumerate().fold(
        (0, 0.0),
        |best, (f, g)| if g > best.1 { (f, g) } else { best },
    );
    (gain > 0.0).then_some(f)
}

/// Up to `passes` Lloyd iterations over `table[1..]` (entry 0 stays put):
/// assign each frame to the predictor leaving the least error, move each
/// predictor to the optimum of its frames, and reseed one that lost all
/// its frames ([`most_gain`]). Stops once no frame changes predictor.
fn lloyd(covs: &[Cov], own: &[([i16; 2], f64)], table: &mut [[i16; 2]], passes: usize) {
    let mut assign = vec![usize::MAX; covs.len()];
    let mut errs = vec![0.0; covs.len()];
    for _ in 0..passes {
        let pairs: Vec<[f64; 2]> = table.iter().map(|&c| to_f64(c)).collect();
        let mut sums = vec![([0.0; 6], 0); table.len()];
        let mut changed = false;
        for ((r, k), err) in covs.iter().zip(&mut assign).zip(&mut errs) {
            let mut best = (residual(r, pairs[0]), 0);
            for (j, &pair) in pairs.iter().enumerate().skip(1) {
                let e = residual(r, pair);
                if e < best.0 {
                    best = (e, j);
                }
            }
            changed |= best.1 != *k;
            (*err, *k) = best;
            let (sum, count) = &mut sums[best.1];
            for (s, x) in sum.iter_mut().zip(r) {
                *s += x;
            }
            *count += 1;
        }
        for (pair, (sum, count)) in table.iter_mut().zip(&sums).skip(1) {
            if *count > 0 {
                *pair = optimal(sum);
            } else if let Some(f) = most_gain(&errs, own) {
                *pair = own[f].0;
                errs[f] = own[f].1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// [`Cov`] of every frame that is not silent (silent frames constrain
/// nothing).
fn frame_covs(pcm: &[i16]) -> Vec<Cov> {
    let mut covs = Vec::with_capacity(pcm.len().div_ceil(SAMPLES_PER_FRAME));
    let (mut x1, mut x2) = (0.0, 0.0);
    for (frame, real) in frames(pcm) {
        let mut r = [0.0; 6];
        for &x in &frame[..real] {
            let x0 = f64::from(x);
            for (s, v) in r
                .iter_mut()
                .zip([x0 * x0, x0 * x1, x0 * x2, x1 * x1, x1 * x2, x2 * x2])
            {
                *s += v;
            }
            (x2, x1) = (x1, x0);
        }
        if r.iter().any(|&v| v != 0.0) {
            covs.push(r);
        }
    }
    covs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const RATE: f64 = 32000.0;

    /// −1..1 to 16-bit.
    fn pcm(xs: impl IntoIterator<Item = f64>) -> Vec<i16> {
        let to_i16 = |x: f64| (x * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
        xs.into_iter().map(to_i16).collect()
    }

    /// `len` samples of `f(t)`.
    fn synth(len: usize, f: impl Fn(f64) -> f64) -> Vec<i16> {
        pcm((0..len).map(|i| f(i as f64 / RATE)))
    }

    /// 440 Hz + 1320 Hz, 2 s.
    fn tone() -> Vec<i16> {
        synth(64000, |t| {
            0.45 * (TAU * 440.0 * t).sin() + 0.3 * (TAU * 1320.0 * t).sin()
        })
    }

    /// Linear chirp 50 Hz → 8 kHz over 2 s.
    fn chirp() -> Vec<i16> {
        let (f0, f1, d) = (50.0, 8000.0, 2.0);
        synth(64000, |t| {
            0.6 * (TAU * (f0 * t + (f1 - f0) * t * t / (2.0 * d))).sin()
        })
    }

    /// Deterministic noise in −1..1.
    struct Lcg(u32);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f64::from(self.0 >> 8) / f64::from(1u32 << 23) - 1.0
        }
    }

    /// A decaying noise burst through a one-pole lowpass, 1 s.
    fn pluck() -> Vec<i16> {
        let mut rng = Lcg(1);
        let alpha = 1.0 - (-TAU * 400.0 / RATE).exp();
        let mut y = 0.0;
        let raw: Vec<f64> = (0..32000)
            .map(|i| {
                let x = rng.next() * (-(i as f64) / RATE / 0.15).exp();
                y += alpha * (x - y);
                y
            })
            .collect();
        let peak = raw.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        pcm(raw.iter().map(|v| v / peak * 0.8))
    }

    /// A 25 % pulse arpeggio with a chip's soft output filter, 0.5 s.
    fn chiptune() -> Vec<i16> {
        let notes = [261.63, 329.63, 392.0, 523.25];
        let (mut phase, mut y) = (0.0, 0.0);
        pcm((0..16000).map(|i| {
            phase = (phase + notes[(i / 2000) % 4] / RATE) % 1.0;
            y += 0.5 * (if phase < 0.25 { 0.5 } else { -0.5 } - y);
            y
        }))
    }

    /// Full-scale square that the decoder has to clamp, 0.5 s.
    fn clipping_square() -> Vec<i16> {
        (0..16000)
            .map(|i| {
                if (i / 40) % 2 == 0 {
                    i16::MAX
                } else {
                    i16::MIN
                }
            })
            .collect()
    }

    fn round_trip(pcm: &[i16]) -> (Encoded, Vec<i16>) {
        let enc = encode(pcm);
        let out = decode(&enc.coefs, &enc.data, enc.samples);
        (enc, out)
    }

    #[test]
    fn quality() {
        for (name, pcm, min) in [
            ("tone", tone(), 30.0),
            ("chirp", chirp(), 30.0),
            ("pluck", pluck(), 30.0),
            ("chiptune", chiptune(), 30.0),
            ("clipping square", clipping_square(), 20.0),
        ] {
            let (enc, out) = round_trip(&pcm);
            let snr = snr_db(&pcm, &out);
            eprintln!("{name}: {snr:.1} dB, coefs {:?}", enc.coefs);
            assert!(snr >= min, "{name}: {snr:.1} dB < {min} dB");
        }
        // The rails are reachable (the encoder leans on the clamp).
        let (_, out) = round_trip(&clipping_square());
        assert!(out.contains(&i16::MAX) && out.contains(&i16::MIN));
    }

    #[test]
    fn encoder_simulates_the_decoder() {
        for pcm in [tone(), chirp(), pluck(), chiptune(), clipping_square()] {
            let (enc, sim) = encode_with_output(&pcm);
            assert_eq!(decode(&enc.coefs, &enc.data, enc.samples), sim);
        }
    }

    #[test]
    fn silence_and_lengths() {
        for len in [0, 1, 13, 14, 15, 28, 1000] {
            let (enc, out) = round_trip(&vec![0; len]);
            assert_eq!(out, vec![0; len]);
            assert_eq!(enc.data.len(), data_bytes(len));
            assert_eq!(enc.samples, len);
        }
        for len in [1, 13, 14, 15] {
            let pcm: Vec<i16> = (0..len).map(|i| (i as i16 - 7) * 1500).collect();
            let (enc, out) = round_trip(&pcm);
            assert_eq!(enc.data.len(), data_bytes(len));
            assert_eq!(enc.samples, len);
            assert_eq!(out.len(), len);
            let snr = snr_db(&pcm, &out);
            eprintln!("ramp of {len}: {snr:.1} dB");
            assert!(snr >= 15.0, "ramp of {len}: {snr:.1} dB");
        }
        assert_eq!(
            [0, 1, 13, 14, 15, 28, 29].map(data_bytes),
            [0, 8, 8, 8, 16, 16, 24]
        );
    }

    #[test]
    fn hand_built_frames() {
        let mut coefs = [0; 16];
        // Predictor 1: c1 = 2.0, c2 = −1.0 (extends the line through h2, h1).
        coefs[2] = 4096;
        coefs[3] = -2048;
        // Predictor 5: c1 = 0.75, c2 = 0.125.
        coefs[10] = 1536;
        coefs[11] = 256;
        #[rustfmt::skip]
        let data = [
            // Predictor 1, shift 12; nibbles 7, 0, −8, −8, −8, −1, 0, 1, 0, …
            0x1C, 0x70, 0x88, 0x8F, 0x01, 0x00, 0x00, 0x00,
            // Predictor 5, shift 0; nibbles 3, −2, 0, …
            0x50, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        // Frame 0: s = (n·4096·2048 + 1024 + 4096·h1 − 2048·h2) >> 11
        //            = n·4096 + 2·h1 − h2 + floor(0.5).
        #[rustfmt::skip]
        let frame0: [i16; 14] = [
            28672,  // 7·4096
            32767,  // 0 + 2·28672 − 0 = 57344 → clamped
            4094,   // −32768 + 2·32767 − 28672
            -32768, // −32768 + 2·4094 − 32767 = −57347 → clamped
            -32768, // −32768 − 2·32768 − 4094 → clamped
            -32768, // −4096 − 2·32768 + 32768 = −36864 → clamped
            -32768, // 0 − 2·32768 + 32768
            -28672, // 4096 − 2·32768 + 32768
            -24576, -20480, -16384, -12288, -8192, -4096, // the line, n = 0
        ];
        // Frame 1: s = (n·2048 + 1024 + 1536·h1 + 256·h2) >> 11, floored.
        let frame1: [i16; 3] = [
            -4093, // (6144 + 1024 − 6291456 − 2097152) / 2048 = −4092.5
            -3584, // (−4096 + 1024 − 6286848 − 1048576) / 2048 = −3583.25
            -3200, // (0 + 1024 − 5505024 − 1047808) / 2048 = −3199.125
        ];
        let out = decode(&coefs, &data, 17);
        assert_eq!(out[..14], frame0);
        assert_eq!(out[14..], frame1);
        let ctx = Context {
            header: 0x50,
            hist1: -4096,
            hist2: -8192,
        };
        assert_eq!(decode_from(&coefs, &data[8..], 3, ctx), frame1);
        let enc = Encoded {
            coefs,
            data: data.to_vec(),
            samples: 17,
        };
        assert_eq!(context_at(&enc, 1), ctx);
    }

    #[test]
    fn streaming_matches_one_shot() {
        let pcm = chirp();
        let enc = encode(&pcm);
        let whole = decode(&enc.coefs, &enc.data, enc.samples);
        for every in [1, 7, 512] {
            let ctxs = contexts(&enc, every);
            assert_eq!(ctxs.len(), enc.samples.div_ceil(every * SAMPLES_PER_FRAME));
            let mut out = Vec::new();
            for (k, &ctx) in ctxs.iter().enumerate() {
                let start = k * every * SAMPLES_PER_FRAME;
                let n = (enc.samples - start).min(every * SAMPLES_PER_FRAME);
                assert_eq!(ctx, context_at(&enc, k * every));
                assert_eq!(ctx.header, enc.data[k * every * BYTES_PER_FRAME]);
                out.extend(decode_from(
                    &enc.coefs,
                    &enc.data[k * every * BYTES_PER_FRAME..],
                    n,
                    ctx,
                ));
            }
            assert_eq!(out, whole, "every {every}");
        }
    }

    #[test]
    fn context_at_the_ends() {
        // 320 frames exactly, then 321 with a partial last one.
        for len in [4480, 4486] {
            let pcm = &chirp()[..len];
            let enc = encode(pcm);
            let frames = enc.data.len() / BYTES_PER_FRAME;
            let ctx0 = context_at(&enc, 0);
            assert_eq!(
                ctx0,
                Context {
                    header: enc.data[0],
                    hist1: 0,
                    hist2: 0
                }
            );
            let full = decode(&enc.coefs, &enc.data, frames * SAMPLES_PER_FRAME);
            let end = context_at(&enc, frames);
            assert_eq!(
                end,
                Context {
                    header: 0,
                    hist1: full[full.len() - 1],
                    hist2: full[full.len() - 2]
                }
            );
            if len % SAMPLES_PER_FRAME == 0 {
                let out = decode(&enc.coefs, &enc.data, enc.samples);
                assert_eq!((end.hist1, end.hist2), (out[len - 1], out[len - 2]));
            }
        }
    }

    #[test]
    fn deterministic() {
        for pcm in [tone(), pluck()] {
            assert_eq!(encode(&pcm), encode(&pcm));
        }
    }

    #[test]
    fn snr_basics() {
        assert_eq!(snr_db(&[1, 2, 3], &[1, 2, 3]), f64::INFINITY);
        let snr = snr_db(&[1000, -1000], &[1100, -900]);
        assert!((snr - 20.0).abs() < 1e-9, "{snr}");
    }
}
