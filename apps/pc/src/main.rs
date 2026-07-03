//! PC glue binary. Fase 0: an empty window proving the workspace runs.
//! Fase 1 adds the wgpu renderer from `platform-pc` and drives a `Game`.
//!
//! Smoke-test hook: set `TRINO_SMOKE_FRAMES=N` to exit cleanly after N
//! redraws (used by CI, which has no one to close the window).

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct App {
    window: Option<Window>,
    frames: u64,
    smoke_frames: Option<u64>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title("Trino")
                .with_inner_size(LogicalSize::new(1280.0, 720.0));
            let window = event_loop
                .create_window(attrs)
                .expect("failed to create window");
            window.request_redraw();
            self.window = Some(window);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.frames += 1;
                if let Some(max) = self.smoke_frames
                    && self.frames >= max
                {
                    event_loop.exit();
                    return;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let smoke_frames = std::env::var("TRINO_SMOKE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App {
        window: None,
        frames: 0,
        smoke_frames,
    };
    event_loop.run_app(&mut app).expect("event loop error");
}
