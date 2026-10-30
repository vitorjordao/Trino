//! Zones on the PC's other threads (issue #130) through the public API: the
//! CPU T&L workers of a heavy batch and the audio stream thread record on
//! tracks of their own (`worker 1..`, `audio`), nested, and nothing is lost
//! while the main thread records a busy frame of its own.
//!
//! `cargo test -p trino-platform-pc --features trino-core/trace --test trace_threads`
//! (CI's desktop job). Without the feature the test checks that nothing
//! binds.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use trino_asset_pipeline::primitive::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::trace::{self, TracePlatform, track};
use trino_core::{Color, Material, ModelId, ModelParams, Renderer, Transform3, Vec3};
use trino_platform_pc::audio::stream::{Job, StreamSource, Streamer};
use trino_platform_pc::trace_host::TraceHost;
use trino_platform_pc::{PcRenderer, SimProfile};

const BALL: ModelId = ModelId(9);
const WORKERS: usize = 4;

#[test]
fn tnl_workers_and_the_audio_stream_record_on_their_own_tracks() {
    let lines = Rc::new(RefCell::new(String::new()));
    let sink = lines.clone();
    let host = TraceHost::start(
        TracePlatform::Pc,
        "pc-threads",
        Box::new(move |l: &str| sink.borrow_mut().push_str(l)),
    );
    if !trace::ENABLED {
        assert!(host.is_none());
        assert!(!trace::bind_thread(track::AUDIO).is_bound());
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
    // Every draw on the CPU T&L, over the workers: 8 spheres of ~2300
    // triangles each are past `PARALLEL_MIN_TRIS`.
    r.set_hw_tnl(false);
    r.set_tnl_threads(WORKERS);
    let mesh = Primitive {
        shape: Shape::Sphere { radius: 0.3 },
        segments: 48,
        color: [90, 160, 220, 255],
        pivot: Pivot::Center,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap();
    r.upload_mesh(BALL, mesh);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 1.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    };

    // A looping stream on the audio thread, drained like a mixer would.
    let streamer = Streamer::spawn();
    let src = StreamSource::Pcm {
        rate: 22_050,
        data: Arc::new((0..4096).map(|i| (i as f32 * 0.01).sin()).collect()),
    };
    let (job, ring) = Job::start(&src, 48_000, true, 2048).unwrap();
    assert!(streamer.send(job).is_ok(), "the stream thread runs");

    const FRAMES: usize = 12;
    let mut sink = [0.0f32; 1024];
    for _ in 0..FRAMES {
        r.begin_frame(Color::BLACK);
        r.set_camera(&camera);
        r.set_light(&DEFAULT_LIGHT);
        for i in 0..8 {
            let t = Transform3 {
                position: Vec3::new(i as f32 * 0.4 - 1.4, 0.0, 0.0),
                ..Transform3::default()
            };
            r.draw_model(BALL, &t, Material::VertexLit, &ModelParams::DEFAULT);
        }
        r.end_frame();
        ring.pop(&mut sink);
        host.frame();
        std::thread::sleep(std::time::Duration::from_millis(6));
    }
    drop(streamer); // joins the thread: its last zones are in its lane
    host.finish();

    let t = trino_trace::collect(&lines.borrow()).unwrap();
    let s = t.session("pc-threads").unwrap();
    assert_eq!(s.dropped, 0, "no event lost");
    let on = |trk: u8, name: &str| -> Vec<&trino_trace::Zone> {
        s.zones
            .iter()
            .filter(|z| z.track == trk && s.name_of(z.name) == name)
            .collect()
    };
    // Each T&L worker's chunks on its own track, once a frame.
    for w in 1..=WORKERS as u8 {
        let chunks = on(track::on(track::WORKER, w), "tnl chunk");
        assert_eq!(chunks.len(), FRAMES, "worker {w}: {chunks:?}");
        assert!(chunks.iter().all(|z| z.depth == 0));
    }
    // ...inside their frame's `cpu tnl` zone on the main thread.
    let cpu_tnl = on(track::CPU, "cpu tnl");
    for c in on(track::on(track::WORKER, 1), "tnl chunk") {
        assert!(
            cpu_tnl
                .iter()
                .any(|z| z.start <= c.start && c.end() <= z.end()),
            "{c:?} outside every cpu tnl zone"
        );
    }
    // The audio thread's fills, each job nested one deeper.
    let fills = on(track::AUDIO, "stream fill");
    let jobs = on(track::AUDIO, "stream job");
    assert!(!fills.is_empty() && !jobs.is_empty());
    assert!(fills.iter().all(|z| z.depth == 0) && jobs.iter().all(|z| z.depth == 1));
    for j in &jobs {
        assert!(
            fills
                .iter()
                .any(|f| f.start <= j.start && j.end() <= f.end())
        );
    }
    // Nothing of theirs on the main thread's track.
    assert!(on(track::CPU, "tnl chunk").is_empty() && on(track::CPU, "stream job").is_empty());
    // The viewers name the tracks.
    let json = trino_trace::chrome::write(&t).to_string();
    for label in ["\"worker 1\"", "\"worker 4\"", "\"audio\""] {
        assert!(
            json.contains(label),
            "{label} missing from the Perfetto JSON"
        );
    }
}
