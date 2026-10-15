//! Live console budget for the Game view: what the last frame cost against
//! the selected profile's `Caps` (N64 / 3DS / PC). Pure data here; the
//! viewport draws it as an overlay (green < 75%, amber < 100%, red over).

use trino_core::perf::{FRAME_30FPS_US, N64_COST};
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

/// What the Game view drew for entities with LOD levels this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LodCounts {
    /// Still on their full mesh (closer than the first level).
    pub full: u32,
    /// On a lighter mesh level.
    pub light: u32,
    /// Drawn as impostors (one billboard each).
    pub impostor: u32,
    /// Culled by a level with nothing to draw.
    pub culled: u32,
}

impl LodCounts {
    pub fn total(&self) -> u32 {
        self.full + self.light + self.impostor + self.culled
    }

    /// `LOD 3 full · 5 light · 12 impostor · 2 culled` (None without LOD).
    pub fn summary(&self) -> Option<String> {
        (self.total() > 0).then(|| {
            format!(
                "LOD {} full · {} light · {} imp · {} cull",
                self.full, self.light, self.impostor, self.culled
            )
        })
    }
}

/// Budget rows for one frame. Unlimited caps (`u32::MAX`, the PC profile)
/// never warn. `lod` adds the impostors' cost (billboards are not T&L
/// work) to the N64 time.
pub fn rows(stats: &FrameStats, caps: &Caps, lod: &LodCounts) -> Vec<Row> {
    let lim = |v: u32| if v == u32::MAX { u64::MAX } else { v as u64 };
    let mut out = vec![
        Row {
            label: "Triangles",
            used: stats.tris as u64,
            limit: lim(caps.max_tris_per_frame),
            unit: "",
            level: level(stats.tris as u64, lim(caps.max_tris_per_frame)),
            hint: "3D triangles after culling/clipping/subdivision, against the rasterizer's \
                   cap (Caps::max_tris_per_frame). On the N64 the CPU time binds far earlier: \
                   see N64 3D time",
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
    if *caps == Caps::N64 {
        // The triangle cap says little about the N64's CPU T&L; the
        // calibrated cost model turns this frame's deterministic T&L work
        // into emulated N64 time (3D only: T&L + sort + submit).
        let us = (N64_COST.estimate_us(&stats.tnl, stats.models)
            + N64_COST.billboards_us(lod.impostor)) as u64;
        let limit = FRAME_30FPS_US as u64;
        out.push(Row {
            label: "N64 3D time",
            used: us,
            limit,
            unit: "us",
            level: level(us, limit),
            hint: "estimated N64 time of this frame's 3D (T&L + sort + submit, plus \
                   ~44 µs per impostor billboard) vs one 30 fps frame — \
                   trino_core::perf::N64_COST, calibrated on ares",
        });
    }
    if lod.impostor > 0 {
        out.push(Row {
            label: "Impostors",
            used: lod.impostor as u64,
            limit: u64::MAX,
            unit: "",
            level: Level::Ok,
            hint: "LOD levels drawn as impostors: 2 textured triangles each (on the N64 \
                   they are CPU-submitted — check the N64 3D time)",
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn n64_profile_estimates_3d_time() {
        let stats = FrameStats {
            models: 1,
            tnl: trino_core::TnlStats {
                verts: 1790,
                tris_in: 1068,
                emitted: 552,
                ..Default::default()
            },
            ..FrameStats::default()
        };
        let r = rows(&stats, &Caps::N64, &LodCounts::default());
        let time = r.iter().find(|r| r.label == "N64 3D time").unwrap();
        // KayKit's original 1068-tri door alone: about two 30 fps frames.
        assert_eq!(time.level, Level::Over);
        assert!(time.used > 50_000, "{}", time.used);
        assert!(
            rows(&stats, &Caps::PC, &LodCounts::default())
                .iter()
                .all(|r| r.label != "N64 3D time")
        );
    }

    #[test]
    fn levels_follow_the_n64_caps() {
        let stats = FrameStats {
            tris: 3500,
            sprites: 10,
            models: 40,
            max_texture_bytes: 8192,
            ..FrameStats::default()
        };
        let r = rows(&stats, &Caps::N64, &LodCounts::default());
        assert_eq!(r[0].level, Level::Near, "3500 / 4000 tris");
        assert_eq!(r[1].level, Level::Ok);
        assert_eq!(r[2].level, Level::Over, "8 KB texture vs 4 KB TMEM");
        assert!((r[0].fraction() - 0.875).abs() < 1e-6);
    }

    #[test]
    fn impostors_add_their_billboard_cost_to_the_n64_time() {
        let stats = FrameStats::default();
        let lod = LodCounts {
            full: 1,
            light: 2,
            impostor: 10,
            culled: 3,
        };
        let r = rows(&stats, &Caps::N64, &lod);
        let time = r.iter().find(|r| r.label == "N64 3D time").unwrap();
        assert_eq!(time.used, 440, "10 billboards at 44 µs");
        assert_eq!(r.iter().find(|r| r.label == "Impostors").unwrap().used, 10);
        assert_eq!(lod.total(), 16);
        assert_eq!(
            lod.summary().unwrap(),
            "LOD 1 full · 2 light · 10 imp · 3 cull"
        );
        assert!(LodCounts::default().summary().is_none());
    }

    #[test]
    fn pc_profile_never_warns() {
        let stats = FrameStats {
            tris: 1_000_000,
            sprites: 100_000,
            models: 1,
            max_texture_bytes: 1 << 30,
            ..FrameStats::default()
        };
        assert!(
            rows(&stats, &Caps::PC, &LodCounts::default())
                .iter()
                .all(|r| r.level == Level::Ok)
        );
    }
}
