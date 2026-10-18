//! The Web's textures (issue #47, ADR 0017 §6): KTX2 files holding Basis
//! Universal data, transcoded in the browser to whatever its GPU samples —
//! BC on desktops, ETC2 or ASTC on phones, RGBA8 where none is there.
//!
//! - **`KTX2-ETC1S`**: BasisLZ-supercompressed ETC1S, the smallest download
//!   (often under a bit a texel). Transcodes to BC1/BC3, ETC2 or ASTC.
//! - **`KTX2-UASTC`**: UASTC 4×4 blocks (one byte a texel), close to the
//!   master. Transcodes to BC7, ASTC or ETC2.
//!
//! Binomial's encoder (the `basis-universal` crate, a host tool only) makes
//! a `.basis` file; [`basis_to_ktx2`] moves its data into the KTX2
//! container the way the encoder's own `create_ktx2_file` does: the ETC1S
//! codebooks and Huffman tables become the BasisLZ global data, the slices
//! become level 0. The browser side is Binomial's transcoder compiled to
//! WebAssembly (`apps/web/vendor/basis`), the same code as
//! [`decode`]'s, so the PSNR the bake measures is what a browser shows.

use std::sync::Once;

use basis_universal::{
    BasisTextureFormat, ColorSpace, Compressor, CompressorParams, TranscodeParameters, Transcoder,
    TranscoderTextureFormat,
};

/// The two Basis Universal modes a Web texture can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ktx2Mode {
    Etc1s,
    Uastc,
}

impl Ktx2Mode {
    pub const ALL: [Ktx2Mode; 2] = [Ktx2Mode::Etc1s, Ktx2Mode::Uastc];

    /// The manifest's name: `KTX2-ETC1S`, `KTX2-UASTC`.
    pub fn name(self) -> &'static str {
        match self {
            Ktx2Mode::Etc1s => "KTX2-ETC1S",
            Ktx2Mode::Uastc => "KTX2-UASTC",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// GPU bytes once transcoded: ETC1S opaque to BC1 / ETC2 RGB (half a
    /// byte a texel), with alpha to BC3 / ETC2 RGBA, UASTC to BC7 / ASTC
    /// (a byte).
    pub fn gpu_bytes(self, width: u32, height: u32, alpha: bool) -> u64 {
        let texels = width.div_ceil(4) as u64 * height.div_ceil(4) as u64 * 16;
        match (self, alpha) {
            (Ktx2Mode::Etc1s, false) => texels / 2,
            _ => texels,
        }
    }
}

/// A texture encoded for the Web.
#[derive(Clone, Debug, PartialEq)]
pub struct Encoded {
    /// The `.ktx2` file.
    pub ktx2: Vec<u8>,
    /// What every browser samples from it (RGBA8, the transcoder's
    /// reference decode).
    pub decoded: Vec<u8>,
    /// Some texel of the master is below full alpha (the file carries an
    /// alpha channel).
    pub alpha: bool,
}

static TRANSCODER: Once = Once::new();

/// Encode `rgba` (`width`×`height`) in `mode`: the KTX2 file and its
/// decode. Deterministic (one encoder thread, fixed settings), so bakes
/// write the same bytes every time.
pub fn encode(mode: Ktx2Mode, width: u32, height: u32, rgba: &[u8]) -> Result<Encoded, String> {
    assert_eq!(rgba.len(), (width * height * 4) as usize);
    let mut params = CompressorParams::new();
    params.set_basis_format(match mode {
        Ktx2Mode::Etc1s => BasisTextureFormat::ETC1S,
        Ktx2Mode::Uastc => BasisTextureFormat::UASTC4x4,
    });
    // The engine's colors are display-referred (sRGB values sampled as
    // they are): perceptual error metrics, and the KTX2 file says sRGB.
    params.set_color_space(ColorSpace::Srgb);
    params.set_etc1s_quality_level(basis_universal::ETC1S_QUALITY_MAX);
    params.set_uastc_quality_level(basis_universal::UASTC_QUALITY_DEFAULT);
    params.set_generate_mipmaps(false);
    params.set_print_status_to_stdout(false);
    params.source_image_mut(0).init(rgba, width, height, 4);
    let mut compressor = Compressor::new(1);
    // SAFETY: the parameters are the crate's own setters on one RGBA8
    // image of the size given; nothing else is configured.
    unsafe {
        if !compressor.init(&params) {
            return Err(format!(
                "{}: the Basis encoder refused the image",
                mode.name()
            ));
        }
        compressor
            .process()
            .map_err(|e| format!("{}: Basis encoder: {e:?}", mode.name()))?;
    }
    let basis = compressor.basis_file().to_vec();
    let alpha = rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
    let ktx2 = basis_to_ktx2(&basis, mode, alpha)?;
    let decoded = decode_basis(&basis, width, height)?;
    Ok(Encoded {
        ktx2,
        decoded,
        alpha,
    })
}

/// [`encode`], remembered: `auto` measures a texture and the bake then
/// writes the pick, `texture report` and the editor ask again — each a
/// full Basis encode. Keyed by mode, size and a hash of the pixels; a few
/// hundred entries at most (a project's textures).
pub fn encode_cached(
    mode: Ktx2Mode,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Encoded, String> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Key = (Ktx2Mode, u32, u32, u64);
    static CACHE: OnceLock<Mutex<HashMap<Key, Encoded>>> = OnceLock::new();
    // FNV-1a over the pixels (and their length through the size).
    let hash = rgba.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let key = (mode, width, height, hash);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(e) = cache.lock().unwrap().get(&key) {
        return Ok(e.clone());
    }
    let e = encode(mode, width, height, rgba)?;
    let mut map = cache.lock().unwrap();
    if map.len() > 512 {
        map.clear();
    }
    map.insert(key, e.clone());
    Ok(e)
}

/// The `.basis` file decoded to RGBA8 by Binomial's transcoder (the code
/// the browser runs).
fn decode_basis(basis: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    TRANSCODER.call_once(basis_universal::transcoder_init);
    let mut t = Transcoder::new();
    t.prepare_transcoding(basis)
        .map_err(|_| "Basis transcoder: bad file".to_string())?;
    let out = t
        .transcode_image_level(
            basis,
            TranscoderTextureFormat::RGBA32,
            TranscodeParameters {
                image_index: 0,
                level_index: 0,
                ..Default::default()
            },
        )
        .map_err(|e| format!("Basis transcoder: {e:?}"))?;
    if out.len() != (width * height * 4) as usize {
        return Err(format!(
            "Basis transcoder: {} bytes for {width}x{height}",
            out.len()
        ));
    }
    Ok(out)
}

/// Little-endian read of `n` bytes at `at` (`.basis` headers pack 2-, 3-
/// and 4-byte fields).
fn le(b: &[u8], at: usize, n: usize) -> Result<u64, String> {
    let bytes = b
        .get(at..at + n)
        .ok_or_else(|| "truncated .basis file".to_string())?;
    Ok(bytes
        .iter()
        .rev()
        .fold(0u64, |v, &byte| (v << 8) | byte as u64))
}

/// The 12-byte KTX2 identifier.
pub const KTX2_MAGIC: [u8; 12] = [
    0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
];

/// KTX2 supercompression schemes.
const SS_NONE: u32 = 0;
const SS_BASISLZ: u32 = 1;

/// The data format descriptors Binomial's encoder writes (`basisu_comp.cpp`,
/// `g_ktx2_*_dfd`), before the transfer function and channel fix-ups.
const DFD_ETC1S: [u8; 44] = [
    0x2C, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0, 0x28, 0x0, 0xA3, 0x1, 0x2, 0x0, 0x3, 0x3,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x3F, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0xFF, 0xFF, 0xFF, 0xFF,
];
const DFD_ETC1S_ALPHA: [u8; 60] = [
    0x3C, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0, 0x38, 0x0, 0xA3, 0x1, 0x2, 0x0, 0x3, 0x3,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x3F, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0xFF, 0xFF, 0xFF, 0xFF, 0x40, 0x0, 0x3F, 0xF, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0xFF, 0xFF, 0xFF, 0xFF,
];
const DFD_UASTC: [u8; 44] = [
    0x2C, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0, 0x28, 0x0, 0xA6, 0x1, 0x2, 0x0, 0x3, 0x3,
    0x0, 0x0, 0x10, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x7F, 0x4, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0xFF, 0xFF, 0xFF, 0xFF,
];
/// KHR_DF_TRANSFER_SRGB.
const TRANSFER_SRGB: u32 = 2;
/// The UASTC channel ids: RGB, RGBA.
const UASTC_RGB: u32 = 0;
const UASTC_RGBA: u32 = 3;

/// Move a one-image, one-level `.basis` file into a KTX2 container
/// (`basis_compressor::create_ktx2_file`, without its optional key/value
/// data).
pub fn basis_to_ktx2(basis: &[u8], mode: Ktx2Mode, alpha: bool) -> Result<Vec<u8>, String> {
    // basis_file_header (basisu_file_headers.h), packed little-endian.
    if le(basis, 0, 2)? != u64::from(u16::from_le_bytes(*b"sB")) {
        return Err("not a .basis file".into());
    }
    let total_slices = le(basis, 14, 3)? as usize;
    let endpoints = le(basis, 39, 2)? as u16;
    let (ep_at, ep_len) = (le(basis, 41, 4)? as usize, le(basis, 45, 3)? as usize);
    let selectors = le(basis, 48, 2)? as u16;
    let (sel_at, sel_len) = (le(basis, 50, 4)? as usize, le(basis, 54, 3)? as usize);
    let (tab_at, tab_len) = (le(basis, 57, 4)? as usize, le(basis, 61, 4)? as usize);
    let slices_at = le(basis, 65, 4)? as usize;
    let range = |at: usize, len: usize| {
        basis
            .get(at..at + len)
            .ok_or_else(|| "truncated .basis file".to_string())
    };

    // basis_slice_desc: 23 bytes each. One image: an RGB slice and, with
    // alpha (ETC1S), an alpha slice.
    let mut level = Vec::new();
    // ktx2_etc1s_image_desc: flags, rgb offset/length, alpha offset/length.
    let mut image = [0u32; 5];
    let (mut width, mut height) = (0, 0);
    for s in 0..total_slices {
        let d = slices_at + s * 23;
        if le(basis, d, 3)? != 0 || le(basis, d + 3, 1)? != 0 {
            return Err("only one image of one level converts".into());
        }
        let flags = le(basis, d + 4, 1)?;
        width = le(basis, d + 5, 2)? as u32;
        height = le(basis, d + 7, 2)? as u32;
        let (at, len) = (
            le(basis, d + 13, 4)? as usize,
            le(basis, d + 17, 4)? as usize,
        );
        let offset = level.len() as u32;
        level.extend_from_slice(range(at, len)?);
        if flags & 1 != 0 {
            image[3] = offset;
            image[4] = len as u32;
        } else {
            image[1] = offset;
            image[2] = len as u32;
        }
    }

    let mut sgd = Vec::new();
    if mode == Ktx2Mode::Etc1s {
        sgd.extend_from_slice(&endpoints.to_le_bytes());
        sgd.extend_from_slice(&selectors.to_le_bytes());
        for v in [ep_len, sel_len, tab_len, 0] {
            sgd.extend_from_slice(&(v as u32).to_le_bytes());
        }
        for v in image {
            sgd.extend_from_slice(&v.to_le_bytes());
        }
        sgd.extend_from_slice(range(ep_at, ep_len)?);
        sgd.extend_from_slice(range(sel_at, sel_len)?);
        sgd.extend_from_slice(range(tab_at, tab_len)?);
    }

    let mut dfd = match (mode, alpha) {
        (Ktx2Mode::Etc1s, false) => DFD_ETC1S.to_vec(),
        (Ktx2Mode::Etc1s, true) => DFD_ETC1S_ALPHA.to_vec(),
        (Ktx2Mode::Uastc, _) => DFD_UASTC.to_vec(),
    };
    let word = |dfd: &[u8], i: usize| u32::from_le_bytes(dfd[i * 4..i * 4 + 4].try_into().unwrap());
    let set = |dfd: &mut Vec<u8>, i: usize, v: u32| {
        dfd[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes())
    };
    // Transfer function (byte 2 of word 3).
    let w3 = (word(&dfd, 3) & !(0xFF << 16)) | (TRANSFER_SRGB << 16);
    set(&mut dfd, 3, w3);
    if mode == Ktx2Mode::Etc1s {
        // Supercompressed: bytesPlane0 = 0.
        let w5 = word(&dfd, 5) & !0xFF;
        set(&mut dfd, 5, w5);
    } else {
        let channel = if alpha { UASTC_RGBA } else { UASTC_RGB };
        let w7 = (word(&dfd, 7) & !(0xF << 24)) | (channel << 24);
        set(&mut dfd, 7, w7);
    }

    // Header (80 bytes), one level index entry (24), the DFD, the global
    // data (8-aligned), the level (16-aligned when not supercompressed).
    let mut out = vec![0u8; 80 + 24];
    let dfd_at = out.len();
    out.extend_from_slice(&dfd);
    let sgd_at = if sgd.is_empty() {
        0
    } else {
        out.resize(out.len().next_multiple_of(8), 0);
        let at = out.len();
        out.extend_from_slice(&sgd);
        at
    };
    if mode == Ktx2Mode::Uastc {
        out.resize(out.len().next_multiple_of(16), 0);
    }
    let level_at = out.len();
    out.extend_from_slice(&level);

    let scheme = match mode {
        Ktx2Mode::Etc1s => SS_BASISLZ,
        Ktx2Mode::Uastc => SS_NONE,
    };
    let mut header = KTX2_MAGIC.to_vec();
    // vkFormat (undefined), typeSize, width, height, depth, layers, faces,
    // levels, supercompression, DFD offset/length, KVD offset/length.
    for v in [
        0u32,
        1,
        width,
        height,
        0,
        0,
        1,
        1,
        scheme,
        dfd_at as u32,
        dfd.len() as u32,
        0,
        0,
    ] {
        header.extend_from_slice(&v.to_le_bytes());
    }
    header.extend_from_slice(&(sgd_at as u64).to_le_bytes());
    header.extend_from_slice(&(sgd.len() as u64).to_le_bytes());
    debug_assert_eq!(header.len(), 80);
    out[..80].copy_from_slice(&header);
    // Level 0: offset, length, uncompressed length (0 for BasisLZ).
    out[80..88].copy_from_slice(&(level_at as u64).to_le_bytes());
    out[88..96].copy_from_slice(&(level.len() as u64).to_le_bytes());
    let uncompressed = if scheme == SS_NONE { level.len() } else { 0 };
    out[96..104].copy_from_slice(&(uncompressed as u64).to_le_bytes());
    Ok(out)
}

/// What a KTX2 file's header says (the parts a loader checks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ktx2Info {
    pub width: u32,
    pub height: u32,
    pub mode: Ktx2Mode,
    /// The DFD carries an alpha channel.
    pub alpha: bool,
}

/// Read a KTX2 file's header and DFD: a one-level 2D Basis Universal
/// texture, or an error naming what isn't.
pub fn parse(file: &[u8]) -> Result<Ktx2Info, String> {
    if file.len() < 104 || file[..12] != KTX2_MAGIC {
        return Err("not a KTX2 file".into());
    }
    let u32_at = |at: usize| u32::from_le_bytes(file[at..at + 4].try_into().unwrap());
    let (width, height) = (u32_at(20), u32_at(24));
    if u32_at(12) != 0 || u32_at(28) != 0 || u32_at(36) != 1 || u32_at(40) != 1 {
        return Err("only one-level 2D Basis Universal KTX2 textures load".into());
    }
    let (dfd_at, dfd_len) = (u32_at(48) as usize, u32_at(52) as usize);
    let dfd = file
        .get(dfd_at..dfd_at + dfd_len)
        .filter(|d| d.len() >= 44)
        .ok_or("truncated KTX2 DFD")?;
    let model = dfd[12];
    let (mode, alpha) = match (u32_at(44), model) {
        // KHR_DF_MODEL_ETC1S = 163: a second sample is the alpha slice.
        (SS_BASISLZ, 0xA3) => (Ktx2Mode::Etc1s, dfd.len() >= 60),
        // KHR_DF_MODEL_UASTC = 166: the channel id says RGBA.
        (SS_NONE, 0xA6) => (Ktx2Mode::Uastc, (dfd[31] & 0xF) as u32 == UASTC_RGBA),
        (s, m) => return Err(format!("not Basis Universal (scheme {s}, model {m})")),
    };
    let level_at = u64::from_le_bytes(file[80..88].try_into().unwrap()) as usize;
    let level_len = u64::from_le_bytes(file[88..96].try_into().unwrap()) as usize;
    if level_at
        .checked_add(level_len)
        .is_none_or(|end| end > file.len())
    {
        return Err("truncated KTX2 level".into());
    }
    Ok(Ktx2Info {
        width,
        height,
        mode,
        alpha,
    })
}

/// A KTX2 file of [`encode`]'s back to its `.basis` form, for the native
/// transcoder (tests and tools: the container round trip). One image, one
/// level.
pub fn ktx2_to_basis(file: &[u8]) -> Result<Vec<u8>, String> {
    let info = parse(file)?;
    let u64_at = |at: usize| u64::from_le_bytes(file[at..at + 8].try_into().unwrap()) as usize;
    let (level_at, level_len) = (u64_at(80), u64_at(88));
    let level = &file[level_at..level_at + level_len];
    let (sgd_at, sgd_len) = (u64_at(64), u64_at(72));
    let sgd = &file[sgd_at..sgd_at + sgd_len];
    let mut slices: Vec<(u8, &[u8])> = Vec::new();
    let mut codebooks: [&[u8]; 3] = [&[], &[], &[]];
    let (mut endpoints, mut selectors) = (0u16, 0u16);
    match info.mode {
        Ktx2Mode::Uastc => slices.push((0, level)),
        Ktx2Mode::Etc1s => {
            let r = |at: usize| u32::from_le_bytes(sgd[at..at + 4].try_into().unwrap()) as usize;
            endpoints = u16::from_le_bytes([sgd[0], sgd[1]]);
            selectors = u16::from_le_bytes([sgd[2], sgd[3]]);
            let (ep, sel, tab) = (r(4), r(8), r(12));
            let desc = 20;
            let (rgb_at, rgb_len, a_at, a_len) =
                (r(desc + 4), r(desc + 8), r(desc + 12), r(desc + 16));
            let books = desc + 20;
            codebooks = [
                &sgd[books..books + ep],
                &sgd[books + ep..books + ep + sel],
                &sgd[books + ep + sel..books + ep + sel + tab],
            ];
            slices.push((0, &level[rgb_at..rgb_at + rgb_len]));
            if a_len > 0 {
                slices.push((1, &level[a_at..a_at + a_len]));
            }
        }
    }
    // A .basis file: header (77 bytes), slice descs, codebooks, slices.
    let header_len = 77;
    let descs_at = header_len;
    let mut data_at = descs_at + 23 * slices.len();
    let mut book_at = [0usize; 3];
    for (i, b) in codebooks.iter().enumerate() {
        book_at[i] = data_at;
        data_at += b.len();
    }
    let mut out = vec![0u8; data_at];
    let put = |out: &mut Vec<u8>, at: usize, v: u64, n: usize| {
        for i in 0..n {
            out[at + i] = (v >> (8 * i)) as u8;
        }
    };
    put(&mut out, 0, u16::from_le_bytes(*b"sB") as u64, 2);
    put(&mut out, 2, 0x13, 2);
    put(&mut out, 4, header_len as u64, 2);
    put(&mut out, 14, slices.len() as u64, 3);
    put(&mut out, 17, 1, 3);
    put(
        &mut out,
        20,
        match info.mode {
            Ktx2Mode::Etc1s => 0,
            Ktx2Mode::Uastc => 1,
        },
        1,
    );
    // Header flags (basisu_file_headers.h): ETC1S = 1, HasAlphaSlices = 4,
    // SRGB = 16.
    let etc1s = if info.mode == Ktx2Mode::Etc1s { 1 } else { 0 };
    let flags = etc1s | if info.alpha { 4 } else { 0 } | 16;
    put(&mut out, 21, flags, 2);
    put(&mut out, 39, endpoints as u64, 2);
    put(&mut out, 41, book_at[0] as u64, 4);
    put(&mut out, 45, codebooks[0].len() as u64, 3);
    put(&mut out, 48, selectors as u64, 2);
    put(&mut out, 50, book_at[1] as u64, 4);
    put(&mut out, 54, codebooks[1].len() as u64, 3);
    put(&mut out, 57, book_at[2] as u64, 4);
    put(&mut out, 61, codebooks[2].len() as u64, 4);
    put(&mut out, 65, descs_at as u64, 4);
    for (i, b) in codebooks.iter().enumerate() {
        out[book_at[i]..book_at[i] + b.len()].copy_from_slice(b);
    }
    for (i, (slice_flags, bytes)) in slices.iter().enumerate() {
        let d = descs_at + 23 * i;
        let at = out.len();
        out.extend_from_slice(bytes);
        let alpha_flag = if *slice_flags == 1 { 1 } else { 0 };
        let iframe = 2;
        put(&mut out, d + 4, alpha_flag | iframe, 1);
        put(&mut out, d + 5, info.width as u64, 2);
        put(&mut out, d + 7, info.height as u64, 2);
        put(&mut out, d + 9, info.width.div_ceil(4) as u64, 2);
        put(&mut out, d + 11, info.height.div_ceil(4) as u64, 2);
        put(&mut out, d + 13, at as u64, 4);
        put(&mut out, d + 17, bytes.len() as u64, 4);
    }
    let data_size = out.len() - header_len;
    put(&mut out, 8, data_size as u64, 4);
    Ok(out)
}

/// A KTX2 file of [`encode`]'s decoded to RGBA8 natively (through
/// [`ktx2_to_basis`] and Binomial's transcoder).
pub fn decode(file: &[u8]) -> Result<Vec<u8>, String> {
    let info = parse(file)?;
    decode_basis(&ktx2_to_basis(file)?, info.width, info.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient with a checker of alpha (or opaque).
    fn image(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let a = if alpha && (x / 8 + y / 8) % 2 == 1 {
                    96
                } else {
                    255
                };
                [
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x ^ y) * 4) as u8,
                    a,
                ]
            })
            .collect()
    }

    #[test]
    fn both_modes_round_trip_through_the_container() {
        for mode in Ktx2Mode::ALL {
            for alpha in [false, true] {
                let rgba = image(64, 32, alpha);
                let e = encode(mode, 64, 32, &rgba).unwrap();
                let info = parse(&e.ktx2).unwrap();
                assert_eq!(
                    info,
                    Ktx2Info {
                        width: 64,
                        height: 32,
                        mode,
                        alpha,
                    },
                    "{mode:?} alpha={alpha}"
                );
                // The container holds exactly the encoder's data: decoding
                // it gives the encoder's own decode.
                assert_eq!(decode(&e.ktx2).unwrap(), e.decoded, "{mode:?} {alpha}");
                let db = super::super::psnr(&rgba, &e.decoded);
                let floor = match mode {
                    Ktx2Mode::Etc1s => 30.0,
                    Ktx2Mode::Uastc => 38.0,
                };
                assert!(db > floor, "{mode:?} alpha={alpha}: {db:.1} dB");
            }
        }
    }

    #[test]
    fn encoding_is_deterministic_and_etc1s_is_the_smaller_download() {
        let rgba = image(64, 64, false);
        let a = encode(Ktx2Mode::Etc1s, 64, 64, &rgba).unwrap();
        let b = encode(Ktx2Mode::Etc1s, 64, 64, &rgba).unwrap();
        assert_eq!(a.ktx2, b.ktx2);
        let u = encode(Ktx2Mode::Uastc, 64, 64, &rgba).unwrap();
        assert!(
            a.ktx2.len() < u.ktx2.len(),
            "{} vs {}",
            a.ktx2.len(),
            u.ktx2.len()
        );
        assert_eq!(Ktx2Mode::Etc1s.gpu_bytes(64, 64, false), 64 * 64 / 2);
        assert_eq!(Ktx2Mode::Uastc.gpu_bytes(64, 64, false), 64 * 64);
    }

    #[test]
    fn parse_rejects_other_files() {
        assert!(parse(b"\x89PNG....").is_err());
        let mut f = encode(Ktx2Mode::Uastc, 8, 8, &image(8, 8, false))
            .unwrap()
            .ktx2;
        f[40] = 2; // two levels
        assert!(parse(&f).is_err());
        let truncated = &encode(Ktx2Mode::Etc1s, 8, 8, &image(8, 8, false))
            .unwrap()
            .ktx2[..110];
        assert!(parse(truncated).is_err());
    }
}
