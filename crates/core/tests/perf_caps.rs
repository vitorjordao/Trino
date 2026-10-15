//! Console budgets: `Caps` texture validation per target, the calibrated
//! N64 cost model applied to real T&L counters, and the render profile a
//! console backend accumulates and prints for `cargo xtask test n64|3ds`.

mod common;

use common::*;
use trino_core::perf::{
    FRAME_30FPS_US, FrameTimes, MAX_PROFILED_MODELS, N64_COST, RenderProfile, StageTimes,
};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Mesh, TnlStats, tessellate};
use trino_core::{Caps, CapsError, Mat34, ModelId, ModelParams};

#[test]
fn caps_validate_textures_per_console() {
    // N64: 4 KB TMEM, 256 px max side, 16-bit color.
    assert_eq!(Caps::N64.validate_texture(32, 32, 2), Ok(()));
    assert_eq!(
        Caps::N64.validate_texture(64, 32, 2),
        Ok(()),
        "exactly 4 KB"
    );
    assert_eq!(
        Caps::N64.validate_texture(64, 64, 2),
        Err(CapsError::TextureExceedsMemory {
            bytes: 8192,
            budget: 4096
        })
    );
    assert_eq!(
        Caps::N64.validate_texture(300, 4, 1),
        Err(CapsError::TextureTooLarge {
            width: 300,
            height: 4,
            max_dim: 256
        })
    );
    assert!(matches!(
        Caps::N64.validate_texture(4, 257, 1),
        Err(CapsError::TextureTooLarge { max_dim: 256, .. })
    ));
    // 3DS and PC are roomier.
    assert_eq!(Caps::N3DS.validate_texture(1024, 1024, 4), Ok(()));
    assert!(Caps::N3DS.validate_texture(2048, 16, 4).is_err());
    assert_eq!(Caps::PC.validate_texture(4096, 4096, 4), Ok(()));
    assert_eq!(
        (Caps::N64.screen_width, Caps::N64.screen_height),
        (320, 240)
    );
    assert_eq!(
        (Caps::N3DS.screen_width, Caps::N3DS.screen_height),
        (400, 240)
    );
    assert_eq!(Caps::N64.save_bytes, 2048);
    assert_eq!(Caps::N64.max_3d_us_per_frame, 50_000);
    assert_eq!(FRAME_30FPS_US, 33_333);
}

#[test]
fn n64_cost_model_prices_real_tnl_work() {
    let blob = bounded_cube();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let s = tessellate(
        &mesh,
        &Mat34::IDENTITY,
        &Camera3::default(),
        &DEFAULT_LIGHT,
        &ModelParams::DEFAULT,
        SCREEN,
        &mut |_| {},
    );
    // 24 verts, 12 tris, 2 emitted, one draw.
    let want = (7.1f32 * 24.0 + 17.3 * 12.0 + 183.8 + 37.0 * 2.0) as u32;
    assert_eq!(N64_COST.estimate_us(&s, 1), want);
    let tnl_only = (7.1f32 * 24.0 + 17.3 * 12.0 + 183.8) as u32;
    assert_eq!(N64_COST.tnl_us(&s, 1), tnl_only, "sort + submit excluded");
    // A culled draw costs only its fixed per-draw part.
    let culled = TnlStats {
        culled: 1,
        ..TnlStats::default()
    };
    assert_eq!(N64_COST.estimate_us(&culled, 1), 183);
    // The per-asset figure: half the faces toward the camera.
    let per_asset = N64_COST.mesh_draw_us(24, 12);
    assert_eq!(
        per_asset,
        (7.1f32 * 24.0 + 17.3 * 12.0 + 183.8 + 37.0 * 6.0) as u32
    );
    // Splits add their own term.
    let split = TnlStats {
        splits: 10,
        ..TnlStats::default()
    };
    assert_eq!(N64_COST.estimate_us(&split, 0), 255);
}

#[test]
fn render_profile_accumulates_and_reports_per_frame() {
    let mut p = RenderProfile {
        frames: 2,
        ..RenderProfile::default()
    };
    let a = TnlStats {
        verts: 24,
        tris_in: 12,
        emitted: 2,
        ..TnlStats::default()
    };
    let b = TnlStats {
        verts: 8,
        tris_in: 4,
        emitted: 4,
        splits: 2,
        ..TnlStats::default()
    };
    for _ in 0..2 {
        p.record_model(ModelId(0xA), 300, &a);
        p.record_model(ModelId(0xB), 900, &b);
        p.record_model(ModelId(0xA), 100, &a);
    }
    p.stages = StageTimes {
        begin: 10,
        tnl: p.stages.tnl,
        sort: 20,
        submit: 30,
        sprites: 40,
        end: 50,
    };
    assert_eq!(p.stages.tnl, 2600);
    assert_eq!(p.stages.total(), 10 + 2600 + 20 + 30 + 40 + 50);
    assert_eq!(p.draws(), 6);
    assert_eq!(p.model_count, 2);
    assert_eq!(p.tnl.verts, 24 * 4 + 8 * 2);
    let by_cost = p.models_by_cost();
    assert_eq!(by_cost[0].model, ModelId(0xB), "1800 us beats 800 us");
    assert_eq!((by_cost[0].calls, by_cost[0].us), (2, 1800));
    assert_eq!((by_cost[1].calls, by_cost[1].us), (4, 800));

    let times = FrameTimes {
        update: 400,
        render: 6000,
        vblanks: 4,
    };
    let mut out = String::new();
    p.write_report("test-scene", &times, &mut out).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out}");
    assert_eq!(
        lines[0],
        "TRINO_PERF_SCENE:test-scene frames=2 update=200 render=3000 begin=5 tnl=1300 sort=10 \
         submit=15 sprites=20 end=25 game=1625 vblanks=4 verts=56 tris_in=28 clipped=0 splits=2 \
         backfaced=0 emitted=8 culled=0 draws=3 rsp_draws=0 rsp_tris=0"
    );
    assert_eq!(
        lines[1],
        "TRINO_PERF_MODEL:test-scene model=0000000b calls=1 us=900 verts=8 tris_in=4 emitted=4 splits=2"
    );
    assert!(lines[2].starts_with("TRINO_PERF_MODEL:test-scene model=0000000a calls=2 us=400 "));
}

#[test]
fn models_past_the_tracked_limit_pool_into_other() {
    let mut p = RenderProfile::default();
    let t = TnlStats {
        verts: 1,
        ..TnlStats::default()
    };
    for i in 0..(MAX_PROFILED_MODELS as u32 + 3) {
        p.record_model(ModelId(i + 1), 10, &t);
    }
    assert_eq!(p.model_count, MAX_PROFILED_MODELS);
    assert_eq!((p.other.calls, p.other.us, p.other.tnl.verts), (3, 30, 3));
    assert_eq!(p.draws(), MAX_PROFILED_MODELS as u32 + 3);
    let mut out = String::new();
    // frames = 0 reports as one frame (no division by zero).
    p.write_report("big", &FrameTimes::default(), &mut out)
        .unwrap();
    assert_eq!(
        out.lines().count(),
        1 + MAX_PROFILED_MODELS + 1,
        "the pool gets a line"
    );
    assert!(
        out.lines()
            .last()
            .unwrap()
            .contains("model=00000000 calls=3 us=30")
    );
}

/// A sink that refuses output (a full debug channel).
struct Full(usize);

impl core::fmt::Write for Full {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.0 < s.len() {
            return Err(core::fmt::Error);
        }
        self.0 -= s.len();
        Ok(())
    }
}

#[test]
fn report_write_errors_propagate() {
    let mut p = RenderProfile::default();
    p.record_model(ModelId(1), 5, &TnlStats::default());
    assert!(
        p.write_report("s", &FrameTimes::default(), &mut Full(0))
            .is_err()
    );
    // Room for the scene line only: the model line fails.
    let mut scene_only = String::new();
    RenderProfile::default()
        .write_report("s", &FrameTimes::default(), &mut scene_only)
        .unwrap();
    let room = scene_only.len() + 10;
    assert!(
        p.write_report("s", &FrameTimes::default(), &mut Full(room))
            .is_err()
    );
}
