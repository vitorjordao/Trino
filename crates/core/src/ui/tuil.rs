//! `TUIL`: a screen baked for the runtime (`ui/<name>` blobs on every
//! target). Zero-copy: [`UiScreen::parse`] checks the header, the tree and
//! the string pool once, then nodes decode on demand.
//!
//! Layout (little-endian), version 2 (version 1 still loads: its records
//! are 94 bytes and have no `cvd_icon`):
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `TUIL` |
//! | 4 | 2 | version (2) |
//! | 6 | 2 | flags: bit 0 fractional scaling, bit 1 wrap focus |
//! | 8 | 4 | reference width, height (u16 each) |
//! | 12 | 2 | node count |
//! | 14 | 2 | record size (≥ 102, ≥ 94 in version 1; longer records are read by their prefix) |
//! | 16 | 2 | first focus node (`0xffff` = none) |
//! | 18 | 2 | reserved |
//! | 20 | 8 | name, on_back (string refs) |
//! | 28 | 4 | records offset |
//! | 32 | 8 | string pool offset, length |
//! | 40 | 4 | theme font (`FontId`) |
//! | 44 | 4 | theme text scale (u8), 3 reserved |
//! | 48 | 36 | theme colors (RGBA): text, dim, panel, button, button text, focus, focus text, accent, pressed |
//!
//! A string ref is `u16 offset, u16 length` into the pool (UTF-8). Each
//! node record is [`RECORD`] bytes, in pre-order: kind, flags (bit 0 has
//! color, 1 bottom screen, 2 bleed, 3 has font), parent, end, anchor,
//! visible_on, offset, size (kind + value per axis), min, max, padding,
//! text scale, align, valign, overflow, color, font, four focus overrides,
//! seven string refs (id, text, key, bind, action, visible_if, enabled_if),
//! a 16-byte payload per kind (see [`encode_record`]) and, from version 2,
//! the `cvd_icon` (sprite id, width and height as u16; sprite 0 = none).

use super::UiError;
use super::node::{
    Align, Anchor, CvdIcon, Dir, ImageFit, Justify, NodeKind, Nodes, Overflow, Scaling, Size,
    Targets, Theme, UiNode, UiScreen, VAlign,
};
use crate::math::Color;
use crate::render::{FontId, ModelId, Screen, SpriteId, TextAlign};

pub const MAGIC: &[u8; 4] = b"TUIL";
pub const VERSION: u16 = 2;
/// Header bytes.
pub const HEADER: usize = 84;
/// Node record bytes (version 2).
pub const RECORD: usize = 102;
/// Node record bytes in version 1 (no `cvd_icon`).
pub const RECORD_V1: usize = 94;

/// A baked screen's node records and string pool ([`Nodes::Baked`]).
#[derive(Clone, Copy, Debug)]
pub struct Records<'a> {
    records: &'a [u8],
    size: usize,
    count: usize,
    pool: &'a str,
    /// Whether the records carry a `cvd_icon` (version 2).
    icons: bool,
}

impl<'a> Records<'a> {
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn str(&self, b: &[u8], at: usize) -> &'a str {
        let off = rd16(b, at) as usize;
        let len = rd16(b, at + 2) as usize;
        self.pool.get(off..off + len).unwrap_or("")
    }

    /// Node `i` (panics past [`len`](Self::len)).
    pub fn node(&self, i: usize) -> UiNode<'a> {
        assert!(i < self.count, "TUIL node {i} of {}", self.count);
        let b = &self.records[i * self.size..i * self.size + self.size];
        let flags = b[1];
        let s = |at| self.str(b, at);
        UiNode {
            kind: decode_kind(b),
            id: s(50),
            parent: rd16(b, 2),
            end: rd16(b, 4),
            screen: if flags & 2 != 0 {
                Screen::Bottom
            } else {
                Screen::Top
            },
            anchor: *Anchor::ALL.get(b[6] as usize).unwrap_or(&Anchor::TopLeft),
            offset: (rd16(b, 8) as i16, rd16(b, 10) as i16),
            size: (size_of(b[12], rd16(b, 14)), size_of(b[13], rd16(b, 16))),
            min: (rd16(b, 18), rd16(b, 20)),
            max: (rd16(b, 22), rd16(b, 24)),
            padding: [b[26], b[27], b[28], b[29]],
            bleed: flags & 4 != 0,
            visible_on: Targets(b[7]),
            color: (flags & 1 != 0).then(|| rd_color(b, 34)),
            font: (flags & 8 != 0).then(|| FontId(rd32(b, 38))),
            text_scale: b[30],
            align: match b[31] {
                1 => TextAlign::Center,
                2 => TextAlign::Right,
                _ => TextAlign::Left,
            },
            valign: match b[32] {
                0 => VAlign::Top,
                2 => VAlign::Bottom,
                _ => VAlign::Middle,
            },
            overflow: match b[33] {
                1 => Overflow::Clip,
                2 => Overflow::Ellipsis,
                3 => Overflow::Wrap,
                _ => Overflow::Visible,
            },
            text: s(54),
            key: s(58),
            bind: s(62),
            action: s(66),
            visible_if: s(70),
            enabled_if: s(74),
            nav: [rd16(b, 42), rd16(b, 44), rd16(b, 46), rd16(b, 48)],
            cvd_icon: if self.icons && rd32(b, 94) != 0 {
                Some(CvdIcon {
                    sprite: SpriteId(rd32(b, 94)),
                    size: (rd16(b, 98), rd16(b, 100)),
                })
            } else {
                None
            },
        }
    }
}

impl<'a> UiScreen<'a> {
    /// Read a baked `TUIL` blob.
    pub fn parse(bytes: &'a [u8]) -> Result<UiScreen<'a>, UiError> {
        if bytes.len() < HEADER {
            return Err(UiError::Truncated);
        }
        if &bytes[0..4] != MAGIC {
            return Err(UiError::BadMagic);
        }
        let version = rd16(bytes, 4);
        if version != VERSION && version != 1 {
            return Err(UiError::UnsupportedVersion(version));
        }
        let count = rd16(bytes, 12) as usize;
        let size = rd16(bytes, 14) as usize;
        let at = rd32(bytes, 28) as usize;
        let pool_at = rd32(bytes, 32) as usize;
        let pool_len = rd32(bytes, 36) as usize;
        if size < if version == 1 { RECORD_V1 } else { RECORD } {
            return Err(UiError::Truncated);
        }
        let records = at
            .checked_add(count * size)
            .and_then(|end| bytes.get(at..end))
            .ok_or(UiError::Truncated)?;
        let pool = pool_at
            .checked_add(pool_len)
            .and_then(|end| bytes.get(pool_at..end))
            .ok_or(UiError::Truncated)?;
        let pool = core::str::from_utf8(pool).map_err(|_| UiError::BadString)?;
        let records = Records {
            records,
            size,
            count,
            pool,
            icons: version >= 2,
        };
        // Every string ref must land on characters of the pool.
        for i in 0..count {
            let b = &records.records[i * size..];
            for k in 0..7 {
                let off = rd16(b, 50 + 4 * k) as usize;
                let len = rd16(b, 52 + 4 * k) as usize;
                if pool.get(off..off + len).is_none() {
                    return Err(UiError::BadString);
                }
            }
        }
        let header_str = |at: usize| {
            let off = rd16(bytes, at) as usize;
            let len = rd16(bytes, at + 2) as usize;
            pool.get(off..off + len).ok_or(UiError::BadString)
        };
        let flags = rd16(bytes, 6);
        let c = |k: usize| rd_color(bytes, 48 + 4 * k);
        let screen = UiScreen {
            name: header_str(20)?,
            reference: (rd16(bytes, 8).max(1), rd16(bytes, 10).max(1)),
            scaling: if flags & 1 != 0 {
                Scaling::Fractional
            } else {
                Scaling::Integer
            },
            wrap: flags & 2 != 0,
            focus: rd16(bytes, 16),
            on_back: header_str(24)?,
            theme: Theme {
                font: FontId(rd32(bytes, 40)),
                text_scale: bytes[44].max(1),
                text: c(0),
                dim: c(1),
                panel: c(2),
                button: c(3),
                button_text: c(4),
                focus: c(5),
                focus_text: c(6),
                accent: c(7),
                pressed: c(8),
            },
            nodes: Nodes::Baked(records),
        };
        screen.check_tree()?;
        Ok(screen)
    }
}

/// The header of a screen with `count` nodes whose records start at
/// [`HEADER`] and whose pool follows them. `str_ref` places a string in
/// the pool and returns its `(offset, length)`. Host tools (the bake)
/// assemble header + records + pool; this keeps the layout in one file.
pub fn encode_header(
    screen: &UiScreen<'_>,
    count: usize,
    pool_len: usize,
    str_ref: &mut dyn FnMut(&str) -> (u16, u16),
) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0..4].copy_from_slice(MAGIC);
    wr16(&mut h, 4, VERSION);
    let flags = u16::from(screen.scaling == Scaling::Fractional) | (u16::from(screen.wrap) << 1);
    wr16(&mut h, 6, flags);
    wr16(&mut h, 8, screen.reference.0);
    wr16(&mut h, 10, screen.reference.1);
    wr16(&mut h, 12, count as u16);
    wr16(&mut h, 14, RECORD as u16);
    wr16(&mut h, 16, screen.focus);
    let (o, l) = str_ref(screen.name);
    wr16(&mut h, 20, o);
    wr16(&mut h, 22, l);
    let (o, l) = str_ref(screen.on_back);
    wr16(&mut h, 24, o);
    wr16(&mut h, 26, l);
    wr32(&mut h, 28, HEADER as u32);
    wr32(&mut h, 32, (HEADER + count * RECORD) as u32);
    wr32(&mut h, 36, pool_len as u32);
    let t = &screen.theme;
    wr32(&mut h, 40, t.font.0);
    h[44] = t.text_scale;
    for (k, c) in [
        t.text,
        t.dim,
        t.panel,
        t.button,
        t.button_text,
        t.focus,
        t.focus_text,
        t.accent,
        t.pressed,
    ]
    .into_iter()
    .enumerate()
    {
        wr_color(&mut h, 48 + 4 * k, c);
    }
    h
}

/// One node's record (see the module doc); `str_ref` as in
/// [`encode_header`].
pub fn encode_record(
    node: &UiNode<'_>,
    str_ref: &mut dyn FnMut(&str) -> (u16, u16),
) -> [u8; RECORD] {
    let mut b = [0u8; RECORD];
    let (code, payload) = encode_kind(&node.kind);
    b[0] = code;
    b[1] = u8::from(node.color.is_some())
        | (u8::from(node.screen == Screen::Bottom) << 1)
        | (u8::from(node.bleed) << 2)
        | (u8::from(node.font.is_some()) << 3);
    wr16(&mut b, 2, node.parent);
    wr16(&mut b, 4, node.end);
    b[6] = Anchor::ALL
        .iter()
        .position(|a| *a == node.anchor)
        .unwrap_or(0) as u8;
    b[7] = node.visible_on.0;
    wr16(&mut b, 8, node.offset.0 as u16);
    wr16(&mut b, 10, node.offset.1 as u16);
    let (kx, vx) = size_code(node.size.0);
    let (ky, vy) = size_code(node.size.1);
    b[12] = kx;
    b[13] = ky;
    wr16(&mut b, 14, vx);
    wr16(&mut b, 16, vy);
    wr16(&mut b, 18, node.min.0);
    wr16(&mut b, 20, node.min.1);
    wr16(&mut b, 22, node.max.0);
    wr16(&mut b, 24, node.max.1);
    b[26..30].copy_from_slice(&node.padding);
    b[30] = node.text_scale;
    b[31] = match node.align {
        TextAlign::Left => 0,
        TextAlign::Center => 1,
        TextAlign::Right => 2,
    };
    b[32] = match node.valign {
        VAlign::Top => 0,
        VAlign::Middle => 1,
        VAlign::Bottom => 2,
    };
    b[33] = match node.overflow {
        Overflow::Visible => 0,
        Overflow::Clip => 1,
        Overflow::Ellipsis => 2,
        Overflow::Wrap => 3,
    };
    wr_color(&mut b, 34, node.color.unwrap_or(Color::TRANSPARENT));
    wr32(&mut b, 38, node.font.map_or(0, |f| f.0));
    for (k, t) in node.nav.iter().enumerate() {
        wr16(&mut b, 42 + 2 * k, *t);
    }
    for (k, s) in [
        node.id,
        node.text,
        node.key,
        node.bind,
        node.action,
        node.visible_if,
        node.enabled_if,
    ]
    .into_iter()
    .enumerate()
    {
        let (o, l) = str_ref(s);
        wr16(&mut b, 50 + 4 * k, o);
        wr16(&mut b, 52 + 4 * k, l);
    }
    b[78..94].copy_from_slice(&payload);
    if let Some(icon) = node.cvd_icon {
        wr32(&mut b, 94, icon.sprite.0);
        wr16(&mut b, 98, icon.size.0);
        wr16(&mut b, 100, icon.size.1);
    }
    b
}

fn encode_kind(kind: &NodeKind) -> (u8, [u8; 16]) {
    let mut p = [0u8; 16];
    let code = match *kind {
        NodeKind::Overlay => 0,
        NodeKind::Stack {
            dir,
            gap,
            align,
            justify,
        } => {
            p[0] = dir as u8;
            p[1] = align as u8;
            p[2] = justify as u8;
            wr16(&mut p, 4, gap as u16);
            1
        }
        NodeKind::Grid { cols, gap } => {
            p[0] = cols;
            wr16(&mut p, 2, gap.0 as u16);
            wr16(&mut p, 4, gap.1 as u16);
            2
        }
        NodeKind::Scroll { dir, gap } => {
            p[0] = dir as u8;
            wr16(&mut p, 2, gap as u16);
            3
        }
        NodeKind::List { dir, gap, max } => {
            p[0] = dir as u8;
            wr16(&mut p, 2, gap as u16);
            wr16(&mut p, 4, max);
            4
        }
        NodeKind::Text => 5,
        NodeKind::Image {
            sprite,
            src,
            nine,
            fit,
        } => {
            wr32(&mut p, 0, sprite.0);
            wr16(&mut p, 4, src.0);
            wr16(&mut p, 6, src.1);
            p[8..12].copy_from_slice(&nine);
            p[12] = fit as u8;
            6
        }
        NodeKind::Rect {
            border,
            border_color,
        } => {
            p[0] = border;
            wr_color(&mut p, 4, border_color);
            7
        }
        NodeKind::Button => 8,
        NodeKind::Toggle => 9,
        NodeKind::Slider { min, max, step } => {
            wr32(&mut p, 0, min as u32);
            wr32(&mut p, 4, max as u32);
            wr32(&mut p, 8, step as u32);
            10
        }
        NodeKind::Model3d {
            model,
            yaw,
            pitch,
            spin,
            extent,
            lift,
        } => {
            wr32(&mut p, 0, model.0);
            wr16(&mut p, 4, yaw as u16);
            wr16(&mut p, 6, pitch as u16);
            wr16(&mut p, 8, spin as u16);
            wr16(&mut p, 10, extent);
            wr16(&mut p, 12, lift as u16);
            11
        }
    };
    (code, p)
}

fn decode_kind(b: &[u8]) -> NodeKind {
    let p = &b[78..94];
    let dir = |v: u8| {
        if v == Dir::Horizontal as u8 {
            Dir::Horizontal
        } else {
            Dir::Vertical
        }
    };
    let i16_at = |at| rd16(p, at) as i16;
    match b[0] {
        1 => NodeKind::Stack {
            dir: dir(p[0]),
            align: match p[1] {
                1 => Align::Center,
                2 => Align::End,
                3 => Align::Stretch,
                _ => Align::Start,
            },
            justify: match p[2] {
                1 => Justify::Center,
                2 => Justify::End,
                3 => Justify::SpaceBetween,
                _ => Justify::Start,
            },
            gap: i16_at(4),
        },
        2 => NodeKind::Grid {
            cols: p[0].max(1),
            gap: (i16_at(2), i16_at(4)),
        },
        3 => NodeKind::Scroll {
            dir: dir(p[0]),
            gap: i16_at(2),
        },
        4 => NodeKind::List {
            dir: dir(p[0]),
            gap: i16_at(2),
            max: rd16(p, 4),
        },
        5 => NodeKind::Text,
        6 => NodeKind::Image {
            sprite: SpriteId(rd32(p, 0)),
            src: (rd16(p, 4), rd16(p, 6)),
            nine: [p[8], p[9], p[10], p[11]],
            fit: match p[12] {
                1 => ImageFit::Contain,
                2 => ImageFit::Integer,
                _ => ImageFit::Stretch,
            },
        },
        7 => NodeKind::Rect {
            border: p[0],
            border_color: rd_color(p, 4),
        },
        8 => NodeKind::Button,
        9 => NodeKind::Toggle,
        10 => NodeKind::Slider {
            min: rd32(p, 0) as i32,
            max: rd32(p, 4) as i32,
            step: rd32(p, 8) as i32,
        },
        11 => NodeKind::Model3d {
            model: ModelId(rd32(p, 0)),
            yaw: i16_at(4),
            pitch: i16_at(6),
            spin: i16_at(8),
            extent: rd16(p, 10),
            lift: i16_at(12),
        },
        _ => NodeKind::Overlay,
    }
}

fn size_code(s: Size) -> (u8, u16) {
    match s {
        Size::Fit => (0, 0),
        Size::Px(v) => (1, v),
        Size::Percent(v) => (2, v),
        Size::Fill => (3, 0),
    }
}

fn size_of(code: u8, v: u16) -> Size {
    match code {
        1 => Size::Px(v),
        2 => Size::Percent(v),
        3 => Size::Fill,
        _ => Size::Fit,
    }
}

fn rd16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn rd_color(b: &[u8], at: usize) -> Color {
    Color::rgba(b[at], b[at + 1], b[at + 2], b[at + 3])
}

fn wr16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn wr32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn wr_color(b: &mut [u8], at: usize, c: Color) {
    b[at..at + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
}
