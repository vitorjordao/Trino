//! An RGBA picture a game hands the platform to keep
//! ([`Platform::save_image`](crate::Platform::save_image), Bongo-Kart #183,
//! ADR 0045 §4): a photo-mode shot, a race's result card, a ghost's
//! thumbnail.
//!
//! The pixels are the game's (8-bit RGBA, rows top to bottom, no padding):
//! a frame it read back, a picture it composed. Hosts write them as PNG: the
//! PC into `<save folder>/images/<name>.png`, the Web as a download. The
//! consoles keep nothing (`SaveError::Unsupported`).

/// An image as 8-bit RGBA texels, row-major from the top-left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RgbaImage<'a> {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: &'a [u8],
}

/// The longest image side a host writes.
pub const IMAGE_MAX_SIDE: u32 = 8192;
/// The longest image name ([`image_name_ok`]).
pub const IMAGE_NAME_MAX: usize = 64;

impl<'a> RgbaImage<'a> {
    /// `None` when `pixels` isn't `width * height * 4` bytes, a side is 0 or
    /// past [`IMAGE_MAX_SIDE`].
    pub fn new(width: u32, height: u32, pixels: &'a [u8]) -> Option<RgbaImage<'a>> {
        let img = RgbaImage {
            width,
            height,
            pixels,
        };
        img.is_valid().then_some(img)
    }

    /// The sizes agree and fit (a struct built by hand is checked again by
    /// every host).
    pub fn is_valid(&self) -> bool {
        let sides = (1..=IMAGE_MAX_SIDE).contains(&self.width)
            && (1..=IMAGE_MAX_SIDE).contains(&self.height);
        sides && (self.width as u64 * self.height as u64 * 4) == self.pixels.len() as u64
    }

    /// The texel at `(x, y)`, or `None` outside.
    pub fn texel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.pixels.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }
}

/// A name a host can turn into a file name on every OS and a download:
/// 1..=[`IMAGE_NAME_MAX`] ASCII letters, digits, `-` and `_` (no
/// extension, no folders).
pub fn image_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= IMAGE_NAME_MAX
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_must_agree_and_names_stay_plain() {
        let px = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let img = RgbaImage::new(2, 1, &px).unwrap();
        assert_eq!(img.texel(1, 0), Some([5, 6, 7, 8]));
        assert_eq!(img.texel(2, 0), None);
        assert!(RgbaImage::new(1, 1, &px).is_none());
        assert!(RgbaImage::new(0, 2, &[]).is_none());
        assert!(RgbaImage::new(IMAGE_MAX_SIDE + 1, 1, &px).is_none());
        let wrapped = RgbaImage {
            width: u32::MAX,
            height: u32::MAX,
            pixels: &px,
        };
        assert!(!wrapped.is_valid());
        assert!(image_name_ok("race-03_result"));
        for bad in ["", "a/b", "..", "x.png", "é", &"a".repeat(65)] {
            assert!(!image_name_ok(bad), "{bad}");
        }
    }
}
