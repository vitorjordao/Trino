//! The PC's trace (issue #62) through the public API: `TraceHost` installs
//! the ring, a headless frame records the renderer's CPU zones (draw, batch,
//! encode, queue submit, present), its counters and frame marks, and — on an
//! adapter with timestamp queries — the GPU passes on the `gpu` track; the
//! lines collect into a session `trino-trace` reads.
//!
//! The zones exist only with `trino-core/trace`:
//! `cargo test -p trino-platform-pc --features trino-core/trace --test trace_zones`
//! (CI's lint job). Without the feature the test checks that nothing records.

use std::cell::RefCell;
use std::rc::Rc;

use trino_asset_pipeline::primitive::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::trace::{self, TracePlatform, track};
use trino_core::{Color, Material, ModelId, ModelParams, Renderer, Transform3, Vec3};
use trino_platform_pc::trace_host::TraceHost;
use trino_platform_pc::{PcRenderer, SimProfile};

const BOX: ModelId = ModelId(7);

#[test]
fn a_traced_pc_frame_records_cpu_and_gpu_zones() {
    let lines = Rc::new(RefCell::new(String::new()));
    let sink = lines.clone();
    let host = TraceHost::start(
        TracePlatform::Pc,
        "pc-test",
        Box::new(move |l: &str| sink.borrow_mut().push_str(l)),
    );
    if !trace::ENABLED {
        assert!(host.is_none(), "no ring without the feature");
        assert!(!trace::is_on());
        return;
    }
    let mut host = host.expect("the ring installs");
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            host.finish();
            return;
        }
    };
    r.set_internal_size(320, 180);
    let mesh = Primitive {
        shape: Shape::Box {
            size: [1.0, 1.0, 1.0],
        },
        color: [200, 120, 40, 255],
        pivot: Pivot::Center,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap();
    r.upload_mesh(BOX, mesh);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 1.0, -3.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    };
    // Enough frames for the timestamps of the first ones to read back.
    for _ in 0..8 {
        r.begin_frame(Color::BLACK);
        r.set_camera(&camera);
        r.set_light(&DEFAULT_LIGHT);
        r.draw_model(
            BOX,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.end_frame();
        host.frame();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let gpu_timestamps = r.has_gpu_timer();
    host.finish();
    assert!(!trace::is_on(), "finish uninstalls");

    let t = trino_trace::collect(&lines.borrow()).unwrap();
    let s = t.session("pc-test").unwrap();
    assert_eq!(s.platform, "pc");
    assert_eq!(s.frames.len(), 8);
    let names = |trk: u8| -> Vec<&str> {
        s.zones
            .iter()
            .filter(|z| z.track == trk)
            .map(|z| s.name_of(z.name))
            .collect()
    };
    let cpu = names(track::CPU);
    for want in ["draw", "batch", "end", "encode", "queue submit", "present"] {
        assert!(cpu.contains(&want), "{want} missing from {cpu:?}");
    }
    // `end` holds the encode, the submit and the present of its frame.
    let end = s.zones.iter().find(|z| s.name_of(z.name) == "end").unwrap();
    let encode = s
        .zones
        .iter()
        .find(|z| s.name_of(z.name) == "encode" && z.start >= end.start)
        .unwrap();
    assert!(encode.depth > end.depth && encode.end() <= end.end());
    let draws: Vec<u64> = s
        .counters
        .iter()
        .filter(|c| s.name_of(c.name) == "draws")
        .map(|c| c.value)
        .collect();
    assert_eq!(draws, [1; 8]);
    if gpu_timestamps {
        let gpu = names(track::GPU);
        assert!(
            gpu.contains(&"gpu frame") && gpu.contains(&"scene"),
            "GPU zones: {gpu:?}"
        );
    } else {
        eprintln!("this adapter has no timestamp queries: no GPU zones");
    }
}
