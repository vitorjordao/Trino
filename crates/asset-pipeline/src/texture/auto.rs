//! `formats = { 3ds = "auto" }` and the PSNR guard: every format a platform
//! has is measured against the master, and `auto` takes the smallest one
//! that keeps the threshold. No content heuristics decide: an ETC1 of an
//! image with alpha, or an A8 of a colored one, simply measures low.
//!
//! - **Threshold:** `min_psnr` when the texture sets it, else its
//!   `quality` (`low` 30 dB, `normal` 35, `high` 40).
//! - **Ties:** among formats of the same size, the higher PSNR; on the N64
//!   a format that fits TMEM comes first.
//! - **Explicit formats** are the author's choice: only a texture that sets
//!   `min_psnr` checks them, and a miss fails the bake with the formats that
//!   would pass.
//! - **Not measured** (no PSNR): an N64 palette the master doesn't fit
//!   losslessly (mksprite's quantizer), a BCn texture whose sides are not
//!   multiples of 4. `auto` never picks those.

use super::{BcFormat, Ktx2Mode, N3dsFormat, N64Format, ktx2, n64, psnr};
use crate::Platform;

/// A texture's quality preset (`quality = "high"` in the manifest).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    Low,
    #[default]
    Normal,
    High,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Low, Quality::Normal, Quality::High];

    pub fn name(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Normal => "normal",
            Quality::High => "high",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.name() == name)
    }

    /// The PSNR `auto` keeps at this preset.
    pub fn min_psnr(self) -> f64 {
        match self {
            Quality::Low => 30.0,
            Quality::Normal => 35.0,
            Quality::High => 40.0,
        }
    }
}

/// What a sprite or font declares about one platform's texture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextureSettings {
    /// The declared format (`"auto"` included); `None` takes `default`.
    pub format: Option<String>,
    pub quality: Quality,
    pub min_psnr: Option<f64>,
    /// The asset kind's own default (fonts: CI4 on the N64), before the
    /// platform's.
    pub default: Option<&'static str>,
}

/// Where the pixels come from: a font atlas is staged differently for the
/// N64's intensity formats (`n64::stage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Sprite,
    FontAtlas,
}

/// What the master holds, for the report's reason line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Content {
    pub opaque: bool,
    /// Every alpha is 0 or 255.
    pub binary_alpha: bool,
    /// Every visible texel is gray.
    pub gray: bool,
    /// Every visible texel is white.
    pub white: bool,
}

impl Content {
    pub fn of(rgba: &[u8]) -> Self {
        let px = rgba.as_chunks::<4>().0;
        let visible = || px.iter().filter(|p| p[3] > 0);
        Content {
            opaque: px.iter().all(|p| p[3] == 255),
            binary_alpha: px.iter().all(|p| p[3] == 0 || p[3] == 255),
            gray: visible().all(|p| p[0] == p[1] && p[1] == p[2]),
            white: visible().all(|p| p[..3] == [255, 255, 255]),
        }
    }

    pub fn describe(self) -> &'static str {
        match (
            self.white && !self.opaque,
            self.gray,
            self.opaque,
            self.binary_alpha,
        ) {
            (true, _, _, _) => "white mask",
            (_, true, true, _) => "gray, opaque",
            (_, true, false, _) => "gray with alpha",
            (_, false, true, _) => "opaque color",
            (_, false, false, true) => "color with 1-bit alpha",
            _ => "color with alpha",
        }
    }
}

/// One format measured against the master.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub format: String,
    /// Bytes as the platform stores it (3DS: padded to a power of two;
    /// N64: texels plus palette).
    pub bytes: u64,
    /// PSNR in dB (`INFINITY` when lossless), `None` when not measured.
    pub psnr: Option<f64>,
    /// Fits the platform's texture memory as one load (N64 TMEM).
    pub fits: bool,
    /// Why it isn't measured, or why it doesn't fit.
    pub note: Option<String>,
}

/// The format a texture bakes to on one platform, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub format: String,
    /// Picked by `auto` (else declared or the platform default).
    pub auto: bool,
    pub bytes: u64,
    pub psnr: Option<f64>,
    /// The PSNR the choice had to keep, if any.
    pub threshold: Option<f64>,
    pub content: Content,
    /// One line: what `auto` weighed, or "declared".
    pub reason: String,
    /// Every format measured (all of the platform's for `auto`, else the
    /// declared one).
    pub candidates: Vec<Candidate>,
}

/// The hardware formats of `platform`, smallest first.
pub fn formats(platform: Platform) -> Vec<&'static str> {
    match platform {
        Platform::Pc => vec!["BC1", "BC3", "BC7", "RGBA8"],
        Platform::N64 => N64Format::ALL.iter().map(|f| f.name()).collect(),
        Platform::N3ds => {
            let mut v = N3dsFormat::ALL.to_vec();
            v.sort_by_key(|f| f.bits_per_texel());
            v.into_iter().map(|f| f.name()).collect()
        }
        // Smallest download first.
        Platform::Web => vec!["KTX2-ETC1S", "KTX2-UASTC", "RGBA8"],
    }
}

/// Measure `format` on `platform` (a hardware format, not `auto`).
pub fn measure(
    platform: Platform,
    source: Source,
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Candidate, String> {
    let unknown = || format!("`{format}` is not a {} texture format", platform.key());
    let mut c = Candidate {
        format: format.to_string(),
        bytes: 0,
        psnr: None,
        fits: true,
        note: None,
    };
    match platform {
        Platform::Pc if format == "RGBA8" => {
            c.bytes = width as u64 * height as u64 * 4;
            c.psnr = Some(f64::INFINITY);
        }
        Platform::Pc => {
            let f = BcFormat::parse(format).ok_or_else(unknown)?;
            if BcFormat::fits(width, height) {
                c.bytes = f.texture_bytes(width, height);
                c.psnr = Some(psnr(rgba, &f.preview(width, height, rgba)));
            } else {
                c.bytes = width as u64 * height as u64 * f.bits_per_texel() as u64 / 8;
                c.note = Some(format!(
                    "{width}x{height} is not whole 4x4 blocks: BCn needs sides that are multiples of 4"
                ));
            }
        }
        Platform::N3ds => {
            let f = N3dsFormat::parse(format).ok_or_else(unknown)?;
            c.bytes = f.texture_bytes(width, height);
            c.psnr = Some(psnr(rgba, &f.preview(width, height, rgba)));
        }
        // The Web's bytes are the download (the `.ktx2` file, or the raw
        // texels); its VRAM is `Ktx2Mode::gpu_bytes` (`texture::report`).
        Platform::Web if format == "RGBA8" => {
            c.bytes = width as u64 * height as u64 * 4;
            c.psnr = Some(f64::INFINITY);
        }
        Platform::Web => {
            let mode = Ktx2Mode::parse(format).ok_or_else(unknown)?;
            if BcFormat::fits(width, height) {
                let e = ktx2::encode_cached(mode, width, height, rgba)?;
                c.bytes = e.ktx2.len() as u64;
                c.psnr = Some(psnr(rgba, &e.decoded));
            } else {
                c.bytes = mode.gpu_bytes(width, height, true);
                c.note = Some(format!(
                    "{width}x{height} is not whole 4x4 blocks: the GPU formats KTX2 transcodes to need sides that are multiples of 4"
                ));
            }
        }
        Platform::N64 => {
            let f = N64Format::parse(format).ok_or_else(unknown)?;
            c.bytes = f.bytes(width, height);
            match f.preview(width, height, &n64::stage(source, f, rgba)) {
                Ok(p) => c.psnr = Some(psnr(rgba, &p)),
                Err(e) => c.note = Some(e),
            }
            if !f.fits_tmem(width, height) {
                let (used, room) = f.tmem(width, height);
                c.fits = false;
                let overflow = format!("{used} TMEM bytes > {room}: 3D draws can't load it whole");
                c.note = Some(match c.note.take() {
                    Some(n) => format!("{n}; {overflow}"),
                    None => overflow,
                });
            }
        }
    }
    Ok(c)
}

/// Every format of `platform` measured on the master, smallest first.
pub fn candidates(
    platform: Platform,
    source: Source,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Vec<Candidate> {
    formats(platform)
        .into_iter()
        .map(|f| measure(platform, source, f, width, height, rgba).expect("own format"))
        .collect()
}

pub fn db(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_infinite() => "lossless".into(),
        Some(v) => format!("{v:.1} dB"),
        None => "not measured".into(),
    }
}

/// The format `logical` bakes to on `platform`: the declared one (checked
/// against `min_psnr` when set) or `auto`'s pick.
pub fn choose(
    platform: Platform,
    logical: &str,
    source: Source,
    settings: &TextureSettings,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Choice, String> {
    let content = Content::of(rgba);
    let requested = match &settings.format {
        Some(f) => f.clone(),
        None => settings
            .default
            .or(platform.default_texture_format())
            .map(String::from)
            .ok_or_else(|| {
                format!(
                    "`{logical}`: no format declared for {} and this platform has no safe default \
                     — add `formats = {{ {} = \"auto\" }}` (or CI4/CI8/RGBA5551/I4/I8)",
                    platform.key(),
                    platform.key()
                )
            })?,
    };
    if requested != "auto" {
        if !platform
            .supported_texture_formats()
            .contains(&requested.as_str())
        {
            return Err(format!(
                "`{logical}`: format `{requested}` is not supported on {} (supported: auto, {})",
                platform.key(),
                platform.supported_texture_formats().join(", ")
            ));
        }
        let c = measure(platform, source, &requested, width, height, rgba)?;
        if let (Platform::Pc | Platform::Web, Some(note)) = (platform, &c.note) {
            return Err(format!("`{logical}`: {requested}: {note}"));
        }
        if let (Some(min), Some(got)) = (settings.min_psnr, c.psnr)
            && got < min
        {
            let all = candidates(platform, source, width, height, rgba);
            let pass: Vec<&str> = all
                .iter()
                .filter(|o| o.psnr.is_some_and(|p| p >= min))
                .map(|o| o.format.as_str())
                .collect();
            let auto = pick(&all, min).map_or(String::new(), |p| {
                format!("`auto` (picks {}, {}), ", p.format, db(p.psnr))
            });
            return Err(format!(
                "`{logical}`: {} {requested} keeps {} < min_psnr {min} ({}) — use {auto}a format \
                 that passes ({}), or lower `min_psnr`",
                platform.key(),
                db(Some(got)),
                content.describe(),
                if pass.is_empty() {
                    "none".into()
                } else {
                    pass.join(", ")
                }
            ));
        }
        return Ok(Choice {
            format: requested,
            auto: false,
            bytes: c.bytes,
            psnr: c.psnr,
            threshold: settings.min_psnr,
            content,
            reason: if settings.format.is_some() {
                "declared".into()
            } else if settings.default.is_some() {
                format!("default for fonts on {}", platform.key())
            } else {
                format!("{}'s default", platform.key())
            },
            candidates: vec![c],
        });
    }
    let threshold = settings.min_psnr.unwrap_or(settings.quality.min_psnr());
    let all = candidates(platform, source, width, height, rgba);
    let Some(best) = pick(&all, threshold).cloned() else {
        let best = all
            .iter()
            .filter(|c| c.psnr.is_some())
            .max_by(|a, b| a.psnr.partial_cmp(&b.psnr).expect("no NaN"));
        return Err(format!(
            "`{logical}`: no {} format keeps {threshold} dB ({}; best {}) — lower `min_psnr` or \
             `quality`, or declare a format",
            platform.key(),
            content.describe(),
            best.map_or("none measured".into(), |b| format!(
                "{} {}",
                b.format,
                db(b.psnr)
            ))
        ));
    };
    let smaller: Vec<String> = all
        .iter()
        .filter(|c| c.bytes < best.bytes || (c.bytes == best.bytes && c.format != best.format))
        .map(|c| format!("{} {}", c.format, db(c.psnr)))
        .collect();
    let reason = format!(
        "auto ({}, keep {threshold} dB): {} {}{}{}",
        content.describe(),
        best.format,
        db(best.psnr),
        if smaller.is_empty() {
            String::new()
        } else {
            format!("; not {}", smaller.join(", "))
        },
        if best.fits {
            ""
        } else {
            "; nothing that keeps it fits TMEM"
        }
    );
    Ok(Choice {
        format: best.format.clone(),
        auto: true,
        bytes: best.bytes,
        psnr: best.psnr,
        threshold: Some(threshold),
        content,
        reason,
        candidates: all,
    })
}

/// The smallest candidate that keeps `threshold` (fitting memory first;
/// the higher PSNR among equal sizes).
fn pick(all: &[Candidate], threshold: f64) -> Option<&Candidate> {
    all.iter()
        .filter(|c| c.psnr.is_some_and(|p| p >= threshold))
        .min_by(|a, b| {
            (!a.fits, a.bytes)
                .cmp(&(!b.fits, b.bytes))
                .then(b.psnr.partial_cmp(&a.psnr).expect("no NaN"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(format: Option<&str>) -> TextureSettings {
        TextureSettings {
            format: format.map(String::from),
            ..Default::default()
        }
    }

    /// A smooth opaque gradient, a white mask, a gray ramp, colored alpha.
    fn fixture(kind: &str) -> Vec<u8> {
        (0..32 * 32u32)
            .flat_map(|i| {
                let (x, y) = (i % 32, i / 32);
                match kind {
                    "opaque" => [(x * 4) as u8, (y * 2) as u8, 128, 255],
                    "mask" => {
                        let a = ((x as i32 - 16).abs() * 16).min(255) as u8;
                        [255, 255, 255, a]
                    }
                    "gray" => [(x * 8) as u8, (x * 8) as u8, (x * 8) as u8, 255],
                    // An N64 glow: intensity and coverage alike.
                    "glow" => [(x * 8) as u8; 4],
                    _ => [(x * 8) as u8, 40, (y * 8) as u8, (y * 8) as u8],
                }
            })
            .collect()
    }

    fn auto(platform: Platform, kind: &str) -> Choice {
        choose(
            platform,
            "sprites/x",
            Source::Sprite,
            &settings(Some("auto")),
            32,
            32,
            &fixture(kind),
        )
        .unwrap()
    }

    #[test]
    fn auto_picks_by_content_without_being_told() {
        assert_eq!(auto(Platform::N3ds, "opaque").format, "ETC1");
        assert_eq!(auto(Platform::N3ds, "mask").format, "A8");
        assert_eq!(
            auto(Platform::N3ds, "gray").format,
            "ETC1",
            "a gray ramp is ETC1-friendly"
        );
        // A full alpha ramp in 4 bits keeps ~33 dB: RGBA8 at the default
        // 35, ETC1A4 at `low` (30).
        assert_eq!(auto(Platform::N3ds, "alpha").format, "RGBA8");
        let low = TextureSettings {
            format: Some("auto".into()),
            quality: Quality::Low,
            ..Default::default()
        };
        let c = choose(
            Platform::N3ds,
            "sprites/x",
            Source::Sprite,
            &low,
            32,
            32,
            &fixture("alpha"),
        );
        assert_eq!(c.unwrap().format, "ETC1A4");
        assert_eq!(auto(Platform::Pc, "opaque").format, "BC1");
        // BC3 and BC7 are the same size: the higher PSNR (BC7) wins.
        let mask = auto(Platform::Pc, "mask");
        assert_eq!(mask.format, "BC7", "{}", mask.reason);
        assert!(mask.reason.contains("BC3 "), "{}", mask.reason);
        // The RDP samples I4/I8 as (i, i, i, i): an opaque gray image would
        // turn translucent, so it takes a palette; a glow takes intensity.
        assert_eq!(auto(Platform::N64, "gray").format, "CI8");
        let glow = auto(Platform::N64, "glow");
        assert!(glow.format.starts_with('I'), "{}", glow.reason);
        // The reason names the content and what lost.
        let c = auto(Platform::N3ds, "mask");
        assert!(
            c.reason.contains("white mask") && c.reason.contains("ETC1 "),
            "{}",
            c.reason
        );
    }

    #[test]
    fn quality_and_min_psnr_move_the_pick() {
        let rgba = fixture("alpha");
        let at = |s: TextureSettings| {
            choose(
                Platform::N3ds,
                "sprites/x",
                Source::Sprite,
                &s,
                32,
                32,
                &rgba,
            )
            .unwrap()
            .format
        };
        let strict = at(TextureSettings {
            format: Some("auto".into()),
            min_psnr: Some(80.0),
            ..Default::default()
        });
        assert_eq!(strict, "RGBA8");
        let low = at(TextureSettings {
            format: Some("auto".into()),
            quality: Quality::Low,
            ..Default::default()
        });
        let high = at(TextureSettings {
            format: Some("auto".into()),
            quality: Quality::High,
            ..Default::default()
        });
        let size = |f: &str| N3dsFormat::parse(f).unwrap().bits_per_texel();
        assert!(size(&low) <= size(&high), "{low} vs {high}");
    }

    #[test]
    fn an_explicit_format_below_min_psnr_fails_with_the_way_out() {
        let s = TextureSettings {
            format: Some("ETC1".into()),
            min_psnr: Some(35.0),
            ..Default::default()
        };
        let err = choose(
            Platform::N3ds,
            "sprites/x",
            Source::Sprite,
            &s,
            32,
            32,
            &fixture("mask"),
        )
        .unwrap_err();
        assert!(
            err.contains("sprites/x") && err.contains("min_psnr 35"),
            "{err}"
        );
        assert!(
            err.contains("`auto` (picks A8, lossless)") && err.contains("RGBA8"),
            "{err}"
        );
        // Without min_psnr the declared format is the author's call.
        let c = choose(
            Platform::N3ds,
            "sprites/x",
            Source::Sprite,
            &settings(Some("ETC1")),
            32,
            32,
            &fixture("mask"),
        )
        .unwrap();
        assert!(!c.auto && c.reason == "declared" && c.psnr.unwrap() < 35.0);
    }

    #[test]
    fn bc_needs_whole_blocks_and_n64_needs_a_format() {
        let rgba = vec![255u8; 6 * 6 * 4];
        let c = choose(
            Platform::Pc,
            "sprites/x",
            Source::Sprite,
            &settings(None),
            6,
            6,
            &rgba,
        )
        .unwrap();
        assert_eq!(c.format, "RGBA8", "{}", c.reason);
        let err = choose(
            Platform::Pc,
            "sprites/x",
            Source::Sprite,
            &settings(Some("BC1")),
            6,
            6,
            &rgba,
        )
        .unwrap_err();
        assert!(err.contains("multiples of 4"), "{err}");
        let err = choose(
            Platform::N64,
            "sprites/x",
            Source::Sprite,
            &settings(None),
            6,
            6,
            &rgba,
        )
        .unwrap_err();
        assert!(err.contains("auto"), "{err}");
        let err = choose(
            Platform::N3ds,
            "sprites/x",
            Source::Sprite,
            &settings(Some("BC1")),
            6,
            6,
            &rgba,
        )
        .unwrap_err();
        assert!(err.contains("supported: auto, RGBA8"), "{err}");
    }

    #[test]
    fn a_font_atlas_is_staged_for_n64_intensity() {
        let rgba = fixture("mask");
        let font = measure(Platform::N64, Source::FontAtlas, "I8", 32, 32, &rgba).unwrap();
        let sprite = measure(Platform::N64, Source::Sprite, "I8", 32, 32, &rgba).unwrap();
        // A white-mask sprite is staged by its coverage too (`n64::stage`):
        // far closer than mksprite's solid white, though (i, i, i, i)
        // still attenuates twice.
        assert_eq!(font.psnr, sprite.psnr);
        let raw = psnr(&rgba, &N64Format::I8.preview(32, 32, &rgba).unwrap());
        assert!(font.psnr.unwrap() > raw + 10.0, "{font:?} vs {raw}");
    }
}
