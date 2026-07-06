//! ASCII tilemap: the level format for 2D games.
//!
//! Levels are plain rectangular text (one byte per tile) so they work
//! identically on every platform with zero allocation — the map borrows the
//! level string, which games embed with `include_str!` or load from the
//! platform's filesystem.
//!
//! The engine assigns meaning to a few bytes ([`Tile`]); games are free to
//! interpret the rest (spawn points, pickups, ...) via [`Tilemap::cells`].

use crate::math::Vec2;

/// Side of a square tile in pixels. One size for the whole engine keeps
/// bake formats and collision in lockstep across platforms.
pub const TILE_SIZE: f32 = 16.0;

/// Byte values with engine-defined meaning.
pub mod tile {
    /// Empty space.
    pub const EMPTY: u8 = b'.';
    /// Solid ground (drawn with the `sprites/ground` handle by convention).
    pub const GROUND: u8 = b'#';
    /// Solid brick (drawn with `sprites/brick` by convention).
    pub const BRICK: u8 = b'B';
    /// Collectible coin (not solid).
    pub const COIN: u8 = b'C';
    /// Level goal (not solid).
    pub const FLAG: u8 = b'F';
    /// Player spawn point (not solid, not drawn).
    pub const SPAWN: u8 = b'P';
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TilemapError {
    Empty,
    /// Line `row` has `found` tiles; the first line set `expected`.
    NotRectangular {
        row: usize,
        expected: usize,
        found: usize,
    },
}

/// A parsed (borrowed) tilemap. Zero-copy: indexes straight into the level
/// string.
#[derive(Clone, Copy, Debug)]
pub struct Tilemap<'a> {
    lines: &'a [u8],
    /// Bytes per row in `lines`, including the `\n`.
    stride: usize,
    pub width: usize,
    pub height: usize,
}

impl<'a> Tilemap<'a> {
    /// Parse a rectangular ASCII level. Trailing newline optional. CRLF
    /// levels parse too, but only when every line uses it — `stride` must
    /// be uniform (git checkouts with `core.autocrlf` produce exactly that).
    pub fn parse(level: &'a str) -> Result<Self, TilemapError> {
        let bytes = level.as_bytes();
        let mut width = 0usize;
        let mut height = 0usize;
        let mut current = 0usize;
        // Bytes per row including the terminator(s): width+1 for LF levels,
        // width+2 for CRLF. Uniform line endings make it constant.
        let mut stride = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                if width == 0 {
                    width = current;
                    stride = i + 1;
                } else if current != width {
                    return Err(TilemapError::NotRectangular {
                        row: height,
                        expected: width,
                        found: current,
                    });
                }
                height += 1;
                current = 0;
            } else if b == b'\r' {
                // Tolerated (Windows checkouts); not counted as a tile.
            } else {
                current += 1;
            }
        }
        // Last line without a trailing newline.
        if current > 0 {
            if width == 0 {
                width = current;
                stride = width + 1;
            } else if current != width {
                return Err(TilemapError::NotRectangular {
                    row: height,
                    expected: width,
                    found: current,
                });
            }
            height += 1;
        }
        if width == 0 || height == 0 {
            return Err(TilemapError::Empty);
        }
        Ok(Tilemap {
            lines: bytes,
            stride,
            width,
            height,
        })
    }

    /// Tile at (tx, ty). Outside the map: [`tile::EMPTY`] — levels draw
    /// their own borders; falling off the map is the game's business.
    #[inline]
    pub fn tile(&self, tx: i32, ty: i32) -> u8 {
        if tx < 0 || ty < 0 || tx as usize >= self.width || ty as usize >= self.height {
            return tile::EMPTY;
        }
        self.lines[ty as usize * self.stride + tx as usize]
    }

    /// Solid for collision purposes.
    #[inline]
    pub fn is_solid(&self, tx: i32, ty: i32) -> bool {
        matches!(self.tile(tx, ty), tile::GROUND | tile::BRICK)
    }

    /// Iterate every cell as `(tx, ty, tile_byte)`.
    pub fn cells(&self) -> impl Iterator<Item = (usize, usize, u8)> + '_ {
        let map = *self;
        (0..self.height).flat_map(move |ty| {
            (0..map.width).map(move |tx| (tx, ty, map.lines[ty * map.stride + tx]))
        })
    }

    /// Top-left pixel position of a cell.
    #[inline]
    pub fn cell_pos(&self, tx: usize, ty: usize) -> Vec2 {
        Vec2::new(tx as f32 * TILE_SIZE, ty as f32 * TILE_SIZE)
    }

    #[inline]
    pub fn pixel_size(&self) -> Vec2 {
        Vec2::new(
            self.width as f32 * TILE_SIZE,
            self.height as f32 * TILE_SIZE,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVEL: &str = "\
.....\n\
..C..\n\
.P.F.\n\
#####";

    #[test]
    fn parses_dimensions_and_tiles() {
        let map = Tilemap::parse(LEVEL).unwrap();
        assert_eq!((map.width, map.height), (5, 4));
        assert_eq!(map.tile(0, 0), tile::EMPTY);
        assert_eq!(map.tile(2, 1), tile::COIN);
        assert_eq!(map.tile(1, 2), tile::SPAWN);
        assert_eq!(map.tile(3, 2), tile::FLAG);
        assert!(map.is_solid(0, 3));
        assert!(!map.is_solid(2, 1));
    }

    #[test]
    fn outside_is_empty() {
        let map = Tilemap::parse(LEVEL).unwrap();
        assert_eq!(map.tile(-1, 0), tile::EMPTY);
        assert_eq!(map.tile(0, 99), tile::EMPTY);
        assert!(!map.is_solid(-1, 3));
    }

    #[test]
    fn trailing_newline_is_optional() {
        let with = Tilemap::parse("..\n##\n").unwrap();
        let without = Tilemap::parse("..\n##").unwrap();
        assert_eq!((with.width, with.height), (2, 2));
        assert_eq!((without.width, without.height), (2, 2));
    }

    #[test]
    fn crlf_levels_parse_identically() {
        // Windows git checkouts (core.autocrlf) hand the game CRLF levels.
        let lf = Tilemap::parse(".C.\n###\n").unwrap();
        let crlf = Tilemap::parse(".C.\r\n###\r\n").unwrap();
        assert_eq!((crlf.width, crlf.height), (lf.width, lf.height));
        for ty in 0..lf.height as i32 {
            for tx in 0..lf.width as i32 {
                assert_eq!(crlf.tile(tx, ty), lf.tile(tx, ty), "({tx},{ty})");
            }
        }
    }

    #[test]
    fn rejects_ragged_lines() {
        assert!(matches!(
            Tilemap::parse("...\n..\n"),
            Err(TilemapError::NotRectangular {
                row: 1,
                expected: 3,
                found: 2
            })
        ));
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(Tilemap::parse(""), Err(TilemapError::Empty)));
    }

    #[test]
    fn cells_iterates_row_major() {
        let map = Tilemap::parse("AB\nCD").unwrap();
        let all: Vec<_> = map.cells().collect();
        assert_eq!(
            all,
            [(0, 0, b'A'), (1, 0, b'B'), (0, 1, b'C'), (1, 1, b'D')]
        );
    }
}
