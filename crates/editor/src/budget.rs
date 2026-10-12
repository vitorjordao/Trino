//! Live console budget for the Game view: what the last frame cost against
//! the selected profile's `Caps` (N64 / 3DS / PC). Pure data here; the
//! viewport draws it as an overlay (green < 75%, amber < 100%, red over).

use trino_core::{Caps, FrameStats};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Near,
    Over,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub used: u64,
    pub limit: u64,
    pub unit: &'static str,
    pub level: Level,
    pub hint: &'static str,
}

impl Row {
    pub fn fraction(&self) -> f32 {
        if self.limit == 0 {
            0.0
        } else {
            (self.used as f64 / self.limit as f64) as f32
        }
    }
}

fn level(used: u64, limit: u64) -> Level {
    if limit == u64::MAX || limit == 0 {
        return Level::Ok;
    }
    if used > limit {
        Level::Over
    } else if used * 4 > limit * 3 {
        Level::Near
    } else {
        Level::Ok
    }
}

/// Budget rows for one frame. Unlimited caps (`u32::MAX`, the PC profile)
/// never warn.
pub fn rows(stats: &FrameStats, caps: &Caps) -> Vec<Row> {
    let lim = |v: u32| if v == u32::MAX { u64::MAX } else { v as u64 };
    let mut out = vec![
        Row {
            label: "Triangles",
            used: stats.tris as u64,
            limit: lim(caps.max_tris_per_frame),
            unit: "",
            level: level(stats.tris as u64, lim(caps.max_tris_per_frame)),
            hint: "3D triangles after culling/clipping/subdivision (Caps::max_tris_per_frame)",
        },
        Row {
            label: "Sprites",
            used: stats.sprites as u64,
            limit: lim(caps.max_sprites_per_frame),
            unit: "",
            level: level(stats.sprites as u64, lim(caps.max_sprites_per_frame)),
            hint: "sprite draws incl. text glyphs (Caps::max_sprites_per_frame)",
        },
        Row {
            label: "Texture",
            used: stats.max_texture_bytes as u64,
            limit: lim(caps.texture_memory_bytes),
            unit: "B",
            level: level(
                stats.max_texture_bytes as u64,
                lim(caps.texture_memory_bytes),
            ),
            hint: "largest single texture load, worst case 16-bit texels (CI4/CI8 \
                   use less — the PC cannot know the console format)",
        },
    ];
    out.push(Row {
        label: "Models",
        used: stats.models as u64,
        limit: u64::MAX,
        unit: "",
        level: Level::Ok,
        hint: "draw_model calls that reached the T&L (each costs CPU on consoles)",
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_n64_caps() {
        let stats = FrameStats {
            tris: 3500,
            sprites: 10,
            models: 40,
            max_texture_bytes: 8192,
        };
        let r = rows(&stats, &Caps::N64);
        assert_eq!(r[0].level, Level::Near, "3500 / 4000 tris");
        assert_eq!(r[1].level, Level::Ok);
        assert_eq!(r[2].level, Level::Over, "8 KB texture vs 4 KB TMEM");
        assert!((r[0].fraction() - 0.875).abs() < 1e-6);
    }

    #[test]
    fn pc_profile_never_warns() {
        let stats = FrameStats {
            tris: 1_000_000,
            sprites: 100_000,
            models: 1,
            max_texture_bytes: 1 << 30,
        };
        assert!(rows(&stats, &Caps::PC).iter().all(|r| r.level == Level::Ok));
    }
}
