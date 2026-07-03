//! PC glue binary: winit window + trino-platform-pc backend + example game.
//!
//! Env knobs:
//! - `TRINO_SMOKE_FRAMES=N` — exit cleanly after N frames (CI smoke tests).
//! - `TRINO_SIM=native|n64|3ds` — console simulation profile (default n64,
//!   so the example looks the same everywhere until Fase 2's config lands).

use std::sync::Arc;
use std::time::Instant;

use trino_core::{Game, Input, Vec2};
use trino_platform_pc::{PcAudio, PcInput, PcRenderer, SimProfile};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

struct Ctx {
    window: Arc<Window>,
    renderer: PcRenderer,
    audio: PcAudio,
    input: PcInput,
    game: hello_sprite::HelloGame,
    last_frame: Instant,
}

struct App {
    ctx: Option<Ctx>,
    profile: SimProfile,
    frames: u64,
    smoke_frames: Option<u64>,
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Ctx {
        let attrs = Window::default_attributes()
            .with_title("Trino — hello-sprite")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let mut renderer =
            pollster::block_on(PcRenderer::new_windowed(window.clone(), self.profile))
                .expect("failed to initialize wgpu renderer");
        let mut audio = PcAudio::new();

        // Placeholder assets, procedurally generated. The asset pipeline
        // (Fase 2) replaces this with baked data behind the same handles.
        let size = hello_sprite::PLAYER_SIZE;
        renderer.upload_sprite(
            hello_sprite::PLAYER_SPRITE,
            size,
            size,
            &checkerboard(size, [230, 70, 70, 255], [250, 250, 250, 255]),
        );
        audio.upload_sound(hello_sprite::BEEP, beep(audio.sample_rate()));

        let (iw, ih) = renderer.internal_size();
        let game = hello_sprite::HelloGame::new(Vec2::new(iw as f32, ih as f32));

        window.request_redraw();
        Ctx {
            window,
            renderer,
            audio,
            input: PcInput::new(),
            game,
            last_frame: Instant::now(),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ctx.is_none() {
            self.ctx = Some(self.init(event_loop));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(ctx) = self.ctx.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => ctx.renderer.resize(size.width, size.height),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat: false,
                        ..
                    },
                ..
            } => ctx.input.handle_key(code, state == ElementState::Pressed),
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                // Clamp dt: debugger pauses and window drags must not
                // teleport the simulation.
                let dt = (now - ctx.last_frame).as_secs_f32().min(0.1);
                ctx.last_frame = now;

                let input = ctx.input.poll();
                ctx.game.update(&input, &mut ctx.audio, dt);
                ctx.game.render(&mut ctx.renderer);

                self.frames += 1;
                if let Some(max) = self.smoke_frames
                    && self.frames >= max
                {
                    event_loop.exit();
                    return;
                }
                ctx.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn checkerboard(size: u32, a: [u8; 4], b: [u8; 4]) -> Vec<u8> {
    let cell = (size / 4).max(1);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let pick = ((x / cell) + (y / cell)).is_multiple_of(2);
            pixels.extend_from_slice(if pick { &a } else { &b });
        }
    }
    pixels
}

/// 440 Hz sine, 150 ms, with a linear fade-out to avoid a click.
fn beep(sample_rate: u32) -> Vec<f32> {
    let len = (sample_rate as f32 * 0.15) as usize;
    (0..len)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let fade = 1.0 - i as f32 / len as f32;
            (t * 440.0 * core::f32::consts::TAU).sin() * 0.25 * fade
        })
        .collect()
}

fn main() {
    let smoke_frames = std::env::var("TRINO_SMOKE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());
    let profile = std::env::var("TRINO_SIM")
        .ok()
        .and_then(|v| SimProfile::parse(&v))
        .unwrap_or(SimProfile::N64);

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App {
        ctx: None,
        profile,
        frames: 0,
        smoke_frames,
    };
    event_loop.run_app(&mut app).expect("event loop error");
}
