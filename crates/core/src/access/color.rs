//! Color-vision math, deterministic and allocation-free: sRGB ↔ linear
//! through tables, the simulation of each [`CvdMode`] (Machado, Oliveira and
//! Fernandes 2009 at severity 1.0, in linear RGB; grayscale is the Rec. 709
//! luminance), the WCAG contrast ratio and the CIE76 ΔE in CIELAB (D65).
//!
//! The consoles never simulate at run time — their CVD support is the
//! palette swap ([`super::AccessData::color`]). The simulation is here so the
//! bake, `cargo xtask access check`, the editor, the PC's simulation view
//! and games that pick between existing colors (Solid Eleven's kits) all use
//! the same numbers.

use super::CvdMode;
use crate::math::Color;
use crate::math3d;

/// A linear-light RGB triple, 0..=1 per channel.
pub type Linear = [f32; 3];

/// The 8-bit sRGB code → linear light (IEC 61966-2-1).
pub fn to_linear(c: u8) -> f32 {
    SRGB_TO_LINEAR[c as usize]
}

/// Linear light → the nearest 8-bit sRGB code (the same as rounding the
/// encoded value: the search runs over the codes' midpoints).
pub fn to_srgb(l: f32) -> u8 {
    // The first midpoint above `l` is the code (a binary search: 8 steps).
    let (mut lo, mut hi) = (0usize, LINEAR_MIDPOINTS.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if l >= LINEAR_MIDPOINTS[mid] {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo as u8
}

/// A color's RGB in linear light (alpha is ignored).
pub fn linear(c: Color) -> Linear {
    [to_linear(c.r), to_linear(c.g), to_linear(c.b)]
}

/// Linear RGB back to a color with alpha `a`, each channel clamped.
pub fn from_linear(l: Linear, a: u8) -> Color {
    Color::rgba(
        to_srgb(l[0].clamp(0.0, 1.0)),
        to_srgb(l[1].clamp(0.0, 1.0)),
        to_srgb(l[2].clamp(0.0, 1.0)),
        a,
    )
}

/// Machado 2009, severity 1.0, linear RGB: protanopia.
pub const PROTANOPIA: [[f32; 3]; 3] = [
    [0.152286, 1.052583, -0.204868],
    [0.114503, 0.786281, 0.099216],
    [-0.003882, -0.048116, 1.051998],
];
/// Machado 2009, severity 1.0, linear RGB: deuteranopia.
pub const DEUTERANOPIA: [[f32; 3]; 3] = [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.011820, 0.042940, 0.968881],
];
/// Machado 2009, severity 1.0, linear RGB: tritanopia.
pub const TRITANOPIA: [[f32; 3]; 3] = [
    [1.255528, -0.076749, -0.178779],
    [-0.078411, 0.930809, 0.147602],
    [0.004733, 0.691367, 0.303900],
];
/// Rec. 709 luminance in every row: grayscale.
pub const GRAYSCALE: [[f32; 3]; 3] = [
    [0.2126, 0.7152, 0.0722],
    [0.2126, 0.7152, 0.0722],
    [0.2126, 0.7152, 0.0722],
];
/// The identity (no simulation).
pub const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// The linear-RGB matrix that simulates `mode` (the identity for `Off`).
/// Each row sums to 1, so grays (white and black included) stay as they are.
pub const fn matrix(mode: CvdMode) -> &'static [[f32; 3]; 3] {
    match mode {
        CvdMode::Off => &IDENTITY,
        CvdMode::Protanopia => &PROTANOPIA,
        CvdMode::Deuteranopia => &DEUTERANOPIA,
        CvdMode::Tritanopia => &TRITANOPIA,
        CvdMode::Grayscale => &GRAYSCALE,
    }
}

fn apply(m: &[[f32; 3]; 3], l: Linear) -> Linear {
    let row = |r: &[f32; 3]| r[0] * l[0] + r[1] * l[1] + r[2] * l[2];
    [row(&m[0]), row(&m[1]), row(&m[2])]
}

/// Where daltonization moves the error a viewer with `mode` can't see
/// (Fidaner, Lin and Ozguven 2005, in linear RGB): a red-green loss goes
/// into green and blue, a blue-yellow loss into red and green. No color at
/// all has nowhere to go (zeros: grayscale and `Off` stay as they are).
pub const fn error_shift(mode: CvdMode) -> [[f32; 3]; 3] {
    match mode {
        CvdMode::Protanopia | CvdMode::Deuteranopia => {
            [[0.0, 0.0, 0.0], [0.7, 1.0, 0.0], [0.7, 0.0, 1.0]]
        }
        CvdMode::Tritanopia => [[1.0, 0.0, 0.7], [0.0, 1.0, 0.7], [0.0, 0.0, 0.0]],
        CvdMode::Off | CvdMode::Grayscale => [[0.0; 3]; 3],
    }
}

/// The daltonization matrix for `mode` (issue #64), linear RGB:
/// `C = I + E·(I − S)`, the color plus the error the viewer misses
/// ([`matrix`]'s `S`) moved to the channels they see ([`error_shift`]'s
/// `E`). Grays stay gray (`S` keeps them, so the error is zero); `Off` and
/// `Grayscale` are the identity.
pub fn correction(mode: CvdMode) -> [[f32; 3]; 3] {
    let s = matrix(mode);
    let e = error_shift(mode);
    let mut out = IDENTITY;
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            // (E · (I − S))[r][c]
            let mut sum = 0.0;
            for (k, ek) in e[r].iter().enumerate() {
                let i_minus_s = if k == c { 1.0 } else { 0.0 } - s[k][c];
                sum += ek * i_minus_s;
            }
            *v += sum;
        }
    }
    out
}

/// `c` daltonized for `mode` (alpha kept): what the PC's correction filter
/// draws, the CPU reference its shader is held to.
pub fn correct(mode: CvdMode, c: Color) -> Color {
    if matches!(mode, CvdMode::Off | CvdMode::Grayscale) {
        return c;
    }
    from_linear(apply(&correction(mode), linear(c)), c.a)
}

/// How a viewer with `mode` sees `c` (alpha kept). `Off` returns `c`.
pub fn simulate(mode: CvdMode, c: Color) -> Color {
    if mode == CvdMode::Off {
        return c;
    }
    from_linear(apply(matrix(mode), linear(c)), c.a)
}

/// WCAG relative luminance, 0 (black) ..= 1 (white).
pub fn luminance(c: Color) -> f32 {
    let l = linear(c);
    0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
}

/// WCAG contrast ratio of two colors, 1 ..= 21 (order does not matter).
/// Text wants 4.5 (3 when large); a high-contrast theme 7.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// CIELAB (D65 white) of a color: `[L*, a*, b*]`.
pub fn lab(c: Color) -> [f32; 3] {
    let l = linear(c);
    // sRGB (D65) → XYZ, then normalized by the white point.
    let x = (0.4124564 * l[0] + 0.3575761 * l[1] + 0.1804375 * l[2]) / 0.95047;
    let y = 0.2126729 * l[0] + 0.7151522 * l[1] + 0.0721750 * l[2];
    let z = (0.0193339 * l[0] + 0.119192 * l[1] + 0.9503041 * l[2]) / 1.08883;
    let (fx, fy, fz) = (lab_f(x), lab_f(y), lab_f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_f(t: f32) -> f32 {
    const E: f32 = 216.0 / 24389.0; // (6/29)^3
    const K: f32 = 24389.0 / 27.0;
    if t > E {
        cbrt(t)
    } else {
        (K * t + 16.0) / 116.0
    }
}

/// The cube root of a positive number (Newton's method from a bit-level
/// guess; deterministic like the rest of the core's math).
pub fn cbrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits(x.to_bits() / 3 + 0x2a51_67af);
    for _ in 0..4 {
        y -= (y * y * y - x) / (3.0 * y * y);
    }
    y
}

/// CIE76 color difference: the distance in CIELAB. About 2.3 is "just
/// noticeable"; colors that must read as different teams or states want
/// [`MIN_DELTA_E`] or more.
pub fn delta_e(a: Color, b: Color) -> f32 {
    let (p, q) = (lab(a), lab(b));
    let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    math3d::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
}

/// The default distance two semantic colors keep apart in every mode
/// (`min_delta_e` in `assets/palettes.ron` overrides it).
pub const MIN_DELTA_E: f32 = 20.0;

/// How far apart `a` and `b` look to a viewer with `mode`.
pub fn delta_e_seen(mode: CvdMode, a: Color, b: Color) -> f32 {
    delta_e(simulate(mode, a), simulate(mode, b))
}

const SRGB_TO_LINEAR: [f32; 256] = [
    0.0,
    0.000303527,
    0.000607054,
    0.000910581,
    0.001214108,
    0.001517635,
    0.001821162,
    0.0021246888,
    0.002428216,
    0.0027317428,
    0.00303527,
    0.0033465358,
    0.0036765074,
    0.004024717,
    0.004391442,
    0.0047769533,
    0.0051815165,
    0.0056053917,
    0.006048833,
    0.0065120906,
    0.00699541,
    0.007499032,
    0.008023193,
    0.008568126,
    0.009134059,
    0.009721218,
    0.010329823,
    0.010960094,
    0.011612245,
    0.012286488,
    0.0129830325,
    0.013702083,
    0.014443844,
    0.015208514,
    0.015996294,
    0.016807375,
    0.017641954,
    0.01850022,
    0.019382361,
    0.020288562,
    0.02121901,
    0.022173885,
    0.023153367,
    0.024157632,
    0.02518686,
    0.026241222,
    0.027320892,
    0.02842604,
    0.029556835,
    0.030713445,
    0.031896032,
    0.033104766,
    0.034339808,
    0.035601314,
    0.03688945,
    0.038204372,
    0.039546236,
    0.0409152,
    0.04231141,
    0.04373503,
    0.045186203,
    0.046665087,
    0.048171826,
    0.049706567,
    0.051269457,
    0.052860647,
    0.054480277,
    0.05612849,
    0.05780543,
    0.059511237,
    0.061246052,
    0.063010015,
    0.064803265,
    0.06662594,
    0.06847817,
    0.070360094,
    0.07227185,
    0.07421357,
    0.07618538,
    0.07818742,
    0.08021982,
    0.08228271,
    0.08437621,
    0.08650046,
    0.08865558,
    0.09084171,
    0.093058966,
    0.09530747,
    0.09758735,
    0.099898726,
    0.10224173,
    0.104616486,
    0.107023105,
    0.10946171,
    0.11193243,
    0.114435375,
    0.116970666,
    0.11953843,
    0.122138776,
    0.12477182,
    0.12743768,
    0.13013647,
    0.13286832,
    0.13563333,
    0.13843161,
    0.14126329,
    0.14412847,
    0.14702727,
    0.14995979,
    0.15292615,
    0.15592647,
    0.15896083,
    0.16202937,
    0.1651322,
    0.1682694,
    0.17144111,
    0.1746474,
    0.17788842,
    0.18116425,
    0.18447499,
    0.18782078,
    0.19120169,
    0.19461784,
    0.19806932,
    0.20155625,
    0.20507874,
    0.20863687,
    0.21223076,
    0.2158605,
    0.2195262,
    0.22322796,
    0.22696587,
    0.23074006,
    0.23455058,
    0.23839757,
    0.24228112,
    0.24620132,
    0.25015828,
    0.2541521,
    0.25818285,
    0.26225066,
    0.2663556,
    0.2704978,
    0.2746773,
    0.27889428,
    0.28314874,
    0.28744084,
    0.29177064,
    0.29613826,
    0.30054379,
    0.3049873,
    0.30946892,
    0.31398872,
    0.31854677,
    0.3231432,
    0.3277781,
    0.33245152,
    0.33716363,
    0.34191442,
    0.34670407,
    0.3515326,
    0.35640013,
    0.3613068,
    0.3662526,
    0.3712377,
    0.37626213,
    0.38132602,
    0.38642943,
    0.39157248,
    0.39675522,
    0.40197778,
    0.4072402,
    0.4125426,
    0.41788507,
    0.42326766,
    0.4286905,
    0.43415365,
    0.43965718,
    0.4452012,
    0.4507858,
    0.45641103,
    0.462077,
    0.4677838,
    0.47353148,
    0.47932017,
    0.48514995,
    0.49102086,
    0.49693298,
    0.5028865,
    0.50888133,
    0.5149177,
    0.52099556,
    0.5271151,
    0.5332764,
    0.5394795,
    0.54572445,
    0.55201143,
    0.5583404,
    0.5647115,
    0.57112485,
    0.57758045,
    0.58407843,
    0.59061885,
    0.59720176,
    0.60382736,
    0.61049557,
    0.6172066,
    0.6239604,
    0.63075715,
    0.63759685,
    0.6444797,
    0.65140563,
    0.65837485,
    0.6653873,
    0.67244315,
    0.6795425,
    0.6866853,
    0.69387174,
    0.7011019,
    0.70837575,
    0.7156935,
    0.7230551,
    0.73046076,
    0.7379104,
    0.7454042,
    0.7529422,
    0.7605245,
    0.76815116,
    0.7758222,
    0.7835378,
    0.7912979,
    0.7991027,
    0.80695224,
    0.8148466,
    0.82278574,
    0.8307699,
    0.838799,
    0.8468732,
    0.8549926,
    0.8631572,
    0.8713671,
    0.8796224,
    0.8879231,
    0.8962694,
    0.9046612,
    0.91309863,
    0.92158186,
    0.9301109,
    0.9386857,
    0.9473065,
    0.9559733,
    0.9646863,
    0.9734453,
    0.9822506,
    0.9911021,
    1.0,
];

const LINEAR_MIDPOINTS: [f32; 255] = [
    0.0001517635,
    0.0004552905,
    0.0007588175,
    0.0010623444,
    0.0013658714,
    0.0016693984,
    0.0019729254,
    0.0022764525,
    0.0025799794,
    0.0028835062,
    0.0031883009,
    0.0035092593,
    0.003848315,
    0.004205748,
    0.004581833,
    0.0049768374,
    0.005391024,
    0.0058246506,
    0.0062779696,
    0.0067512277,
    0.0072446684,
    0.0077585303,
    0.0082930485,
    0.008848453,
    0.0094249705,
    0.010022826,
    0.010642237,
    0.011283421,
    0.0119465925,
    0.01263196,
    0.013339732,
    0.014070112,
    0.014823303,
    0.015599503,
    0.01639891,
    0.017221715,
    0.018068114,
    0.018938294,
    0.019832443,
    0.020750744,
    0.021693382,
    0.022660539,
    0.02365239,
    0.024669115,
    0.025710888,
    0.026777882,
    0.02787027,
    0.02898822,
    0.030131903,
    0.03130148,
    0.032497123,
    0.03371899,
    0.034967244,
    0.036242045,
    0.037543554,
    0.038871925,
    0.04022732,
    0.041609887,
    0.043019786,
    0.044457164,
    0.04592217,
    0.047414962,
    0.048935685,
    0.050484486,
    0.052061506,
    0.053666897,
    0.055300802,
    0.05696336,
    0.058654718,
    0.060375012,
    0.062124383,
    0.063902974,
    0.06571092,
    0.06754835,
    0.06941541,
    0.071312234,
    0.073238954,
    0.07519571,
    0.07718261,
    0.07919982,
    0.08124744,
    0.083325624,
    0.08543449,
    0.087574154,
    0.08974477,
    0.09194644,
    0.0941793,
    0.096443474,
    0.098739095,
    0.10106627,
    0.10342513,
    0.105815805,
    0.1082384,
    0.110693045,
    0.11317986,
    0.11569897,
    0.11825048,
    0.12083452,
    0.1234512,
    0.12610064,
    0.12878296,
    0.13149826,
    0.13424668,
    0.1370283,
    0.13984327,
    0.14269169,
    0.14557366,
    0.14848931,
    0.15143873,
    0.15442206,
    0.15743938,
    0.16049083,
    0.1635765,
    0.16669649,
    0.16985093,
    0.17303991,
    0.17626357,
    0.17952198,
    0.18281525,
    0.1861435,
    0.18950683,
    0.19290535,
    0.19633915,
    0.19980834,
    0.20331304,
    0.20685335,
    0.21042934,
    0.21404114,
    0.21768884,
    0.22137256,
    0.2250924,
    0.22884843,
    0.23264076,
    0.2364695,
    0.24033478,
    0.24423663,
    0.2481752,
    0.25215057,
    0.25616285,
    0.26021212,
    0.26429847,
    0.26842204,
    0.2725829,
    0.2767811,
    0.2810168,
    0.2852901,
    0.28960103,
    0.29394972,
    0.2983363,
    0.3027608,
    0.30722335,
    0.31172404,
    0.31626296,
    0.32084018,
    0.32545584,
    0.33010998,
    0.33480275,
    0.33953416,
    0.34430438,
    0.34911346,
    0.3539615,
    0.35884857,
    0.36377478,
    0.36874023,
    0.37374496,
    0.37878913,
    0.38387278,
    0.388996,
    0.3941589,
    0.39936152,
    0.40460402,
    0.40988642,
    0.41520882,
    0.42057136,
    0.42597404,
    0.43141702,
    0.43690035,
    0.44242412,
    0.44798842,
    0.4535933,
    0.45923892,
    0.4649253,
    0.47065252,
    0.4764207,
    0.48222992,
    0.48808023,
    0.49397177,
    0.49990454,
    0.5058787,
    0.5118943,
    0.5179514,
    0.5240501,
    0.5301905,
    0.5363727,
    0.54259676,
    0.5488627,
    0.55517066,
    0.5615207,
    0.5679129,
    0.5743473,
    0.58082414,
    0.58734334,
    0.593905,
    0.6005092,
    0.6071561,
    0.6138457,
    0.6205781,
    0.62735337,
    0.6341716,
    0.6410329,
    0.64793724,
    0.6548848,
    0.66187567,
    0.6689098,
    0.67598736,
    0.68310845,
    0.6902731,
    0.69748133,
    0.7047334,
    0.71202916,
    0.7193688,
    0.72675246,
    0.73418003,
    0.7416518,
    0.7491677,
    0.7567278,
    0.7643323,
    0.7719811,
    0.7796744,
    0.7874123,
    0.79519475,
    0.8030219,
    0.81089383,
    0.8188105,
    0.8267722,
    0.8347788,
    0.8428305,
    0.8509273,
    0.8590692,
    0.8672565,
    0.87548906,
    0.88376707,
    0.89209056,
    0.9004596,
    0.9088742,
    0.91733456,
    0.9258406,
    0.9343926,
    0.94299036,
    0.95163417,
    0.96032405,
    0.96906,
    0.97784215,
    0.98667055,
    0.99554527,
];

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::rgb(200, 40, 40);
    const GREEN: Color = Color::rgb(40, 160, 60);
    const BLUE: Color = Color::rgb(40, 90, 220);

    #[test]
    fn transfer_tables_round_trip_every_code() {
        for c in 0..=255u8 {
            assert_eq!(to_srgb(to_linear(c)), c);
        }
        assert_eq!(to_srgb(0.0), 0);
        assert_eq!(to_srgb(1.0), 255);
        assert_eq!(to_srgb(0.5), 188); // round(255 × 0.7354)
    }

    /// Reference values: the published matrices with the exact sRGB
    /// transfer, rounded (an independent float64 implementation).
    #[test]
    fn simulation_matches_the_reference_values() {
        let cases = [
            (CvdMode::Deuteranopia, RED, (131, 117, 32)),
            (CvdMode::Deuteranopia, GREEN, (149, 136, 68)),
            (CvdMode::Protanopia, RED, (92, 82, 38)),
            (CvdMode::Protanopia, GREEN, (163, 145, 49)),
            (CvdMode::Tritanopia, RED, (221, 0, 43)),
            (CvdMode::Tritanopia, GREEN, (0, 156, 139)),
            (CvdMode::Tritanopia, BLUE, (0, 123, 146)),
            (CvdMode::Grayscale, RED, (104, 104, 104)),
            (CvdMode::Grayscale, GREEN, (139, 139, 139)),
        ];
        for (mode, c, (r, g, b)) in cases {
            let s = simulate(mode, c);
            let near = |x: u8, y: u8| x.abs_diff(y) <= 1;
            assert!(
                near(s.r, r) && near(s.g, g) && near(s.b, b),
                "{mode:?} {c:?} -> {s:?}, reference ({r}, {g}, {b})"
            );
        }
        assert_eq!(simulate(CvdMode::Off, RED), RED);
    }

    #[test]
    fn grays_stay_gray_in_every_mode() {
        for mode in CvdMode::ALL {
            for v in [0u8, 64, 128, 200, 255] {
                let s = simulate(mode, Color::rgba(v, v, v, 77));
                assert!(s.r.abs_diff(v) <= 1 && s.g.abs_diff(v) <= 1 && s.b.abs_diff(v) <= 1);
                assert_eq!(s.a, 77, "alpha is kept");
            }
            let m = matrix(mode);
            for row in m {
                assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn lab_contrast_and_delta_e_match_their_definitions() {
        let l = lab(Color::rgb(255, 0, 0));
        let want = [53.2408, 80.0925, 67.2032];
        for k in 0..3 {
            assert!((l[k] - want[k]).abs() < 0.05, "{l:?}");
        }
        let d = delta_e(Color::rgb(255, 0, 0), Color::rgb(0, 0, 255));
        assert!((d - 176.314).abs() < 0.1, "{d}");
        assert!((contrast_ratio(Color::WHITE, Color::BLACK) - 21.0).abs() < 1e-3);
        assert!((contrast_ratio(Color::BLACK, Color::WHITE) - 21.0).abs() < 1e-3);
        assert!((contrast_ratio(RED, RED) - 1.0).abs() < 1e-6);
        // Red and green: far apart for most viewers, close for a
        // deuteranope.
        assert!(delta_e_seen(CvdMode::Off, RED, GREEN) > 60.0);
        assert!((delta_e_seen(CvdMode::Deuteranopia, RED, GREEN) - 11.09).abs() < 0.05);
        for x in [1e-6f32, 0.008, 0.5, 1.0, 8.0, 1000.0] {
            let c = cbrt(x);
            assert!((c * c * c - x).abs() / x < 1e-5, "{x}");
        }
    }

    #[test]
    fn daltonization_keeps_grays_and_pulls_confused_pairs_apart() {
        for mode in CvdMode::ALL {
            for v in [0u8, 64, 128, 200, 255] {
                let g = Color::rgba(v, v, v, 9);
                let c = correct(mode, g);
                assert!(c.r.abs_diff(v) <= 1 && c.g.abs_diff(v) <= 1 && c.b.abs_diff(v) <= 1);
                assert_eq!(c.a, 9);
            }
        }
        assert_eq!(correction(CvdMode::Off), IDENTITY);
        assert_eq!(correction(CvdMode::Grayscale), IDENTITY);
        // The pairs each viewer confuses, corrected, look farther apart to
        // that same viewer.
        let pairs = [
            (CvdMode::Protanopia, RED, GREEN),
            (CvdMode::Deuteranopia, RED, GREEN),
            (
                CvdMode::Tritanopia,
                Color::rgb(40, 90, 230),
                Color::rgb(40, 170, 120),
            ),
        ];
        for (mode, a, b) in pairs {
            let before = delta_e_seen(mode, a, b);
            let after = delta_e_seen(mode, correct(mode, a), correct(mode, b));
            assert!(after > before * 1.3, "{mode:?}: {before} -> {after}");
        }
    }
}
