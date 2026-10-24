//! wgpu 2D sprite renderer with an internal offscreen framebuffer.
//!
//! Two construction modes:
//! - [`PcRenderer::new_windowed`]: renders offscreen, then blits to the
//!   window surface with nearest-neighbor integer upscaling (letterboxed).
//! - [`PcRenderer::new_headless`]: offscreen only; tests read pixels back
//!   with [`PcRenderer::read_offscreen`]. This is also the path the editor
//!   viewport reuses in Fase 3 (render-to-texture).

mod ao;
mod batch;
mod bench;
mod cpu_tnl;
mod cvd;
mod depth_blit;
mod gpu_timer;
mod gpu_tnl;
mod gpu_zones;
mod lights;
pub(crate) mod pipes;
mod post;
pub use post::PostView;
mod precompile;
#[doc(hidden)]
pub use precompile::Hold as PrecompileHold;
mod shadow;
mod shadow_map;
mod stereo;
pub use stereo::{FLIP_FRAMES as STEREO_FLIP_FRAMES, StereoView};
mod upscale;
mod water;

use std::collections::HashMap;
use std::sync::Arc;
use web_time::Instant;

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, MeshLayout, PcGpuReject, ScreenTri, SurfaceScratch,
    SurfaceVert, TnlContext, TnlScratch,
};
use trino_core::{
    AmbientOcclusion, AoSetting, AoTier, Caps, Color, FontId, FrameLimit, FrameStats, Instance,
    LightList, LightTier, LocalLight, Mat34, Material, ModelId, ModelParams, Palette,
    QualitySettings, Renderer, Ripple, Screen, ShadowFrame, ShadowPlane, ShadowSetting,
    ShadowSettings, ShadowTier, SpriteId, SpriteParams, TexFilter, TextParams, Transform3, Vec2,
    WaterState, WaterTier,
};

use crate::sim::SimProfile;
pub use ao::AoView;
use batch::{
    Cmd, INSTANCE_STRIDE, Segment, TRI_VERTEX_STRIDE, TexKey, TriKind, TriVertex, build_segments,
    make_command,
};
pub use bench::{Bench, Calibration};
use cvd::CvdKind;
pub use shadow::ShadowView;

const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Debug)]
pub enum RendererError {
    NoAdapter(String),
    Device(String),
    Surface(String),
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoAdapter(e) => write!(f, "no compatible GPU adapter: {e}"),
            RendererError::Device(e) => write!(f, "failed to open GPU device: {e}"),
            RendererError::Surface(e) => write!(f, "surface error: {e}"),
        }
    }
}

impl std::error::Error for RendererError {}

struct SpriteEntry {
    bind_group: wgpu::BindGroup,
    /// Kept for the 3D (wrapping) bind group, created on first 3D use.
    view: wgpu::TextureView,
    size: (u32, u32),
    /// GPU bytes of the texture, and whether it is block-compressed
    /// ([`PcRenderer::sprite_memory`]).
    bytes: u64,
    compressed: bool,
    /// Some texel is below full alpha (it blends): a model drawn with it
    /// stays on the CPU path (`PcGpuReject::TextureAlpha`).
    alpha: bool,
}

/// A block-compressed sprite format the renderer uploads as is
/// ([`PcRenderer::upload_sprite_compressed`]); 4×4 blocks in row-major
/// order. The PC bakes BC; the Web transcodes its KTX2 textures to
/// whichever of these the browser's GPU samples (issue #47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressedFormat {
    /// 8 bytes a block: color with 1-bit alpha.
    Bc1,
    /// 16 bytes: BC1 color plus interpolated alpha.
    Bc3,
    /// 16 bytes: RGBA.
    Bc7,
    /// 8 bytes: ETC2 color (phones; ETC1 blocks are ETC2 blocks).
    Etc2Rgb8,
    /// 16 bytes: ETC2 color plus EAC alpha.
    Etc2Rgba8,
    /// 16 bytes: ASTC 4×4 (RGBA).
    Astc4x4,
}

impl CompressedFormat {
    fn wgpu(self) -> wgpu::TextureFormat {
        // Unorm, like the RGBA8 sprites (`OFFSCREEN_FORMAT`): colors pass
        // through unconverted.
        match self {
            CompressedFormat::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm,
            CompressedFormat::Bc3 => wgpu::TextureFormat::Bc3RgbaUnorm,
            CompressedFormat::Bc7 => wgpu::TextureFormat::Bc7RgbaUnorm,
            CompressedFormat::Etc2Rgb8 => wgpu::TextureFormat::Etc2Rgb8Unorm,
            CompressedFormat::Etc2Rgba8 => wgpu::TextureFormat::Etc2Rgba8Unorm,
            CompressedFormat::Astc4x4 => wgpu::TextureFormat::Astc {
                block: wgpu::AstcBlock::B4x4,
                channel: wgpu::AstcChannel::Unorm,
            },
        }
    }

    pub fn block_bytes(self) -> u32 {
        match self {
            CompressedFormat::Bc1 | CompressedFormat::Etc2Rgb8 => 8,
            CompressedFormat::Bc3
            | CompressedFormat::Bc7
            | CompressedFormat::Etc2Rgba8
            | CompressedFormat::Astc4x4 => 16,
        }
    }

    /// The device feature that samples it.
    pub fn feature(self) -> wgpu::Features {
        match self {
            CompressedFormat::Bc1 | CompressedFormat::Bc3 | CompressedFormat::Bc7 => {
                wgpu::Features::TEXTURE_COMPRESSION_BC
            }
            CompressedFormat::Etc2Rgb8 | CompressedFormat::Etc2Rgba8 => {
                wgpu::Features::TEXTURE_COMPRESSION_ETC2
            }
            CompressedFormat::Astc4x4 => wgpu::Features::TEXTURE_COMPRESSION_ASTC,
        }
    }

    /// A BC format (the PC's; [`PcRenderer::set_bc`] gates them).
    pub fn is_bc(self) -> bool {
        matches!(
            self,
            CompressedFormat::Bc1 | CompressedFormat::Bc3 | CompressedFormat::Bc7
        )
    }
}

/// What the device behind a renderer can't do, by backend
/// ([`PcRenderer::new_canvas`] on WebGL2). Desktop backends and WebGPU
/// have everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BackendLimits {
    /// wgpu's GL backend in a browser: WebGL2's downlevel limits and no
    /// multisampled depth sampling (no MSAA).
    webgl2: bool,
    /// The adapter can't render or blend `Rgba16Float` (WebGL2 without
    /// `EXT_color_buffer_float`): the post stack runs over an 8-bit scene.
    no_float_targets: bool,
}

/// The graphics API a browser renderer asks for ([`PcRenderer::new_canvas`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebBackend {
    /// WebGPU when the browser gives an adapter, else WebGL2.
    #[default]
    Auto,
    WebGpu,
    WebGl2,
}

impl WebBackend {
    /// `auto`, `webgpu`, `webgl2` (`webgl` and `gl` too): the Web app's
    /// `?backend=` value.
    pub fn parse(s: &str) -> Option<WebBackend> {
        match s.to_ascii_lowercase().as_str() {
            "auto" | "" => Some(WebBackend::Auto),
            "webgpu" | "gpu" => Some(WebBackend::WebGpu),
            "webgl2" | "webgl" | "gl" => Some(WebBackend::WebGl2),
            _ => None,
        }
    }
}

/// The sprite textures a renderer holds ([`PcRenderer::sprite_memory`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpriteMemory {
    pub textures: u32,
    /// GPU bytes of every sprite texture.
    pub bytes: u64,
    /// Of those, the block-compressed ones.
    pub compressed: u32,
    pub compressed_bytes: u64,
}

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// The present modes the surface offers (the frame limit picks one).
    modes: Vec<wgpu::PresentMode>,
}

/// A registered font: its glyph table (validated at upload) and the direct
/// index of its codepoints below 256.
struct FontEntry {
    table: Box<[u8]>,
    index: trino_core::text::GlyphIndex,
}

impl FontEntry {
    fn font(&self) -> trino_core::text::BitmapFont<'_> {
        trino_core::text::BitmapFont::parse(&self.table)
            .expect("validated on upload")
            .with_index(&self.index)
    }
}

/// Where a PC frame's rendering time goes on the CPU (wall clock, µs), and
/// what it hands the GPU: [`PcRenderer::frame_times`] of the last frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PcFrameTimes {
    /// The software T&L of every model draw (`tessellate_with` /
    /// `tessellate_surface`).
    pub tnl_us: u32,
    /// Model batches: the painter sort and the expansion into GPU
    /// vertices.
    pub batch_us: u32,
    /// `end_frame`'s uploads, command encoding, submits and present.
    pub submit_us: u32,
    /// GPU draw calls (instanced sprite runs and triangle runs, both
    /// screens).
    pub draw_calls: u32,
    /// Model draws shaded per pixel (`tessellate_surface`: a local light,
    /// gloss or wetness reaches them); the rest take `tessellate_with`.
    pub lit_draws: u32,
    /// Bytes written to GPU buffers (sprite instances, triangle vertices,
    /// light sets, AO parameters).
    pub upload_bytes: u32,
    /// Screen-space AO passes run (0 or 1: the frame's 3D, once).
    pub ao_passes: u32,
    /// Pipeline changes between those draw calls (what a browser's
    /// WebGL2 or WebGPU pays for in JavaScript, issue #47).
    pub pipeline_switches: u32,
    /// Upscale passes run (0 or 1: a render scale other than 1 stretched
    /// the frame's 3D onto the picture, issue #55).
    pub upscales: u32,
    /// Shadow map passes run (0 or 1: the light pass and its composite,
    /// the frame's 3D, once).
    pub shadow_maps: u32,
    /// Post-processing passes run (0 or 1: the frame's 3D through the
    /// post stack, issue #64), and the levels of its bloom chain.
    pub post_passes: u32,
    pub bloom_levels: u32,
    /// Render pipelines the render thread compiled during this frame
    /// (lazy first uses; 0 once a full [`PcRenderer::precompile`] ran:
    /// `TRINO_FRAME`'s `pipes=`, issue #101).
    pub pipelines: u32,
}

/// Where the renderer keeps what outlives a run (issue #101).
#[derive(Clone, Debug, Default)]
pub struct RendererOptions {
    /// The folder of the persistent pipeline cache
    /// (`<dir>/<wgpu::util::pipeline_cache_key>.bin`): the player's save
    /// folder, never a build-tree path. `None`: no cache file. Only Vulkan
    /// adapters with `PIPELINE_CACHE` use it; other drivers keep their own.
    pub pipeline_cache_dir: Option<std::path::PathBuf>,
}

/// Pipelines created so far on this renderer's device
/// ([`PcRenderer::pipeline_counts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineCounts {
    /// On the render thread (startup and lazy first uses).
    pub render_thread: u32,
    /// By precompile workers.
    pub precompiled: u32,
    /// On the render thread during the last frame (since the frame
    /// before ended: what `update` built counts).
    pub last_frame: u32,
    /// Built outside any renderer's scope, process-wide (a lazy site that
    /// lost its scope: always 0).
    pub unscoped: u32,
}

/// The pipeline cache a renderer was created with, and its file.
struct CacheFile {
    cache: wgpu::PipelineCache,
    path: std::path::PathBuf,
}

/// Microseconds since `t0`.
/// The instance flags of [`PcRenderer::new_headless`]: the build's defaults
/// without [`wgpu::InstanceFlags::DEBUG`], so no object names reach the
/// driver, then the `WGPU_*` environment variables (`WGPU_DEBUG=1` puts the
/// names back). On AMD RADV, `vkSetDebugUtilsObjectNameEXT` from one
/// thread while another created its device crashed whole test binaries
/// inside `libvulkan` (GitHub #94: 9 runs in 12 of `engine_checks`).
pub fn headless_flags() -> wgpu::InstanceFlags {
    (wgpu::InstanceFlags::from_build_config() - wgpu::InstanceFlags::DEBUG).with_env()
}

fn micros(t0: Instant) -> u32 {
    t0.elapsed().as_micros().min(u32::MAX as u128) as u32
}

pub struct PcRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<SurfaceState>,
    offscreen_view: wgpu::TextureView,
    offscreen_tex: wgpu::Texture,
    depth_view: wgpu::TextureView,
    /// The depth target itself: copied for the water's depth foam.
    depth_tex: wgpu::Texture,
    sprite_pipeline: wgpu::RenderPipeline,
    tri_pipeline: wgpu::RenderPipeline,
    /// Same as `tri_pipeline` with clamped 3-point sampling (billboards).
    tri_clamp_pipeline: wgpu::RenderPipeline,
    /// Per-pixel lighting (`LightTier::PerPixel`): local lights, specular
    /// and wetness on top of the vertex color.
    lit_pipeline: wgpu::RenderPipeline,
    /// What the scene pipelines are built from, and the four again for a
    /// multisampled target, built by the first [`PcRenderer::set_msaa`]
    /// that turns it on: sprite, tri, tri clamp, lit. (Building them for
    /// every renderer made parallel headless tests crash RADV.)
    sources: SceneSources,
    ms_pipelines: Option<[wgpu::RenderPipeline; 4]>,
    /// Samples per pixel of the main screen's scene pass (1 = off), and
    /// its multisampled color and depth (resolved into the offscreen
    /// target at the end of the pass).
    msaa: u32,
    msaa_targets: Option<MsaaTargets>,
    /// WebGL2 (wgpu's GL backend in a browser): no multisampled depth to
    /// sample (GLSL ES 3.0 has no `sampler2DMS`), so [`PcRenderer::set_msaa`]
    /// keeps one sample, and shaders read depth as a float texture
    /// ([`depth_as_float`]).
    webgl2: bool,
    /// WebGL2's depth copy for the water (built on its first use).
    depth_blit: Option<depth_blit::DepthBlit>,
    lights_buf: wgpu::Buffer,
    lights_bg: wgpu::BindGroup,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group: wgpu::BindGroup,
    blit_sampler: wgpu::Sampler,
    sprite_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    globals_bg: wgpu::BindGroup,
    globals_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    instance_capacity: u64,
    tri_buf: wgpu::Buffer,
    tri_capacity: u64,
    sprites: HashMap<u32, SpriteEntry>,
    /// Sprite textures as 3D textures (base + detail layer, their samplers),
    /// by texture state.
    tex3d: HashMap<TexKey, wgpu::BindGroup>,
    tex3d_bgl: wgpu::BindGroupLayout,
    /// Wrapping (models) and clamp-to-edge (billboards: sprite regions must
    /// not bleed) samplers, nearest and linear.
    tex3d_nearest: wgpu::Sampler,
    tex3d_linear: wgpu::Sampler,
    clamp_nearest: wgpu::Sampler,
    clamp_linear: wgpu::Sampler,
    /// 1x1 white: what untextured triangles "sample" (×1 = unchanged), and
    /// a draw without a detail layer.
    white_view: wgpu::TextureView,
    white_bg: wgpu::BindGroup,
    /// What `TexFilter::Default` means (`set_texture_filter`; the profile's
    /// own: nearest native, the 3-point look on N64, bilinear on 3DS).
    tex_filter: TexFilter,
    /// TMDL blobs with their validated layout (parsed once, at register).
    meshes: HashMap<u32, (Vec<u8>, MeshLayout)>,
    /// Glyph tables (`TFNT`) by font handle; the atlas is the sprite with
    /// the same handle. `Arc` so `draw_text` can lay out while drawing.
    fonts: HashMap<u32, Arc<FontEntry>>,
    commands: Vec<Cmd>,
    /// Counters of the frame being recorded / the last finished frame.
    frame_stats: FrameStats,
    /// The N64 profile's frame as the console would split it (strict mode).
    n64: N64Split,
    last_stats: FrameStats,
    /// The native profile's draws off the GPU T&L path, per
    /// [`PcGpuReject`], of the frame being recorded / the last one.
    pc_gpu_off: [u32; PcGpuReject::COUNT],
    last_pc_gpu_off: [u32; PcGpuReject::COUNT],
    /// CPU time per rendering stage and the GPU work handed over, of the
    /// frame being recorded / the last finished frame ([`PcFrameTimes`]).
    times: PcFrameTimes,
    last_times: PcFrameTimes,
    tri_verts: Vec<TriVertex>,
    /// Triangles of the current model batch: consecutive `draw_model` calls
    /// accumulate here and depth-sort together (painter across meshes) when
    /// a sprite draw, a camera change or `end_frame` flushes the batch.
    pending_tris: Vec<ScreenTri>,
    /// Texture of each pending triangle (parallel to `pending_tris`; only
    /// allocated into when the batch has textured draws).
    pending_tex: Vec<Option<TexKey>>,
    /// Per-pixel lighting inputs of each pending triangle (parallel; only
    /// allocated into once a lit draw joins the batch).
    pending_surf: Vec<Option<SurfTri>>,
    /// This frame's local lights and wetness (`set_local_lights`,
    /// `set_wetness`; cleared at `begin_frame`), and the lighting states
    /// lit draws used (the lit pipeline's uniform).
    locals: LightList,
    wetness: f32,
    /// This frame's screen-space AO (`set_ambient_occlusion`, cleared at
    /// `begin_frame`), what shows of it (`set_ao_view`, `TRINO_AO`) and
    /// its passes (built on first use).
    ao: Option<AmbientOcclusion>,
    ao_view: AoView,
    ao_pass: Option<ao::AoPass>,
    /// The same over the multisampled depth (MSAA on).
    ao_pass_ms: Option<ao::AoPass>,
    light_sets: lights::LightSets,
    /// This frame's shadows (`set_shadows`, `set_shadow_receivers`;
    /// cleared at `begin_frame`), what shows of them (`set_shadow_view`,
    /// `TRINO_SHADOWS`), the batch's blob and planar shadow triangles
    /// (drawn after its opaque ones, before its water), their pipelines
    /// (built on first use, again for MSAA) and the softness the blob
    /// texture was made with.
    shadows: ShadowFrame,
    shadow_game: Option<ShadowSettings>,
    shadow_view: ShadowView,
    pending_shadow: Vec<(ScreenTri, shadow::ShadowRun)>,
    shadow_pipes: Option<shadow::ShadowPipes>,
    shadow_pipes_ms: Option<shadow::ShadowPipes>,
    blob_softness: Option<u8>,
    /// The Map tier: this frame's casters (drawn into the map at the end
    /// of the 3D), their world triangles, and the map's passes (built on
    /// first use, again over the multisampled depth).
    map_casters: Vec<MapCaster>,
    map_verts: Vec<[f32; 3]>,
    shadow_map: Option<shadow_map::MapPass>,
    shadow_map_ms: Option<shadow_map::MapPass>,
    /// This frame's water clock and ripples (`set_water`, cleared at
    /// `begin_frame`), the water surfaces drawn (the water pipelines'
    /// uniform) and their GPU side.
    water_state: WaterState,
    water_sets: water::WaterSets,
    water: WaterGpu,
    /// The T&L's working memory for per-pixel lit draws.
    surf_scratch: Box<SurfaceScratch>,
    camera: Camera3,
    light: Light,
    /// T&L constants of `camera` + `light` at the current internal size
    /// (rebuilt when any of them changes).
    tnl_ctx: Option<(TnlContext, LightList)>,
    /// The T&L's working memory, reused by every draw (boxed: ~9 KB).
    tnl_scratch: Box<TnlScratch>,
    clear: Color,
    caps: Caps,
    profile: SimProfile,
    internal_size: (u32, u32),
    n64_look: bool,
    /// N64 Video Interface dedither ("restore") filter, run on the 16-bit
    /// look at `end_frame` — see [`PcRenderer::set_vi_dedither`].
    vi_dedither: bool,
    vi_pipeline: wgpu::RenderPipeline,
    vi_tex: wgpu::Texture,
    /// The color-vision simulation view (issue #60): off unless a tool
    /// asks ([`PcRenderer::set_cvd_view`], `TRINO_CVD`).
    cvd_view: trino_core::CvdMode,
    /// Its pass, built on first use.
    cvd: Option<cvd::CvdPass>,
    /// The player's daltonization filter (issue #64): the mode the whole
    /// picture is corrected for on the native profile and the Web
    /// ([`PcRenderer::set_cvd_filter`]; the host passes
    /// `AccessSettings::cvd` when `cvd_filter` is on).
    cvd_filter: trino_core::CvdMode,
    /// The post stack (issue #64): this frame's settings
    /// (`Renderer::set_post`, cleared at `begin_frame`), what shows of them
    /// ([`PcRenderer::set_post_view`], `TRINO_POST`), whether the scene
    /// targets exist for it, the HDR twins of the scene pipelines (single,
    /// multisampled), the bloom + composite pass (all built on first use),
    /// the grading LUTs uploaded, whether a frame used it (precompile) and
    /// the grain's frame counter.
    post: Option<trino_core::PostSettings>,
    post_view: PostView,
    post_targets: bool,
    hdr_sets: [Option<Box<post::HdrSet>>; 2],
    post_pass: Option<post::PostPass>,
    luts: HashMap<trino_core::LutId, post::LutTex>,
    post_used: bool,
    post_seed: u32,
    /// The adapter renders and blends `Rgba16Float`.
    hdr_ok: bool,
    /// Stereoscopic 3D (issue #63, the 3DS profile): this frame's
    /// recording and eye, the state each eye's replay starts from, how the
    /// eyes are shown and the simulated 3D slider
    /// ([`PcRenderer::set_stereo_view`], `TRINO_STEREO`), and the pass that
    /// keeps the eyes and composes the preview (built on first use).
    stereo: stereo::Stereo,
    stereo_start: Box<TopState>,
    stereo_view: StereoView,
    stereo_slider: f32,
    stereo_pass: Option<stereo::StereoPass>,
    /// The brightest full-screen fill this frame (relative luminance ×
    /// alpha × coverage), for the strobe check (issue #60).
    flash_level: f32,
    /// Strobes in the stream of frames (more than three flashes a second);
    /// strict mode warns on each.
    flash_monitor: trino_core::FlashMonitor,
    flash_frames: u64,
    strict: bool,
    /// The main screen's safe area when the host knows better than the
    /// profile (a test, a TV's overscan setting).
    safe_area: Option<trino_core::Insets>,
    /// The window's unsafe borders, window pixels (`left, top, right,
    /// bottom`): a phone's notch and home bar on the Web.
    window_safe: [f32; 4],
    /// The last UI frame's counts ([`Renderer::ui_stats`]).
    ui_stats: Option<trino_core::UiStats>,
    /// The 3DS profile's touch screen (320×240, 2D and 3D).
    second: Option<SecondScreen>,
    /// "name (backend)" of the GPU adapter.
    adapter_name: String,
    /// The device samples BC textures (`TEXTURE_COMPRESSION_BC`) and
    /// [`PcRenderer::set_bc`] hasn't turned them off.
    bc: bool,
    /// Where draws go now (0 main, 1 second). `set_screen(1)` renders the
    /// main screen right away and keeps its counts (commands, triangles):
    /// strict mode's budgets and the stats cover the whole frame.
    screen: u8,
    main_cmds: u32,
    main_tris: u32,
    /// The GPU T&L path (issue #51): on for the native profile unless
    /// `TRINO_PC_TNL=cpu` or [`PcRenderer::set_hw_tnl`] turned it off; its
    /// pipelines and resident meshes, built on the first GPU draw.
    hw_tnl: bool,
    gpu: Option<gpu_tnl::GpuTnl>,
    /// This device's pipeline cache and counters ([`pipes`]).
    pipe_entry: Arc<pipes::DeviceEntry>,
    /// The file the pipeline cache saves to.
    cache_file: Option<std::path::PathBuf>,
    /// The warm-up of the last [`PcRenderer::precompile`].
    warm: precompile::Warmup,
    /// Its threads ([`trino_core::ThreadSetting`], `TRINO_THREADS`).
    precompile_threads: u32,
    /// A warm-up ran since the pipeline cache was last saved.
    cache_unsaved: bool,
    /// Tests: the next warm-up's jobs wait at this gate
    /// ([`PcRenderer::hold_precompile`]).
    warm_hold: Option<Arc<precompile::Hold>>,
    /// The render thread's pipeline count at the last `end_frame`: a frame
    /// counts what was built since (between frames too: `set_msaa`, a
    /// preset applied in `update`).
    pipes_at_begin: u32,
    /// Group 0's layout (the GPU T&L pipelines share it).
    globals_bgl: wgpu::BindGroupLayout,
    /// The native profile's CPU draws of the current batch (tessellated at
    /// the flush, over `tnl_threads` workers), the T&L contexts they use
    /// and the workers' scratch memory.
    cpu_jobs: Vec<cpu_tnl::CpuJob>,
    job_ctxs: Vec<TnlContext>,
    workers: Vec<cpu_tnl::Scratch>,
    tnl_threads: usize,
    /// The quality settings this renderer applies (issue #55,
    /// [`PcRenderer::apply_quality`]; the native profile only).
    quality: QualityState,
    /// The 3D's own targets while its size differs from the picture's, or
    /// dynamic resolution is on (one allocation for its whole range), and
    /// the upscale onto the picture.
    scene: Option<upscale::SceneTargets>,
    upscale: Option<upscale::Upscale>,
    /// Dynamic resolution: the controller, the GPU timer it reads where the
    /// adapter has timestamps (built on first use), the host's frame
    /// interval otherwise, and the last measurement.
    dynres: Option<crate::dynres::DynResController>,
    gpu_timer: Option<gpu_timer::GpuTimer>,
    gpu_timer_tried: bool,
    /// The trace's GPU zones (issue #62): built on the first frame a trace
    /// records, on adapters with timestamps.
    gpu_zones: Option<gpu_zones::GpuZones>,
    gpu_zones_tried: bool,
    host_frame_us: Option<u32>,
    last_gpu_us: u32,
    /// Bilinear 3D textures' sampler with the quality's anisotropy (`None`
    /// at 1×).
    tex3d_aniso: Option<wgpu::Sampler>,
    /// What the game asked for with `set_texture_filter` (`Default`: the
    /// quality's, else the profile's).
    game_filter: TexFilter,
    /// The adapter's kind ([`PcRenderer::gpu_class`]).
    gpu_class: trino_core::GpuClass,
    /// Render without showing the frame (the auto-detect bench).
    no_present: bool,
}

/// The quality knobs a renderer holds ([`PcRenderer::apply_quality`]).
#[derive(Clone, Copy, Debug)]
struct QualityState {
    /// The settings last applied (`None`: never, the renderer's defaults).
    applied: Option<QualitySettings>,
    /// The 3D's scale now (dynamic resolution moves it).
    scale: f32,
    /// Caps the game's AO quality, or drops the pass (`None`: the game's
    /// as it sets it).
    ao: Option<AoSetting>,
    /// The shadows' level (`None`: never applied, the full Map tier).
    shadows: Option<ShadowSetting>,
    /// The post stack's level (`None`: never applied, High).
    post: Option<trino_core::PostSetting>,
    lod_bias: f32,
    sharpen: f32,
    anisotropy: u8,
    filter: TexFilter,
    frame_limit: FrameLimit,
    /// The display's refresh rate (the host's monitor; 60 by default).
    refresh_hz: u32,
}

impl QualityState {
    const DEFAULT: QualityState = QualityState {
        applied: None,
        scale: 1.0,
        ao: None,
        shadows: None,
        post: None,
        lod_bias: 1.0,
        sharpen: 0.0,
        anisotropy: 1,
        filter: TexFilter::Default,
        frame_limit: FrameLimit::VSync,
        refresh_hz: 60,
    };
}

/// What a lit triangle adds to its vertices: the surface, the gloss and the
/// light set (or, for water, its water set).
#[derive(Clone, Copy, Debug)]
struct SurfTri {
    v: [SurfaceVert; 3],
    gloss: f32,
    set: u32,
    /// The local lights that reach the draw (`lights::reach_mask`).
    mask: u32,
    /// A water triangle (`set` is a water set): the water pipelines, after
    /// the batch's other triangles.
    water: bool,
}

const NO_SURFACE: SurfaceVert = SurfaceVert {
    pos: trino_core::Vec3::ZERO,
    normal: trino_core::Vec3::ZERO,
    base: Color::WHITE,
};

/// The water pipelines and their group 3: the water sets and a copy of
/// the scene's depth (the per-pixel tier's foam reads it).
struct WaterGpu {
    bgl: wgpu::BindGroupLayout,
    buf: wgpu::Buffer,
    depth_copy: wgpu::Texture,
    bg: wgpu::BindGroup,
    /// WebGL2's nearest sampler for the depth copy (`depth_as_float`).
    nearest: Option<wgpu::Sampler>,
    /// `fs_water`: the per-pixel tier (native).
    per_pixel: wgpu::RenderPipeline,
    /// `fs_water_vertex`: the console tiers' combiner (N64 look, 3DS).
    vertex: wgpu::RenderPipeline,
    /// Groups 0..2 of the water layout (globals, texture, light sets), kept
    /// to build the multisampled set.
    groups: [wgpu::BindGroupLayout; 3],
    /// The same pipelines for the multisampled scene pass, and a
    /// multisampled depth copy: built by the first [`PcRenderer::set_msaa`].
    ms: Option<WaterMs>,
}

/// The water pipelines over a multisampled target (MSAA on). The foam
/// reads the multisampled depth's first sample, as the SSAO does.
struct WaterMs {
    bgl: wgpu::BindGroupLayout,
    depth_copy: wgpu::Texture,
    bg: wgpu::BindGroup,
    per_pixel: wgpu::RenderPipeline,
    vertex: wgpu::RenderPipeline,
}

/// WebGL2's copy of a shader that reads a depth texture. naga's GLSL
/// backend can't `textureLoad` a `texture_depth_2d`, so the binding becomes
/// an unfilterable float texture (WebGPU and wgpu accept a depth view
/// there, [`depth_sample_type`]); and wgpu's GL backend leaves a depth
/// texture's own filter at GL's mipmapped-linear default, which makes it
/// incomplete for a depth format (a `texelFetch` reads 0), so each
/// shader's one depth read samples it through a nearest sampler
/// ([`NEAREST_DEPTH_BINDING`]) at the texel's center.
pub(crate) fn depth_as_float(source: &str) -> String {
    let mut out = source.replace("texture_depth_2d", "texture_2d<f32>");
    for (texture, group, binding) in [("t_scene_depth", 3, 2), ("t_depth", 0, 3)] {
        let read = format!("return textureLoad({texture}, p, 0);");
        if !out.contains(&read) {
            continue;
        }
        let sampler = format!("s_{texture}");
        out = out.replace(
            &read,
            &format!(
                "return textureSampleLevel({texture}, {sampler}, (vec2<f32>(p) + vec2<f32>(0.5)) \
                 / vec2<f32>(textureDimensions({texture})), 0.0).r;"
            ),
        );
        let decl = format!("var {texture}: texture_2d<f32>;");
        out = out.replace(
            &decl,
            &format!("{decl}\n@group({group}) @binding({binding}) var {sampler}: sampler;"),
        );
    }
    out
}

/// The binding of the nearest sampler [`depth_as_float`] adds on WebGL2:
/// the water's group 3 and the AO's group 0.
pub(crate) const NEAREST_DEPTH_BINDING: (u32, u32) = (2, 3);

/// A non-filtering sampler for the WebGL2 depth reads.
pub(crate) fn nearest_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("trino-depth-nearest"),
        ..Default::default()
    })
}

/// The layout entry of that sampler at `binding`.
pub(crate) fn nearest_sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
        count: None,
    }
}

/// How a shader binds the scene's depth: as depth, or on WebGL2 as an
/// unfilterable float texture ([`depth_as_float`]).
pub(crate) fn depth_sample_type(webgl2: bool) -> wgpu::TextureSampleType {
    if webgl2 {
        wgpu::TextureSampleType::Float { filterable: false }
    } else {
        wgpu::TextureSampleType::Depth
    }
}

/// The group 3 layout of the water pipelines: the sets and the scene's
/// depth copy (multisampled with MSAA on).
fn water_bgl(device: &wgpu::Device, multisampled: bool, webgl2: bool) -> wgpu::BindGroupLayout {
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: depth_sample_type(webgl2),
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        },
    ];
    // WebGL2 reads the depth through a nearest sampler (`depth_as_float`).
    if webgl2 {
        entries.push(nearest_sampler_entry(NEAREST_DEPTH_BINDING.0));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("trino-water-bgl"),
        entries: &entries,
    })
}

/// The two water pipelines (`fs_water`, `fs_water_vertex`) for a target
/// with `samples` samples per pixel. Translucent: depth-tested, never
/// written.
fn water_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    groups: [&wgpu::BindGroupLayout; 4],
    samples: u32,
    format: wgpu::TextureFormat,
) -> [wgpu::RenderPipeline; 2] {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("trino-water-layout"),
        bind_group_layouts: &groups.map(Some),
        immediate_size: 0,
    });
    let attrs = lit_attributes();
    let make = |fragment: &str| {
        crate::renderer::pipes::render(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("trino-water-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_tri_lit"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: TRI_VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            },
        )
    };
    [make("fs_water"), make("fs_water_vertex")]
}

impl WaterGpu {
    fn depth_copy(device: &wgpu::Device, size: (u32, u32), samples: u32) -> wgpu::Texture {
        // A render attachment too: multisampled textures must be one
        // (WebGPU), and WebGL2 copies the depth by drawing it
        // (`depth_blit`).
        let usage = wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT;
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-water-depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage,
            view_formats: &[],
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        bgl: &wgpu::BindGroupLayout,
        buf: &wgpu::Buffer,
        depth: &wgpu::Texture,
        nearest: Option<&wgpu::Sampler>,
    ) -> wgpu::BindGroup {
        let view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
        ];
        if let Some(s) = nearest {
            entries.push(wgpu::BindGroupEntry {
                binding: NEAREST_DEPTH_BINDING.0,
                resource: wgpu::BindingResource::Sampler(s),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-water-bg"),
            layout: bgl,
            entries: &entries,
        })
    }

    /// Builds the multisampled set once (MSAA turned on).
    fn ensure_ms(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.ms.is_some() {
            return;
        }
        let built = Self::ms_pipelines(device, &self.groups);
        self.install_ms(device, size, built);
    }

    /// The multisampled set's layout and pipelines: the part of
    /// [`ensure_ms`](Self::ensure_ms) a precompile job runs on a worker.
    fn ms_pipelines(
        device: &wgpu::Device,
        groups: &[wgpu::BindGroupLayout; 3],
    ) -> (wgpu::BindGroupLayout, [wgpu::RenderPipeline; 2]) {
        let source = include_str!("shaders.wgsl")
            .replace("texture_depth_2d", "texture_depth_multisampled_2d");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("trino-shaders-ms-water"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let bgl = water_bgl(device, true, false);
        let [g0, g1, g2] = groups;
        let pipes = water_pipelines(
            device,
            &shader,
            [g0, g1, g2, &bgl],
            MSAA_SAMPLES,
            OFFSCREEN_FORMAT,
        );
        (bgl, pipes)
    }

    /// Finish the multisampled set from built pipelines (its depth copy and
    /// bind group); kept if it already exists.
    fn install_ms(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        (bgl, [per_pixel, vertex]): (wgpu::BindGroupLayout, [wgpu::RenderPipeline; 2]),
    ) {
        if self.ms.is_some() {
            return;
        }
        let depth_copy = Self::depth_copy(device, size, MSAA_SAMPLES);
        let bg = Self::bind_group(device, &bgl, &self.buf, &depth_copy, None);
        self.ms = Some(WaterMs {
            bgl,
            depth_copy,
            bg,
            per_pixel,
            vertex,
        });
    }

    /// A new size: new depth copies (and their bind groups).
    fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        self.depth_copy = Self::depth_copy(device, size, 1);
        self.bg = Self::bind_group(
            device,
            &self.bgl,
            &self.buf,
            &self.depth_copy,
            self.nearest.as_ref(),
        );
        if let Some(ms) = &mut self.ms {
            ms.depth_copy = Self::depth_copy(device, size, MSAA_SAMPLES);
            ms.bg = Self::bind_group(device, &ms.bgl, &self.buf, &ms.depth_copy, None);
        }
    }
}

/// What `TexFilter::Default` means on a profile: nearest texels on the PC
/// (pixel art stays crisp, as always), the RDP's 3-point bilinear in the
/// N64 look, bilinear on the 3DS (its GPU default).
fn default_filter(profile: SimProfile) -> TexFilter {
    match profile {
        SimProfile::Native | SimProfile::Web => TexFilter::Point,
        SimProfile::N64 | SimProfile::N3ds => TexFilter::Bilinear,
    }
}

/// A 3D texture bind group: the base texture and the detail layer, each
/// with its sampler.
fn create_tex3d_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    base: (&wgpu::TextureView, &wgpu::Sampler),
    detail: (&wgpu::TextureView, &wgpu::Sampler),
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("trino-tex3d-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(base.0),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(base.1),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(detail.0),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(detail.1),
            },
        ],
    })
}

/// The second screen's own target, globals and blit source.
struct SecondScreen {
    size: (u32, u32),
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    globals_bg: wgpu::BindGroup,
    blit_bg: wgpu::BindGroup,
}

/// Offscreen color target + its view + the matching depth buffer.
fn create_targets(
    device: &wgpu::Device,
    size: (u32, u32),
) -> (
    wgpu::Texture,
    wgpu::TextureView,
    wgpu::TextureView,
    wgpu::Texture,
) {
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let offscreen_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trino-offscreen"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let offscreen_view = offscreen_tex.create_view(&wgpu::TextureViewDescriptor::default());
    // Depth buffer for the 3D triangle pipeline (sprites test Always).
    let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trino-depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        // Read back by the screen-space AO passes; copied for the water's
        // depth foam.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
    (offscreen_tex, offscreen_view, depth_view, depth_tex)
}

/// Samples per pixel when multisampling is on.
pub const MSAA_SAMPLES: u32 = 4;
/// The shader and layouts the scene pipelines are built from, kept so the
/// multisampled set ([`PcRenderer::set_msaa`]) is only built when asked for.
#[derive(Clone)]
struct SceneSources {
    shader: wgpu::ShaderModule,
    sprite_layout: wgpu::PipelineLayout,
    tri_layout: wgpu::PipelineLayout,
    lit_layout: wgpu::PipelineLayout,
}

/// The screen-space triangle vertex (`TriVertex`): position, color, depth,
/// params, UV and the detail layer's UV.
fn tri_attributes() -> [wgpu::VertexAttribute; 6] {
    [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 24,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 28,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 32,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 88,
            shader_location: 10,
        },
    ]
}

/// The lit vertex: the triangle's attributes plus the per-pixel inputs
/// (world position, normal, base color, light set, gloss).
fn lit_attributes() -> Vec<wgpu::VertexAttribute> {
    let extra = [
        (wgpu::VertexFormat::Float32x3, 40, 5),
        (wgpu::VertexFormat::Float32x3, 52, 6),
        (wgpu::VertexFormat::Float32x4, 64, 7),
        (wgpu::VertexFormat::Float32, 80, 8),
        (wgpu::VertexFormat::Float32, 84, 9),
    ];
    let mut a = tri_attributes().to_vec();
    a.extend(
        extra
            .iter()
            .map(|&(format, offset, shader_location)| wgpu::VertexAttribute {
                format,
                offset,
                shader_location,
            }),
    );
    a
}

/// The four scene pipelines for a target with `samples` samples per pixel:
/// sprite, tri, tri clamp (billboards), lit.
fn scene_pipelines(
    device: &wgpu::Device,
    src: &SceneSources,
    samples: u32,
    format: wgpu::TextureFormat,
) -> [wgpu::RenderPipeline; 4] {
    let SceneSources {
        shader,
        sprite_layout,
        tri_layout,
        lit_layout,
    } = src;
    let instance_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 24,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 32,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 40,
            shader_location: 5,
        },
    ];

    let make_sprite_pipeline = |samples: u32| {
        crate::renderer::pipes::render(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("trino-sprite-pipeline"),
                layout: Some(sprite_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_sprite"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: INSTANCE_STRIDE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attrs,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_sprite"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                // Sprites are 2D overlay: draw in submission order, never test
                // or write depth (matches the N64's rdpq sprite path).
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            },
        )
    };
    let tri_attrs = tri_attributes();
    let make_tri_pipeline = |fragment: &str, samples: u32| {
        crate::renderer::pipes::render(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("trino-tri-pipeline"),
                layout: Some(tri_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_tri"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: TRI_VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &tri_attrs,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                // Real per-pixel occlusion: interpenetrating meshes (a cap
                // through a head) resolve correctly, which no painter sort can.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            },
        )
    };
    let lit_attrs = lit_attributes();
    let make_lit_pipeline = |samples: u32| {
        crate::renderer::pipes::render(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("trino-lit-pipeline"),
                layout: Some(lit_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_tri_lit"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: TRI_VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &lit_attrs,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_tri_lit"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            },
        )
    };
    [
        make_sprite_pipeline(samples),
        make_tri_pipeline("fs_tri", samples),
        make_tri_pipeline("fs_tri_clamp", samples),
        make_lit_pipeline(samples),
    ]
}

/// How a scene pass treats the multisampled targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MsPass {
    /// Single-sampled: MSAA off, the second screen, the 2D after the AO.
    Off,
    /// Multisampled, the scene's last pass: the color resolves into the
    /// offscreen target, the depth is kept when the AO pass reads it.
    Resolve { keep_depth: bool },
    /// Multisampled, more of the scene to come in another pass (the
    /// per-pixel water reads a copy of the depth in between): color and
    /// depth are stored, nothing resolves yet.
    Hold,
}

/// The multisampled color and depth of the scene pass (resolved into the
/// offscreen target).
struct MsaaTargets {
    /// The color's format: the picture's, or the post stack's HDR one.
    format: wgpu::TextureFormat,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// The depth texture itself: the per-pixel water copies it.
    depth_tex: wgpu::Texture,
}

fn create_msaa_targets(
    device: &wgpu::Device,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> MsaaTargets {
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let make = |label, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: MSAA_SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let target = wgpu::TextureUsages::RENDER_ATTACHMENT;
    let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
    let color = make("trino-msaa-color", format, target);
    // The SSAO pass reads it; the per-pixel water copies it.
    let depth_tex = make(
        "trino-msaa-depth",
        wgpu::TextureFormat::Depth32Float,
        target | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
    );
    MsaaTargets {
        format,
        color: view(&color),
        depth: view(&depth_tex),
        depth_tex,
    }
}

/// Scratch target of the VI pass (copied back into the offscreen target).
fn create_vi_target(device: &wgpu::Device, size: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trino-vi"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn create_blit_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("trino-blit-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// Uniform buffer contents; layout mirrored by `Globals` in shaders.wgsl.
fn globals_bytes(width: u32, height: u32, n64_look: bool) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&(width as f32).to_le_bytes());
    b[4..8].copy_from_slice(&(height as f32).to_le_bytes());
    b[8..12].copy_from_slice(&(n64_look as u32).to_le_bytes());
    b
}

/// `TRINO_POWER=low|high`: the adapter to ask for where a machine has
/// two (an iGPU and a discrete GPU), for measurements. Default: wgpu's.
fn power_from_env() -> wgpu::PowerPreference {
    match std::env::var("TRINO_POWER").as_deref() {
        Ok("low") => wgpu::PowerPreference::LowPower,
        Ok("high") => wgpu::PowerPreference::HighPerformance,
        _ => wgpu::PowerPreference::default(),
    }
}

/// `TRINO_PC_TNL` ([`pc_tnl_setting`]). The console profiles always keep
/// the CPU path.
fn hw_tnl_from_env() -> bool {
    pc_tnl_setting(std::env::var("TRINO_PC_TNL").ok().as_deref())
}

/// `TRINO_PC_TNL`'s value: `cpu` (or `0`, `off`) keeps every draw on the
/// CPU T&L; `gpu`, `auto` and anything else (unset too) send the draws
/// `pc_gpu_check` accepts to the GPU.
fn pc_tnl_setting(value: Option<&str>) -> bool {
    !matches!(value, Some("cpu" | "0" | "off"))
}

/// `TRINO_PC_TNL_THREADS`: workers of the native profile's CPU T&L (1 =
/// the calling thread only). Default: the machine's parallelism, at most 8.
/// The precompile workers: `TRINO_THREADS` (`auto`, `N`, `-N`), else all
/// logical CPUs but one, at least one ([`trino_core::ThreadSetting`]).
fn precompile_threads_from_env() -> u32 {
    let setting = std::env::var("TRINO_THREADS")
        .ok()
        .and_then(|v| trino_core::ThreadSetting::parse(&v))
        .unwrap_or_default();
    setting.resolve(available_cpus())
}

/// Logical CPUs (1 when the OS won't say; 1 on the Web).
pub fn available_cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// The cache file of `info`'s adapter in `dir`, opened into a
/// [`wgpu::PipelineCache`] (`None` without `PIPELINE_CACHE` or a key).
/// A missing, foreign or corrupt file starts an empty cache (`fallback`).
fn open_pipeline_cache(
    device: &wgpu::Device,
    info: &wgpu::AdapterInfo,
    dir: &std::path::Path,
) -> Option<CacheFile> {
    if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        return None;
    }
    let key = wgpu::util::pipeline_cache_key(info)?;
    let path = dir.join(format!("{key}.bin"));
    let data = std::fs::read(&path).ok();
    // SAFETY: the data is what `PipelineCache::get_data` wrote for an
    // adapter with this key; wgpu and the driver validate its header and
    // `fallback` starts empty when they reject it.
    let cache = unsafe {
        device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
            label: Some("trino-pipeline-cache"),
            data: data.as_deref(),
            fallback: true,
        })
    };
    Some(CacheFile { cache, path })
}

fn tnl_threads_from_env() -> usize {
    std::env::var("TRINO_PC_TNL_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(8)
        })
        .max(1)
}

impl PcRenderer {
    /// Renderer that presents to a window.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_windowed(
        window: Arc<winit::window::Window>,
        profile: SimProfile,
    ) -> Result<Self, RendererError> {
        Self::new_windowed_with(window, profile, &RendererOptions::default()).await
    }

    /// [`new_windowed`](Self::new_windowed) with [`RendererOptions`] (the
    /// runner's pipeline cache folder).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new_windowed_with(
        window: Arc<winit::window::Window>,
        profile: SimProfile,
        options: &RendererOptions,
    ) -> Result<Self, RendererError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| RendererError::Surface(e.to_string()))?;
        let size = window.inner_size();
        let mut r = Self::init(
            instance,
            Some((surface, size.width.max(1), size.height.max(1))),
            profile,
            options,
        )
        .await?;
        // A game window honors TRINO_AO=off|on|mask (headless renderers,
        // the tests', never read it).
        if let Some(view) = std::env::var("TRINO_AO")
            .ok()
            .and_then(|v| AoView::parse(&v))
        {
            r.set_ao_view(view);
        }
        // And TRINO_STEREO=anaglyph|side-by-side|left|right|flip[:slider]
        // (the 3DS profile's stereo preview, issue #63).
        if let Some((view, slider)) = std::env::var("TRINO_STEREO")
            .ok()
            .and_then(|v| StereoView::parse_with_slider(&v))
        {
            r.set_stereo_view(view);
            r.set_stereo_slider(slider);
        }
        // And TRINO_CVD=protanopia|deuteranopia|tritanopia|grayscale.
        if let Some(mode) = std::env::var("TRINO_CVD")
            .ok()
            .and_then(|v| trino_core::CvdMode::parse(&v))
        {
            r.set_cvd_view(mode);
        }
        // And TRINO_SHADOWS=off|debug.
        if let Some(view) = std::env::var("TRINO_SHADOWS")
            .ok()
            .and_then(|v| ShadowView::parse(&v))
        {
            r.set_shadow_view(view);
        }
        Ok(r)
    }

    /// Renderer that presents to a browser canvas (the Web backend, issue
    /// #47), `width`×`height` physical pixels: WebGPU where the browser has
    /// it ([`WebBackend::Auto`] falls back to WebGL2 when `navigator.gpu`
    /// gives no adapter), or the API `backend` names.
    #[cfg(target_arch = "wasm32")]
    pub async fn new_canvas(
        canvas: web_sys::HtmlCanvasElement,
        width: u32,
        height: u32,
        profile: SimProfile,
        backend: WebBackend,
    ) -> Result<Self, RendererError> {
        /// A page's display: wgpu creates canvas surfaces against one.
        #[derive(Debug)]
        struct WebDisplay;
        impl wgpu::rwh::HasDisplayHandle for WebDisplay {
            fn display_handle(
                &self,
            ) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
                let raw = wgpu::rwh::RawDisplayHandle::Web(wgpu::rwh::WebDisplayHandle::new());
                // SAFETY: a web display handle holds no pointer; it stays
                // valid for the page's life.
                Ok(unsafe { wgpu::rwh::DisplayHandle::borrow_raw(raw) })
            }
        }
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(WebDisplay));
        desc.backends = match backend {
            WebBackend::Auto => wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            WebBackend::WebGpu => wgpu::Backends::BROWSER_WEBGPU,
            WebBackend::WebGl2 => wgpu::Backends::GL,
        };
        if backend == WebBackend::WebGpu && !wgpu::util::is_browser_webgpu_supported().await {
            return Err(RendererError::NoAdapter(
                "this browser gives no WebGPU adapter (try ?backend=webgl2)".into(),
            ));
        }
        let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| RendererError::Surface(e.to_string()))?;
        Self::init(
            instance,
            Some((surface, width.max(1), height.max(1))),
            profile,
            &RendererOptions::default(),
        )
        .await
    }

    /// Offscreen-only renderer for tests and tooling.
    ///
    /// Its instance gives the driver no debug names ([`headless_flags`]):
    /// tests create renderers on parallel threads, and naming objects then
    /// crashed RADV's Vulkan loader (GitHub #94). `WGPU_DEBUG=1` turns the
    /// names back on for a debugger or a capture tool.
    pub async fn new_headless(profile: SimProfile) -> Result<Self, RendererError> {
        Self::new_headless_with(profile, &RendererOptions::default()).await
    }

    /// [`new_headless`](Self::new_headless) with [`RendererOptions`] (tests
    /// of the pipeline cache file).
    pub async fn new_headless_with(
        profile: SimProfile,
        options: &RendererOptions,
    ) -> Result<Self, RendererError> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.flags = headless_flags();
        let instance = wgpu::Instance::new(desc);
        Self::init(instance, None, profile, options).await
    }

    /// Offscreen renderer on an existing device — the editor viewport path:
    /// sharing eframe's device lets the offscreen texture be registered
    /// directly as an egui texture (render-to-texture, zero copies).
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue, profile: SimProfile) -> Self {
        Self::build(
            device,
            queue,
            None,
            profile,
            "editor device".into(),
            BackendLimits::default(),
            None,
        )
    }

    async fn init(
        instance: wgpu::Instance,
        surface: Option<(wgpu::Surface<'static>, u32, u32)>,
        profile: SimProfile,
        options: &RendererOptions,
    ) -> Result<Self, RendererError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface.as_ref().map(|(s, _, _)| s),
                power_preference: power_from_env(),
                ..Default::default()
            })
            .await
            .map_err(|e| RendererError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        // WebGL2 (wgpu's GL backend in a browser) has the downlevel WebGL2
        // limits: no storage buffers, 16 KB uniform blocks, no sampling of
        // multisampled depth.
        let float = adapter.get_texture_format_features(post::HDR_FORMAT);
        let limits = BackendLimits {
            webgl2: cfg!(target_arch = "wasm32") && info.backend == wgpu::Backend::Gl,
            no_float_targets: !float
                .allowed_usages
                .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
                || !float
                    .flags
                    .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE),
        };

        // Compressed textures where the adapter samples them: BC on every
        // desktop GPU (`TRINO_BC=0` leaves it off, the runner then uploads
        // decoded pixels), ETC2 and ASTC on phones (the Web's KTX2
        // textures transcode to whichever the browser has).
        let bc_off = std::env::var("TRINO_BC").is_ok_and(|v| v == "0");
        // Timestamps time the GPU's frame for dynamic resolution (#55).
        let mut wanted = wgpu::Features::TEXTURE_COMPRESSION_ETC2
            | wgpu::Features::TEXTURE_COMPRESSION_ASTC
            | wgpu::Features::TIMESTAMP_QUERY;
        // A persistent pipeline cache where the driver offers one (Vulkan,
        // issue #101), only when the host gives it a folder.
        if options.pipeline_cache_dir.is_some() {
            wanted |= wgpu::Features::PIPELINE_CACHE;
        }
        if !bc_off {
            wanted |= wgpu::Features::TEXTURE_COMPRESSION_BC;
        }
        let required_features = adapter.features() & wanted;
        let base = if limits.webgl2 {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            wgpu::Limits::downlevel_defaults()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("trino-pc"),
                required_features,
                // Downlevel limits, but the adapter's real texture size:
                // a 4K or ultrawide window (surface) and full-resolution
                // pictures need more than downlevel's 2048.
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d.min(8192),
                    ..base
                },
                ..Default::default()
            })
            .await
            .map_err(|e| RendererError::Device(e.to_string()))?;

        let surface = surface.map(|(surface, width, height)| {
            let surface_caps = surface.get_capabilities(&adapter);
            // Prefer a non-sRGB format so 8-bit colors pass through the blit
            // unchanged (retro palettes must not get gamma-shifted twice).
            // 8-bit formats first: some compositors (Wayland) list 16-bit
            // normalized formats ahead of them, and those need a device
            // feature (TEXTURE_FORMAT_16BIT_NORM) the renderer does not
            // request — picking one panicked at pipeline creation.
            use wgpu::TextureFormat as F;
            let format = [F::Bgra8Unorm, F::Rgba8Unorm]
                .into_iter()
                .find(|f| surface_caps.formats.contains(f))
                .or_else(|| {
                    surface_caps
                        .formats
                        .iter()
                        .copied()
                        .find(|f| !f.is_srgb() && f.required_features().is_empty())
                })
                .unwrap_or(surface_caps.formats[0]);
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width,
                height,
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: surface_caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            surface.configure(&device, &config);
            SurfaceState {
                surface,
                config,
                modes: surface_caps.present_modes.clone(),
            }
        });

        // Browsers hide the GPU's name from WebGPU pages.
        let shown = if info.name.is_empty() {
            "browser adapter"
        } else {
            &info.name
        };
        let name = format!("{shown} ({:?})", info.backend);
        let cache = options
            .pipeline_cache_dir
            .as_deref()
            .and_then(|dir| open_pipeline_cache(&device, &info, dir));
        let mut r = Self::build(device, queue, surface, profile, name, limits, cache);
        r.gpu_class = bench::classify_adapter(&info);
        Ok(r)
    }

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: Option<SurfaceState>,
        profile: SimProfile,
        adapter_name: String,
        limits: BackendLimits,
        cache: Option<CacheFile>,
    ) -> Self {
        let (cache, cache_file) = match cache {
            Some(c) => (Some(c.cache), Some(c.path)),
            None => (None, None),
        };
        let pipe_entry = pipes::DeviceEntry::new(cache);
        let _pipes = pipes::scope(&pipe_entry);
        let device_bc = device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let internal_size = profile.internal_resolution();
        let mut caps = profile.caps();
        // The picture size games see is the internal resolution (the
        // native profile's table says 1080p, its picture starts at 640×480).
        caps.screen_width = internal_size.0 as u16;
        caps.screen_height = internal_size.1 as u16;
        // The N64 profile emulates the console's output (3-point filtering,
        // RGBA5551 + dither) by default; TRINO_LOOK=off|n64 overrides.
        let n64_look = match std::env::var("TRINO_LOOK").as_deref() {
            Ok("off" | "0" | "native") => false,
            Ok("n64" | "1" | "on") => true,
            _ => profile == SimProfile::N64,
        };
        // Strict mode: enforce the profile's Caps at development time.
        let strict = matches!(std::env::var("TRINO_STRICT").as_deref(), Ok("1" | "true"));

        let (offscreen_tex, offscreen_view, depth_view, depth_tex) =
            create_targets(&device, internal_size);

        // The 3DS GPU samples textures bilinearly by default (citro2d), so
        // its sim profile does too; N64/native use nearest (the N64's
        // 3-point filter is emulated in the shader instead).
        let sprite_filter = if profile == SimProfile::N3ds {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-sprite-sampler"),
            mag_filter: sprite_filter,
            min_filter: sprite_filter,
            ..Default::default()
        });
        // 3D draws choose per draw (`TexFilter`): nearest and linear, each
        // wrapping and clamped (the N64 look filters in the shader, from
        // nearest texels).
        let tex3d_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-tex3d-nearest"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let clamp_nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-clamp-nearest"),
            ..Default::default()
        });
        let tex3d_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-tex3d-linear"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let clamp_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-clamp-linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // The window blit stays nearest: consoles output at native
        // resolution; the integer upscale is a PC artifact and must not blur.
        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("trino-blit-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let source = include_str!("shaders.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shaders.wgsl"),
            source: wgpu::ShaderSource::Wgsl(if limits.webgl2 {
                depth_as_float(source).into()
            } else {
                source.into()
            }),
        });

        // Group 0: globals uniform (internal resolution).
        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-globals-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &globals_buf,
            0,
            &globals_bytes(internal_size.0, internal_size.1, n64_look),
        );
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-globals-bg"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        // Group layout shared by sprite textures and the blit source.
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-sprite-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&texture_bgl)],
            immediate_size: 0,
        });

        // 3D triangle pipeline: same offscreen target; vertex colors
        // modulated by one texture per segment (white when untextured) and
        // its detail layer (white when none).
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let tex3d_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-tex3d-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                tex_entry(2),
                sampler_entry(3),
            ],
        });
        let tri_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-tri-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&tex3d_bgl)],
            immediate_size: 0,
        });

        // Lit pipeline: the same vertices with the per-pixel inputs, and
        // the frame's light sets in group 2.
        let lights_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trino-lights-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-light-sets"),
            size: (lights::SET_BYTES * lights::MAX_SETS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-lights-bg"),
            layout: &lights_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: lights_buf.as_entire_binding(),
            }],
        });
        let lit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-lit-layout"),
            bind_group_layouts: &[Some(&globals_bgl), Some(&tex3d_bgl), Some(&lights_bgl)],
            immediate_size: 0,
        });
        let sources = SceneSources {
            shader,
            sprite_layout,
            tri_layout,
            lit_layout,
        };
        let [
            sprite_pipeline,
            tri_pipeline,
            tri_clamp_pipeline,
            lit_pipeline,
        ] = scene_pipelines(&device, &sources, 1, OFFSCREEN_FORMAT);

        // Water: the lit vertices, groups 0..2 as the lit pipeline (the
        // light sets stay bound) and the water sets + the scene's depth in
        // group 3. Translucent: depth-tested, never written.
        let water_bgl = water_bgl(&device, false, limits.webgl2);
        let water_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-water-sets"),
            size: (water::SET_BYTES * water::MAX_SETS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let [water_per_pixel, water_vertex] = water_pipelines(
            &device,
            &sources.shader,
            [&globals_bgl, &tex3d_bgl, &lights_bgl, &water_bgl],
            1,
            OFFSCREEN_FORMAT,
        );
        let water_depth = WaterGpu::depth_copy(&device, internal_size, 1);
        let water_nearest = limits.webgl2.then(|| nearest_sampler(&device));
        let water = WaterGpu {
            bg: WaterGpu::bind_group(
                &device,
                &water_bgl,
                &water_buf,
                &water_depth,
                water_nearest.as_ref(),
            ),
            nearest: water_nearest,
            bgl: water_bgl,
            buf: water_buf,
            depth_copy: water_depth,
            per_pixel: water_per_pixel,
            vertex: water_vertex,
            groups: [globals_bgl.clone(), tex3d_bgl.clone(), lights_bgl.clone()],
            ms: None,
        };

        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("trino-blit-layout"),
            bind_group_layouts: &[Some(&texture_bgl)],
            immediate_size: 0,
        });
        let blit_format = surface
            .as_ref()
            .map(|s| s.config.format)
            .unwrap_or(OFFSCREEN_FORMAT);
        let blit_pipeline = crate::renderer::pipes::render(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("trino-blit-pipeline"),
                layout: Some(&blit_layout),
                vertex: wgpu::VertexState {
                    module: &sources.shader,
                    entry_point: Some("vs_blit"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &sources.shader,
                    entry_point: Some("fs_blit"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: blit_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );

        let blit_bind_group =
            create_blit_bind_group(&device, &texture_bgl, &offscreen_view, &blit_sampler);
        // The 3DS profile simulates the touch screen too.
        let second = (profile == SimProfile::N3ds).then(|| {
            let size = (320, 240);
            let (tex, view, depth_view, _) = create_targets(&device, size);
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-globals-2"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buf, 0, &globals_bytes(size.0, size.1, false));
            let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trino-globals-bg-2"),
                layout: &globals_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            let blit_bg = create_blit_bind_group(&device, &texture_bgl, &view, &blit_sampler);
            SecondScreen {
                size,
                tex,
                view,
                depth_view,
                globals_bg,
                blit_bg,
            }
        });
        // VI pass: reads the offscreen target (through the blit bind group)
        // into a scratch texture of the same format.
        let vi_pipeline = crate::renderer::pipes::render(
            &device,
            wgpu::RenderPipelineDescriptor {
                label: Some("trino-vi-pipeline"),
                layout: Some(&blit_layout),
                vertex: wgpu::VertexState {
                    module: &sources.shader,
                    entry_point: Some("vs_blit"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &sources.shader,
                    entry_point: Some("fs_vi_dedither"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OFFSCREEN_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let vi_tex = create_vi_target(&device, internal_size);

        let white_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let white_bg = create_tex3d_bind_group(
            &device,
            &tex3d_bgl,
            (&white_view, &tex3d_nearest),
            (&white_view, &tex3d_nearest),
        );
        let tex_filter = default_filter(profile);

        let instance_capacity = 1024 * INSTANCE_STRIDE;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-instances"),
            size: instance_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tri_capacity = 3 * 1024 * TRI_VERTEX_STRIDE;
        let tri_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-tri-verts"),
            size: tri_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // What the constructor built is not the first frame's.
        let pipes_at_start = pipe_entry
            .render_thread
            .load(std::sync::atomic::Ordering::Relaxed);
        PcRenderer {
            device,
            queue,
            surface,
            offscreen_view,
            offscreen_tex,
            depth_view,
            depth_tex,
            sprite_pipeline,
            tri_pipeline,
            tri_clamp_pipeline,
            lit_pipeline,
            sources,
            ms_pipelines: None,
            msaa: 1,
            webgl2: limits.webgl2,
            depth_blit: None,
            msaa_targets: None,
            lights_buf,
            lights_bg,
            blit_pipeline,
            blit_bind_group,
            blit_sampler,
            sprite_bgl: texture_bgl,
            sampler,
            globals_bg,
            globals_buf,
            instance_buf,
            instance_capacity,
            tri_buf,
            tri_capacity,
            sprites: HashMap::new(),
            tex3d: HashMap::new(),
            tex3d_bgl,
            tex3d_nearest,
            tex3d_linear,
            clamp_nearest,
            clamp_linear,
            white_view,
            white_bg,
            tex_filter,
            meshes: HashMap::new(),
            fonts: HashMap::new(),
            commands: Vec::new(),
            frame_stats: FrameStats::default(),
            n64: N64Split::default(),
            pc_gpu_off: [0; PcGpuReject::COUNT],
            last_pc_gpu_off: [0; PcGpuReject::COUNT],
            times: PcFrameTimes::default(),
            last_times: PcFrameTimes::default(),
            last_stats: FrameStats::default(),
            tri_verts: Vec::new(),
            pending_tris: Vec::new(),
            pending_tex: Vec::new(),
            pending_surf: Vec::new(),
            locals: LightList::EMPTY,
            wetness: 0.0,
            ao: None,
            ao_view: AoView::Game,
            ao_pass: None,
            ao_pass_ms: None,
            light_sets: lights::LightSets::default(),
            water_state: WaterState::NONE,
            water_sets: water::WaterSets::default(),
            water,
            shadows: ShadowFrame::OFF,
            shadow_game: None,
            shadow_view: std::env::var("TRINO_SHADOWS")
                .ok()
                .and_then(|v| ShadowView::parse(&v))
                .unwrap_or_default(),
            pending_shadow: Vec::new(),
            shadow_pipes: None,
            shadow_pipes_ms: None,
            map_casters: Vec::new(),
            map_verts: Vec::new(),
            shadow_map: None,
            shadow_map_ms: None,
            blob_softness: None,
            surf_scratch: Box::new(SurfaceScratch::new()),
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
            tnl_ctx: None,
            tnl_scratch: Box::new(TnlScratch::new()),
            clear: Color::BLACK,
            caps,
            profile,
            internal_size,
            n64_look,
            vi_dedither: true,
            cvd_view: trino_core::CvdMode::Off,
            cvd: None,
            cvd_filter: trino_core::CvdMode::Off,
            post: None,
            post_view: std::env::var("TRINO_POST")
                .ok()
                .and_then(|v| PostView::parse(&v))
                .unwrap_or_default(),
            post_targets: false,
            hdr_sets: [None, None],
            post_pass: None,
            luts: HashMap::new(),
            post_used: false,
            post_seed: 0,
            hdr_ok: !limits.no_float_targets,
            stereo: stereo::Stereo::new(),
            stereo_start: Box::new(TopState {
                camera: Camera3::default(),
                light: DEFAULT_LIGHT,
                locals: LightList::EMPTY,
                game_filter: TexFilter::Default,
                wetness: 0.0,
                water: WaterState::NONE,
                shadow_game: None,
                shadows: ShadowFrame::OFF,
            }),
            stereo_view: StereoView::Off,
            stereo_slider: 1.0,
            stereo_pass: None,
            flash_level: 0.0,
            flash_monitor: trino_core::FlashMonitor::new(),
            flash_frames: 0,
            vi_pipeline,
            vi_tex,
            strict,
            safe_area: None,
            window_safe: [0.0; 4],
            ui_stats: None,
            second,
            screen: 0,
            main_cmds: 0,
            main_tris: 0,
            bc: device_bc,
            adapter_name,
            hw_tnl: hw_tnl_from_env(),
            gpu: None,
            pipe_entry,
            cache_file,
            warm: precompile::Warmup::default(),
            precompile_threads: precompile_threads_from_env(),
            pipes_at_begin: pipes_at_start,
            cache_unsaved: false,
            warm_hold: None,
            globals_bgl,
            cpu_jobs: Vec::new(),
            job_ctxs: Vec::new(),
            workers: Vec::new(),
            tnl_threads: tnl_threads_from_env(),
            quality: QualityState::DEFAULT,
            scene: None,
            upscale: None,
            dynres: None,
            gpu_timer: None,
            gpu_timer_tried: false,
            gpu_zones: None,
            gpu_zones_tried: false,
            host_frame_us: None,
            last_gpu_us: 0,
            tex3d_aniso: None,
            game_filter: TexFilter::Default,
            gpu_class: trino_core::GpuClass::Unknown,
            no_present: false,
        }
    }

    /// Register a baked TMDL mesh under a stable handle. Re-uploading the
    /// same id replaces the content (live reload).
    pub fn upload_mesh(&mut self, id: ModelId, tmdl: Vec<u8>) {
        let layout = match Mesh::from_tmdl(&tmdl) {
            Ok(m) => m.layout(&tmdl),
            Err(e) => panic!("mesh {id:?}: invalid TMDL blob: {e:?}"),
        };
        // Resident on the GPU at register when the GPU T&L path is on;
        // otherwise (and after a re-upload) at its first GPU draw.
        if self.gpu_tnl_active() {
            let mesh = Mesh::from_layout(&tmdl, &layout);
            self.ensure_gpu_tnl();
            if let Some(gpu) = &mut self.gpu {
                gpu.upload_mesh(&self.device, &self.queue, id.0, &mesh);
            }
        } else if let Some(gpu) = &mut self.gpu {
            gpu.forget_mesh(id.0);
        }
        self.meshes.insert(id.0, (tmdl, layout));
    }

    /// Send the draws `pc_gpu_check` accepts to the GPU (the default on the
    /// native profile; `TRINO_PC_TNL=cpu` starts with it off), or keep
    /// every draw on the CPU T&L — the A/B switch for regressions and
    /// bisecting. The console profiles always keep the CPU path. Takes
    /// effect at the next draw.
    pub fn set_hw_tnl(&mut self, on: bool) {
        self.hw_tnl = on;
    }

    /// Does the GPU T&L path take draws now ([`PcRenderer::set_hw_tnl`],
    /// the native profile, not the N64 look)?
    pub fn hw_tnl(&self) -> bool {
        self.gpu_tnl_active()
    }

    /// The [`PcRenderer::set_hw_tnl`] switch itself (on a console profile
    /// too, where it has no effect).
    pub fn hw_tnl_setting(&self) -> bool {
        self.hw_tnl
    }

    /// Workers of the native profile's CPU T&L (1 = on the calling thread;
    /// default `TRINO_PC_TNL_THREADS` or the machine's parallelism, at most
    /// 8). The triangles and counters are the same for any count.
    pub fn set_tnl_threads(&mut self, threads: usize) {
        // A browser's wasm has no threads (no SharedArrayBuffer build).
        self.tnl_threads = if cfg!(target_arch = "wasm32") {
            1
        } else {
            threads.max(1)
        };
    }

    pub fn tnl_threads(&self) -> usize {
        self.tnl_threads
    }

    /// Bytes of mesh vertices resident on the GPU (the GPU T&L path's
    /// buffers, uploaded once per mesh).
    pub fn resident_mesh_bytes(&self) -> u64 {
        self.gpu.as_ref().map_or(0, |g| g.resident_bytes())
    }

    fn gpu_tnl_active(&self) -> bool {
        self.hw_tnl && self.profile.is_native() && !self.n64_look
    }

    /// Build the GPU T&L path's pipelines and buffers (on first use).
    fn ensure_gpu_tnl(&mut self) {
        let _pipes = pipes::scope(&self.pipe_entry);
        self.warm_wait(precompile::JobKind::GpuTnl);
        if self.msaa > 1 {
            self.warm_wait(precompile::JobKind::GpuTnlMs);
        }
        let samples = self.msaa;
        if self.gpu.is_none() {
            self.warm_wait(precompile::JobKind::GpuTnl);
        }
        let _pipes = pipes::scope(&self.pipe_entry);
        let gpu = self.gpu.get_or_insert_with(|| {
            gpu_tnl::GpuTnl::new(&self.device, [&self.globals_bgl, &self.tex3d_bgl])
        });
        if samples > 1 {
            gpu.ensure_ms(&self.device, samples);
        }
    }

    /// Build ahead the passes this renderer would otherwise compile the
    /// first time a frame needs them (issue #101, ADR 0031). `Background`
    /// runs them on [`precompile_threads`](Self::precompile_threads)
    /// workers (one per frame on the Web) while frames keep presenting;
    /// `Blocking` returns when they are built. A new request first finishes
    /// the last one. Never run by the constructors: eager pipelines in every
    /// renderer made parallel headless tests crash on RADV (#94).
    pub fn precompile(
        &mut self,
        request: &trino_core::PrecompileRequest,
    ) -> trino_core::PrecompileStatus {
        for b in self.warm.finish() {
            self.warm_install(b);
        }
        let jobs = self.precompile_plan(request.all_settings);
        if jobs.is_empty() {
            // Nothing left: this request's status says so (0 jobs).
            self.warm = precompile::Warmup::default();
        } else {
            let inputs = self.precompile_inputs(request.all_settings);
            self.warm.start(jobs, inputs, self.precompile_threads);
            self.cache_unsaved = true;
            if request.mode == trino_core::PrecompileMode::Blocking {
                for b in self.warm.finish() {
                    self.warm_install(b);
                }
            }
        }
        self.warm.status()
    }

    /// Progress of the last [`precompile`](Self::precompile).
    pub fn precompile_status(&self) -> trino_core::PrecompileStatus {
        self.warm.status()
    }

    /// **Test hook:** the next [`precompile`](Self::precompile)'s workers
    /// take a job each and wait, the job in flight, until the returned
    /// gate's `release()`; a frame that needs one of those passes waits.
    #[doc(hidden)]
    pub fn hold_precompile(&mut self) -> Arc<precompile::Hold> {
        let h = Arc::new(precompile::Hold::default());
        self.warm_hold = Some(h.clone());
        h
    }

    /// **Test hook:** jobs a precompile worker holds right now.
    #[doc(hidden)]
    pub fn precompile_in_flight(&self) -> u32 {
        self.warm.running()
    }

    /// Workers a [`precompile`](Self::precompile) uses (at least 1).
    pub fn set_precompile_threads(&mut self, threads: u32) {
        self.precompile_threads = threads.max(1);
    }

    /// See [`set_precompile_threads`](Self::set_precompile_threads):
    /// `TRINO_THREADS`, else all CPUs but one.
    pub fn precompile_threads(&self) -> u32 {
        self.precompile_threads
    }

    /// Pipelines created on this renderer's device so far.
    pub fn pipeline_counts(&self) -> PipelineCounts {
        use std::sync::atomic::Ordering::Relaxed;
        PipelineCounts {
            render_thread: self.pipe_entry.render_thread.load(Relaxed),
            precompiled: self.pipe_entry.warm.load(Relaxed),
            last_frame: self.last_times.pipelines,
            unscoped: pipes::unscoped(),
        }
    }

    /// The persistent pipeline cache is in use (a Vulkan adapter with
    /// `PIPELINE_CACHE` and a [`RendererOptions::pipeline_cache_dir`]).
    pub fn has_pipeline_cache(&self) -> bool {
        self.pipe_entry.cache.is_some() && self.cache_file.is_some()
    }

    /// Write the pipeline cache to its file (atomically: a temporary file
    /// renamed over it). `Ok(None)` without a cache; the bytes written
    /// otherwise. Hosts call it once a warm-up finished and at exit.
    pub fn save_pipeline_cache(&self) -> std::io::Result<Option<u64>> {
        let (Some(cache), Some(path)) = (&self.pipe_entry.cache, &self.cache_file) else {
            return Ok(None);
        };
        let Some(data) = cache.get_data() else {
            return Ok(None);
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, path)?;
        Ok(Some(data.len() as u64))
    }

    /// What a request builds: every lazily built pass the profile can
    /// reach (`all`), or those the current settings draw with, minus the
    /// ones already built.
    fn precompile_plan(&self, all: bool) -> Vec<precompile::JobKind> {
        use precompile::JobKind as J;
        let mut jobs = Vec::new();
        let native = self.profile.is_native();
        let want_ms = native && !self.webgl2 && (all || self.msaa > 1);
        if want_ms && self.ms_pipelines.is_none() {
            jobs.push(J::MsScene);
        }
        if self.shadow_pipes.is_none() {
            jobs.push(J::ShadowPipes(1));
        }
        if want_ms && self.shadow_pipes_ms.is_none() {
            jobs.push(J::ShadowPipes(MSAA_SAMPLES));
        }
        if self.caps.ao_tier == trino_core::AoTier::ScreenSpace {
            if self.ao_pass.is_none() {
                jobs.push(J::Ao(false));
            }
            if want_ms && self.ao_pass_ms.is_none() {
                jobs.push(J::Ao(true));
            }
        }
        if self.caps.shadow_tier == ShadowTier::Map {
            if self.shadow_map.is_none() {
                jobs.push(J::ShadowMap(false));
            }
            if want_ms && self.shadow_map_ms.is_none() {
                jobs.push(J::ShadowMap(true));
            }
        }
        let scaled = self.render_scale() != 1.0 || self.dynres.is_some();
        if native && (all || scaled) && self.upscale.is_none() {
            jobs.push(J::Upscale);
        }
        if self.webgl2 && self.depth_blit.is_none() {
            jobs.push(J::DepthBlit);
        }
        let modes: &[trino_core::CvdMode] = if all {
            &[
                trino_core::CvdMode::Protanopia,
                trino_core::CvdMode::Deuteranopia,
                trino_core::CvdMode::Tritanopia,
                trino_core::CvdMode::Grayscale,
            ]
        } else if self.cvd_view != trino_core::CvdMode::Off {
            core::slice::from_ref(&self.cvd_view)
        } else {
            &[]
        };
        for &m in modes {
            if !self
                .cvd
                .as_ref()
                .is_some_and(|c| c.has(m, CvdKind::Simulate))
            {
                jobs.push(J::Cvd(m, CvdKind::Simulate));
            }
        }
        // The post stack (issue #64): the Full tier, every pass when `all`,
        // else what a frame already drew with.
        let full = self.caps.post_tier == trino_core::PostTier::Full;
        if full && (all || self.post_used || self.post_view != PostView::Game) {
            if self.post_pass.is_none() {
                jobs.push(J::Post);
            }
            if self.hdr_ok {
                if self.hdr_sets[0].is_none() {
                    jobs.push(J::Hdr(false));
                }
                if want_ms && self.hdr_sets[1].is_none() {
                    jobs.push(J::Hdr(true));
                }
                if let Some(g) = &self.gpu
                    && self.gpu_tnl_active()
                {
                    if !g.has_hdr(false) {
                        jobs.push(J::HdrGpu(false));
                    }
                    if want_ms && !g.has_hdr(true) {
                        jobs.push(J::HdrGpu(true));
                    }
                }
            }
        }
        // The correction filter: the three modes it corrects, or the one
        // the player asked for.
        let fixes: &[trino_core::CvdMode] = if !full {
            &[]
        } else if all {
            &[
                trino_core::CvdMode::Protanopia,
                trino_core::CvdMode::Deuteranopia,
                trino_core::CvdMode::Tritanopia,
            ]
        } else if self.frame_cvd_filter() != trino_core::CvdMode::Off {
            core::slice::from_ref(&self.cvd_filter)
        } else {
            &[]
        };
        for &m in fixes {
            if !self
                .cvd
                .as_ref()
                .is_some_and(|c| c.has(m, CvdKind::Correct))
            {
                jobs.push(J::Cvd(m, CvdKind::Correct));
            }
        }
        // The stereo preview's composites (the 3DS profile, issue #63).
        if self.caps.stereo {
            for view in [StereoView::Anaglyph, StereoView::SideBySide] {
                let wanted = all || self.stereo_view == view;
                if wanted && !self.stereo_pass.as_ref().is_some_and(|p| p.has(view)) {
                    jobs.push(J::Stereo(view));
                }
            }
        }
        if self.gpu_tnl_active() {
            match &self.gpu {
                None => jobs.push(J::GpuTnl),
                Some(g) if want_ms && !g.has_ms() => jobs.push(J::GpuTnlMs),
                Some(_) => {}
            }
        }
        if want_ms && self.water.ms.is_none() {
            jobs.push(J::WaterMs);
        }
        jobs
    }

    fn precompile_inputs(&mut self, all: bool) -> precompile::Inputs {
        let want_ms = self.profile.is_native() && !self.webgl2 && (all || self.msaa > 1);
        // The color-vision pass's layout must be the one its bind groups
        // use: the pass exists before its pipelines are built.
        let cvd_layout = self
            .cvd
            .get_or_insert_with(|| cvd::CvdPass::new(&self.device))
            .layout()
            .clone();
        let stereo_layout = self
            .stereo_pass
            .get_or_insert_with(|| stereo::StereoPass::new(&self.device))
            .layout()
            .clone();
        precompile::Inputs {
            entry: self.pipe_entry.clone(),
            hold: self.warm_hold.take(),
            device: self.device.clone(),
            sources: self.sources.clone(),
            webgl2: self.webgl2,
            tnl_groups: [self.globals_bgl.clone(), self.tex3d_bgl.clone()],
            gpu_ms: want_ms,
            gpu_ms_source: self.gpu.as_ref().map(|g| g.ms_source()),
            cvd_layout,
            stereo_layout,
            water_groups: self.water.groups.clone(),
            water_bgl: self.water.bgl.clone(),
            hdr: self.hdr_ok,
        }
    }

    /// Put a built pass where the lazy code looks for it (an existing one
    /// stays: the frame built it first).
    fn warm_install(&mut self, built: precompile::Built) {
        use precompile::Built as B;
        match built {
            B::MsScene(p) => {
                self.ms_pipelines.get_or_insert(*p);
            }
            B::ShadowPipes(1, p) => {
                self.shadow_pipes.get_or_insert(*p);
            }
            B::ShadowPipes(_, p) => {
                self.shadow_pipes_ms.get_or_insert(*p);
            }
            B::Ao(false, p) => {
                self.ao_pass.get_or_insert(*p);
            }
            B::Ao(true, p) => {
                self.ao_pass_ms.get_or_insert(*p);
            }
            B::ShadowMap(false, p) => {
                self.shadow_map.get_or_insert(*p);
            }
            B::ShadowMap(true, p) => {
                self.shadow_map_ms.get_or_insert(*p);
            }
            B::Upscale(p) => {
                self.upscale.get_or_insert(*p);
            }
            B::DepthBlit(p) => {
                self.depth_blit.get_or_insert(*p);
            }
            B::Cvd(mode, kind, p) => self
                .cvd
                .get_or_insert_with(|| cvd::CvdPass::new(&self.device))
                .install(mode, kind, p),
            B::Hdr(ms, set) => {
                self.hdr_sets[ms as usize].get_or_insert(set);
            }
            B::HdrGpu(ms, p) => {
                if let Some(g) = &mut self.gpu {
                    g.install_hdr(ms, p);
                }
            }
            B::Post(p) => {
                self.post_pass.get_or_insert(*p);
            }
            B::Stereo(view, p) => self
                .stereo_pass
                .get_or_insert_with(|| stereo::StereoPass::new(&self.device))
                .install(view, p),
            B::GpuTnl(g) => {
                self.gpu.get_or_insert(*g);
            }
            B::GpuTnlMs(p) => {
                if let Some(g) = &mut self.gpu {
                    g.install_ms(p);
                }
            }
            B::WaterMs(bgl, p) => {
                let size = self.scene_extent();
                self.water.install_ms(&self.device, size, (bgl, p));
            }
        }
    }

    /// The render thread is about to build `kind` lazily: take it from the
    /// warm-up (wait for it when a worker is on it).
    fn warm_wait(&mut self, kind: precompile::JobKind) {
        if self.warm.is_idle() {
            return;
        }
        for b in self.warm.wait_for(kind) {
            self.warm_install(b);
        }
    }

    /// Install what the warm-up finished (each `begin_frame`; the Web
    /// builds one job here).
    fn warm_poll(&mut self) {
        if !self.warm.is_idle() {
            let built = self.warm.poll();
            for b in built {
                self.warm_install(b);
            }
        }
        // Once per warm-up, however it ended (a frame may have taken its
        // last job): what it compiled starts the next launch warm.
        if self.warm.is_idle() && self.cache_unsaved {
            self.cache_unsaved = false;
            if let Err(e) = self.save_pipeline_cache() {
                eprintln!("[trino] pipeline cache: {e}");
            }
        }
    }

    /// Register a baked font's glyph table (`TFNT`) under its handle; the
    /// atlas must be uploaded as the sprite with the same handle.
    /// Re-uploading replaces it (live reload).
    pub fn upload_font(&mut self, id: FontId, table: Vec<u8>) {
        let index = match trino_core::text::BitmapFont::parse(&table) {
            Ok(font) => trino_core::text::GlyphIndex::build(&font),
            Err(e) => panic!("font {id:?}: invalid TFNT table: {e:?}"),
        };
        let table = table.into_boxed_slice();
        self.fonts
            .insert(id.0, Arc::new(FontEntry { table, index }));
    }

    /// Toggle the N64 output emulation (3-point filtering, RGBA5551 +
    /// ordered dither). Defaults to on for [`SimProfile::N64`].
    pub fn set_n64_look(&mut self, on: bool) {
        // 3D bind groups pick their sampler by the look.
        self.tex3d.clear();
        if self.n64_look != on {
            self.n64_look = on;
            self.queue.write_buffer(
                &self.globals_buf,
                0,
                &globals_bytes(self.internal_size.0, self.internal_size.1, on),
            );
        }
    }

    pub fn n64_look(&self) -> bool {
        self.n64_look
    }

    /// The current frame's 3D on the N64 (the N64 profile only; zeros
    /// otherwise): CPU and RSP time estimated by `trino_core::perf::N64_COST`,
    /// each draw sent where the console sends it ([`N64Estimate`]).
    pub fn n64_estimate(&self) -> N64Estimate {
        let rsp = trino_core::perf::RspWork {
            draws: self.n64.rsp_draws,
            tris: self.n64.rsp_tris,
            water_draws: self.n64.water_draws,
            water_tris: self.n64.water_tris,
        };
        let (cpu_us, rsp_us) =
            trino_core::perf::N64_COST.split_us(&self.n64.cpu, self.n64.cpu_draws, &rsp);
        let cpu_us = cpu_us + trino_core::perf::N64_COST.blobs_us(self.n64.shadow_blobs);
        N64Estimate {
            cpu_us,
            rsp_us,
            cpu_draws: self.n64.cpu_draws,
            rsp_draws: self.n64.rsp_draws,
            rsp_tris: self.n64.rsp_tris,
        }
    }

    /// Toggle strict mode: content that busts the profile's [`Caps`] panics
    /// with an actionable message instead of silently working on PC only.
    /// Defaults to off; `TRINO_STRICT=1` enables it.
    pub fn set_strict(&mut self, on: bool) {
        self.strict = on;
    }

    /// Override the main screen's safe area ([`Renderer::safe_area`]);
    /// `None` goes back to the profile's (`Caps::safe_area`: the N64's TV
    /// overscan, zero elsewhere).
    pub fn set_safe_area(&mut self, insets: Option<trino_core::Insets>) {
        self.safe_area = insets;
    }

    /// The window's unsafe borders in window pixels (`left, top, right,
    /// bottom`; the Web's CSS `env(safe-area-inset-*)` × the device pixel
    /// ratio): [`Renderer::safe_area`] adds what of them the letterbox's
    /// bars do not already cover, in picture pixels, as the window and
    /// the picture change size.
    pub fn set_window_safe_area(&mut self, window: [f32; 4]) {
        self.window_safe = window;
    }

    /// The counts the UI system reported last ([`trino_core::ui`]): nodes
    /// laid out, draws, overflowing texts.
    pub fn last_ui_stats(&self) -> Option<trino_core::UiStats> {
        self.ui_stats
    }

    /// The console profile this renderer simulates.
    pub fn profile(&self) -> SimProfile {
        self.profile
    }

    /// Simulate the profile's console on `tier` hardware (issue #54): the
    /// N64 with the Expansion Pak, the New 3DS. The look stays the
    /// console's; [`Renderer::caps`] becomes the tier's table (RAM, cores,
    /// the crowd and particle scales) so a game's `caps().hw_tier` checks
    /// run here as on the console. The PC and Web profiles have one tier.
    pub fn set_hw_tier(&mut self, tier: trino_core::HwTier) {
        self.caps = self.caps.with_tier(tier);
    }

    /// The hardware tier [`Renderer::caps`] describes.
    pub fn hw_tier(&self) -> trino_core::HwTier {
        self.caps.hw_tier
    }

    /// Change the internal framebuffer resolution (the editor renders its
    /// viewport at the panel's pixel size). Recreates the offscreen color
    /// and depth targets; sprites, meshes and pipelines are kept. Callers
    /// holding [`offscreen_view`](Self::offscreen_view) must re-fetch it.
    /// Budgets ([`Caps`]) still come from the profile.
    pub fn set_internal_size(&mut self, width: u32, height: u32) {
        let max = self.device.limits().max_texture_dimension_2d.min(8192);
        let size = (width.clamp(16, max), height.clamp(16, max));
        if size == self.internal_size {
            return;
        }
        let (tex, view, depth, depth_tex) = create_targets(&self.device, size);
        self.blit_bind_group =
            create_blit_bind_group(&self.device, &self.sprite_bgl, &view, &self.blit_sampler);
        self.offscreen_tex = tex;
        self.offscreen_view = view;
        self.depth_view = depth;
        self.depth_tex = depth_tex;
        self.vi_tex = create_vi_target(&self.device, size);
        self.internal_size = size;
        // The 3D's targets follow the picture (at the render scale).
        self.rebuild_scene_targets(true);
        // Games read the picture size from the caps (culling, layout).
        self.caps.screen_width = size.0 as u16;
        self.caps.screen_height = size.1 as u16;
        self.queue.write_buffer(
            &self.globals_buf,
            0,
            &globals_bytes(size.0, size.1, self.n64_look),
        );
    }

    /// Multisample the main screen's scene (edges of triangles and sprites)
    /// with [`MSAA_SAMPLES`] samples per pixel, or not: the PC's own
    /// profile at its best (`trino-runner-pc` and the PC app turn it on for
    /// it). The console profiles ignore it — their look is the console's.
    /// Off by default (goldens are single-sampled).
    pub fn set_msaa(&mut self, on: bool) {
        let _pipes = pipes::scope(&self.pipe_entry);
        let samples = if on && self.profile.is_native() && !self.webgl2 {
            MSAA_SAMPLES
        } else {
            1
        };
        if samples == self.msaa {
            return;
        }
        self.msaa = samples;
        if samples > 1 {
            use precompile::JobKind as J;
            for kind in [J::MsScene, J::WaterMs, J::GpuTnl, J::GpuTnlMs] {
                self.warm_wait(kind);
            }
        }
        if samples > 1 && self.ms_pipelines.is_none() {
            self.ms_pipelines = Some(scene_pipelines(
                &self.device,
                &self.sources,
                samples,
                OFFSCREEN_FORMAT,
            ));
        }
        if samples > 1 {
            self.water.ensure_ms(&self.device, self.scene_extent());
            if let Some(gpu) = &mut self.gpu {
                gpu.ensure_ms(&self.device, samples);
            }
        }
        self.msaa_targets = (samples > 1)
            .then(|| create_msaa_targets(&self.device, self.scene_extent(), self.msaa_format()));
        if let Some(ao) = &mut self.ao_pass_ms {
            ao.invalidate();
        }
        if let Some(m) = &mut self.shadow_map_ms {
            m.invalidate();
        }
    }

    /// Samples per pixel of the scene (1 = no multisampling).
    pub fn msaa(&self) -> u32 {
        self.msaa
    }

    /// Apply quality settings ([`QualitySettings`], issue #55, ADR 0023):
    /// MSAA, the render scale and dynamic resolution, the AO's quality (or
    /// none), the LOD bias [`Renderer::lod_bias`] reports, the textures'
    /// filter and anisotropy, the upscale's sharpening and the present mode
    /// of the frame limit. The native profile (and the Web) only: the
    /// console profiles keep their console's look and ignore it, like
    /// `set_msaa`. Hosts call it at start-up and on every change; the
    /// sleep-based cap of `FrameLimit::Fps` is theirs
    /// ([`crate::pacing::FrameLimiter`]).
    pub fn apply_quality(&mut self, settings: &QualitySettings) {
        if !self.profile.is_native() {
            return;
        }
        let q = settings.clamped();
        self.set_msaa(q.msaa > 1);
        self.quality.ao = Some(q.ao);
        self.quality.shadows = Some(q.shadows);
        self.quality.post = Some(q.post);
        self.quality.lod_bias = q.lod_bias;
        self.quality.sharpen = q.sharpen;
        self.quality.frame_limit = q.frame_limit;
        if q.texture_filter != self.quality.filter {
            self.quality.filter = q.texture_filter;
            self.tex_filter = self.resolve_filter(self.game_filter);
        }
        self.set_anisotropy(q.anisotropy);
        let changed = self.quality.applied.is_none_or(|a| {
            a.render_scale != q.render_scale || a.dynamic_resolution != q.dynamic_resolution
        });
        self.quality.applied = Some(q);
        if changed {
            let start = q.start_scale();
            self.dynres = q.dynamic_resolution.map(|d| {
                crate::dynres::DynResController::new(
                    d.target_for(self.frame_interval_us()),
                    d.min_scale,
                    d.max_scale,
                    start,
                )
            });
            self.set_render_scale(start);
        } else if let (Some(c), Some(d)) = (&mut self.dynres, q.dynamic_resolution) {
            c.set_target(d.target_for(q.frame_limit.interval_us(self.quality.refresh_hz)));
        }
        self.apply_present_mode();
    }

    /// The quality settings last applied (`None`: never; the console
    /// profiles never take any).
    pub fn quality(&self) -> Option<QualitySettings> {
        self.quality.applied
    }

    /// The 3D's render scale now (1 = the picture's resolution; dynamic
    /// resolution moves it).
    pub fn render_scale(&self) -> f32 {
        self.quality.scale
    }

    /// The 3D's size now: the picture's at a scale of 1.
    pub fn scene_size(&self) -> (u32, u32) {
        self.scene.as_ref().map_or(self.internal_size, |s| s.size)
    }

    /// The size the 3D's targets (and their MSAA and water copies) are
    /// allocated at: [`scene_size`](Self::scene_size) or, under dynamic
    /// resolution, the largest its range reaches.
    pub fn scene_extent(&self) -> (u32, u32) {
        self.scene.as_ref().map_or(self.internal_size, |s| s.extent)
    }

    /// Draw the 3D at `scale` × the picture's resolution (0.25..=2.0),
    /// upscaled onto it (the native profile; a no-op elsewhere). Dynamic
    /// resolution calls it as it goes.
    pub fn set_render_scale(&mut self, scale: f32) {
        if !self.profile.is_native() {
            return;
        }
        self.quality.scale = scale.clamp(0.25, 2.0);
        self.rebuild_scene_targets(false);
    }

    /// The display's refresh rate (the host's monitor): what VSync and the
    /// dynamic resolution's default target are measured against.
    pub fn set_refresh_hz(&mut self, hz: u32) {
        self.quality.refresh_hz = hz.clamp(20, 1000);
        if let (Some(c), Some(d)) = (
            &mut self.dynres,
            self.quality.applied.and_then(|q| q.dynamic_resolution),
        ) {
            c.set_target(
                d.target_for(
                    self.quality
                        .frame_limit
                        .interval_us(self.quality.refresh_hz),
                ),
            );
        }
        self.apply_present_mode();
    }

    /// The display's refresh rate [`set_refresh_hz`](Self::set_refresh_hz)
    /// gave (60 by default).
    pub fn refresh_hz(&self) -> u32 {
        self.quality.refresh_hz
    }

    /// The host's frame interval, µs: what dynamic resolution holds where
    /// the adapter has no timestamp queries (WebGL2, some drivers). Call it
    /// once per frame, after `end_frame`.
    pub fn report_frame_time(&mut self, us: u32) {
        self.host_frame_us = Some(us);
    }

    /// The last measured GPU frame time, µs (timestamp queries, else the
    /// host's frame interval; 0 before the first measurement).
    pub fn gpu_frame_us(&self) -> u32 {
        self.last_gpu_us
    }

    /// The adapter times its frames with timestamp queries (else dynamic
    /// resolution follows the host's frame intervals).
    pub fn has_gpu_timer(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
    }

    /// The present mode the window uses now (`None` headless).
    pub fn present_mode(&self) -> Option<wgpu::PresentMode> {
        self.surface.as_ref().map(|s| s.config.present_mode)
    }

    /// The frame interval the limit and the display give, µs.
    fn frame_interval_us(&self) -> u32 {
        self.quality
            .frame_limit
            .interval_us(self.quality.refresh_hz)
    }

    fn apply_present_mode(&mut self) {
        let Some(ss) = &mut self.surface else {
            return;
        };
        let mode = crate::pacing::present_mode(
            self.quality.frame_limit,
            self.quality.refresh_hz,
            &ss.modes,
        );
        // FIFO is what AutoVsync gives; keep the configuration untouched
        // until a limit asks for something else.
        let current = match ss.config.present_mode {
            wgpu::PresentMode::AutoVsync => wgpu::PresentMode::Fifo,
            m => m,
        };
        if mode != current {
            ss.config.present_mode = mode;
            ss.surface.configure(&self.device, &ss.config);
        }
    }

    fn set_anisotropy(&mut self, n: u8) {
        if n == self.quality.anisotropy {
            return;
        }
        self.quality.anisotropy = n;
        self.tex3d_aniso = (n > 1).then(|| {
            self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("trino-tex3d-aniso"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp: n as u16,
                ..Default::default()
            })
        });
        // Bind groups hold their samplers.
        self.tex3d.clear();
    }

    /// What a game's filter means here: `Default` is the quality's filter,
    /// else the profile's.
    fn resolve_filter(&self, filter: TexFilter) -> TexFilter {
        match filter {
            TexFilter::Default => match self.quality.filter {
                TexFilter::Default => default_filter(self.profile),
                f => f,
            },
            f => f,
        }
    }

    /// The 3D's targets for the current scale: the picture's own at 1, a
    /// scaled set otherwise. Under dynamic resolution one set covers its
    /// whole range (allocated at the largest size, drawn at the current
    /// one), so its steps allocate nothing. `force`: the picture changed,
    /// rebuild even at the same size.
    fn rebuild_scene_targets(&mut self, force: bool) {
        let max = self.device.limits().max_texture_dimension_2d.min(8192);
        let native = self.profile.is_native();
        let size = if native {
            upscale::scene_size(self.internal_size, self.quality.scale, max)
        } else {
            self.internal_size
        };
        let range = self.dynres.as_ref().filter(|_| native).map(|c| c.range().1);
        let extent = range.map_or(size, |top| {
            let e = upscale::scene_size(self.internal_size, top.max(self.quality.scale), max);
            (e.0.max(size.0), e.1.max(size.1))
        });
        let scaled = range.is_some() || size != self.internal_size;
        // The post stack draws the 3D into its own targets at any scale,
        // in HDR where the adapter can.
        let post = native && self.post_targets;
        let need = scaled || post;
        let format = if post && self.hdr_ok {
            post::HDR_FORMAT
        } else {
            OFFSCREEN_FORMAT
        };
        if !force {
            match &mut self.scene {
                Some(t) if need && t.extent == extent && t.format == format => {
                    t.size = size;
                    return;
                }
                None if !need => return,
                _ => {}
            }
        }
        self.scene = need.then(|| upscale::SceneTargets::new(&self.device, extent, size, format));
        let alloc = self.scene_extent();
        self.water.resize(&self.device, alloc);
        if self.msaa > 1 {
            self.msaa_targets = Some(create_msaa_targets(&self.device, alloc, self.msaa_format()));
        }
        for ao in [&mut self.ao_pass, &mut self.ao_pass_ms]
            .into_iter()
            .flatten()
        {
            ao.invalidate();
        }
        for m in [&mut self.shadow_map, &mut self.shadow_map_ms]
            .into_iter()
            .flatten()
        {
            m.invalidate();
        }
        for set in self.hdr_sets.iter_mut().flatten() {
            set.ao.invalidate();
            set.map.invalidate();
        }
    }

    /// After a frame: read the GPU timer (or take the host's interval) and
    /// let dynamic resolution move the scale.
    fn after_frame(&mut self) {
        if let Some(z) = &mut self.gpu_zones {
            z.read(&self.device);
        }
        let measured = match &mut self.gpu_timer {
            Some(t) => t.read(&self.device),
            None => self.host_frame_us.take(),
        };
        let Some(us) = measured else {
            return;
        };
        self.last_gpu_us = us;
        let timed = self.gpu_timer.is_some();
        let Some(c) = &mut self.dynres else {
            return;
        };
        c.set_intervals(!timed);
        if !c.record(us) {
            return;
        }
        let scale = c.scale();
        if (scale - self.quality.scale).abs() > 1e-4 {
            self.set_render_scale(scale);
        }
    }

    /// Toggle the N64 VI dedither filter (on by default; it only runs with
    /// the N64 look). The console enables the same filter
    /// (`FILTERS_RESAMPLE_ANTIALIAS_DEDITHER` in the N64 shim): each
    /// pixel's 5-bit channels move one 8-bit step toward each of its 8
    /// neighbours that differ — the restore filter that turns the RDP's
    /// magic-square dither back into smooth gradients.
    pub fn set_vi_dedither(&mut self, on: bool) {
        self.vi_dedither = on;
    }

    pub fn vi_dedither(&self) -> bool {
        self.vi_dedither
    }

    /// How a stereo frame shows on this window (the 3DS profile only,
    /// issue #63): a preview mode, or `Off` (the simulated 3D slider down:
    /// frames are mono). A review tool for the editor, `cargo xtask ui shot
    /// --stereo` and `TRINO_STEREO`; the eyes themselves are the console's.
    pub fn set_stereo_view(&mut self, view: StereoView) {
        self.stereo_view = view;
    }

    pub fn stereo_view(&self) -> StereoView {
        self.stereo_view
    }

    /// The simulated 3D slider (0..=1, 1 by default) under a preview mode.
    pub fn set_stereo_slider(&mut self, slider: f32) {
        self.stereo_slider = if slider.is_nan() {
            0.0
        } else {
            slider.clamp(0.0, 1.0)
        };
    }

    pub fn stereo_slider(&self) -> f32 {
        self.stereo_slider
    }

    /// The last frame's eye (RGBA8, the main target's size): `None` unless
    /// it drew two.
    pub fn read_eye(&self, eye: trino_core::Eye) -> Option<Vec<u8>> {
        if self.last_stats.stereo.eyes != 2 || eye == trino_core::Eye::Mono {
            return None;
        }
        let tex = self.stereo_pass.as_ref()?.eye_texture(eye)?;
        self.try_read_texture(tex, self.internal_size)
    }

    /// The stereo eye the T&L projects for now: its shift and the
    /// convergence (`(0, _)` outside a replay).
    fn eye_view(&self) -> (f32, f32) {
        let f = &self.stereo.frame;
        (f.eye_shift(self.stereo.eye), f.convergence)
    }

    /// The recording, when this top-screen call is to be recorded (a stereo
    /// frame before its replay); else marks the top screen drawn.
    fn recording(&mut self) -> Option<&mut stereo::Recording> {
        if self.screen != 0 {
            return None;
        }
        if self.stereo.recording {
            return Some(&mut self.stereo.rec);
        }
        self.stereo.top_drawn = true;
        None
    }

    /// Like [`recording`](Self::recording) for a state change.
    fn recording_state(&mut self) -> Option<&mut stereo::Recording> {
        (self.screen == 0 && self.stereo.recording).then_some(&mut self.stereo.rec)
    }

    fn top_state(&self) -> TopState {
        TopState {
            camera: self.camera,
            light: self.light,
            locals: self.locals,
            game_filter: self.game_filter,
            wetness: self.wetness,
            water: self.water_state,
            shadow_game: self.shadow_game,
            shadows: self.shadows,
        }
    }

    /// Back to `start`, with empty draw lists: one eye's replay begins.
    fn restart_top(&mut self, start: &TopState) {
        self.camera = start.camera;
        self.light = start.light;
        self.locals = start.locals;
        self.game_filter = start.game_filter;
        self.tex_filter = self.resolve_filter(start.game_filter);
        self.wetness = start.wetness;
        self.water_state = start.water;
        self.shadow_game = start.shadow_game;
        let stats = self.shadows.stats;
        self.shadows = start.shadows;
        self.shadows.stats = stats;
        self.clear_top_lists();
        self.tnl_ctx = None;
    }

    /// The top screen's recorded draws (what `draw_commands` renders).
    fn clear_top_lists(&mut self) {
        self.commands.clear();
        self.tri_verts.clear();
        self.pending_tris.clear();
        self.pending_tex.clear();
        self.pending_surf.clear();
        self.light_sets.clear();
        self.water_sets.clear();
        self.pending_shadow.clear();
        self.map_casters.clear();
        self.cpu_jobs.clear();
        self.job_ctxs.clear();
    }

    /// Draw the recorded top screen once per eye, as the 3DS does: each
    /// eye replayed from the state recording began with into the main
    /// target, kept in its texture, then the preview composed into the
    /// main target. Leaves the state the recording ended with, mono.
    fn replay_eyes(&mut self) {
        self.stereo.recording = false;
        let rec = std::mem::take(&mut self.stereo.rec);
        let start = *self.stereo_start;
        let size = self.internal_size;
        let (mut cmds, mut tris) = (0, 0);
        for eye in [trino_core::Eye::Left, trino_core::Eye::Right] {
            self.restart_top(&start);
            self.stereo.eye = eye;
            self.stereo.depth = trino_core::LayerDepth::SCREEN;
            trino_core::stereo::replay(self, &rec.cmds, &rec.data());
            self.flush_model_batch();
            cmds += self.commands.len() as u32;
            tris += (self.tri_verts.len() / 3) as u32;
            self.draw_commands(false);
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            self.stereo_pass
                .get_or_insert_with(|| stereo::StereoPass::new(&self.device))
                .store_eye(&self.device, &mut encoder, &self.offscreen_tex, size, eye);
            self.queue.submit([encoder.finish()]);
        }
        let view = self.stereo_view;
        if view.composes() {
            self.warm_wait(precompile::JobKind::Stereo(view));
        }
        {
            let _pipes = pipes::scope(&self.pipe_entry);
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            self.stereo_pass
                .get_or_insert_with(|| stereo::StereoPass::new(&self.device))
                .compose(
                    &self.device,
                    &mut encoder,
                    &self.offscreen_tex,
                    &self.offscreen_view,
                    size,
                    view,
                    self.flash_frames as u32,
                );
            self.queue.submit([encoder.finish()]);
        }
        self.clear_top_lists();
        self.tnl_ctx = None;
        (self.main_cmds, self.main_tris) = (cmds, tris);
        self.stereo.eye = trino_core::Eye::Mono;
        self.stereo.rec = rec;
    }

    /// What the screen-space AO shows ([`AoView`]): as the game sets it
    /// (the default), off, always on, or the occlusion alone. The editor's
    /// Scene view toggle and `TRINO_AO=off|on|mask`. Only the native
    /// profile runs the pass (`Caps::ao_tier`); the console profiles show
    /// what the consoles do, the AO baked into the models.
    /// Show every frame as a viewer with `mode`'s color vision sees it
    /// (Machado 2009; `Off` = as drawn): a review view for the editor,
    /// `cargo xtask access shot` and `TRINO_CVD`, never a game's look. It
    /// applies at `end_frame`, after the VI filter, to both screens, so the
    /// window, the editor and read-backs all see it.
    pub fn set_cvd_view(&mut self, mode: trino_core::CvdMode) {
        self.cvd_view = mode;
    }

    pub fn cvd_view(&self) -> trino_core::CvdMode {
        self.cvd_view
    }

    /// Strobes seen so far: frames where a full-screen fill (a quarter of
    /// the picture or more) brightened the frame more than three times in
    /// a second (frames counted at 60 Hz). Strict mode logs a warning on
    /// each; `trino_core::ScreenFlash` under `flash_guard` never makes one.
    pub fn flash_strobes(&self) -> u32 {
        self.flash_monitor.strobes
    }

    /// What the post stack shows (issue #64): as the game sets it, never,
    /// always ([`trino_core::PostSettings::FILMIC`] when the game sets
    /// none) or the bloom alone — the editor's Scene view toggle and
    /// `TRINO_POST=off|on|bloom`. Only the native profile (and the Web)
    /// runs the stack ([`trino_core::Caps::post_tier`]).
    pub fn set_post_view(&mut self, view: PostView) {
        self.post_view = view;
    }

    pub fn post_view(&self) -> PostView {
        self.post_view
    }

    /// The adapter renders the post stack's scene in `Rgba16Float` (else
    /// 8-bit: no range past 1, the rest of the stack the same).
    pub fn post_hdr(&self) -> bool {
        self.hdr_ok
    }

    /// Register a baked grading LUT (`TLUT`, a `[luts.*]` asset's blob)
    /// under its handle; a re-upload replaces it (live reload). Hosts call
    /// it for every blob that starts with `TLUT`
    /// ([`trino_core::post::TLUT_MAGIC`]). Returns the GPU bytes.
    pub fn upload_lut(
        &mut self,
        id: trino_core::LutId,
        bytes: &[u8],
    ) -> Result<u64, trino_core::LutError> {
        let lut = trino_core::Lut::parse(bytes)?;
        let tex = post::LutTex::new(&self.device, &self.queue, &lut);
        self.luts.insert(id, tex);
        Ok(post::LutTex::bytes(lut.size()))
    }

    /// A baked blob the host loads: when it is a grading LUT (`TLUT`
    /// magic), also upload it as one (its handle is the blob's). Returns
    /// whether it was a LUT; a broken one is logged and skipped.
    pub fn take_lut_blob(&mut self, id: u32, bytes: &[u8]) -> bool {
        if !bytes.starts_with(&trino_core::post::TLUT_MAGIC) {
            return false;
        }
        if let Err(e) = self.upload_lut(trino_core::LutId(id), bytes) {
            eprintln!("[trino] LUT {id:08x}: {e:?}");
        }
        true
    }

    /// The player's accessibility settings the renderer honors itself:
    /// the daltonization filter (`cvd_filter` on: `cvd`'s correction).
    /// Hosts call it every frame with `Platform::access()`.
    pub fn apply_access(&mut self, access: &trino_core::AccessSettings) {
        self.set_cvd_filter(if access.cvd_filter {
            access.cvd
        } else {
            trino_core::CvdMode::Off
        });
    }

    /// Forget a LUT (its group unloaded); returns the bytes freed.
    pub fn remove_lut(&mut self, id: trino_core::LutId) -> u64 {
        self.luts
            .remove(&id)
            .map_or(0, |t| post::LutTex::bytes(t.size))
    }

    pub fn has_lut(&self, id: trino_core::LutId) -> bool {
        self.luts.contains_key(&id)
    }

    /// The player's daltonization filter (issue #64): every frame of the
    /// native profile and the Web is corrected for `mode` (Fidaner 2005
    /// over the Machado 2009 simulation, `trino_core::access::color::
    /// correction`), the HUD included, before the simulation view. `Off`
    /// and `Grayscale` draw nothing. Hosts pass `AccessSettings::cvd` while
    /// `cvd_filter` is on; the console profiles ignore it (the consoles'
    /// answer is the semantic palettes).
    pub fn set_cvd_filter(&mut self, mode: trino_core::CvdMode) {
        self.cvd_filter = mode;
    }

    pub fn cvd_filter(&self) -> trino_core::CvdMode {
        self.cvd_filter
    }

    /// The filter's mode this frame (`Off` where it doesn't run).
    fn frame_cvd_filter(&self) -> trino_core::CvdMode {
        use trino_core::CvdMode as M;
        match self.cvd_filter {
            M::Off | M::Grayscale => M::Off,
            _ if self.caps.post_tier != trino_core::PostTier::Full => M::Off,
            m => m,
        }
    }

    /// The post settings this frame runs with and whether it shows the
    /// bloom alone: the game's (or the view's), on the Full tier only, the
    /// quality preset's level applied (Off keeps the tonemap and the
    /// grading and drops the bloom, vignette and grain).
    fn frame_post(&self) -> Option<(trino_core::PostSettings, bool)> {
        if self.caps.post_tier != trino_core::PostTier::Full {
            return None;
        }
        let (mut p, mask) = self.post_view.resolve(self.post)?;
        if self.quality.post == Some(trino_core::PostSetting::Off) && !mask {
            p.bloom = None;
            p.vignette = 0.0;
            p.grain = 0.0;
        }
        Some((p, mask))
    }

    /// The preset's bloom: its most levels and whether the chain starts at
    /// half the picture's size (`None`: no bloom). High when never applied
    /// (smoke runs and goldens pin High).
    fn bloom_cap(&self) -> Option<(u32, bool)> {
        use trino_core::PostSetting as P;
        match self.quality.post.unwrap_or(P::High) {
            P::Off => None,
            P::Low => Some((3, false)),
            P::Medium => Some((4, true)),
            P::High => Some((post::MAX_LEVELS, true)),
        }
    }

    pub fn set_ao_view(&mut self, view: AoView) {
        self.ao_view = view;
    }

    pub fn ao_view(&self) -> AoView {
        self.ao_view
    }

    /// The AO this frame runs with, and whether it shows as the mask: the
    /// game's, its quality replaced (or the pass dropped) by the quality
    /// settings', then the view's override.
    fn frame_ao(&self) -> Option<(AmbientOcclusion, bool)> {
        if self.caps.ao_tier != AoTier::ScreenSpace {
            return None;
        }
        let ao = match self.quality.ao {
            None => self.ao,
            Some(level) => self.ao.and_then(|a| {
                level
                    .cap(a.quality)
                    .map(|quality| AmbientOcclusion { quality, ..a })
            }),
        };
        self.ao_view.resolve(ao)
    }

    /// View of the internal framebuffer — the editor registers this as an
    /// egui texture to show the game inside a viewport panel.
    pub fn offscreen_view(&self) -> &wgpu::TextureView {
        &self.offscreen_view
    }

    /// Upload an RGBA8 texture for `id`. Re-uploading the same id replaces
    /// the content (this is what makes asset live-reload work: handles are
    /// stable, content is swapped in place).
    pub fn upload_sprite(&mut self, id: SpriteId, width: u32, height: u32, rgba: &[u8]) {
        assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "sprite {id:?}: pixel data does not match {width}x{height} RGBA8"
        );
        let alpha = rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
        self.install_sprite(
            id,
            (width, height),
            OFFSCREEN_FORMAT,
            rgba,
            width * 4,
            alpha,
        );
    }

    /// Does this renderer sample block-compressed sprites
    /// ([`PcRenderer::upload_sprite_bc`])? The device has
    /// `TEXTURE_COMPRESSION_BC` (requested where the adapter has it, unless
    /// `TRINO_BC=0`) and [`PcRenderer::set_bc`] left it on.
    pub fn supports_bc(&self) -> bool {
        self.bc
    }

    /// Does [`PcRenderer::upload_sprite_compressed`] take `format`: the
    /// device samples it (BC also needs [`PcRenderer::set_bc`] on).
    pub fn supports_compressed(&self, format: CompressedFormat) -> bool {
        if format.is_bc() {
            self.bc
        } else {
            self.device.features().contains(format.feature())
        }
    }

    /// Turn BC uploads off (`false`: [`PcRenderer::upload_sprite_bc`]
    /// refuses, as on a GPU without BC) or back on where the device has it.
    pub fn set_bc(&mut self, on: bool) {
        self.bc = on
            && self
                .device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    }

    /// Upload a BC texture for `id` ([`PcRenderer::upload_sprite_compressed`]
    /// with its alpha read from the blocks).
    pub fn upload_sprite_bc(
        &mut self,
        id: SpriteId,
        width: u32,
        height: u32,
        format: CompressedFormat,
        blocks: &[u8],
    ) -> bool {
        self.upload_sprite_compressed(id, width, height, format, blocks, None)
    }

    /// Upload a block-compressed texture for `id` (`width` and `height`
    /// multiples of 4, `blocks` in row-major order), replacing any previous
    /// one. Returns `false` and uploads nothing when the renderer doesn't
    /// sample `format` ([`PcRenderer::supports_compressed`]): the caller
    /// uploads decoded RGBA8 instead. `alpha`: whether some texel blends
    /// (a model drawn with it stays on the CPU path); `None` reads it from
    /// BC blocks, and takes the ETC2 RGBA and ASTC formats as blending.
    pub fn upload_sprite_compressed(
        &mut self,
        id: SpriteId,
        width: u32,
        height: u32,
        format: CompressedFormat,
        blocks: &[u8],
        alpha: Option<bool>,
    ) -> bool {
        if !self.supports_compressed(format) {
            return false;
        }
        assert!(
            width.is_multiple_of(4) && height.is_multiple_of(4) && width > 0 && height > 0,
            "sprite {id:?}: {width}x{height} is not whole 4x4 blocks"
        );
        let row = width / 4 * format.block_bytes();
        assert_eq!(
            blocks.len(),
            (row * height / 4) as usize,
            "sprite {id:?}: block data does not match {width}x{height} {format:?}"
        );
        let alpha = alpha.unwrap_or_else(|| match format {
            CompressedFormat::Bc1 | CompressedFormat::Bc3 | CompressedFormat::Bc7 => {
                gpu_tnl::bc_alpha(format, blocks)
            }
            CompressedFormat::Etc2Rgb8 => false,
            CompressedFormat::Etc2Rgba8 | CompressedFormat::Astc4x4 => true,
        });
        self.install_sprite(id, (width, height), format.wgpu(), blocks, row, alpha);
        true
    }

    /// The sprite textures held now: how many, their GPU bytes, how many
    /// are block-compressed (the PC's VRAM figure in `texture report` and
    /// the `TRINO_TEXTURES` line).
    pub fn sprite_memory(&self) -> SpriteMemory {
        let mut m = SpriteMemory::default();
        for e in self.sprites.values() {
            m.textures += 1;
            m.bytes += e.bytes;
            if e.compressed {
                m.compressed += 1;
                m.compressed_bytes += e.bytes;
            }
        }
        m
    }

    /// Create the texture, write `data` (rows of `row_bytes`: pixels, or
    /// 4×4 blocks for a compressed format) and register its bind group
    /// under `id`. `alpha`: some texel is below full alpha.
    fn install_sprite(
        &mut self,
        id: SpriteId,
        (width, height): (u32, u32),
        format: wgpu::TextureFormat,
        data: &[u8],
        row_bytes: u32,
        alpha: bool,
    ) {
        let rows = if format == OFFSCREEN_FORMAT {
            height
        } else {
            height / 4
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("trino-sprite"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trino-sprite-bg"),
            layout: &self.sprite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.tex3d
            .retain(|k, _| k.id != id.0 && k.detail.is_none_or(|d| d.sprite != id));
        self.sprites.insert(
            id.0,
            SpriteEntry {
                bind_group,
                view,
                size: (width, height),
                bytes: (row_bytes * rows) as u64,
                compressed: format != OFFSCREEN_FORMAT,
                alpha,
            },
        );
    }

    /// Free a sprite's texture (its asset group unloaded, issue #56);
    /// returns the GPU bytes it held. Later draws of the handle draw nothing
    /// and count `FrameStats::not_loaded`.
    pub fn remove_sprite(&mut self, id: SpriteId) -> u64 {
        self.tex3d
            .retain(|k, _| k.id != id.0 && k.detail.is_none_or(|d| d.sprite != id));
        self.sprites.remove(&id.0).map_or(0, |e| e.bytes)
    }

    /// Free a mesh ([`PcRenderer::remove_sprite`]); returns its TMDL bytes.
    pub fn remove_mesh(&mut self, id: ModelId) -> usize {
        if let Some(gpu) = &mut self.gpu {
            gpu.forget_mesh(id.0);
        }
        self.meshes.remove(&id.0).map_or(0, |(b, _)| b.len())
    }

    /// Free a font's glyph table (its atlas is the sprite of the same
    /// handle); returns the table's bytes.
    pub fn remove_font(&mut self, id: FontId) -> usize {
        self.fonts.remove(&id.0).map_or(0, |f| f.table.len())
    }

    /// GPU bytes of a sprite's texture (0 when not loaded).
    pub fn sprite_bytes(&self, id: SpriteId) -> u64 {
        self.sprites.get(&id.0).map_or(0, |e| e.bytes)
    }

    /// TMDL bytes of a mesh (0 when not loaded).
    pub fn mesh_bytes(&self, id: ModelId) -> usize {
        self.meshes.get(&id.0).map_or(0, |(b, _)| b.len())
    }

    /// Glyph-table bytes of a font (0 when not loaded).
    pub fn font_bytes(&self, id: FontId) -> usize {
        self.fonts.get(&id.0).map_or(0, |f| f.table.len())
    }

    /// Window resized: reconfigure the surface (offscreen stays fixed).
    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(ss) = &mut self.surface
            && width > 0
            && height > 0
        {
            ss.config.width = width;
            ss.config.height = height;
            ss.surface.configure(&self.device, &ss.config);
        }
    }

    /// What a window of `output` physical pixels shows right now: the
    /// internal frame upscaled and centered exactly like the present pass
    /// (nearest, whole-number scale, black bars), composed on the CPU.
    /// Test/tooling path (screenshots of fullscreen/ultrawide layouts).
    pub fn present_preview(&self, output: (u32, u32)) -> Vec<u8> {
        let main = self.internal_size;
        let second = self.second.as_ref().map(|sc| sc.size);
        let src = self.read_offscreen();
        let src2 = second.map(|_| self.read_second_screen());
        let mut out = vec![0u8; (output.0 * output.1 * 4) as usize];
        for oy in 0..output.1 {
            for ox in 0..output.0 {
                let at = ((oy * output.0 + ox) * 4) as usize;
                out[at + 3] = 255;
                let hit = window_to_screen(main, second, output, ox as f32 + 0.5, oy as f32 + 0.5);
                if let Some((p, screen)) = hit {
                    let (w, buf) = match (screen, &src2, second) {
                        (1, Some(b), Some(s)) => (s.0, b),
                        _ => (main.0, &src),
                    };
                    let from = ((p.y as u32 * w + p.x as u32) * 4) as usize;
                    out[at..at + 4].copy_from_slice(&buf[from..from + 4]);
                }
            }
        }
        out
    }

    /// Read the second screen back (RGBA8), like
    /// [`read_offscreen`](Self::read_offscreen). Empty without one.
    pub fn read_second_screen(&self) -> Vec<u8> {
        match &self.second {
            Some(sc) => self.read_texture(&sc.tex, sc.size),
            None => Vec::new(),
        }
    }

    /// Block until the GPU has finished everything submitted (frame-time
    /// measurements include the GPU's work).
    pub fn wait_gpu(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// The adapter's name and backend (for performance reports).
    pub fn adapter_info(&self) -> String {
        self.adapter_name.clone()
    }

    /// The last finished frame's CPU time per rendering stage and the GPU
    /// work it handed over ([`PcFrameTimes`]).
    pub fn frame_times(&self) -> PcFrameTimes {
        self.last_times
    }

    /// The last finished frame's model draws that stay off the native
    /// profile's GPU T&L path, per reason ([`PcGpuReject::index`]); the
    /// draws it takes are `stats().tnl.pc_gpu_draws`. All zero on the
    /// console profiles, which never take it (issue #51).
    pub fn pc_gpu_rejects(&self) -> [u32; PcGpuReject::COUNT] {
        self.last_pc_gpu_off
    }

    /// Read the internal framebuffer back as tightly-packed RGBA8.
    /// Test/tooling path — synchronous, not for per-frame use.
    pub fn read_offscreen(&self) -> Vec<u8> {
        self.read_texture(&self.offscreen_tex, self.internal_size)
    }

    /// [`PcRenderer::read_offscreen`] where the backend can wait for the
    /// GPU: `None` in a browser on WebGPU, which maps buffers only once
    /// the page's script yields (read it with
    /// [`PcRenderer::start_read_offscreen`] there).
    pub fn try_read_offscreen(&self) -> Option<Vec<u8>> {
        self.try_read_texture(&self.offscreen_tex, self.internal_size)
    }

    /// Start reading the internal framebuffer back without waiting: poll
    /// the returned [`PendingRead`] once per frame (a browser maps the
    /// buffer between frames). The Web's golden frame takes this path.
    pub fn start_read_offscreen(&self) -> PendingRead {
        let (buffer, padded) = self.copy_to_readback(&self.offscreen_tex, self.internal_size);
        let done = std::sync::Arc::new(std::sync::Mutex::new(None));
        let flag = done.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            *flag.lock().unwrap() = Some(r.map_err(|e| e.to_string()));
        });
        PendingRead {
            buffer,
            padded,
            size: self.internal_size,
            done,
        }
    }

    /// Poll a [`PendingRead`]: the pixels once the GPU has them.
    pub fn poll_read(&self, read: &PendingRead) -> Option<Vec<u8>> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let state = read.done.lock().unwrap().clone();
        match state {
            Some(Ok(())) => Some(unpad_rows(
                &read.buffer.slice(..).get_mapped_range(),
                read.size,
                read.padded,
            )),
            Some(Err(e)) => panic!("failed to map readback buffer: {e}"),
            None => None,
        }
    }

    fn read_texture(&self, tex: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        self.try_read_texture(tex, size)
            .expect("map_async callback never ran (a browser's WebGPU can't wait)")
    }

    /// Copy `tex` into a mappable buffer: the buffer and its padded row
    /// pitch.
    fn copy_to_readback(&self, tex: &wgpu::Texture, size: (u32, u32)) -> (wgpu::Buffer, u32) {
        let (w, h) = size;
        let padded = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trino-readback"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        (buffer, padded)
    }

    /// Read `tex` back, waiting for the GPU; `None` where the backend
    /// can't wait (a browser's WebGPU).
    fn try_read_texture(&self, tex: &wgpu::Texture, size: (u32, u32)) -> Option<Vec<u8>> {
        // A browser's WebGPU maps only after the script yields: give up
        // before allocating. A dropped buffer isn't freed there until the
        // page's GC runs, and a probe per pixel leaked enough mapped
        // read-backs that Chromium on SwiftShader destroyed the device.
        if cfg!(target_arch = "wasm32") && !self.webgl2 {
            return None;
        }
        let (buffer, padded) = self.copy_to_readback(tex, size);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        // On WebGL2 the poll runs the callback; on WebGPU it fires only
        // after the script yields, so don't block (a browser can't).
        let mapped = if cfg!(target_arch = "wasm32") {
            rx.try_recv().ok()?
        } else {
            rx.recv().expect("map_async callback dropped")
        };
        mapped.expect("failed to map readback buffer");
        let data = slice.get_mapped_range();
        Some(unpad_rows(&data, size, padded))
    }

    pub fn internal_size(&self) -> (u32, u32) {
        self.internal_size
    }

    /// Window pixel (physical, as winit reports the cursor) → game pixel,
    /// through the same letterboxed integer upscale the window shows.
    /// `None` outside the picture or without a window surface.
    pub fn window_to_game(&self, x: f64, y: f64) -> Option<trino_core::Vec2> {
        self.window_to_pointer(x, y)
            .and_then(|(p, screen)| (screen == 0).then_some(p))
    }

    /// Window pixel → (position, screen): with the 3DS profile the window
    /// shows both screens stacked, and the second screen is 1.
    pub fn window_to_pointer(&self, x: f64, y: f64) -> Option<(trino_core::Vec2, u8)> {
        let ss = self.surface.as_ref()?;
        window_to_screen(
            self.internal_size,
            self.second.as_ref().map(|sc| sc.size),
            (ss.config.width, ss.config.height),
            x as f32,
            y as f32,
        )
    }

    /// Depth-sort the pending model batch and enqueue it as triangle
    /// commands (painter's order across every mesh of the batch). Batches
    /// with textured draws are then grouped by texture (stable: depth order
    /// holds within each texture) — one texture bind per group, the same
    /// grouping the N64 needs to load TMEM once per texture.
    ///
    /// The native profile's batch also holds its CPU draws as jobs (run
    /// here, [`cpu_tnl`]) and its GPU T&L draws, which become instanced
    /// draw calls ahead of the CPU triangles: opaque and depth-tested, they
    /// need no painter order.
    fn flush_model_batch(&mut self) {
        let gpu = self.gpu.as_ref().is_some_and(|g| g.has_pending());
        if self.pending_tris.is_empty()
            && self.cpu_jobs.is_empty()
            && self.pending_shadow.is_empty()
            && !gpu
        {
            return;
        }
        trino_core::zone!("batch");
        self.run_cpu_jobs();
        let t0 = Instant::now();
        if let Some(g) = self.gpu.as_mut().filter(|g| g.has_pending()) {
            let r = g.flush_batch();
            self.commands.push(Cmd::Meshes {
                first: r.start,
                count: r.end - r.start,
            });
        }
        if !self.pending_tris.is_empty() || !self.pending_shadow.is_empty() {
            self.flush_model_batch_now();
        }
        self.times.batch_us += micros(t0);
    }

    /// Tessellate the batch's CPU jobs (over the workers when there is
    /// enough work) and append their triangles in submission order, as the
    /// draws would have one by one.
    fn run_cpu_jobs(&mut self) {
        if self.cpu_jobs.is_empty() {
            return;
        }
        trino_core::zone!("cpu tnl");
        let t0 = Instant::now();
        let jobs = std::mem::take(&mut self.cpu_jobs);
        let chunks = cpu_tnl::run(
            &self.meshes,
            &self.job_ctxs,
            &jobs,
            &mut self.workers,
            self.tnl_threads,
        );
        let mut next = jobs.iter();
        for chunk in &chunks {
            let mut at = 0;
            for &(n, stats) in &chunk.per_job {
                let Some(job) = next.next() else { break };
                let n = n as usize;
                let before = self.pending_tris.len();
                self.pending_tris.extend_from_slice(&chunk.tris[at..at + n]);
                if matches!(
                    job.kind,
                    cpu_tnl::JobKind::Lit { .. } | cpu_tnl::JobKind::Water { .. }
                ) {
                    self.pending_surf.resize(before, None);
                    self.pending_surf.extend_from_slice(&chunk.surf[at..at + n]);
                }
                self.tag_pending(before, n, job.texture);
                self.frame_stats.tnl.add(&stats);
                at += n;
            }
        }
        self.cpu_jobs = jobs;
        self.cpu_jobs.clear();
        self.job_ctxs.clear();
        self.times.tnl_us += micros(t0);
    }

    /// [`Self::flush_model_batch`]'s work: sort the pending triangles and
    /// expand them into GPU vertices.
    fn flush_model_batch_now(&mut self) {
        // Painter order as keys (texture group, depth, submission index) —
        // the same order the console backends draw in.
        let mut keys: Vec<(u64, u64)> = self
            .pending_tris
            .iter()
            .enumerate()
            .map(|(i, tri)| {
                let group = match self.pending_tex.get(i).copied().flatten() {
                    Some(t) => (((t.id as u64) << 1) | t.clamp as u64) + 1,
                    None => 0,
                };
                // Per-pixel lit triangles group apart too (another pipeline):
                // interleaved with plain ones in depth order they cut the
                // batch into a draw call per switch. The z-buffer resolves
                // the order between groups. Water comes last: it blends over
                // what the batch drew under it, and writes no depth.
                let surf = self.pending_surf.get(i).copied().flatten();
                let lit = surf.is_some() as u64;
                let water = surf.is_some_and(|s| s.water) as u64;
                (
                    (water << 63) | (group << 1) | lit,
                    trino_core::render3d::painter_key(tri.depth, i),
                )
            })
            .collect();
        keys.sort_unstable();
        let order: Vec<(Option<TexKey>, ScreenTri, Option<SurfTri>)> = keys
            .iter()
            .map(|&(_, key)| {
                let i = trino_core::render3d::painter_index(key);
                (
                    self.pending_tex.get(i).copied().flatten(),
                    self.pending_tris[i],
                    self.pending_surf.get(i).copied().flatten(),
                )
            })
            .collect();
        // Water comes last in the keys: the batch's shadows go in between,
        // over the opaque surfaces and under the water.
        let water_at = keys
            .iter()
            .position(|&(k, _)| k >> 63 == 1)
            .unwrap_or(keys.len());
        self.push_tri_runs(&order[..water_at]);
        self.push_shadow_runs();
        self.push_tri_runs(&order[water_at..]);
        self.pending_tris.clear();
        self.pending_tex.clear();
        self.pending_surf.clear();
    }

    /// Append `order`'s triangles as GPU vertices and commands, a command
    /// per run of one texture and pipeline.
    fn push_tri_runs(&mut self, order: &[(Option<TexKey>, ScreenTri, Option<SurfTri>)]) {
        let rgba = |c: Color| {
            [
                c.r as f32 / 255.0,
                c.g as f32 / 255.0,
                c.b as f32 / 255.0,
                c.a as f32 / 255.0,
            ]
        };
        let kind = |s: &Option<SurfTri>| match s {
            None => TriKind::Plain,
            Some(s) if s.water => TriKind::Water,
            Some(_) => TriKind::Lit,
        };
        let mut run_start = self.tri_verts.len() as u32;
        let mut run = order.first().map(|(t, _, s)| (*t, kind(s)));
        for (tex, tri, surf) in order {
            let lit = surf.is_some();
            let k = kind(surf);
            if Some((*tex, k)) != run {
                let first = run_start;
                let count = self.tri_verts.len() as u32 - first;
                if let Some((texture, kind)) = run {
                    self.commands.push(Cmd::Tris {
                        first,
                        count,
                        texture,
                        kind,
                    });
                }
                run_start = self.tri_verts.len() as u32;
                run = Some((*tex, k));
            }
            for i in 0..3 {
                let p = tri.pts[i];
                let mut v = TriVertex {
                    pos: [p.x, p.y],
                    color: rgba(tri.colors[i]),
                    depth: tri.z[i],
                    w: if tex.is_some() || lit { tri.w[i] } else { 1.0 },
                    uv: [tri.uv[i].x, tri.uv[i].y],
                    world: [0.0; 3],
                    normal: [0.0; 3],
                    base: [0.0; 4],
                    gloss: 0.0,
                    set: 0.0,
                    tex: match tex {
                        Some(t) => [
                            (t.filter == TexFilter::Point) as u32 as f32,
                            t.detail
                                .filter(|d| self.sprites.contains_key(&d.sprite.0))
                                .map_or(0.0, |d| d.scale()),
                        ],
                        None => [0.0; 2],
                    },
                };
                if let Some(s) = surf {
                    let sv = &s.v[i];
                    v.world = [sv.pos.x, sv.pos.y, sv.pos.z];
                    v.normal = [sv.normal.x, sv.normal.y, sv.normal.z];
                    v.base = rgba(sv.base);
                    v.gloss = s.gloss;
                    // The set (0..8) and the light mask, packed: exact in f32
                    // (a water set for water triangles).
                    v.set = (s.set + 8 * s.mask) as f32;
                }
                self.tri_verts.push(v);
            }
        }
        if let Some((texture, kind)) = run {
            let count = self.tri_verts.len() as u32 - run_start;
            self.commands.push(Cmd::Tris {
                first: run_start,
                count,
                texture,
                kind,
            });
        }
    }

    /// The batch's shadow triangles (ADR 0025): blobs, then planar ones,
    /// each far to near; a run drawn once per pixel goes in twice (mark,
    /// apply).
    fn push_shadow_runs(&mut self) {
        if self.pending_shadow.is_empty() {
            return;
        }
        let mut shadow = std::mem::take(&mut self.pending_shadow);
        shadow.sort_by_key(|(t, run)| (*run, !t.depth.to_bits()));
        let blob = TexKey::billboard(shadow::BLOB_SPRITE.0, TexFilter::Bilinear);
        let mut at = 0;
        while at < shadow.len() {
            let run = shadow[at].1;
            let end = at + shadow[at..].iter().take_while(|(_, r)| *r == run).count();
            let first = self.tri_verts.len() as u32;
            for (tri, _) in &shadow[at..end] {
                for i in 0..3 {
                    let p = tri.pts[i];
                    let c = tri.colors[i];
                    let textured = run == shadow::ShadowRun::Blob;
                    self.tri_verts.push(TriVertex {
                        pos: [p.x, p.y],
                        color: [
                            c.r as f32 / 255.0,
                            c.g as f32 / 255.0,
                            c.b as f32 / 255.0,
                            c.a as f32 / 255.0,
                        ],
                        depth: tri.z[i],
                        w: if textured { tri.w[i] } else { 1.0 },
                        uv: [tri.uv[i].x, tri.uv[i].y],
                        world: [0.0; 3],
                        normal: [0.0; 3],
                        base: [0.0; 4],
                        gloss: 0.0,
                        set: 0.0,
                        tex: [0.0; 2],
                    });
                }
            }
            let count = self.tri_verts.len() as u32 - first;
            let texture = (run == shadow::ShadowRun::Blob).then_some(blob);
            let kinds: &[TriKind] = match run {
                shadow::ShadowRun::PlanarOnce => &[TriKind::ShadowMark, TriKind::ShadowApply],
                _ => &[TriKind::Shadow],
            };
            for &kind in kinds {
                self.commands.push(Cmd::Tris {
                    first,
                    count,
                    texture,
                    kind,
                });
            }
            at = end;
        }
        shadow.clear();
        self.pending_shadow = shadow;
    }

    /// The shadow `params.shadow` casts for a draw of `model` at `matrix`
    /// (ADR 0025): a blob or a planar shadow into the batch's shadow
    /// bucket. Every profile draws them like its console.
    fn cast_shadow(&mut self, model: ModelId, matrix: &Mat34, params: &ModelParams) {
        use trino_core::shadow::{self as sh, ShadowPlan};
        if self.shadows.tier() == ShadowTier::None {
            return;
        }
        let plan = self.shadows.plan(params.shadow);
        if plan == ShadowPlan::None {
            return;
        }
        let screen = self.current_screen_size();
        let eye = self.eye_view();
        let ctx = tnl_context(
            &mut self.tnl_ctx,
            &self.camera,
            &self.light,
            screen,
            &[],
            eye,
        );
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        let sphere = sh::caster_sphere(mesh.bounds(), matrix);
        match plan {
            ShadowPlan::None => {}
            ShadowPlan::Blob { radius } => {
                let Some(b) = self.shadows.blob(&ctx, sphere.0, sphere.1, radius) else {
                    return;
                };
                let mut out = [EMPTY_TRI; 7];
                let n = sh::blob_tris(&ctx, &self.caps, &b, &mut out);
                if n > 0 {
                    self.shadows.stats.blobs += 1;
                    self.shadows.stats.tris += n as u32;
                    self.pending_shadow
                        .extend(out[..n].iter().map(|t| (*t, shadow::ShadowRun::Blob)));
                    if self.profile == SimProfile::N64 {
                        self.n64.shadow_blobs += 1;
                    }
                }
            }
            ShadowPlan::Planar => {
                let Some(d) = self.shadows.planar(&ctx, sphere.0, matrix, params) else {
                    return;
                };
                // The 3DS draws them once per pixel (its stencil); the N64
                // blends every triangle.
                let run = if self.profile == SimProfile::N64 {
                    shadow::ShadowRun::Planar
                } else {
                    shadow::ShadowRun::PlanarOnce
                };
                let before = self.pending_shadow.len();
                let pending = &mut self.pending_shadow;
                let tnl = sh::planar_tris(
                    &self.shadows,
                    &ctx,
                    &mut self.tnl_scratch,
                    &self.caps,
                    &mesh,
                    sphere,
                    &d,
                    &mut |t| pending.push((t, run)),
                );
                self.frame_stats.tnl.add(&tnl);
                let added = self.pending_shadow.len() - before;
                if added > 0 {
                    self.shadows.stats.planar += 1;
                    self.shadows.stats.tris += added as u32;
                }
                if self.profile == SimProfile::N64 {
                    self.n64.cpu_draws += 1;
                    self.n64.cpu.add(&tnl);
                }
            }
            ShadowPlan::Map => {
                self.shadows.stats.mapped += 1;
                self.map_casters.push(MapCaster {
                    model: model.0,
                    matrix: *matrix,
                    anim: params.anim,
                    sphere,
                });
            }
        }
    }

    /// The shadows this frame draws: the game's through the view
    /// ([`ShadowView`]), at the profile's tier capped by the quality
    /// preset (native: `Off` keeps the planar tier, no map). Rebuilds the
    /// blob texture when the softness changes.
    fn apply_shadow_view(&mut self) {
        let settings = self.shadow_view.resolve(self.shadow_game);
        let cap = match self.quality.shadows {
            Some(ShadowSetting::Off) if self.profile.is_native() => ShadowTier::Planar,
            _ => ShadowTier::Map,
        };
        self.shadows.set(settings.as_ref(), &self.caps, cap);
        if let Some(s) = self.shadows.settings()
            && self.blob_softness != Some(s.softness)
        {
            let softness = s.softness;
            let n = trino_core::shadow::BLOB_TEXTURE_SIZE as u32;
            self.upload_sprite(shadow::BLOB_SPRITE, n, n, &shadow::blob_rgba(softness));
            self.blob_softness = Some(softness);
        }
    }

    /// The Map tier's frame: the casters' world triangles into
    /// `map_verts` (posed, every triangle of each caster over the box) and
    /// the composite's parameters for a `size` picture (its focal length
    /// set by the caller). `None` without a mapped caster.
    fn shadow_map_frame(&mut self, size: (u32, u32)) -> Option<shadow_map::MapUniform> {
        if self.map_casters.is_empty() || self.shadows.tier() != ShadowTier::Map {
            return None;
        }
        let s = *self.shadows.settings()?;
        let max = self.device.limits().max_texture_dimension_2d;
        let map_size = shadow_map::map_size(s.map_size, self.quality.shadows, max);
        let spheres: Vec<_> = self.map_casters.iter().map(|c| c.sphere).collect();
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let fit = trino_core::shadow::map_fit(
            &self.camera,
            aspect,
            self.light.dir,
            s.distance,
            map_size as u16,
            &spheres,
        )?;
        let mut verts = std::mem::take(&mut self.map_verts);
        verts.clear();
        let mut joints = [Mat34::IDENTITY; trino_core::MAX_JOINTS];
        for c in &self.map_casters {
            // Its shadow lands where it is across the box: none outside.
            let p = fit.matrix.transform_point(c.sphere.0);
            let k = c.sphere.1 / fit.radius;
            if p.x.abs() > 1.0 + k || p.y.abs() > 1.0 + k {
                continue;
            }
            let Some((tmdl, layout)) = self.meshes.get(&c.model) else {
                continue;
            };
            let mesh = Mesh::from_layout(tmdl, layout);
            let n = mesh.pose(c.anim, &mut joints);
            verts.extend((0..mesh.index_count).map(|k| {
                let i = mesh.index(k) as usize;
                let p = if n > 0 {
                    mesh.posed_position(i, &joints, n)
                } else {
                    mesh.position(i)
                };
                let w = c.matrix.transform_point(p);
                [w.x, w.y, w.z]
            }));
        }
        self.map_verts = verts;
        let fog = self.light.fog;
        Some(shadow_map::MapUniform {
            size,
            focal: 0.0,
            near: trino_core::render3d::NEAR_PLANE,
            fog: if fog.enabled() {
                (fog.near, fog.far)
            } else {
                (0.0, 0.0)
            },
            view: self.camera.view(),
            fit,
            light: self.light.dir,
            color: s.color,
            softness: s.softness,
            distance: s.distance,
            map_size,
            taps: shadow_map::taps(self.quality.shadows),
            soft_growth: s.soft_growth,
            fade_distance: s.fade_distance,
        })
    }

    /// What the shadows show ([`ShadowView`]: as the game sets them, off,
    /// or in a debug color), from the next `set_shadows` on.
    pub fn set_shadow_view(&mut self, view: ShadowView) {
        self.shadow_view = view;
        self.apply_shadow_view();
    }

    pub fn shadow_view(&self) -> ShadowView {
        self.shadow_view
    }

    /// The tier this frame's shadows draw at (`None` without any).
    pub fn shadow_tier(&self) -> ShadowTier {
        self.shadows.tier()
    }

    /// Build the shadow pipelines a frame's commands use (the MSAA set
    /// when the scene is multisampled).
    fn ensure_shadow_pipes(&mut self) {
        let _pipes = pipes::scope(&self.pipe_entry);
        let used = self.commands.iter().any(|c| {
            matches!(
                c,
                Cmd::Tris {
                    kind: TriKind::Shadow | TriKind::ShadowMark | TriKind::ShadowApply,
                    ..
                }
            )
        });
        if !used {
            return;
        }
        self.warm_wait(precompile::JobKind::ShadowPipes(1));
        if self.msaa > 1 {
            self.warm_wait(precompile::JobKind::ShadowPipes(self.msaa));
        }
        if self.shadow_pipes.is_none() {
            self.shadow_pipes = Some(shadow::ShadowPipes::new(
                &self.device,
                &self.sources,
                1,
                OFFSCREEN_FORMAT,
            ));
        }
        if self.msaa > 1 && self.shadow_pipes_ms.is_none() {
            self.shadow_pipes_ms = Some(shadow::ShadowPipes::new(
                &self.device,
                &self.sources,
                self.msaa,
                OFFSCREEN_FORMAT,
            ));
        }
    }

    /// Record the texture of the `added` triangles appended at `before`
    /// (the tag list is only materialized once a textured draw joins).
    fn tag_pending(&mut self, before: usize, added: usize, key: Option<TexKey>) {
        if key.is_some() || !self.pending_tex.is_empty() {
            self.pending_tex.resize(before, None);
            self.pending_tex.extend(std::iter::repeat_n(key, added));
        }
    }

    /// Bind group of a sprite as a 3D texture — wrapping (models) or
    /// clamped (billboards). Created once, dropped when the sprite is
    /// re-uploaded.
    fn tex3d_group(&mut self, key: TexKey) -> Option<&wgpu::BindGroup> {
        if !self.tex3d.contains_key(&key) {
            let entry = self.sprites.get(&key.id)?;
            // The N64 look filters in the shader (3-point or point), from
            // nearest texels; elsewhere the sampler does.
            let linear = key.filter == TexFilter::Bilinear && !self.n64_look;
            let sampler = match (key.clamp, linear) {
                (true, false) => &self.clamp_nearest,
                (true, true) => &self.clamp_linear,
                (false, false) => &self.tex3d_nearest,
                // The quality's anisotropy (#55) on wrapping bilinear
                // textures.
                (false, true) => self.tex3d_aniso.as_ref().unwrap_or(&self.tex3d_linear),
            };
            let detail = key
                .detail
                .and_then(|d| self.sprites.get(&d.sprite.0))
                .map_or(&self.white_view, |e| &e.view);
            let dsampler = if linear {
                self.tex3d_aniso.as_ref().unwrap_or(&self.tex3d_linear)
            } else {
                &self.tex3d_nearest
            };
            let bg = create_tex3d_bind_group(
                &self.device,
                &self.tex3d_bgl,
                (&entry.view, sampler),
                (detail, dsampler),
            );
            self.tex3d.insert(key, bg);
        }
        self.tex3d.get(&key)
    }

    /// The size of the screen draws go to now: what the T&L projects to.
    fn current_screen_size(&self) -> Vec2 {
        let (w, h) = match (&self.second, self.screen) {
            (Some(sc), 1) => sc.size,
            _ => self.internal_size,
        };
        Vec2::new(w as f32, h as f32)
    }

    /// Render the recorded commands into the main target, or (`second`)
    /// into the second screen's, and submit.
    /// The multisampled color's format: HDR while the post stack draws.
    fn msaa_format(&self) -> wgpu::TextureFormat {
        if self.post_targets && self.hdr_ok {
            post::HDR_FORMAT
        } else {
            OFFSCREEN_FORMAT
        }
    }

    /// Before a main-screen frame: the scene targets, the multisampled
    /// color and the pipelines the post stack draws with (`on`), or back
    /// to the plain frame once it stops.
    fn prepare_post(&mut self, on: bool) {
        use precompile::JobKind as J;
        if on != self.post_targets {
            self.post_targets = on;
            self.rebuild_scene_targets(false);
        }
        let want = self.msaa_format();
        if self.msaa_targets.as_ref().is_some_and(|t| t.format != want) {
            self.msaa_targets = Some(create_msaa_targets(&self.device, self.scene_extent(), want));
            if let Some(ao) = &mut self.ao_pass_ms {
                ao.invalidate();
            }
            if let Some(m) = &mut self.shadow_map_ms {
                m.invalidate();
            }
            if let Some(h) = &mut self.hdr_sets[1] {
                h.ao.invalidate();
                h.map.invalidate();
            }
        }
        if !on {
            return;
        }
        self.post_used = true;
        if self.hdr_ok {
            let ms = self.msaa_targets.is_some() && self.ms_pipelines.is_some();
            let i = ms as usize;
            if self.hdr_sets[i].is_none() {
                self.warm_wait(J::Hdr(ms));
            }
            if self.hdr_sets[i].is_none() {
                if ms {
                    self.water.ensure_ms(&self.device, self.scene_extent());
                }
                self.hdr_sets[i] = Some(Box::new(post::build_hdr_set(
                    &self.device,
                    &self.sources,
                    (&self.water.groups, &self.water.bgl),
                    ms,
                    self.webgl2,
                )));
            }
            if self.gpu.as_ref().is_some_and(|g| !g.has_hdr(ms)) {
                self.warm_wait(J::HdrGpu(ms));
            }
            if let Some(g) = &mut self.gpu
                && !g.has_hdr(ms)
            {
                let (layout, shader) = g.ms_source();
                let samples = if ms { MSAA_SAMPLES } else { 1 };
                let p =
                    gpu_tnl::pipelines(&self.device, &layout, &shader, samples, post::HDR_FORMAT);
                g.install_hdr(ms, p);
            }
        }
        if self.post_pass.is_none() {
            self.warm_wait(J::Post);
        }
    }

    fn draw_commands(&mut self, second: bool) {
        trino_core::zone!("encode");
        let _pipes = pipes::scope(&self.pipe_entry);
        self.ensure_shadow_pipes();
        // The post stack (issue #64): this frame's settings, then its scene
        // targets and pipelines (or their removal once it stops).
        let post_frame = if second { None } else { self.frame_post() };
        if !second {
            self.prepare_post(post_frame.is_some());
        }
        if self.strict {
            // With two screens, the main screen's counts are in `main_*`:
            // the budgets are per frame, both screens together.
            let cmds = self.commands.len() as u32 + self.main_cmds;
            if cmds > self.caps.max_sprites_per_frame {
                panic!(
                    "strict mode: {} sprites this frame busts the {:?} profile budget \
                     of {} — reduce overdraw or batch static content.",
                    cmds, self.profile, self.caps.max_sprites_per_frame
                );
            }
            let tris =
                (self.tri_verts.len() / 3) as u32 + self.main_tris + self.pc_gpu_frame_tris();
            if tris > self.caps.max_tris_per_frame {
                panic!(
                    "strict mode: {tris} 3D triangles this frame busts the {:?} \
                     profile budget of {} — simplify the meshes.",
                    self.profile, self.caps.max_tris_per_frame
                );
            }
            // Every registered mesh counts as resident: the console's RAM
            // for its hardware tier (ADR 0020) is the ceiling.
            let models: usize = self.meshes.values().map(|(b, _)| b.len()).sum();
            if models > self.caps.ram_bytes as usize {
                panic!(
                    "strict mode: {} KB of models are loaded, over the {:?} profile's \
                     {} MB of RAM on the {:?} tier — load fewer, or name the extra ones \
                     for the expanded tier only (`platforms = [\"n64_expanded\"]`, \
                     ADR 0020).",
                    models.div_ceil(1024),
                    self.profile,
                    self.caps.ram_bytes >> 20,
                    self.caps.hw_tier
                );
            }
            if self.caps.max_3d_us_per_frame != u32::MAX {
                let e = self.n64_estimate();
                if e.cpu_us.max(e.rsp_us) > self.caps.max_3d_us_per_frame {
                    panic!(
                        "strict mode: this frame's 3D is ~{:.1} ms of CPU and ~{:.1} ms of \
                         RSP on the {:?} profile (trino_core::perf::N64_COST: {} draws on \
                         the CPU with {} vertices and {} triangles, {} on the RSP with {} \
                         triangles) — over its {:.0} ms ceiling. Fewer draws, fewer \
                         visible triangles, a `lod` level, or draws the RSP takes \
                         (rsp_check; n64-performance skill).",
                        e.cpu_us as f32 / 1000.0,
                        e.rsp_us as f32 / 1000.0,
                        self.profile,
                        self.n64.cpu_draws,
                        self.n64.cpu.verts,
                        self.n64.cpu.tris_in,
                        self.n64.rsp_draws,
                        self.n64.rsp_tris,
                        self.caps.max_3d_us_per_frame as f32 / 1000.0
                    );
                }
            }
        }
        let (instances, segments) = build_segments(&self.commands);
        self.commands.clear();
        // 3D texture bind groups must exist before the pass borrows self.
        let mut textures = Vec::new();
        for seg in &segments {
            match seg {
                Segment::Tris {
                    texture: Some(t), ..
                } => textures.push(*t),
                Segment::Meshes { first, count } => {
                    if let Some(g) = &self.gpu {
                        textures.extend(
                            g.batches(*first..first + count)
                                .iter()
                                .filter_map(|b| b.texture),
                        );
                    }
                }
                _ => {}
            }
        }
        for t in textures {
            let _ = self.tex3d_group(t);
        }
        if let Some(g) = &mut self.gpu {
            self.times.upload_bytes += g.upload(&self.device, &self.queue);
        }

        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        if bytes.len() as u64 > self.instance_capacity {
            self.instance_capacity = (bytes.len() as u64).next_power_of_two();
            self.instance_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-instances"),
                size: self.instance_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instance_buf, 0, bytes);
        }
        self.times.upload_bytes += bytes.len() as u32;

        let tri_bytes: &[u8] = bytemuck::cast_slice(&self.tri_verts);
        if tri_bytes.len() as u64 > self.tri_capacity {
            self.tri_capacity = (tri_bytes.len() as u64).next_power_of_two();
            self.tri_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("trino-tri-verts"),
                size: self.tri_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !tri_bytes.is_empty() {
            self.queue.write_buffer(&self.tri_buf, 0, tri_bytes);
        }
        self.times.upload_bytes += tri_bytes.len() as u32;
        self.tri_verts.clear();
        if segments.iter().any(|s| {
            matches!(
                s,
                Segment::Tris {
                    kind: TriKind::Lit | TriKind::Water,
                    ..
                }
            )
        }) {
            let lights = self.light_sets.bytes();
            self.queue.write_buffer(&self.lights_buf, 0, &lights);
            self.times.upload_bytes += lights.len() as u32;
        }
        // The first water segment: the water sets go up, and the per-pixel
        // tier copies the scene's depth before it (its soft foam reads it).
        let first_water = segments.iter().position(|s| {
            matches!(
                s,
                Segment::Tris {
                    kind: TriKind::Water,
                    ..
                }
            )
        });
        if first_water.is_some() {
            let sets = self.water_sets.bytes();
            self.queue.write_buffer(&self.water.buf, 0, &sets);
            self.times.upload_bytes += sets.len() as u32;
        }
        let depth_copy_at =
            first_water.filter(|_| !second && self.caps.water_tier == WaterTier::PerPixel);

        // In N64 look the framebuffer is 16-bit: the clear (an RDP fill, no
        // dither) keeps the top 5 bits, stored expanded like a 5551 readback
        // (sprites/triangles are dithered+quantized in the shader).
        let ch = |v: u8| -> f64 {
            if self.n64_look {
                let c5 = v >> 3;
                ((c5 << 3) | (c5 >> 2)) as f64 / 255.0
            } else {
                v as f64 / 255.0
            }
        };
        let clear = if second {
            // The touch screen's own background.
            wgpu::Color {
                r: 16.0 / 255.0,
                g: 18.0 / 255.0,
                b: 28.0 / 255.0,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: ch(self.clear.r),
                g: ch(self.clear.g),
                b: ch(self.clear.b),
                a: self.clear.a as f64 / 255.0,
            }
        };
        // Screen-space AO darkens the 3D, not the 2D drawn over it: the
        // pass ends after the frame's last 3D draw, the AO passes run, and
        // the rest (HUD, text) draws on top. A render scale other than 1
        // (issue #55) splits there too: the 3D draws into its own targets
        // at the scaled size and is upscaled onto the picture, and the rest
        // draws at the picture's resolution.
        let ao = if second { None } else { self.frame_ao() };
        let scaled = !second && self.scene.is_some();
        // The Map tier's casters (their world triangles into `map_verts`)
        // and the box over the view: its passes run at the split too,
        // before the AO.
        let map = if second {
            None
        } else {
            let size = if scaled {
                self.scene_size()
            } else {
                self.internal_size
            };
            self.shadow_map_frame(size)
        };
        let split = if ao.is_some() || scaled || map.is_some() || post_frame.is_some() {
            segments
                .iter()
                .rposition(|s| matches!(s, Segment::Tris { .. } | Segment::Meshes { .. }))
                .map(|i| i + 1)
        } else {
            None
        };
        // A scaled frame without 3D (a menu) draws on the picture alone.
        let scene_pass = scaled && split.is_some();
        let (first_part, rest) = segments.split_at(split.unwrap_or(segments.len()));
        // The passes this frame builds lazily below: take them from a
        // running warm-up first (a worker may be compiling one).
        if !self.warm.is_idle() {
            use precompile::JobKind as J;
            let ms = !second
                && self.msaa_targets.is_some()
                && self.ms_pipelines.is_some()
                && (!scaled || scene_pass);
            if depth_copy_at.is_some() && self.webgl2 {
                self.warm_wait(J::DepthBlit);
            }
            if map.is_some() && split.is_some() {
                self.warm_wait(J::ShadowMap(ms));
            }
            if ao.is_some() && split.is_some() {
                self.warm_wait(J::Ao(ms));
            }
            if scene_pass {
                self.warm_wait(J::Upscale);
            }
            if self.cvd_view != trino_core::CvdMode::Off {
                self.warm_wait(J::Cvd(self.cvd_view, CvdKind::Simulate));
            }
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        // Dynamic resolution times the main screen's GPU work.
        let timed = !second && self.dynres.is_some();
        if timed && !self.gpu_timer_tried {
            self.gpu_timer_tried = true;
            self.gpu_timer = gpu_timer::GpuTimer::new(&self.device, &self.queue);
        }
        if let (true, Some(t)) = (timed, &mut self.gpu_timer) {
            t.begin(&mut encoder);
        }
        // The trace's GPU zones (issue #62): the main screen's passes.
        let zoned = !second && (trino_core::trace::is_on() || trino_core::trace::hooked());
        if zoned && !self.gpu_zones_tried {
            self.gpu_zones_tried = true;
            self.gpu_zones = gpu_zones::GpuZones::new(&self.device, &self.queue);
        }
        let mut zones = if zoned { self.gpu_zones.take() } else { None };
        if let Some(z) = &mut zones {
            z.begin(&mut encoder, "scene");
        }
        // Multisampled (the main screen only): the 3D draws into the MSAA
        // targets and resolves into the offscreen one (the scaled scene's
        // when scaled) at the end of the pass. The MSAA targets have the
        // 3D's size, so a scaled frame without 3D draws single-sampled.
        let ms = !second
            && self.msaa_targets.is_some()
            && self.ms_pipelines.is_some()
            && (!scaled || scene_pass);
        let mut load = (wgpu::LoadOp::Clear(clear), wgpu::LoadOp::Clear(1.0));
        let mut first_part = first_part;
        if let Some(at) = depth_copy_at.filter(|&at| at < first_part.len()) {
            // The scene before the water, its depth copied, then the rest.
            // With MSAA the first pass holds its samples (no resolve) and
            // the copy is multisampled too.
            let (before, after) = first_part.split_at(at);
            let hold = if ms { MsPass::Hold } else { MsPass::Off };
            let (draws, switches) =
                self.record_segments(&mut encoder, second, scene_pass, before, load, hold);
            self.times.draw_calls += draws;
            self.times.pipeline_switches += switches;
            let (scene_depth_tex, scene_depth_view) = match (&self.scene, scene_pass) {
                (Some(t), true) => (&t.depth_tex, &t.depth_view),
                _ => (&self.depth_tex, &self.depth_view),
            };
            let (src, dst) = match (&self.msaa_targets, &self.water.ms, ms) {
                (Some(t), Some(w), true) => (&t.depth_tex, &w.depth_copy),
                _ => (scene_depth_tex, &self.water.depth_copy),
            };
            if self.webgl2 {
                // GL can't copy a depth attachment: draw it over.
                let blit = self
                    .depth_blit
                    .get_or_insert_with(|| depth_blit::DepthBlit::new(&self.device));
                let dst = dst.create_view(&wgpu::TextureViewDescriptor::default());
                blit.copy(&self.device, &mut encoder, scene_depth_view, &dst);
            } else {
                // Depth copies are whole textures (WebGPU has no partial
                // ones): under dynamic resolution, its allocated extent.
                encoder.copy_texture_to_texture(
                    src.as_image_copy(),
                    dst.as_image_copy(),
                    src.size(),
                );
            }
            load = (wgpu::LoadOp::Load, wgpu::LoadOp::Load);
            first_part = after;
        }
        let end = if ms {
            MsPass::Resolve {
                keep_depth: (ao.is_some() || map.is_some()) && split.is_some(),
            }
        } else {
            MsPass::Off
        };
        let (draws, switches) =
            self.record_segments(&mut encoder, second, scene_pass, first_part, load, end);
        self.times.draw_calls += draws;
        self.times.pipeline_switches += switches;
        if let (Some(z), Some(_), Some(_)) = (&mut zones, map, split) {
            z.mark(&mut encoder, "shadow map");
        }
        if let (Some(mut params), Some(_)) = (map, split) {
            let (color, scene_depth) = match (&self.scene, scene_pass) {
                (Some(t), true) => (&t.color_view, &t.depth_view),
                _ => (&self.offscreen_view, &self.depth_view),
            };
            params.size = if scene_pass {
                self.scene_size()
            } else {
                self.internal_size
            };
            let half = self.camera.fov_y * 0.5;
            params.focal = trino_core::math3d::cos(half) / trino_core::math3d::sin(half)
                * params.size.1 as f32
                * 0.5;
            // With MSAA the composite reads the multisampled depth (its
            // first sample), as the AO does; into an HDR scene, its twin.
            let hdr = scene_pass
                && self
                    .scene
                    .as_ref()
                    .is_some_and(|t| t.format == post::HDR_FORMAT);
            let (pass, depth) = match (&self.msaa_targets, ms) {
                (Some(t), true) if hdr => (
                    &mut self.hdr_sets[1].as_mut().expect("prepare_post").map,
                    &t.depth,
                ),
                (Some(t), true) => (
                    self.shadow_map_ms.get_or_insert_with(|| {
                        shadow_map::MapPass::new(&self.device, OFFSCREEN_FORMAT, true, false)
                    }),
                    &t.depth,
                ),
                _ if hdr => (
                    &mut self.hdr_sets[0].as_mut().expect("prepare_post").map,
                    scene_depth,
                ),
                _ => (
                    self.shadow_map.get_or_insert_with(|| {
                        shadow_map::MapPass::new(&self.device, OFFSCREEN_FORMAT, false, self.webgl2)
                    }),
                    scene_depth,
                ),
            };
            self.times.upload_bytes += pass.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.map_verts,
                depth,
                color,
                &params,
            );
            self.times.shadow_maps += 1;
        }
        if let (Some((settings, mask)), Some(_)) = (ao, split) {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "ao");
            }
            let (w, h) = if scene_pass {
                self.scene_size()
            } else {
                self.internal_size
            };
            let half = self.camera.fov_y * 0.5;
            let focal =
                trino_core::math3d::cos(half) / trino_core::math3d::sin(half) * h as f32 * 0.5;
            let fog = self.light.fog;
            let params = ao::AoUniform {
                size: (w, h),
                focal,
                near: trino_core::render3d::NEAR_PLANE,
                ao: settings,
                fog: if fog.enabled() {
                    (fog.near, fog.far)
                } else {
                    (0.0, 0.0)
                },
                mask,
            };
            let (color, scene_depth) = match (&self.scene, scene_pass) {
                (Some(t), true) => (&t.color_view, &t.depth_view),
                _ => (&self.offscreen_view, &self.depth_view),
            };
            // With MSAA the occlusion reads the multisampled depth (its
            // first sample); its own pass, as its bind groups hold the view.
            // Into an HDR scene, its twin.
            let hdr = scene_pass
                && self
                    .scene
                    .as_ref()
                    .is_some_and(|t| t.format == post::HDR_FORMAT);
            let (pass, depth) = match (&self.msaa_targets, ms) {
                (Some(t), true) if hdr => (
                    &mut self.hdr_sets[1].as_mut().expect("prepare_post").ao,
                    &t.depth,
                ),
                _ if hdr => (
                    &mut self.hdr_sets[0].as_mut().expect("prepare_post").ao,
                    scene_depth,
                ),
                (Some(t), true) => (
                    self.ao_pass_ms.get_or_insert_with(|| {
                        ao::AoPass::new(&self.device, OFFSCREEN_FORMAT, true, false)
                    }),
                    &t.depth,
                ),
                _ => (
                    self.ao_pass.get_or_insert_with(|| {
                        ao::AoPass::new(&self.device, OFFSCREEN_FORMAT, false, self.webgl2)
                    }),
                    scene_depth,
                ),
            };
            self.times.upload_bytes += pass.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                depth,
                color,
                &params,
            );
            self.times.ao_passes += 1;
        }
        if let (true, Some((settings, bloom_only))) = (scene_pass, post_frame) {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "post");
            }
            // The post stack: bloom, then the composite onto the picture
            // (it upscales a scaled scene too).
            let bloom_cap = self.bloom_cap();
            let lut = settings.grading.and_then(|id| self.luts.get(&id));
            let pass = self
                .post_pass
                .get_or_insert_with(|| post::PostPass::new(&self.device, self.hdr_ok));
            if let Some(scene) = &self.scene {
                let work = pass.encode(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &post::PostFrame {
                        scene,
                        dst: (&self.offscreen_view, self.internal_size),
                        settings: &settings,
                        bloom_only,
                        bloom_cap,
                        lut,
                        sharpen: self.quality.sharpen,
                        seed: self.post_seed,
                    },
                );
                self.post_seed = self.post_seed.wrapping_add(1);
                self.times.post_passes += 1;
                self.times.bloom_levels = work.bloom_levels;
                self.times.upload_bytes += work.upload;
            }
        } else if scene_pass {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "upscale");
            }
            // The scaled 3D onto the picture: bilinear, sharpened below 1.
            let up = self
                .upscale
                .get_or_insert_with(|| upscale::Upscale::new(&self.device));
            if let Some(scene) = &mut self.scene {
                self.times.upload_bytes += up.encode(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    scene,
                    (&self.offscreen_view, self.internal_size),
                    self.quality.sharpen,
                );
                self.times.upscales += 1;
            }
        }
        if split.is_some() && !rest.is_empty() {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "2d");
            }
            // The rest is 2D (no depth test): with MSAA it draws
            // single-sampled over the resolved, occluded frame, and a
            // scaled frame's picture has no depth of its own yet.
            let depth_load = if ms || scene_pass {
                wgpu::LoadOp::Clear(1.0)
            } else {
                wgpu::LoadOp::Load
            };
            let (draws, switches) = self.record_segments(
                &mut encoder,
                second,
                false,
                rest,
                (wgpu::LoadOp::Load, depth_load),
                MsPass::Off,
            );
            self.times.draw_calls += draws;
            self.times.pipeline_switches += switches;
        }

        // N64 Video Interface: dedither the finished 16-bit frame, then put
        // it back where every consumer (window blit, editor, readback)
        // reads it.
        if self.n64_look && self.vi_dedither && !second {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "vi");
            }
            let vi_view = self
                .vi_tex
                .create_view(&wgpu::TextureViewDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("trino-vi-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &vi_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.vi_pipeline);
                pass.set_bind_group(0, &self.blit_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_texture(
                self.vi_tex.as_image_copy(),
                self.offscreen_tex.as_image_copy(),
                wgpu::Extent3d {
                    width: self.internal_size.0,
                    height: self.internal_size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        // The players' correction filter (issue #64), over the whole
        // picture (the HUD too), before the simulation view.
        let fix = if second {
            trino_core::CvdMode::Off
        } else {
            self.frame_cvd_filter()
        };
        if fix != trino_core::CvdMode::Off {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "cvd filter");
            }
            self.warm_wait(precompile::JobKind::Cvd(fix, CvdKind::Correct));
            self.cvd
                .get_or_insert_with(|| cvd::CvdPass::new(&self.device))
                .encode(
                    &self.device,
                    &mut encoder,
                    &self.offscreen_tex,
                    &self.offscreen_view,
                    self.internal_size,
                    fix,
                    CvdKind::Correct,
                    0,
                );
        }
        // The color-vision simulation view, last: what the frame looks like
        // to the viewer it simulates.
        if self.cvd_view != trino_core::CvdMode::Off {
            if let Some(z) = &mut zones {
                z.mark(&mut encoder, "cvd view");
            }
            let (tex, view, size) = match (second, &self.second) {
                (true, Some(s)) => (&s.tex, &s.view, s.size),
                _ => (
                    &self.offscreen_tex,
                    &self.offscreen_view,
                    self.internal_size,
                ),
            };
            self.cvd
                .get_or_insert_with(|| cvd::CvdPass::new(&self.device))
                .encode(
                    &self.device,
                    &mut encoder,
                    tex,
                    view,
                    size,
                    self.cvd_view,
                    CvdKind::Simulate,
                    second as usize,
                );
        }
        if let (true, Some(t)) = (timed, &mut self.gpu_timer) {
            t.end(&mut encoder);
        }
        if let Some(z) = &mut zones {
            z.end(&mut encoder);
        }
        let submit = trino_core::trace::Zone::begin("queue submit");
        self.queue.submit([encoder.finish()]);
        submit.end();
        if let (true, Some(t)) = (timed, &mut self.gpu_timer) {
            t.submitted();
        }
        if let Some(mut z) = zones {
            z.submitted();
            self.gpu_zones = Some(z);
        }
    }

    /// One render pass over `segments` into the main target (or the
    /// second screen's, or with `scene` the scaled 3D's), loading or
    /// clearing color and depth. Returns the GPU draw calls and the
    /// pipeline changes between them.
    fn record_segments(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        second: bool,
        scene: bool,
        segments: &[Segment],
        (color_load, depth_load): (wgpu::LoadOp<wgpu::Color>, wgpu::LoadOp<f32>),
        ms: MsPass,
    ) -> (u32, u32) {
        let (color_view, depth_view, globals_bg) = match (&self.second, second, &self.scene) {
            (Some(sc), true, _) => (&sc.view, &sc.depth_view, &sc.globals_bg),
            (_, false, Some(t)) if scene => (&t.color_view, &t.depth_view, &self.globals_bg),
            _ => (&self.offscreen_view, &self.depth_view, &self.globals_bg),
        };
        // Multisampled: draw into the MSAA targets, resolve the color into
        // the offscreen one at the end of the scene. Nothing is written
        // back but the depth the AO pass reads, unless the scene goes on in
        // another pass (`Hold`: the per-pixel water's depth copy).
        let ms = match (&self.msaa_targets, &self.ms_pipelines, ms) {
            (_, _, MsPass::Off) => None,
            (Some(t), Some(p), mode) => Some((t, p, mode)),
            _ => None,
        };
        let keep = |on: bool| {
            if on {
                wgpu::StoreOp::Store
            } else {
                wgpu::StoreOp::Discard
            }
        };
        let (pass_color, pass_resolve, pass_depth, color_store, depth_store) = match ms {
            Some((t, _, MsPass::Hold)) => (&t.color, None, &t.depth, keep(true), keep(true)),
            Some((t, _, mode)) => (
                &t.color,
                Some(color_view),
                &t.depth,
                keep(false),
                keep(mode == MsPass::Resolve { keep_depth: true }),
            ),
            None => (
                color_view,
                None,
                depth_view,
                wgpu::StoreOp::Store,
                wgpu::StoreOp::Store,
            ),
        };
        // The post stack's HDR scene: every pipeline's twin (issue #64).
        let hdr = !second
            && scene
            && self
                .scene
                .as_ref()
                .is_some_and(|t| t.format == post::HDR_FORMAT);
        let hdr_set = if hdr {
            self.hdr_sets[ms.is_some() as usize].as_deref()
        } else {
            None
        };
        let shadow_pipes = match (hdr_set, ms) {
            (Some(h), _) => Some(&h.shadow),
            (None, Some(_)) => self.shadow_pipes_ms.as_ref(),
            (None, None) => self.shadow_pipes.as_ref(),
        };
        let pipes = match (hdr_set, ms) {
            (Some(h), _) => [&h.scene[0], &h.scene[1], &h.scene[2], &h.scene[3]],
            (None, Some((_, p, _))) => [&p[0], &p[1], &p[2], &p[3]],
            (None, None) => [
                &self.sprite_pipeline,
                &self.tri_pipeline,
                &self.tri_clamp_pipeline,
                &self.lit_pipeline,
            ],
        };
        let mut draws = 0;
        // Pipeline changes: the kind each segment binds (sprites, plain,
        // clamped, lit, water, GPU T&L), counted when it differs.
        let (mut last, mut switches) = (u8::MAX, 0u32);
        let mut bind = |kind: u8| {
            if kind != last {
                switches += (last != u8::MAX) as u32;
                last = kind;
            }
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trino-sprite-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: pass_color,
                resolve_target: pass_resolve,
                ops: wgpu::Operations {
                    load: color_load,
                    store: color_store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: pass_depth,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: depth_store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let (false, Some(t), true) = (second, &self.scene, scene) {
            // The drawn corner of targets kept for dynamic resolution's range.
            if t.size != t.extent {
                pass.set_viewport(0.0, 0.0, t.size.0 as f32, t.size.1 as f32, 0.0, 1.0);
            }
        }
        pass.set_bind_group(0, globals_bg, &[]);
        for segment in segments {
            match segment {
                Segment::Sprites { sprite, instances } => {
                    if let Some(entry) = self.sprites.get(sprite) {
                        pass.set_pipeline(pipes[0]);
                        bind(0);
                        pass.set_vertex_buffer(0, self.instance_buf.slice(..));
                        pass.set_bind_group(1, &entry.bind_group, &[]);
                        pass.draw(0..4, instances.clone());
                        draws += 1;
                    }
                }
                Segment::Tris {
                    first,
                    count,
                    texture,
                    kind,
                } => {
                    let (pipeline, k) = match kind {
                        TriKind::Water => {
                            pass.set_bind_group(2, &self.lights_bg, &[]);
                            let (bg, mut per_pixel, mut vertex) =
                                match (&self.water.ms, ms.is_some()) {
                                    (Some(w), true) => (&w.bg, &w.per_pixel, &w.vertex),
                                    _ => {
                                        (&self.water.bg, &self.water.per_pixel, &self.water.vertex)
                                    }
                                };
                            if let Some(h) = hdr_set {
                                (per_pixel, vertex) = (&h.water[0], &h.water[1]);
                            }
                            pass.set_bind_group(3, bg, &[]);
                            // The second screen (3DS profile) is never the
                            // per-pixel tier.
                            if self.caps.water_tier == WaterTier::PerPixel && !second {
                                (per_pixel, 4)
                            } else {
                                (vertex, 4)
                            }
                        }
                        TriKind::Lit => {
                            pass.set_bind_group(2, &self.lights_bg, &[]);
                            (pipes[3], 3)
                        }
                        TriKind::Plain if texture.is_some_and(|t| t.clamp) => (pipes[2], 2),
                        TriKind::Plain => (pipes[1], 1),
                        TriKind::Shadow | TriKind::ShadowMark | TriKind::ShadowApply => {
                            // Built by `ensure_shadow_pipes` before the
                            // frame's passes.
                            let Some(sp) = shadow_pipes else {
                                continue;
                            };
                            match kind {
                                TriKind::Shadow => (&sp.blend, 6),
                                TriKind::ShadowMark => (&sp.mark, 7),
                                _ => (&sp.apply, 8),
                            }
                        }
                    };
                    bind(k);
                    pass.set_pipeline(pipeline);
                    pass.set_vertex_buffer(0, self.tri_buf.slice(..));
                    let bg = texture
                        .and_then(|t| self.tex3d.get(&t))
                        .unwrap_or(&self.white_bg);
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw(*first..*first + *count, 0..1);
                    draws += 1;
                }
                Segment::Meshes { first, count } => {
                    if let Some(g) = &self.gpu {
                        let texture = |t: Option<TexKey>| {
                            t.and_then(|t| self.tex3d.get(&t)).unwrap_or(&self.white_bg)
                        };
                        bind(5);
                        draws += g.record(
                            &mut pass,
                            *first..first + count,
                            ms.is_some(),
                            hdr,
                            &texture,
                        );
                    }
                }
            }
        }
        drop(pass);
        (draws, switches)
    }

    /// One instanced GPU T&L draw of `model` (`draw_model_instances` on
    /// the GPU path): the copies' instances, the call's pose and texture.
    fn draw_gpu_instances(
        &mut self,
        model: ModelId,
        params: &ModelParams,
        accepted: &[GpuInstanceOf],
    ) {
        if self.gpu.is_none() && self.gpu_tnl_active() {
            self.warm_wait(precompile::JobKind::GpuTnl);
        }
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        let n = accepted.len() as u32;
        self.frame_stats.models += n;
        self.frame_stats.tnl.instanced += n;
        self.frame_stats.tnl.pc_gpu_draws += n;
        self.frame_stats.tnl.pc_gpu_tris += n * (mesh.index_count / 3) as u32;
        if let Some(t) = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0))
        {
            texture_budget(
                TextureBudget {
                    sprites: &self.sprites,
                    caps: &self.caps,
                    strict: self.strict,
                    profile: self.profile,
                    stats: &mut self.frame_stats,
                },
                t,
                true,
            );
        }
        let screen = self.current_screen_size();
        let eye = self.eye_view();
        let ctx = tnl_context(
            &mut self.tnl_ctx,
            &self.camera,
            &self.light,
            screen,
            &[],
            eye,
        );
        let _pipes = pipes::scope(&self.pipe_entry);
        let gpu = self.gpu.get_or_insert_with(|| {
            gpu_tnl::GpuTnl::new(&self.device, [&self.globals_bgl, &self.tex3d_bgl])
        });
        if !gpu.has_mesh(model.0) {
            gpu.upload_mesh(&self.device, &self.queue, model.0, &mesh);
        }
        if self.msaa > 1 {
            gpu.ensure_ms(&self.device, self.msaa);
        }
        let insts: Vec<gpu_tnl::GpuInstance> = accepted.iter().map(|a| a.0).collect();
        gpu.draw(
            &ctx,
            gpu_tnl::DrawKey {
                model: model.0,
                mesh: &mesh,
                params,
                texture: accepted[0].1,
            },
            &insts,
        );
    }

    /// Triangles the GPU T&L path submitted this frame (every copy's).
    fn pc_gpu_frame_tris(&self) -> u32 {
        self.frame_stats.tnl.pc_gpu_tris
    }

    /// The end of a frame: render what is left (the second screen is
    /// cleared even when nothing was drawn on it), then show it.
    fn flush(&mut self) {
        if self.screen == 1 {
            self.draw_commands(true);
        } else {
            self.draw_commands(false);
            if self.second.is_some() {
                self.draw_commands(true);
            }
        }
        self.present();
        self.after_frame();
    }

    /// Blit the finished frame(s) to the window, if there is one.
    fn present(&mut self) {
        trino_core::zone!("present");
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        fn acquire(frame: wgpu::CurrentSurfaceTexture) -> Result<wgpu::SurfaceTexture, bool> {
            use wgpu::CurrentSurfaceTexture as C;
            match frame {
                C::Success(f) | C::Suboptimal(f) => Ok(f),
                // Reconfigure-and-retry cases vs. skip-this-frame cases.
                C::Outdated | C::Lost => Err(true),
                C::Timeout | C::Occluded | C::Validation => Err(false),
            }
        }
        let frame =
            self.surface
                .as_ref()
                .filter(|_| !self.no_present)
                .and_then(|ss| match acquire(ss.surface.get_current_texture()) {
                    Ok(frame) => Some(frame),
                    Err(true) => {
                        ss.surface.configure(&self.device, &ss.config);
                        acquire(ss.surface.get_current_texture()).ok()
                    }
                    Err(false) => None,
                });

        if let (Some(ss), Some(frame)) = (self.surface.as_ref(), &frame) {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trino-blit-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let surface = (ss.config.width, ss.config.height);
            let second = self.second.as_ref().map(|sc| sc.size);
            let (main, below) = present_layout(self.internal_size, second, surface);
            pass.set_pipeline(&self.blit_pipeline);
            let (vx, vy, vw, vh) = main;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_bind_group(0, &self.blit_bind_group, &[]);
            pass.draw(0..3, 0..1);
            if let (Some(sc), Some((vx, vy, vw, vh))) = (&self.second, below) {
                pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
                pass.set_bind_group(0, &sc.blit_bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        self.queue.submit([encoder.finish()]);
        if let Some(frame) = frame {
            frame.present();
        }
    }
}

/// The renderer fields texture accounting reads/writes (disjoint from the
/// mesh being drawn, so it works while a mesh is borrowed).
struct TextureBudget<'a> {
    sprites: &'a HashMap<u32, SpriteEntry>,
    caps: &'a Caps,
    strict: bool,
    profile: SimProfile,
    stats: &'a mut FrameStats,
}

/// The N64 profile's frame, split as the console runs it.
#[derive(Clone, Copy, Default, Debug)]
struct N64Split {
    cpu: trino_core::render3d::TnlStats,
    cpu_draws: u32,
    rsp_draws: u32,
    rsp_tris: u32,
    /// Of those, the water surfaces (priced by the water overlay's terms).
    water_draws: u32,
    water_tris: u32,
    /// Blob shadows (the CPU's, [`trino_core::perf::CostModel::us_per_blob`]).
    shadow_blobs: u32,
}

/// The native profile's GPU T&L decision for one draw (issue #51,
/// [`trino_core::render3d::pc_gpu_check`]), with the reasons only the
/// renderer knows: `lit` (shaded per pixel: its surface varyings come from
/// the CPU path), `alpha` (its texture blends) and `enabled` (the path is
/// on).
fn pc_gpu_decide(
    (ctx, mesh, matrix, params): (&TnlContext, &Mesh, &Mat34, &ModelParams),
    lit: bool,
    alpha: bool,
    enabled: bool,
) -> Result<(), PcGpuReject> {
    use trino_core::render3d::{pc_gpu_check, pc_gpu_mesh_check};
    pc_gpu_mesh_check(mesh)?;
    pc_gpu_check(ctx, mesh, matrix, params)?;
    if lit {
        return Err(PcGpuReject::PerPixelLit);
    }
    if alpha {
        return Err(PcGpuReject::TextureAlpha);
    }
    if !enabled {
        return Err(PcGpuReject::Disabled);
    }
    Ok(())
}

/// The detail layer's UV scale of a textured draw (0 = none, or its sprite
/// isn't loaded), as the screen-space triangles carry it.
fn detail_scale(
    sprites: &HashMap<u32, SpriteEntry>,
    texture: Option<SpriteId>,
    params: &ModelParams,
) -> f32 {
    match (texture, params.detail) {
        (Some(_), Some(d)) if sprites.contains_key(&d.sprite.0) => d.scale(),
        _ => 0.0,
    }
}

/// `ctx`'s index in a batch's T&L contexts (the last one when equal).
fn push_ctx(ctxs: &mut Vec<TnlContext>, ctx: TnlContext) -> u32 {
    if ctxs.last() != Some(&ctx) {
        ctxs.push(ctx);
    }
    ctxs.len() as u32 - 1
}

/// A copy the GPU T&L path accepted, with its texture state.
type GpuInstanceOf = (gpu_tnl::GpuInstance, Option<TexKey>);

/// [`PcRenderer::n64_estimate`]: the frame's 3D on the N64.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct N64Estimate {
    /// CPU time: the draws it transforms, plus each RSP draw's setup.
    pub cpu_us: u32,
    /// RSP time (its triangles); in parallel with the CPU.
    pub rsp_us: u32,
    pub cpu_draws: u32,
    pub rsp_draws: u32,
    pub rsp_tris: u32,
}

/// Texture-memory accounting (+ strict-mode checks) for a 3D/billboard
/// texture: one texture per draw lives in texture memory as a whole
/// (TMEM on N64) — worst-case 16-bit texels, like sprites.
fn texture_budget(r: TextureBudget<'_>, t: SpriteId, tiled: bool) {
    let Some(entry) = r.sprites.get(&t.0) else {
        return;
    };
    let (w, h) = entry.size;
    // One texture per draw lives in texture memory as a whole
    // (TMEM on N64) — worst-case 16-bit texels, like sprites.
    let bpp = if r.caps.color_depth_bits <= 16 { 2 } else { 4 };
    r.stats.max_texture_bytes = r.stats.max_texture_bytes.max(w * h * bpp);
    if r.strict {
        if tiled && (!w.is_power_of_two() || !h.is_power_of_two()) {
            panic!(
                "strict mode: texture {t:?} ({w}x{h}) is used on a 3D model but is \
                 not power-of-two sized — consoles can only tile POT textures."
            );
        }
        if let Err(e) = r.caps.validate_texture(w as u16, h as u16, bpp) {
            panic!(
                "strict mode: 3D texture {t:?} ({w}x{h}) busts the {:?} profile \
                 budget: {e:?}. One texture per draw must fit texture memory \
                 (N64 TMEM: 4 KB — e.g. 32x32 RGBA16 or 64x64 CI4).",
                r.profile
            );
        }
    }
}

/// The renderer state a stereo frame's top-screen calls change: what each
/// eye's replay starts from (issue #63).
#[derive(Clone, Copy)]
struct TopState {
    camera: Camera3,
    light: Light,
    locals: LightList,
    game_filter: TexFilter,
    wetness: f32,
    water: WaterState,
    shadow_game: Option<ShadowSettings>,
    shadows: ShadowFrame,
}

/// The T&L constants for `camera`, `light`, `screen` and the per-vertex
/// local lights (empty on the per-pixel tier), cached in `slot` with the
/// lights they were built for (rebuilt only when one of them changed).
///
/// `eye` is a stereo eye's shift and convergence (issue #63; `(0, _)` the
/// mono picture): the context projects off axis for it, and the cache knows
/// it by the eye's camera.
fn tnl_context(
    slot: &mut Option<(TnlContext, LightList)>,
    camera: &Camera3,
    light: &Light,
    screen: Vec2,
    locals: &[LocalLight],
    eye: (f32, f32),
) -> TnlContext {
    let seen = if eye.0 == 0.0 {
        *camera
    } else {
        trino_core::stereo::eye_camera(camera, eye.0)
    };
    match slot {
        Some((c, l)) if c.matches(&seen, light, screen) && l.as_slice() == locals => *c,
        _ => {
            let mut list = LightList::EMPTY;
            list.set(locals, trino_core::MAX_LOCAL_LIGHTS as u8);
            let c = TnlContext::new(camera, light, screen)
                .with_eye(eye.0, eye.1)
                .with_local_lights(locals);
            *slot = Some((c, list));
            c
        }
    }
}

impl Renderer for PcRenderer {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn begin_frame(&mut self, clear: Color) {
        self.warm_poll();
        self.clear = clear;
        self.stereo.begin();
        self.screen = 0;
        self.main_cmds = 0;
        self.main_tris = 0;
        self.commands.clear();
        self.tri_verts.clear();
        self.pending_tris.clear();
        self.pending_tex.clear();
        self.pending_surf.clear();
        self.locals = LightList::EMPTY;
        self.wetness = 0.0;
        self.ao = None;
        self.post = None;
        self.light_sets.clear();
        self.water_state = WaterState::NONE;
        self.water_sets.clear();
        self.shadows.clear();
        self.shadow_game = None;
        self.pending_shadow.clear();
        self.map_casters.clear();
        self.apply_shadow_view();
        self.frame_stats = FrameStats::default();
        self.n64 = N64Split::default();
        self.pc_gpu_off = [0; PcGpuReject::COUNT];
        self.times = PcFrameTimes::default();
        self.cpu_jobs.clear();
        self.job_ctxs.clear();
        if let Some(gpu) = &mut self.gpu {
            gpu.begin_frame();
        }
        trino_core::trace::frame_mark(self.flash_frames as u32);
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        if let Some(r) = self.recording() {
            r.cmds.push(trino_core::stereo::StereoCmd::Sprite {
                sprite,
                pos,
                params: *params,
            });
            return;
        }
        let pos = Vec2::new(pos.x + self.stereo.shift_2d(), pos.y);
        self.flush_model_batch();
        let Some(entry) = self.sprites.get(&sprite.0) else {
            // Unknown handle or not loaded (its group is not ready): skip,
            // counted. The bake rejects handles no manifest declares.
            self.frame_stats.not_loaded += 1;
            return;
        };
        let cmd = make_command(sprite.0, pos, entry.size, params);
        // Texture pressure at the profile's texel size: what a console
        // must stream into texture memory for this draw (the region only —
        // sub-rect blits load just that on N64/3DS).
        let bpp = if self.caps.color_depth_bits <= 16 {
            2
        } else {
            4
        };
        let (tw, th) = (entry.size.0 as f32, entry.size.1 as f32);
        let (rw, rh) = (
            ((cmd.uv1[0] - cmd.uv0[0]).abs() * tw).round() as u32,
            ((cmd.uv1[1] - cmd.uv0[1]).abs() * th).round() as u32,
        );
        let texels = rw * rh;
        if self.strict {
            // Console formats are 16-bit (RGBA5551) or smaller; indexed
            // formats (CI4/CI8) are validated at bake time where the real
            // format is known — this is the worst-case direct-color check
            // of what one draw loads into texture memory.
            if let Err(e) = self.caps.validate_texture(
                rw.min(u16::MAX as u32) as u16,
                rh.min(u16::MAX as u32) as u16,
                bpp,
            ) {
                panic!(
                    "strict mode: sprite {sprite:?} drawn at {rw}x{rh} busts the {:?} \
                     profile budget: {e:?}. Shrink the sprite, draw it in regions (atlas), \
                     or pick an indexed format (CI4/CI8) in assets/manifest.toml.",
                    self.profile
                );
            }
        }
        self.frame_stats.sprites += 1;
        self.frame_stats.max_texture_bytes = self.frame_stats.max_texture_bytes.max(texels * bpp);
        self.commands.push(Cmd::Sprite(cmd));
    }

    fn fill_rect(&mut self, rect: trino_core::Rect, color: Color) {
        if let Some(r) = self.recording() {
            r.cmds
                .push(trino_core::stereo::StereoCmd::Rect { rect, color });
            return;
        }
        let rect = trino_core::Rect {
            pos: Vec2::new(rect.pos.x + self.stereo.shift_2d(), rect.pos.y),
            size: rect.size,
        };
        // A built-in 1×1 white texel through the sprite path.
        if !self.sprites.contains_key(&FILL_SPRITE.0) {
            self.upload_sprite(FILL_SPRITE, 1, 1, &[255, 255, 255, 255]);
        }
        self.flush_model_batch();
        let params = SpriteParams {
            scale: Vec2::new(rect.size.x, rect.size.y),
            tint: color,
            ..SpriteParams::DEFAULT
        };
        if self.screen == 0 {
            let (w, h) = (self.internal_size.0 as f32, self.internal_size.1 as f32);
            let cw = (rect.pos.x + rect.size.x).min(w) - rect.pos.x.max(0.0);
            let ch = (rect.pos.y + rect.size.y).min(h) - rect.pos.y.max(0.0);
            let coverage = (cw.max(0.0) * ch.max(0.0)) / (w * h).max(1.0);
            if coverage >= 0.25 {
                self.flash_level = self
                    .flash_level
                    .max(trino_core::FlashMonitor::level_of(color, coverage));
            }
        }
        let mut cmd = make_command(FILL_SPRITE.0, rect.pos, (1, 1), &params);
        // The shader's flat-color sentinel (fs_sprite).
        cmd.uv0 = [-2.0, -2.0];
        cmd.uv1 = [-2.0, -2.0];
        self.frame_stats.sprites += 1;
        self.commands.push(Cmd::Sprite(cmd));
    }

    fn set_camera(&mut self, camera: &Camera3) {
        if let Some(r) = self.recording_state() {
            r.cmds.push(trino_core::stereo::StereoCmd::Camera(*camera));
            return;
        }
        self.flush_model_batch();
        self.camera = *camera;
    }

    fn set_light(&mut self, light: &Light) {
        if let Some(r) = self.recording_state() {
            r.cmds.push(trino_core::stereo::StereoCmd::Light(*light));
            return;
        }
        // Lighting is resolved into vertex colors at tessellation time, so
        // a change needs no batch flush.
        self.light = *light;
    }

    fn set_local_lights(&mut self, lights: &[LocalLight]) {
        if let Some(r) = self.recording_state() {
            let s = r.lights(lights);
            r.cmds.push(trino_core::stereo::StereoCmd::LocalLights(s));
            return;
        }
        // Per vertex they are resolved at tessellation; per pixel each lit
        // vertex names its light set. No flush either way.
        self.locals.set(lights, self.caps.max_local_lights);
    }

    fn set_texture_filter(&mut self, filter: TexFilter) {
        if let Some(r) = self.recording_state() {
            r.cmds.push(trino_core::stereo::StereoCmd::Filter(filter));
            return;
        }
        self.game_filter = filter;
        self.tex_filter = self.resolve_filter(filter);
    }

    fn lod_bias(&self) -> f32 {
        self.quality.lod_bias
    }

    fn set_post(&mut self, post: Option<&trino_core::PostSettings>) {
        // Like the AO, never recorded for stereo: only the Full tier (no
        // stereo screen) reads it.
        self.post = post.map(|p| p.clamped());
    }

    fn set_ambient_occlusion(&mut self, ao: Option<&AmbientOcclusion>) {
        self.ao = ao.copied();
    }

    fn set_wetness(&mut self, wetness: f32) {
        if let Some(r) = self.recording_state() {
            r.cmds.push(trino_core::stereo::StereoCmd::Wetness(wetness));
            return;
        }
        self.wetness = wetness.clamp(0.0, 1.0);
    }

    fn set_shadows(&mut self, shadows: Option<&ShadowSettings>) {
        if let Some(r) = self.recording_state() {
            r.cmds
                .push(trino_core::stereo::StereoCmd::Shadows(shadows.copied()));
            return;
        }
        self.shadow_game = shadows.copied();
        self.apply_shadow_view();
    }

    fn set_shadow_receivers(&mut self, receivers: &[ShadowPlane]) {
        if let Some(r) = self.recording_state() {
            let s = r.planes(receivers);
            r.cmds.push(trino_core::stereo::StereoCmd::Receivers(s));
            return;
        }
        self.shadows.set_receivers(receivers);
    }

    fn set_water(&mut self, time: f32, ripples: &[Ripple]) {
        if let Some(r) = self.recording_state() {
            let s = r.ripples(ripples);
            r.cmds
                .push(trino_core::stereo::StereoCmd::Water { time, ripples: s });
            return;
        }
        let cap = self.caps.max_ripples;
        if self.strict {
            let live = ripples.iter().filter(|r| r.alive()).count();
            if live > cap as usize {
                panic!(
                    "strict mode: {live} live ripples this frame, over the {:?} profile's \
                     {cap} (the 3DS water vertex program holds {} in its uniforms; every \
                     target evaluates the first {cap}). Keep a RippleBuffer<{}>.",
                    self.profile,
                    trino_core::MAX_RIPPLES,
                    trino_core::MAX_RIPPLES
                );
            }
        }
        self.water_state = WaterState::new(time, ripples, cap);
    }

    fn draw_model(
        &mut self,
        model: ModelId,
        transform: &Transform3,
        material: Material,
        params: &ModelParams,
    ) {
        self.draw_model_matrix(model, &transform.matrix(), material, params);
    }

    fn draw_model_matrix(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        material: Material,
        params: &ModelParams,
    ) {
        if let Some(r) = self.recording() {
            r.cmds.push(trino_core::stereo::StereoCmd::Model {
                model,
                matrix: *matrix,
                material,
                params: *params,
            });
            return;
        }
        trino_core::zone!("draw");
        if self.gpu.is_none() && self.gpu_tnl_active() {
            self.warm_wait(precompile::JobKind::GpuTnl);
        }
        if params.shadow.casts() {
            self.cast_shadow(model, matrix, params);
        }
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            // Unknown or not loaded (its group is not ready): skip, counted.
            self.frame_stats.not_loaded += 1;
            return;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        self.frame_stats.models += 1;
        let texture = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0));
        if let Some(t) = texture {
            texture_budget(
                TextureBudget {
                    sprites: &self.sprites,
                    caps: &self.caps,
                    strict: self.strict,
                    profile: self.profile,
                    stats: &mut self.frame_stats,
                },
                t,
                true,
            );
        }
        if let (Some(t), Some(d), true) = (texture, params.detail, self.strict) {
            // Base and detail share texture memory (the N64's TMEM), at
            // worst 16-bit texels; the console drops a detail that doesn't
            // fit, strict mode says so.
            let bytes = |id: SpriteId| self.sprites.get(&id.0).map_or(0, |e| e.size.0 * e.size.1);
            let bpp = if self.caps.color_depth_bits <= 16 {
                2
            } else {
                4
            };
            let total = (bytes(t) + bytes(d.sprite)) * bpp;
            if total > self.caps.texture_memory_bytes {
                panic!(
                    "strict mode: texture {t:?} with detail {:?} needs {total} bytes of \
                     texture memory together, over the {:?} profile's {} (N64 TMEM: \
                     base + detail share 4 KB; use a small I4 detail and a non-palette \
                     base).",
                    d.sprite, self.profile, self.caps.texture_memory_bytes
                );
            }
        }
        let screen = self.current_screen_size();
        // The native profile defers its CPU T&L to the batch flush (worker
        // threads, `cpu_tnl`) and sends what `pc_gpu_check` accepts to the
        // GPU; the console profiles tessellate here, as the consoles do.
        let native = self.profile.is_native();
        let texkey = texture.map(|t| TexKey::model(t.0, params, self.tex_filter));
        if let Some(w) = params.water {
            // Water (ADR 0015): displaced and shaded by the core's water
            // T&L; the per-pixel tier also takes each vertex's surface.
            let before = self.pending_tris.len();
            let eye = self.eye_view();
            let ctx = tnl_context(
                &mut self.tnl_ctx,
                &self.camera,
                &self.light,
                screen,
                &[],
                eye,
            )
            .with_water(&self.water_state);
            let set = self.water_sets.index(water::SetKey {
                water: w,
                state: self.water_state,
                gloss: params.gloss,
                detail_scale: match (texture, params.detail) {
                    (Some(_), Some(d)) if self.sprites.contains_key(&d.sprite.0) => d.scale(),
                    _ => 0.0,
                },
                camera: self.camera,
                light: self.light,
            });
            let gloss = params.gloss as f32 / 255.0;
            let per_pixel = self.caps.water_tier == WaterTier::PerPixel;
            if native {
                self.pc_gpu_off[PcGpuReject::Water.index()] += 1;
                let job_ctx = push_ctx(&mut self.job_ctxs, ctx);
                self.cpu_jobs.push(cpu_tnl::CpuJob {
                    kind: cpu_tnl::JobKind::Water {
                        set,
                        gloss,
                        per_pixel,
                    },
                    model: model.0,
                    matrix: *matrix,
                    params: *params,
                    ctx: job_ctx,
                    texture: texkey,
                    tris: (mesh.index_count / 3) as u32,
                });
                return;
            }
            let t0 = Instant::now();
            self.pending_surf.resize(before, None);
            let (pending, surf) = (&mut self.pending_tris, &mut self.pending_surf);
            let tnl = if per_pixel {
                trino_core::render3d::tessellate_surface(
                    &ctx,
                    &mut self.surf_scratch,
                    &mesh,
                    matrix,
                    params,
                    &mut |tri, v| {
                        pending.push(tri);
                        surf.push(Some(SurfTri {
                            v: *v,
                            gloss,
                            set,
                            mask: 0,
                            water: true,
                        }));
                    },
                )
            } else {
                trino_core::render3d::tessellate_with(
                    &ctx,
                    &mut self.tnl_scratch,
                    &mesh,
                    matrix,
                    params,
                    &mut |tri| {
                        pending.push(tri);
                        surf.push(Some(SurfTri {
                            v: [NO_SURFACE; 3],
                            gloss,
                            set,
                            mask: 0,
                            water: true,
                        }));
                    },
                )
            };
            self.times.tnl_us += micros(t0);
            self.frame_stats.tnl.add(&tnl);
            let added = self.pending_tris.len() - before;
            self.tag_pending(before, added, texkey);
            return;
        }
        let before = self.pending_tris.len();
        let per_pixel = self.caps.light_tier == LightTier::PerPixel;
        let eye = self.eye_view();
        let ctx = tnl_context(
            &mut self.tnl_ctx,
            &self.camera,
            &self.light,
            screen,
            if per_pixel {
                &[]
            } else {
                self.locals.as_slice()
            },
            eye,
        );
        // Only the draws a local light reaches (or glossy or wet ones) go
        // through the lit pipeline, and each names the lights that reach it:
        // a frame with a few lamps shades a few draws per pixel, not all.
        let unlit = params.shading == trino_core::Shading::Unlit;
        let reach = if per_pixel && !unlit && !self.locals.is_empty() {
            lights::reach_mask(&mesh, matrix, self.locals.as_slice())
        } else {
            0
        };
        let lit = per_pixel && !unlit && (reach != 0 || self.wetness > 0.0 || params.gloss > 0);
        if native {
            let alpha = texture.is_some_and(|t| self.sprites.get(&t.0).is_some_and(|e| e.alpha));
            let decided = pc_gpu_decide(
                (&ctx, &mesh, matrix, params),
                lit,
                alpha,
                self.gpu_tnl_active(),
            );
            if decided.is_ok() {
                let detail = detail_scale(&self.sprites, texture, params);
                let inst = gpu_tnl::instance(
                    &ctx,
                    &mesh,
                    matrix,
                    params,
                    !trino_core::render3d::split_policy(&ctx, &mesh, params),
                    detail,
                );
                let _pipes = pipes::scope(&self.pipe_entry);
                let gpu = self.gpu.get_or_insert_with(|| {
                    gpu_tnl::GpuTnl::new(&self.device, [&self.globals_bgl, &self.tex3d_bgl])
                });
                if !gpu.has_mesh(model.0) {
                    gpu.upload_mesh(&self.device, &self.queue, model.0, &mesh);
                }
                if self.msaa > 1 {
                    gpu.ensure_ms(&self.device, self.msaa);
                }
                gpu.draw(
                    &ctx,
                    gpu_tnl::DrawKey {
                        model: model.0,
                        mesh: &mesh,
                        params,
                        texture: texkey,
                    },
                    &[inst],
                );
                self.frame_stats.tnl.pc_gpu_draws += 1;
                self.frame_stats.tnl.pc_gpu_tris += (mesh.index_count / 3) as u32;
                return;
            }
            if let Err(r) = decided {
                self.pc_gpu_off[r.index()] += 1;
            }
            self.times.lit_draws += lit as u32;
            let kind = if lit {
                cpu_tnl::JobKind::Lit {
                    set: self.light_sets.index(lights::SetKey::new(
                        &self.camera,
                        &self.light,
                        &self.locals,
                        self.wetness,
                    )),
                    mask: reach,
                    gloss: params.gloss as f32 / 255.0,
                }
            } else {
                cpu_tnl::JobKind::Plain
            };
            let job_ctx = push_ctx(&mut self.job_ctxs, ctx);
            self.cpu_jobs.push(cpu_tnl::CpuJob {
                kind,
                model: model.0,
                matrix: *matrix,
                params: *params,
                ctx: job_ctx,
                texture: texkey,
                tris: (mesh.index_count / 3) as u32,
            });
            return;
        }
        let t0 = Instant::now();
        self.times.lit_draws += lit as u32;
        let tnl = if lit {
            let set = self.light_sets.index(lights::SetKey::new(
                &self.camera,
                &self.light,
                &self.locals,
                self.wetness,
            ));
            let gloss = params.gloss as f32 / 255.0;
            self.pending_surf.resize(before, None);
            let (pending, surf) = (&mut self.pending_tris, &mut self.pending_surf);
            trino_core::render3d::tessellate_surface(
                &ctx,
                &mut self.surf_scratch,
                &mesh,
                matrix,
                params,
                &mut |tri, v| {
                    pending.push(tri);
                    surf.push(Some(SurfTri {
                        v: *v,
                        gloss,
                        set,
                        mask: reach,
                        water: false,
                    }));
                },
            )
        } else {
            let pending = &mut self.pending_tris;
            trino_core::render3d::tessellate_with(
                &ctx,
                &mut self.tnl_scratch,
                &mesh,
                matrix,
                params,
                &mut |tri| pending.push(tri),
            )
        };
        self.times.tnl_us += micros(t0);
        self.frame_stats.tnl.add(&tnl);
        if self.profile == SimProfile::N64 {
            // What the console does with it: its RSP overlay takes the draws
            // `rsp_eligible` accepts (the same rules, from the core); the CPU
            // transforms the rest.
            if let Ok((draws, tris)) = trino_core::render3d::rsp_plan(&ctx, &mesh, matrix, params) {
                self.n64.rsp_draws += draws;
                self.n64.rsp_tris += tris;
                // A water surface on the RSP draws the rings it can there
                // too; the others stay CPU triangles.
                if params.water.is_some() {
                    self.n64.water_draws += draws;
                    self.n64.water_tris += tris;
                    let (ring_tris, cpu) =
                        trino_core::render3d::rsp_water_rings(&ctx, &mesh, matrix, params);
                    self.n64.rsp_tris += ring_tris;
                    self.n64.cpu.add(&cpu);
                }
            } else {
                self.n64.cpu_draws += 1;
                self.n64.cpu.add(&tnl);
            }
        }
        let added = self.pending_tris.len() - before;
        self.tag_pending(before, added, texkey);
    }

    fn draw_model_instances_with(
        &mut self,
        model: ModelId,
        instances: &[Instance],
        palettes: &[Palette],
        params: &ModelParams,
    ) {
        if !self.meshes.contains_key(&model.0) {
            return;
        }
        if !self.gpu_tnl_active() || (self.screen == 0 && self.stereo.recording) {
            // The trait's loop, counted (the CPU path and the console
            // profiles).
            for i in instances {
                let p = i.params(params, palettes);
                self.draw_model_matrix(model, &i.matrix, Material::VertexLit, &p);
                self.frame_stats.tnl.instanced += 1;
            }
            return;
        }
        // The GPU path: every copy it accepts goes into one instanced draw;
        // a copy it doesn't (off screen, translucent) takes the CPU path
        // alone, as a separate draw would.
        let mut accepted: Vec<GpuInstanceOf> = Vec::with_capacity(instances.len());
        let mut rest: Vec<(Mat34, ModelParams)> = Vec::new();
        // The accepted copies' shadows (the rest cast theirs as draws).
        let mut cast: Vec<Mat34> = Vec::new();
        {
            let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
                self.frame_stats.not_loaded += instances.len() as u32;
                return;
            };
            let mesh = Mesh::from_layout(tmdl, layout);
            let screen = self.current_screen_size();
            let eye = self.eye_view();
            let ctx = tnl_context(
                &mut self.tnl_ctx,
                &self.camera,
                &self.light,
                screen,
                &[],
                eye,
            );
            let per_pixel = self.caps.light_tier == LightTier::PerPixel;
            for i in instances {
                let p = i.params(params, palettes);
                let unlit = p.shading == trino_core::Shading::Unlit;
                let reach = if per_pixel && !unlit && !self.locals.is_empty() {
                    lights::reach_mask(&mesh, &i.matrix, self.locals.as_slice())
                } else {
                    0
                };
                let lit = per_pixel && !unlit && (reach != 0 || self.wetness > 0.0 || p.gloss > 0);
                let texture = mesh
                    .texture_for(&p)
                    .filter(|t| self.sprites.contains_key(&t.0));
                let alpha =
                    texture.is_some_and(|t| self.sprites.get(&t.0).is_some_and(|e| e.alpha));
                // Copies share the call's surface but for tint and palette,
                // so one texture state and one pose for all of them.
                if p.water.is_none()
                    && pc_gpu_decide((&ctx, &mesh, &i.matrix, &p), lit, alpha, true).is_ok()
                {
                    if p.shadow.casts() {
                        cast.push(i.matrix);
                    }
                    let detail = detail_scale(&self.sprites, texture, &p);
                    let affine = !trino_core::render3d::split_policy(&ctx, &mesh, &p);
                    accepted.push((
                        gpu_tnl::instance(&ctx, &mesh, &i.matrix, &p, affine, detail),
                        texture.map(|t| TexKey::model(t.0, &p, self.tex_filter)),
                    ));
                } else {
                    rest.push((i.matrix, p));
                }
            }
        }
        for m in &cast {
            self.cast_shadow(model, m, params);
        }
        if !accepted.is_empty() {
            self.draw_gpu_instances(model, params, &accepted);
        }
        for (m, p) in rest {
            self.draw_model_matrix(model, &m, Material::VertexLit, &p);
            self.frame_stats.tnl.instanced += 1;
        }
    }

    fn draw_billboard(
        &mut self,
        sprite: SpriteId,
        pos: trino_core::Vec3,
        params: &trino_core::BillboardParams,
    ) {
        if let Some(r) = self.recording() {
            r.cmds.push(trino_core::stereo::StereoCmd::Billboard {
                sprite,
                pos,
                params: *params,
            });
            return;
        }
        let Some(entry) = self.sprites.get(&sprite.0) else {
            self.frame_stats.not_loaded += 1;
            return;
        };
        let (w, h) = (entry.size.0 as f32, entry.size.1 as f32);
        let uv = trino_core::render3d::region_uv(params.region, w, h);
        texture_budget(
            TextureBudget {
                sprites: &self.sprites,
                caps: &self.caps,
                strict: self.strict,
                profile: self.profile,
                stats: &mut self.frame_stats,
            },
            sprite,
            false,
        );
        let screen = self.current_screen_size();
        let locals = match self.caps.light_tier {
            LightTier::PerPixel => &[],
            LightTier::PerVertex => self.locals.as_slice(),
        };
        let eye = self.eye_view();
        let ctx = tnl_context(
            &mut self.tnl_ctx,
            &self.camera,
            &self.light,
            screen,
            locals,
            eye,
        );
        let Some(tris) = trino_core::render3d::billboard_with(&ctx, pos, params, uv) else {
            return;
        };
        self.frame_stats.sprites += 1;
        let texture = Some(TexKey::billboard(sprite.0, self.tex_filter));
        if self.profile.is_native() {
            // In order with the batch's deferred CPU draws.
            self.cpu_jobs.push(cpu_tnl::CpuJob {
                kind: cpu_tnl::JobKind::Ready(tris),
                model: 0,
                matrix: Mat34::IDENTITY,
                params: ModelParams::DEFAULT,
                ctx: 0,
                texture,
                tris: 2,
            });
            return;
        }
        let before = self.pending_tris.len();
        self.pending_tris.extend_from_slice(&tris);
        self.tag_pending(before, 2, texture);
    }

    fn second_screen(&self) -> Option<Vec2> {
        self.second
            .as_ref()
            .map(|sc| Vec2::new(sc.size.0 as f32, sc.size.1 as f32))
    }

    fn set_screen(&mut self, screen: u8) {
        // Main screen first, then the second (once per frame).
        if screen == 1 && self.screen == 0 && self.second.is_some() {
            if self.stereo.recording {
                self.replay_eyes();
            } else {
                self.flush_model_batch();
                let (cmds, tris) = (
                    self.commands.len() as u32,
                    (self.tri_verts.len() / 3) as u32,
                );
                self.draw_commands(false);
                (self.main_cmds, self.main_tris) = (cmds, tris);
            }
            self.screen = 1;
        }
    }

    fn screen(&self) -> Screen {
        if self.screen == 1 {
            Screen::Bottom
        } else {
            Screen::Top
        }
    }

    fn screen_size(&self, screen: Screen) -> Option<Vec2> {
        match screen {
            Screen::Top => Some(Vec2::new(
                self.internal_size.0 as f32,
                self.internal_size.1 as f32,
            )),
            Screen::Bottom => self.second_screen(),
        }
    }

    fn safe_area(&self, screen: Screen) -> trino_core::Insets {
        let own = match (screen, self.safe_area) {
            (Screen::Top, Some(insets)) => insets,
            (Screen::Top, None) => trino_core::Insets::symmetric(
                self.caps.safe_area.0 as u16,
                self.caps.safe_area.1 as u16,
            ),
            (Screen::Bottom, _) => return trino_core::Insets::ZERO,
        };
        match &self.surface {
            Some(ss) if self.window_safe.iter().any(|v| *v > 0.0) => {
                let window = picture_insets(
                    self.internal_size,
                    (ss.config.width, ss.config.height),
                    self.window_safe,
                );
                trino_core::Insets {
                    left: own.left.max(window.left),
                    top: own.top.max(window.top),
                    right: own.right.max(window.right),
                    bottom: own.bottom.max(window.bottom),
                }
            }
            _ => own,
        }
    }

    fn precompile(
        &mut self,
        request: &trino_core::PrecompileRequest,
    ) -> trino_core::PrecompileStatus {
        PcRenderer::precompile(self, request)
    }

    fn precompile_status(&self) -> trino_core::PrecompileStatus {
        PcRenderer::precompile_status(self)
    }

    fn ui_stats(&mut self, stats: trino_core::UiStats) {
        self.ui_stats = Some(stats);
        if self.strict {
            if stats.nodes > self.caps.max_ui_nodes {
                panic!(
                    "strict mode: a UI screen laid out {} nodes, over the {:?} profile's {} \
                     (Caps::max_ui_nodes) — fewer list items or nodes; `cargo xtask ui \
                     check` reports it per target",
                    stats.nodes, self.profile, self.caps.max_ui_nodes
                );
            }
            if stats.draws > self.caps.max_ui_draws {
                panic!(
                    "strict mode: a UI screen drew {} fills, sprites and glyphs this frame, \
                     over the {:?} profile's {} (Caps::max_ui_draws) — less text or fewer \
                     nodes; `cargo xtask ui check` reports it per target",
                    stats.draws, self.profile, self.caps.max_ui_draws
                );
            }
        }
    }

    fn end_frame(&mut self) {
        // A stereo frame still on the top screen: both eyes now, then only
        // the second screen is left to render.
        let top_done = self.stereo.recording;
        if top_done {
            self.replay_eyes();
        }
        self.flush_model_batch();
        self.frame_stats.shadows = self.shadows.stats;
        self.frame_stats.stereo = self.stereo.frame.stats(self.stereo.rec.cmds.len());
        self.frame_stats.tris =
            self.main_tris + (self.tri_verts.len() / 3) as u32 + self.frame_stats.tnl.pc_gpu_tris;
        self.last_stats = self.frame_stats;
        self.last_pc_gpu_off = self.pc_gpu_off;
        let now = self.flash_frames as f32 / 60.0;
        self.flash_frames += 1;
        if self
            .flash_monitor
            .frame(now, core::mem::take(&mut self.flash_level))
            && self.strict
        {
            eprintln!(
                "strict mode: warning: full-screen flashes more than {} times a second                  (a strobe, frame {}): photosensitive players are at risk. Draw flashes                  with trino_core::ScreenFlash, which keeps them to 3 a second under the                  player's flash_guard (issue #60).",
                trino_core::access::MAX_FLASHES_PER_SECOND,
                self.flash_frames
            );
        }
        if trino_core::trace::is_on() {
            let s = &self.frame_stats;
            trino_core::trace::counter("draws", s.models);
            trino_core::trace::counter("tris", s.tris);
            trino_core::trace::counter("pc_gpu_draws", s.tnl.pc_gpu_draws);
        }
        let t0 = Instant::now();
        trino_core::zone!("end");
        if top_done {
            self.screen = 1;
        }
        self.flush();
        self.times.submit_us += micros(t0);
        let built = self
            .pipe_entry
            .render_thread
            .load(std::sync::atomic::Ordering::Relaxed);
        self.times.pipelines = built.wrapping_sub(self.pipes_at_begin);
        self.pipes_at_begin = built;
        self.last_times = self.times;
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        if let Some(r) = self.recording_state() {
            let s = r.text(text);
            r.cmds.push(trino_core::stereo::StereoCmd::Text {
                font,
                pos,
                text: s,
                params: *params,
            });
            return;
        }
        let Some(entry) = self.fonts.get(&font.0).cloned() else {
            self.frame_stats.not_loaded += 1;
            return;
        };
        let parsed = entry.font();
        trino_core::text::draw(
            self,
            trino_core::text::atlas_of(font),
            &parsed,
            pos,
            text,
            params,
        );
    }

    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        self.fonts
            .get(&font.0)
            .map_or(Vec2::ZERO, |e| e.font().measure(text, scale))
    }

    fn stats(&self) -> FrameStats {
        self.last_stats
    }

    fn read_pixel(&mut self, x: u32, y: u32) -> Option<Color> {
        let (w, h) = self.internal_size;
        if x >= w || y >= h {
            return None;
        }
        // None where the backend can't wait for the GPU (a browser's
        // WebGPU): the self-test reports its probes as skipped there.
        let rgba = self.try_read_offscreen()?;
        let at = ((y * w + x) * 4) as usize;
        Some(Color::rgba(
            rgba[at],
            rgba[at + 1],
            rgba[at + 2],
            rgba[at + 3],
        ))
    }

    fn set_stereo(&mut self, settings: Option<&trino_core::StereoSettings>) {
        use trino_core::stereo::{StereoFrame, StereoMono};
        if self.screen != 0 || self.stereo.recording {
            return;
        }
        // The simulated slider is up only under a preview mode.
        let hw = match self.stereo_view {
            StereoView::Off => 0.0,
            _ => self.stereo_slider,
        };
        // The PC's frame time is not the console's: the preview never drops
        // an eye for the budget.
        let mut f = StereoFrame::resolve(settings, self.caps.stereo, hw, true);
        if f.is_stereo() && self.stereo.top_drawn {
            if self.strict {
                panic!(
                    "strict mode: set_stereo came after this frame's first top-screen \
                     draw, so the frame stays mono. Call it right after begin_frame \
                     (issue #63)."
                );
            }
            f = StereoFrame::mono(StereoMono::Late);
        }
        self.stereo.frame = f;
        if f.is_stereo() {
            self.flush_model_batch();
            self.stereo.recording = true;
            *self.stereo_start = self.top_state();
        }
    }

    fn stereo_eye(&self) -> trino_core::Eye {
        self.stereo.eye
    }

    fn set_layer_depth(&mut self, depth: trino_core::LayerDepth) {
        if let Some(r) = self.recording_state() {
            r.cmds.push(trino_core::stereo::StereoCmd::Depth(depth));
            return;
        }
        self.stereo.depth = depth;
    }

    fn read_eye_pixel(&mut self, eye: trino_core::Eye, x: u32, y: u32) -> Option<Color> {
        if eye == trino_core::Eye::Mono || self.last_stats.stereo.eyes != 2 {
            return match eye {
                trino_core::Eye::Right => None,
                _ => self.read_pixel(x, y),
            };
        }
        let (w, h) = self.internal_size;
        if x >= w || y >= h {
            return None;
        }
        let rgba = self.read_eye(eye)?;
        let at = ((y * w + x) * 4) as usize;
        Some(Color::rgba(
            rgba[at],
            rgba[at + 1],
            rgba[at + 2],
            rgba[at + 3],
        ))
    }

    fn read_screen_pixel(&mut self, screen: Screen, x: u32, y: u32) -> Option<Color> {
        let Screen::Bottom = screen else {
            return self.read_pixel(x, y);
        };
        let (w, h) = self.second.as_ref()?.size;
        if x >= w || y >= h {
            return None;
        }
        let rgba = self.try_read_texture(&self.second.as_ref()?.tex, (w, h))?;
        let at = ((y * w + x) * 4) as usize;
        Some(Color::rgba(
            rgba[at],
            rgba[at + 1],
            rgba[at + 2],
            rgba[at + 3],
        ))
    }
}

/// A framebuffer read under way ([`PcRenderer::start_read_offscreen`]).
impl Drop for PcRenderer {
    fn drop(&mut self) {
        // Workers first: they hold the device and the entry.
        self.warm = precompile::Warmup::default();
    }
}

pub struct PendingRead {
    buffer: wgpu::Buffer,
    padded: u32,
    size: (u32, u32),
    /// Set by the map callback: the map's outcome.
    done: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
}

impl PendingRead {
    /// The framebuffer size being read.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// Tightly packed rows from a readback buffer whose rows are `padded`
/// bytes apart.
fn unpad_rows(data: &[u8], size: (u32, u32), padded: u32) -> Vec<u8> {
    let (w, h) = size;
    let unpadded = (w * 4) as usize;
    let mut out = Vec::with_capacity(unpadded * h as usize);
    for row in 0..h as usize {
        let start = row * padded as usize;
        out.extend_from_slice(&data[start..start + unpadded]);
    }
    out
}

/// The texel `fill_rect` stretches (a handle no asset path hashes to in
/// practice: fnv1a of a reserved name).
const FILL_SPRITE: SpriteId = SpriteId::from_path("\0trino/fill");

/// A caster of the Map tier: what the light pass draws of it.
struct MapCaster {
    model: u32,
    matrix: Mat34,
    anim: Option<trino_core::AnimRef>,
    /// Its bounding sphere, world.
    sphere: (trino_core::Vec3, f32),
}

/// A blank triangle (scratch arrays).
const EMPTY_TRI: ScreenTri = ScreenTri {
    pts: [Vec2::ZERO; 3],
    colors: [Color::BLACK; 3],
    depth: 0.0,
    z: [0.0; 3],
    uv: [Vec2::ZERO; 3],
    w: [1.0; 3],
};

/// Where the internal framebuffer lands in the window: nearest integer
/// upscale, centered (fractional only when the window is smaller than the
/// framebuffer). `(x, y, width, height)` in window pixels.
fn present_rect(internal: (u32, u32), surface: (u32, u32)) -> (f32, f32, f32, f32) {
    let (iw, ih) = (internal.0 as f32, internal.1 as f32);
    let (sw, sh) = (surface.0 as f32, surface.1 as f32);
    let fit = (sw / iw).min(sh / ih);
    let scale = if fit >= 1.0 { fit.floor() } else { fit };
    let (vw, vh) = (iw * scale, ih * scale);
    ((sw - vw) * 0.5, (sh - vh) * 0.5, vw, vh)
}

/// The window's unsafe borders (`left, top, right, bottom`, window
/// pixels) as picture pixels: what the letterbox's bars leave of each,
/// over the upscale, rounded up so nothing lands under a notch.
fn picture_insets(
    internal: (u32, u32),
    surface: (u32, u32),
    window: [f32; 4],
) -> trino_core::Insets {
    let (x, y, w, _) = present_rect(internal, surface);
    let k = (w / internal.0.max(1) as f32).max(1e-3);
    let inside = |inset: f32, bar: f32| ((inset - bar).max(0.0) / k).ceil() as u16;
    trino_core::Insets {
        left: inside(window[0], x),
        top: inside(window[1], y),
        right: inside(window[2], x),
        bottom: inside(window[3], y),
    }
}

/// A picture's place in the window: `(x, y, width, height)`, pixels.
type Viewport = (f32, f32, f32, f32);

/// Where the main picture and (with one) the second screen land in the
/// window: one whole-number scale for both, the second screen centered
/// under the main one (the 3DS's two screens), the pair centered.
fn present_layout(
    main: (u32, u32),
    second: Option<(u32, u32)>,
    surface: (u32, u32),
) -> (Viewport, Option<Viewport>) {
    let Some(sec) = second else {
        return (present_rect(main, surface), None);
    };
    let (tw, th) = (main.0.max(sec.0), main.1 + sec.1);
    let (x, y, w, _) = present_rect((tw, th), surface);
    let k = w / tw as f32;
    let top = (
        x + (tw - main.0) as f32 * 0.5 * k,
        y,
        main.0 as f32 * k,
        main.1 as f32 * k,
    );
    let below = (
        x + (tw - sec.0) as f32 * 0.5 * k,
        y + main.1 as f32 * k,
        sec.0 as f32 * k,
        sec.1 as f32 * k,
    );
    (top, Some(below))
}

/// A window pixel on the presented picture(s): the screen it falls on and
/// the position in that screen's pixels.
fn window_to_screen(
    main: (u32, u32),
    second: Option<(u32, u32)>,
    surface: (u32, u32),
    x: f32,
    y: f32,
) -> Option<(trino_core::Vec2, u8)> {
    let (top, below) = present_layout(main, second, surface);
    let hit = |r: (f32, f32, f32, f32), size: (u32, u32)| {
        let (u, v) = ((x - r.0) / r.2, (y - r.1) / r.3);
        ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v))
            .then(|| trino_core::Vec2::new(u * size.0 as f32, v * size.1 as f32))
    };
    if let Some(p) = hit(top, main) {
        return Some((p, 0));
    }
    let (b, s) = (below?, second?);
    hit(b, s).map(|p| (p, 1))
}

/// Inverse of [`present_rect`] for one window pixel.
#[cfg(test)]
fn window_to_game(
    internal: (u32, u32),
    surface: (u32, u32),
    x: f32,
    y: f32,
) -> Option<trino_core::Vec2> {
    let (vx, vy, vw, vh) = present_rect(internal, surface);
    let (u, v) = ((x - vx) / vw, (y - vy) / vh);
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    Some(trino_core::Vec2::new(
        u * internal.0 as f32,
        v * internal.1 as f32,
    ))
}

#[cfg(test)]
mod headless_tests {
    /// Headless renderers name nothing for the driver (GitHub #94) unless
    /// the run asks with `WGPU_DEBUG`, and keep the build's other flags.
    #[test]
    fn headless_instances_give_the_driver_no_debug_names() {
        let flags = super::headless_flags();
        let asked = std::env::var("WGPU_DEBUG").is_ok_and(|v| v != "0");
        assert_eq!(flags.contains(wgpu::InstanceFlags::DEBUG), asked);
        let rest = wgpu::InstanceFlags::from_build_config() - wgpu::InstanceFlags::DEBUG;
        if std::env::vars().all(|(k, _)| !k.starts_with("WGPU_")) {
            assert_eq!(flags, rest);
        }
    }
}

#[cfg(test)]
mod present_tests {
    use super::*;

    /// Native pictures on real monitors: a whole-number upscale, the same
    /// in x and y (never stretched), bars only where the aspect is out of
    /// 4:3..21:9, and the pointer mapping back to the picture's corners.
    #[test]
    fn native_pictures_are_never_stretched() {
        for out in [
            (1280, 720),
            (1366, 768),
            (1920, 1080),
            (2560, 1080),
            (3440, 1440),
            (3840, 1080),
            (5120, 1440),
            (1280, 1024),
            (3840, 2160),
            (1080, 1920),
        ] {
            for scale in 0..=4 {
                let (w, h, k) = crate::sim::native_internal(out, scale);
                let (x, y, vw, vh) = present_rect((w, h), out);
                assert_eq!(
                    (vw, vh),
                    ((w * k) as f32, (h * k) as f32),
                    "{out:?} {scale}"
                );
                assert!(
                    x >= 0.0 && y >= 0.0,
                    "{out:?}: picture larger than the output"
                );
                let aspect = out.0 as f32 / out.1 as f32;
                if (crate::sim::MIN_ASPECT..=crate::sim::MAX_ASPECT).contains(&aspect) {
                    // In range: at most a few pixels of rounding per side.
                    assert!(
                        x <= k as f32 * 2.0 && y <= k as f32 * 2.0,
                        "{out:?} {scale}: {x},{y}"
                    );
                }
                let p = window_to_game((w, h), out, x + vw - 0.5, y + vh - 0.5).unwrap();
                assert!(p.x < w as f32 && p.y < h as f32 && p.x > w as f32 - 1.0);
                assert_eq!(window_to_game((w, h), out, x - 1.0, y), None);
            }
        }
        // 32:9: a 21:9 picture with side bars.
        let (w, h, k) = crate::sim::native_internal((3840, 1080), 0);
        let (x, _, _, _) = present_rect((w, h), (3840, 1080));
        assert!(x > 400.0, "side bars on 32:9 ({w}x{h} x{k}, bar {x})");
    }

    /// The 3DS profile: top 400×240 over bottom 320×240, one scale, the
    /// bottom centered; the pointer knows which screen it is on.
    #[test]
    fn two_screens_stack_and_map_the_pointer() {
        let (top, below) = present_layout((400, 240), Some((320, 240)), (1280, 960));
        // 400×480 into 1280×960: scale 2.
        assert_eq!(top, (240.0, 0.0, 800.0, 480.0));
        assert_eq!(below, Some((320.0, 480.0, 640.0, 480.0)));
        let s = Some((320, 240));
        let (p, screen) = window_to_screen((400, 240), s, (1280, 960), 250.0, 10.0).unwrap();
        assert_eq!((p.x, p.y, screen), (5.0, 5.0, 0));
        let (p, screen) = window_to_screen((400, 240), s, (1280, 960), 330.0, 500.0).unwrap();
        assert_eq!((p.x, p.y, screen), (5.0, 10.0, 1));
        // Beside the narrower bottom screen: nothing.
        assert_eq!(
            window_to_screen((400, 240), s, (1280, 960), 250.0, 600.0),
            None
        );
    }

    #[test]
    fn window_insets_become_picture_insets_past_the_letterbox() {
        // A phone in landscape, 2532×1170 device pixels, a 141 px notch on
        // the left and a 63 px home bar at the bottom; a 320×240 picture
        // at ×4 (1280×960) leaves bars of 626 px left and right and 105 px
        // top and bottom: they already cover both.
        let i = picture_insets((320, 240), (2532, 1170), [141.0, 0.0, 141.0, 63.0]);
        assert_eq!(i, trino_core::Insets::ZERO);
        // A picture of half the device pixels fills the window at ×2: the
        // notch and the home bar land inside it, halved and rounded up.
        let i = picture_insets((1266, 585), (2532, 1170), [141.0, 0.0, 141.0, 63.0]);
        assert_eq!((i.left, i.top, i.right, i.bottom), (71, 0, 71, 32));
        // Partly covered by the bar: only the rest counts.
        let i = picture_insets((640, 480), (1400, 960), [100.0, 0.0, 0.0, 0.0]);
        assert_eq!(i.left, 20);
    }

    #[test]
    fn letterboxed_integer_upscale_maps_back() {
        // 320x240 in a 1280x720 window: scale 3, 960x720 centered at x=160.
        assert_eq!(
            present_rect((320, 240), (1280, 720)),
            (160.0, 0.0, 960.0, 720.0)
        );
        let p = window_to_game((320, 240), (1280, 720), 160.0 + 30.0, 9.0).unwrap();
        assert_eq!((p.x, p.y), (10.0, 3.0));
        // The black bars are outside the picture.
        assert_eq!(window_to_game((320, 240), (1280, 720), 100.0, 10.0), None);
        assert_eq!(window_to_game((320, 240), (1280, 720), 1125.0, 10.0), None);
        // Smaller window: fractional downscale still maps corners.
        let p = window_to_game((640, 480), (320, 240), 319.0, 239.0).unwrap();
        assert!((p.x - 638.0).abs() < 0.01 && (p.y - 478.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod tnl_setting_tests {
    #[test]
    fn trino_pc_tnl_cpu_turns_the_gpu_path_off() {
        for v in [None, Some("gpu"), Some("auto"), Some("1"), Some("")] {
            assert!(super::pc_tnl_setting(v), "{v:?}");
        }
        for v in ["cpu", "0", "off"] {
            assert!(!super::pc_tnl_setting(Some(v)), "{v}");
        }
    }
}

#[cfg(test)]
mod webgl2_tests {
    use super::*;

    /// Every entry point of `source` through naga's GLSL ES 3.00 backend,
    /// the one wgpu's GL backend runs in a browser without WebGPU. `Err`
    /// names the first entry point that doesn't translate.
    fn glsl_es_300(source: &str) -> Result<usize, String> {
        use naga::back::glsl;
        let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .map_err(|e| format!("{e:?}"))?;
        let options = glsl::Options {
            version: glsl::Version::Embedded {
                version: 300,
                is_webgl: true,
            },
            ..Default::default()
        };
        for ep in &module.entry_points {
            let pipeline = glsl::PipelineOptions {
                shader_stage: ep.stage,
                entry_point: ep.name.clone(),
                multiview: None,
            };
            let mut out = String::new();
            glsl::Writer::new(
                &mut out,
                &module,
                &info,
                &options,
                &pipeline,
                naga::proc::BoundsCheckPolicies::default(),
            )
            .and_then(|mut w| w.write().map(|_| ()))
            .map_err(|e| format!("{}: {e}", ep.name))?;
        }
        Ok(module.entry_points.len())
    }

    /// The scene, AO and GPU T&L shaders as the WebGL2 renderer builds them
    /// (`depth_as_float`) translate to GLSL ES 3.00, entry point by entry
    /// point — and the scene shader as written would not: its depth read is
    /// why the transform exists (the browser's first run failed there).
    #[test]
    fn webgl2_shaders_translate_to_glsl_es_300() {
        for (name, source) in [
            ("shaders.wgsl", include_str!("shaders.wgsl")),
            ("ao.wgsl", include_str!("ao.wgsl")),
            ("shaders_tnl.wgsl", include_str!("shaders_tnl.wgsl")),
            ("depth_blit", depth_blit::SHADER),
            ("upscale.wgsl", upscale::SHADER),
            (
                "cvd (deuteranopia)",
                &cvd::shader(trino_core::CvdMode::Deuteranopia),
            ),
            ("stereo compose", stereo::SHADER),
            ("post.wgsl", post::SHADER),
            (
                "cvd filter (tritanopia)",
                &cvd::shader_for(trino_core::CvdMode::Tritanopia, CvdKind::Correct),
            ),
        ] {
            let n = glsl_es_300(&depth_as_float(source)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(n > 0, "{name}");
        }
        let raw = glsl_es_300(include_str!("shaders.wgsl")).unwrap_err();
        assert!(raw.contains("fs_water"), "{raw}");
        // The shadow map's composite as `MapPass::new` builds it for
        // WebGL2: the scene depth and the blocker search's map read
        // through nearest samplers (#86).
        let map = |s: &str| s.replace("SHADOWMAP", "texture_depth_2d");
        let wgsl = include_str!("shadow_map.wgsl");
        let webgl2 = map(&shadow_map::map_read_as_float(&depth_as_float(wgsl)));
        let n = glsl_es_300(&webgl2).unwrap_or_else(|e| panic!("shadow_map.wgsl: {e}"));
        assert!(n > 0);
        // ...and as written it would not: naga has no GLSL for a depth
        // texture's `textureLoad`, which is why the map read is rewritten.
        let raw = glsl_es_300(&map(&depth_as_float(wgsl))).unwrap_err();
        assert!(raw.contains("textureLoad"), "{raw}");
    }

    /// The transform adds one nearest sampler per depth texture, at the
    /// binding the bind groups give it, and reads the red channel.
    #[test]
    fn depth_reads_go_through_the_nearest_sampler() {
        let water = depth_as_float(include_str!("shaders.wgsl"));
        assert!(!water.contains("texture_depth_2d"));
        let (water_slot, ao_slot) = NEAREST_DEPTH_BINDING;
        assert!(
            water.contains(&format!(
                "@group(3) @binding({water_slot}) var s_t_scene_depth: sampler;"
            )),
            "the water's sampler"
        );
        assert!(water.contains("textureSampleLevel(t_scene_depth, s_t_scene_depth"));
        let ao = depth_as_float(include_str!("ao.wgsl"));
        assert!(ao.contains(&format!(
            "@group(0) @binding({ao_slot}) var s_t_depth: sampler;"
        )));
        // A shader without a depth read is left as it was.
        let tnl = include_str!("shaders_tnl.wgsl");
        assert_eq!(
            depth_as_float(tnl),
            tnl.replace("texture_depth_2d", "texture_2d<f32>")
        );
        assert_eq!(
            depth_sample_type(true),
            wgpu::TextureSampleType::Float { filterable: false }
        );
        assert_eq!(depth_sample_type(false), wgpu::TextureSampleType::Depth);
    }

    #[test]
    fn web_backends_parse_from_the_page_query() {
        assert_eq!(WebBackend::parse("auto"), Some(WebBackend::Auto));
        assert_eq!(WebBackend::parse("WebGPU"), Some(WebBackend::WebGpu));
        assert_eq!(WebBackend::parse("webgl"), Some(WebBackend::WebGl2));
        assert_eq!(WebBackend::parse("vulkan"), None);
        // ETC2 and ASTC uploads for phones, by their device features.
        assert_eq!(CompressedFormat::Etc2Rgb8.block_bytes(), 8);
        assert_eq!(CompressedFormat::Astc4x4.block_bytes(), 16);
        assert_eq!(
            CompressedFormat::Astc4x4.feature(),
            wgpu::Features::TEXTURE_COMPRESSION_ASTC
        );
        assert!(CompressedFormat::Bc7.is_bc() && !CompressedFormat::Etc2Rgba8.is_bc());
    }
}
