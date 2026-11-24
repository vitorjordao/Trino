//! [`Platform::save_image`](trino_core::Platform::save_image) on the PC
//! (Bongo-Kart #183): the game's RGBA picture as `<dir>/<name>.png`, written
//! atomically (a temporary file renamed over the old one).

use std::path::Path;

use trino_core::image::image_name_ok;
use trino_core::{RgbaImage, SaveError};

/// The image's PNG bytes (8-bit RGBA). `None` for an image whose sizes
/// don't agree.
pub fn encode_png(image: &RgbaImage) -> Option<Vec<u8>> {
    if !image.is_valid() {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, image.width, image.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(image.pixels).ok()?;
    }
    Some(out)
}

/// Write `<dir>/<name>.png`. A bad name or image is `Io` (nothing is
/// written), like a full disk.
pub fn write_png(dir: &Path, name: &str, image: &RgbaImage) -> Result<(), SaveError> {
    if !image_name_ok(name) {
        return Err(SaveError::Io);
    }
    let bytes = encode_png(image).ok_or(SaveError::Io)?;
    std::fs::create_dir_all(dir).map_err(|_| SaveError::Io)?;
    let file = dir.join(format!("{name}.png"));
    let tmp = dir.join(format!("{name}.png.tmp"));
    std::fs::write(&tmp, &bytes).map_err(|_| SaveError::Io)?;
    std::fs::rename(&tmp, &file).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        SaveError::Io
    })
}
