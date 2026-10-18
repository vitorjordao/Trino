//! The page's side of a Trino game: the canvas and its size, the frame
//! loop on `requestAnimationFrame`, audio unlock, fullscreen and Pointer
//! Lock, the asset pack, panics.
//!
//! [`WebHost`] is the backend set (renderer, audio, input, platform) on one
//! canvas; [`run`] drives a closure once per animation frame with it;
//! [`run_game`] is the whole thing for a [`Game`] — what a standalone
//! project's `web` crate calls (the Web twin of `trino-runner-pc`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use trino_core::{DisplayInfo, DisplaySettings, Game, Input, InputInfo, Renderer, Vec2};
use trino_render_wgpu::sim::native_internal;
use trino_render_wgpu::{FrameMeter, PcRenderer, SimProfile, WebBackend};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

use crate::assets::{LoadReport, TextureTarget};
use crate::{WebAudio, WebInput, WebPlatform};

/// How a page runs a game.
#[derive(Clone, Debug)]
pub struct WebConfig {
    /// The `<canvas>` element's id (`trino.toml` `[web] canvas_id`).
    pub canvas_id: String,
    /// The save's name in `localStorage`.
    pub save_name: String,
    /// The picture's profile: [`SimProfile::Web`] (the browser's own), or a
    /// console's look for fun (`?sim=n64`).
    pub profile: SimProfile,
    pub backend: WebBackend,
    /// Integer pixel scale of the native-like picture (0 = auto, the PC's
    /// `auto_pixel_scale`).
    pub pixel_scale: u32,
    /// 4× MSAA on the picture (not on WebGL2, which can't sample the
    /// multisampled depth the AO and water read).
    pub msaa: bool,
    /// A fixed canvas size in CSS pixels (`?size=640x480`: tests and
    /// screenshots); `None` fills the canvas's element.
    pub fixed_size: Option<(u32, u32)>,
    /// Where Binomial's transcoder lives (the KTX2 textures).
    pub basis_js: String,
    pub basis_wasm: String,
}

impl WebConfig {
    /// Defaults for a game called `save_name`, refined by the page's
    /// [`Query`] (`?backend=webgl2&sim=n64&scale=2&size=640x480&msaa=0`).
    pub fn new(save_name: &str, query: &Query) -> Self {
        let size = query.get("size").and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
        WebConfig {
            canvas_id: "trino-canvas".into(),
            save_name: save_name.into(),
            profile: query
                .get("sim")
                .and_then(|s| SimProfile::parse(&s))
                .unwrap_or(SimProfile::Web),
            backend: query
                .get("backend")
                .and_then(|s| WebBackend::parse(&s))
                .unwrap_or_default(),
            pixel_scale: query.get("scale").and_then(|s| s.parse().ok()).unwrap_or(0),
            msaa: query.get("msaa").is_none_or(|v| v != "0"),
            fixed_size: size,
            basis_js: "basis/basis_transcoder.js".into(),
            basis_wasm: "basis/basis_transcoder.wasm".into(),
        }
    }
}

/// The page URL's query (`?game=solid-eleven&selftest`).
#[derive(Clone, Debug, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    pub fn from_location() -> Self {
        let search = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();
        let mut out = Vec::new();
        if let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) {
            for entry in params.entries() {
                let Ok(pair) = entry.map(|v| js_sys::Array::from(&v)) else {
                    continue;
                };
                out.push((
                    pair.get(0).as_string().unwrap_or_default(),
                    pair.get(1).as_string().unwrap_or_default(),
                ));
            }
        }
        Query(out)
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    /// The key is present (`?selftest`, `?selftest=1`).
    pub fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, v)| k == key && v != "0")
    }
}

/// Log panics to the console and the page, and end a harness run.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("panicked: {info}");
        crate::js::log(&msg);
        crate::js::trino_error_overlay(&msg);
        crate::js::trino_finish(false, Some(msg));
    }));
}

/// The canvas's `ResizeObserver` and its callback (kept alive together).
type Observer = (web_sys::ResizeObserver, Closure<dyn FnMut(js_sys::Array)>);
/// A `requestAnimationFrame` callback.
type FrameCallback = Closure<dyn FnMut(f64)>;

/// The backend set on one canvas.
pub struct WebHost {
    pub renderer: PcRenderer,
    pub audio: WebAudio,
    pub input: WebInput,
    pub platform: WebPlatform,
    pub meter: FrameMeter,
    pub canvas: HtmlCanvasElement,
    config: WebConfig,
    /// The canvas's device-pixel size, as the `ResizeObserver` last saw it.
    observed: Rc<Cell<(u32, u32)>>,
    /// The size the renderer has.
    applied: (u32, u32),
    /// What a pack load uploaded KTX2 textures as.
    pub texture_target: TextureTarget,
    /// Frames run, the audio state and the input summary last published
    /// to the page (`__trino.info`).
    frames: u64,
    last_audio: Option<bool>,
    last_input: String,
    _observer: Option<Observer>,
}

/// A canvas element's size in device pixels: the exact device-pixel box
/// where the browser reports it (Chrome, Firefox), else the CSS box × the
/// device-pixel ratio (Safari).
fn device_size(entry: &web_sys::ResizeObserverEntry) -> (u32, u32) {
    let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
    let exact = js_sys::Reflect::get(entry, &"devicePixelContentBoxSize".into())
        .ok()
        .filter(|v| !v.is_undefined())
        .map(|v| js_sys::Array::from(&v))
        .and_then(|a| a.get(0).dyn_into::<web_sys::ResizeObserverSize>().ok());
    match exact {
        Some(s) => (s.inline_size() as u32, s.block_size() as u32),
        None => {
            let r = entry.content_rect();
            (
                (r.width() * dpr).round() as u32,
                (r.height() * dpr).round() as u32,
            )
        }
    }
}

impl WebHost {
    /// The renderer on `config.canvas_id` (WebGPU or WebGL2), audio, input
    /// listeners and the platform. Also loads the KTX2 transcoder.
    pub async fn new(config: WebConfig) -> Result<WebHost, String> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let canvas: HtmlCanvasElement = document
            .get_element_by_id(&config.canvas_id)
            .ok_or_else(|| format!("no element #{}", config.canvas_id))?
            .dyn_into()
            .map_err(|_| format!("#{} is not a canvas", config.canvas_id))?;
        let dpr = window.device_pixel_ratio();
        if let Some((w, h)) = config.fixed_size {
            let style = canvas.style();
            let _ = style.set_property("width", &format!("{w}px"));
            let _ = style.set_property("height", &format!("{h}px"));
            let _ = style.set_property("flex", "none");
        }
        let css = (canvas.client_width().max(1), canvas.client_height().max(1));
        let size = match config.fixed_size {
            Some((w, h)) => ((w as f64 * dpr) as u32, (h as f64 * dpr) as u32),
            None => ((css.0 as f64 * dpr) as u32, (css.1 as f64 * dpr) as u32),
        };
        canvas.set_width(size.0.max(1));
        canvas.set_height(size.1.max(1));

        let mut renderer = PcRenderer::new_canvas(
            canvas.clone(),
            size.0,
            size.1,
            config.profile,
            config.backend,
        )
        .await
        .map_err(|e| format!("renderer: {e}"))?;
        renderer.set_msaa(config.msaa);
        crate::js::trino_info("adapter", &renderer.adapter_info());
        crate::js::log(&format!(
            "TRINO_WEB: adapter=\"{}\" profile={:?} canvas={}x{} dpr={dpr} msaa={}",
            renderer.adapter_info(),
            config.profile,
            size.0,
            size.1,
            renderer.msaa()
        ));

        let ready = crate::js::trino_init_basis(&config.basis_js, &config.basis_wasm)
            .await
            .as_bool()
            .unwrap_or(false);
        if !ready {
            crate::js::log("warning: KTX2 transcoder missing: KTX2 textures won't load");
        }

        let observed = Rc::new(Cell::new(size));
        let observer = (config.fixed_size.is_none())
            .then(|| {
                let seen = observed.clone();
                let closure =
                    Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
                        if let Ok(e) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                            let (w, h) = device_size(&e);
                            if w > 0 && h > 0 {
                                seen.set((w, h));
                            }
                        }
                    });
                let observer =
                    web_sys::ResizeObserver::new(closure.as_ref().unchecked_ref()).ok()?;
                observer.observe(&canvas);
                Some((observer, closure))
            })
            .flatten();

        let input = WebInput::attach(&canvas);
        let platform = WebPlatform::new(&config.profile.caps(), &config.save_name);
        let texture_target = TextureTarget::of(&renderer);
        crate::js::trino_info("texture_target", texture_target.name());
        let mut host = WebHost {
            renderer,
            audio: WebAudio::new(),
            input,
            platform,
            meter: FrameMeter::new(config.profile),
            canvas,
            config,
            observed,
            applied: (0, 0),
            texture_target,
            _observer: observer,
            frames: 0,
            last_audio: None,
            last_input: String::new(),
        };
        host.sync_size();
        host.end_audio_state();
        Ok(host)
    }

    /// `running`, `suspended` (waiting for a gesture) or `none` (no Web
    /// Audio: silent).
    fn audio_state(&self) -> &'static str {
        match (self.audio.has_web_audio(), self.audio.running()) {
            (false, _) => "none",
            (true, true) => "running",
            (true, false) => "suspended",
        }
    }

    /// Publish the audio state at start (the banner and `__trino.info`).
    fn end_audio_state(&mut self) {
        let running = self.audio.running();
        crate::js::trino_audio_banner(!running);
        crate::js::trino_info("audio", self.audio_state());
        self.last_audio = Some(running);
    }

    /// Fetch `url` (an asset pack) and upload it.
    pub async fn load_pack(&mut self, url: &str) -> Result<LoadReport, String> {
        let bytes = fetch(url).await?;
        self.load_pack_bytes(&bytes)
            .map_err(|e| format!("{url}: {e}"))
    }

    /// Upload an asset pack already in memory.
    pub fn load_pack_bytes(&mut self, bytes: &[u8]) -> Result<LoadReport, String> {
        let report = crate::assets::load(
            bytes,
            &mut self.renderer,
            &mut self.audio,
            &mut self.platform,
        )?;
        crate::js::log(&report.line(self.texture_target));
        Ok(report)
    }

    /// The game's picture size (the renderer's internal resolution).
    pub fn screen(&self) -> Vec2 {
        let (w, h) = self.renderer.internal_size();
        Vec2::new(w as f32, h as f32)
    }

    /// Apply a canvas resize: the canvas's backing store, the surface, and
    /// on the native-like profiles the picture (the output over the pixel
    /// scale). Returns the new picture size when it changed.
    pub fn sync_size(&mut self) -> Option<Vec2> {
        let size = self.observed.get();
        if size == self.applied {
            return None;
        }
        self.applied = size;
        self.canvas.set_width(size.0.max(1));
        self.canvas.set_height(size.1.max(1));
        self.renderer.resize(size.0.max(1), size.1.max(1));
        {
            let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
            let mut s = self.input.shared.borrow_mut();
            s.canvas_px = (size.0 as f64, size.1 as f64);
            s.dpr = dpr;
        }
        let before = self.renderer.internal_size();
        if self.config.profile.is_native() {
            let (w, h, _) = native_internal(size, self.config.pixel_scale);
            self.renderer.set_internal_size(w, h);
        }
        self.publish_display();
        (self.renderer.internal_size() != before).then(|| self.screen())
    }

    fn publish_display(&mut self) {
        let (w, h) = self.renderer.internal_size();
        let fullscreen = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.fullscreen_element())
            .is_some();
        let screen = web_sys::window()
            .and_then(|w| w.screen().ok())
            .map_or((0, 0), |s| {
                (
                    s.width().unwrap_or(0) as u16,
                    s.height().unwrap_or(0) as u16,
                )
            });
        self.platform.display = Some(DisplayInfo {
            settings: DisplaySettings {
                fullscreen,
                window_w: self.applied.0 as u16,
                window_h: self.applied.1 as u16,
                pixel_scale: self.config.pixel_scale as u8,
                ui_scale: 0,
            },
            output: (self.applied.0 as u16, self.applied.1 as u16),
            monitor: screen,
            screen: (w as u16, h as u16),
            pixel_scale: native_internal(self.applied, self.config.pixel_scale).2 as u8,
        });
    }

    /// Ask the browser for fullscreen on the canvas (it needs a gesture:
    /// a key or click handler's frame).
    pub fn set_fullscreen(&self, on: bool) {
        let document = web_sys::window().and_then(|w| w.document());
        if on {
            if let Err(e) = self.canvas.request_fullscreen() {
                crate::js::log(&format!("warning: fullscreen refused: {e:?}"));
            }
        } else if let Some(d) = document
            && d.fullscreen_element().is_some()
        {
            d.exit_fullscreen();
        }
    }

    /// Hide the cursor and move a virtual pointer by the mouse's deltas
    /// (3D navigation); Escape gives the cursor back (the browser's rule).
    pub fn request_pointer_lock(&self) {
        self.canvas.request_pointer_lock();
    }

    /// The frame's input: gestures unlock audio, the pointer maps to game
    /// pixels.
    pub fn poll_input(&mut self) -> trino_core::InputState {
        if self.input.take_gesture() {
            self.audio.unlock();
        }
        let px = self.input.shared.borrow().pointer_px;
        self.input.pointer = px.and_then(|(x, y)| self.renderer.window_to_pointer(x, y));
        let state = self.input.poll();
        self.platform.input = Some(InputInfo {
            gamepad: self.input.gamepad_status(),
            capturing: None,
        });
        self.platform.pad_name = self.input.gamepad_name();
        // What the game sees, for the harness (and anyone debugging input
        // in devtools: `__trino.info.input`).
        let held: Vec<String> = trino_core::Button::ALL
            .iter()
            .filter(|b| state.is_down(**b))
            .map(|b| format!("{b:?}"))
            .collect();
        let summary = format!(
            "{} stick={},{} pointer={}",
            held.join("+"),
            state.stick.x,
            state.stick.y,
            state
                .pointer
                .map_or("none".into(), |p| format!("{:.0},{:.0}", p.x, p.y))
        );
        if summary != self.last_input {
            crate::js::trino_info("input", &summary);
            self.last_input = summary;
        }
        state
    }

    /// Per-frame housekeeping after the game ran: audio voices and the
    /// banner, the game's display requests, the frame meter.
    pub fn end_frame(&mut self) {
        self.audio.poll();
        let running = self.audio.running();
        if self.last_audio != Some(running) {
            crate::js::trino_audio_banner(!running);
            crate::js::trino_info("audio", self.audio_state());
            self.last_audio = Some(running);
        }
        self.frames += 1;
        crate::js::trino_info("frames", &self.frames.to_string());
        if let Some(req) = self.platform.take_display_request() {
            self.set_fullscreen(req.fullscreen);
        }
        self.meter.record(
            &Renderer::stats(&self.renderer),
            &self.renderer.pc_gpu_rejects(),
        );
    }
}

/// Fetch `url`'s bytes.
pub async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|e| format!("{url}: {e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| format!("{url}: no response"))?;
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    let buf =
        wasm_bindgen_futures::JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?)
            .await
            .map_err(|e| format!("{url}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// What a frame closure wants next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

/// Call `frame(host, dt)` once per animation frame (`dt` in seconds,
/// clamped to 0.1 like the PC) until it returns [`Flow::Stop`].
pub fn run(host: WebHost, mut frame: impl FnMut(&mut WebHost, f32) -> Flow + 'static) {
    let host = Rc::new(RefCell::new(host));
    let last = Rc::new(Cell::new(None::<f64>));
    let tick: Rc<RefCell<Option<FrameCallback>>> = Rc::new(RefCell::new(None));
    let again = tick.clone();
    *tick.borrow_mut() = Some(Closure::<dyn FnMut(f64)>::new(move |now: f64| {
        let dt = last
            .get()
            .map_or(1.0 / 60.0, |t| ((now - t) / 1000.0) as f32)
            .clamp(0.0, 0.1);
        last.set(Some(now));
        let flow = frame(&mut host.borrow_mut(), dt);
        if flow == Flow::Continue
            && let Some(cb) = again.borrow().as_ref()
            && let Some(w) = web_sys::window()
        {
            let _ = w.request_animation_frame(cb.as_ref().unchecked_ref());
        }
    }));
    if let Some(w) = web_sys::window() {
        let _ = w.request_animation_frame(tick.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    }
    // The closure lives as long as the page.
    std::mem::forget(tick);
}

/// Run `make(screen)`'s game on the page: the backend on `config`, the
/// pack at `pack_url`, then a frame per animation frame. Errors show over
/// the canvas.
pub fn run_game<G: Game + 'static>(
    config: WebConfig,
    pack_url: String,
    make: impl FnOnce(Vec2) -> G + 'static,
) {
    install_panic_hook();
    wasm_bindgen_futures::spawn_local(async move {
        let mut host = match WebHost::new(config).await {
            Ok(h) => h,
            Err(e) => return fail(&e),
        };
        if let Err(e) = host.load_pack(&pack_url).await {
            return fail(&e);
        }
        let mut game = make(host.screen());
        // `?frames=N`: a smoke run (`cargo xtask test web` on a project),
        // like the PC's TRINO_SMOKE_FRAMES.
        let smoke = Query::from_location()
            .get("frames")
            .and_then(|f| f.parse::<u64>().ok());
        let mut frames = 0u64;
        run(host, move |host, dt| {
            if let Some(screen) = host.sync_size() {
                game.resize(screen);
            }
            let input = host.poll_input();
            game.update_with(&mut host.platform, &input, &mut host.audio, dt);
            game.render(&mut host.renderer);
            host.end_frame();
            frames += 1;
            if smoke.is_some_and(|n| frames >= n) {
                crate::js::log(&host.meter.line("game"));
                crate::js::trino_finish(true, None);
                return Flow::Stop;
            }
            Flow::Continue
        });
    });
}

/// A fatal setup error: on the page and in the console.
pub fn fail(e: &str) {
    crate::js::log(&format!("TRINO_TEST_FAIL:{e}"));
    crate::js::trino_error_overlay(e);
    crate::js::trino_finish(false, Some(e.to_string()));
}
