//! Shared test doubles for the core integration tests: a recording
//! renderer that runs the real software T&L, a voice-tracking audio
//! backend, a framed memory save medium (the console framing), and
//! hand-built TMDL / TFNT / TSCN blobs that follow the documented layouts.
#![allow(dead_code)]

use std::collections::HashMap;

use trino_core::render3d::{self, Camera3, DEFAULT_LIGHT, Light, Mesh, ScreenTri};
use trino_core::save::{self, HEADER};
use trino_core::text::{self, BitmapFont, Glyph};
use trino_core::{
    Audio, BillboardParams, BlobId, Caps, Color, FontId, FrameStats, Mat34, Material, ModelId,
    ModelParams, MusicId, Platform, Rect, Renderer, SaveError, Screen, SoundId, SoundParams,
    SpriteId, SpriteParams, TextParams, Transform3, Vec2, Vec3, VoiceId,
};

pub const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

// ------------------------------------------------------------ renderer ----

/// One call the recording renderer saw.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Begin(Color),
    Sprite {
        id: SpriteId,
        pos: Vec2,
        params: SpriteParams,
    },
    Fill {
        rect: Rect,
        color: Color,
    },
    Model {
        id: ModelId,
        matrix: Mat34,
        params: ModelParams,
        /// Screen triangles the T&L emitted for this draw.
        emitted: usize,
    },
    Billboard {
        id: SpriteId,
        pos: Vec3,
        params: BillboardParams,
        /// The quad survived `billboard_with` (in front, on screen).
        visible: bool,
    },
    Screen(u8),
    End,
}

/// A renderer that records every call and runs the engine's own T&L for
/// models (registered TMDL blobs) and billboards, so counts are real.
pub struct Recorder {
    pub caps: Caps,
    pub camera: Camera3,
    pub light: Light,
    pub calls: Vec<Call>,
    /// Every screen triangle of the current frame (models + billboards).
    pub tris: Vec<ScreenTri>,
    meshes: HashMap<u32, Vec<u8>>,
    fonts: HashMap<u32, Vec<u8>>,
    frame: FrameStats,
    last: FrameStats,
    screen_index: u8,
    /// What [`Renderer::lod_bias`] reports (a quality preset's, #55).
    pub lod_bias: f32,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub fn new() -> Self {
        Recorder {
            caps: Caps::N64,
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
            calls: Vec::new(),
            tris: Vec::new(),
            meshes: HashMap::new(),
            fonts: HashMap::new(),
            frame: FrameStats::default(),
            last: FrameStats::default(),
            screen_index: 0,
            lod_bias: 1.0,
        }
    }

    pub fn add_mesh(&mut self, id: ModelId, tmdl: Vec<u8>) {
        Mesh::from_tmdl(&tmdl).expect("valid TMDL");
        self.meshes.insert(id.0, tmdl);
    }

    pub fn add_font(&mut self, id: FontId, tfnt: Vec<u8>) {
        BitmapFont::parse(&tfnt).expect("valid TFNT");
        self.fonts.insert(id.0, tfnt);
    }

    pub fn sprites(&self) -> Vec<(SpriteId, Vec2, SpriteParams)> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Sprite { id, pos, params } => Some((*id, *pos, *params)),
                _ => None,
            })
            .collect()
    }

    pub fn models(&self) -> Vec<(ModelId, Mat34, ModelParams, usize)> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Model {
                    id,
                    matrix,
                    params,
                    emitted,
                } => Some((*id, *matrix, *params, *emitted)),
                _ => None,
            })
            .collect()
    }

    pub fn billboards(&self) -> Vec<(SpriteId, Vec3, BillboardParams, bool)> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Billboard {
                    id,
                    pos,
                    params,
                    visible,
                } => Some((*id, *pos, *params, *visible)),
                _ => None,
            })
            .collect()
    }
}

impl Renderer for Recorder {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn lod_bias(&self) -> f32 {
        self.lod_bias
    }

    fn begin_frame(&mut self, clear: Color) {
        self.calls.clear();
        self.tris.clear();
        self.frame = FrameStats::default();
        self.screen_index = 0;
        self.calls.push(Call::Begin(clear));
    }

    fn draw_sprite(&mut self, id: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.frame.sprites += 1;
        self.calls.push(Call::Sprite {
            id,
            pos,
            params: *params,
        });
    }

    fn set_camera(&mut self, camera: &Camera3) {
        self.camera = *camera;
    }

    fn set_light(&mut self, light: &Light) {
        self.light = *light;
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
        _material: Material,
        params: &ModelParams,
    ) {
        let mut emitted = 0;
        if let Some(blob) = self.meshes.get(&model.0) {
            let mesh = Mesh::from_tmdl(blob).expect("validated on add");
            let tris = &mut self.tris;
            let stats = render3d::tessellate(
                &mesh,
                matrix,
                &self.camera,
                &self.light,
                params,
                SCREEN,
                &mut |t| {
                    tris.push(t);
                    emitted += 1;
                },
            );
            self.frame.models += 1;
            self.frame.tnl.add(&stats);
        }
        self.calls.push(Call::Model {
            id: model,
            matrix: *matrix,
            params: *params,
            emitted,
        });
    }

    fn end_frame(&mut self) {
        self.frame.tris = self.tris.len() as u32;
        self.last = self.frame;
        self.calls.push(Call::End);
    }

    fn draw_billboard(&mut self, sprite: SpriteId, pos: Vec3, params: &BillboardParams) {
        let uv = render3d::region_uv(params.region, 64.0, 16.0);
        let quad = render3d::billboard(&self.camera, &self.light, SCREEN, pos, params, uv);
        if let Some(q) = quad {
            self.tris.extend_from_slice(&q);
            self.frame.sprites += 1;
        }
        self.calls.push(Call::Billboard {
            id: sprite,
            pos,
            params: *params,
            visible: quad.is_some(),
        });
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.frame.sprites += 1;
        self.calls.push(Call::Fill { rect, color });
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, s: &str, params: &TextParams) {
        let Some(table) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        let parsed = BitmapFont::parse(&table).expect("validated on add");
        text::draw(self, text::atlas_of(font), &parsed, pos, s, params);
    }

    fn measure_text(&self, font: FontId, s: &str, scale: f32) -> Vec2 {
        self.fonts
            .get(&font.0)
            .and_then(|t| BitmapFont::parse(t).ok())
            .map_or(Vec2::ZERO, |f| f.measure(s, scale))
    }

    fn stats(&self) -> FrameStats {
        self.last
    }

    fn second_screen(&self) -> Option<Vec2> {
        Some(Vec2::new(320.0, 240.0))
    }

    fn set_screen(&mut self, screen: u8) {
        // The backends' order rule: the top screen, then the bottom.
        if screen == 1 && self.screen_index == 0 {
            self.screen_index = screen;
        }
        self.calls.push(Call::Screen(screen));
    }

    fn screen(&self) -> Screen {
        Screen::from_index(self.screen_index).unwrap_or_default()
    }
}

// --------------------------------------------------------------- audio ----

/// A voice the mock audio backend is playing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    pub id: VoiceId,
    pub sound: SoundId,
    pub params: SoundParams,
    pub looped: bool,
}

/// Audio backend with the voice bookkeeping a console mixer does: a fixed
/// voice budget, one music track and a stinger that ducks it.
pub struct MockAudio {
    pub max_voices: usize,
    pub voices: Vec<Voice>,
    pub one_shots: Vec<(SoundId, SoundParams)>,
    pub music: Option<(MusicId, bool)>,
    pub stinger: Option<MusicId>,
    pub master: f32,
    pub music_volume: f32,
    pub stopped: Vec<VoiceId>,
    next: u32,
}

impl MockAudio {
    pub fn new(max_voices: usize) -> Self {
        MockAudio {
            max_voices,
            voices: Vec::new(),
            one_shots: Vec::new(),
            music: None,
            stinger: None,
            master: 1.0,
            music_volume: 1.0,
            stopped: Vec::new(),
            next: 0,
        }
    }

    pub fn voice(&self, id: VoiceId) -> Option<&Voice> {
        self.voices.iter().find(|v| v.id == id)
    }

    /// The music's effective volume (ducked to 30% under a stinger, like
    /// the console channel plan).
    pub fn effective_music_volume(&self) -> f32 {
        let duck = if self.stinger.is_some() { 0.3 } else { 1.0 };
        self.music
            .map_or(0.0, |_| self.master * self.music_volume * duck)
    }

    /// The stinger ended (the mixer reached its last sample).
    pub fn finish_stinger(&mut self) {
        self.stinger = None;
    }
}

impl Audio for MockAudio {
    fn play_sound(&mut self, sound: SoundId) {
        self.one_shots.push((sound, SoundParams::DEFAULT));
    }

    fn play_music(&mut self, music: MusicId, looped: bool) {
        self.music = Some((music, looped));
    }

    fn stop_music(&mut self) {
        self.music = None;
    }

    fn set_master_volume(&mut self, volume: f32) {
        self.master = volume;
    }

    fn play_sound_with(&mut self, sound: SoundId, params: &SoundParams) {
        self.one_shots.push((sound, *params));
    }

    fn play_voice(
        &mut self,
        sound: SoundId,
        params: &SoundParams,
        looped: bool,
    ) -> Option<VoiceId> {
        if self.voices.len() == self.max_voices {
            return None;
        }
        self.next += 1;
        let id = VoiceId(self.next);
        self.voices.push(Voice {
            id,
            sound,
            params: *params,
            looped,
        });
        Some(id)
    }

    fn set_voice(&mut self, voice: VoiceId, params: &SoundParams) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == voice) {
            v.params = *params;
        }
    }

    fn stop_voice(&mut self, voice: VoiceId) {
        self.voices.retain(|v| v.id != voice);
        self.stopped.push(voice);
    }

    fn voice_playing(&self, voice: VoiceId) -> bool {
        self.voice(voice).is_some()
    }

    fn play_stinger(&mut self, music: MusicId) {
        self.stinger = Some(music);
    }

    fn stinger_playing(&self) -> bool {
        self.stinger.is_some()
    }

    fn set_music_volume(&mut self, volume: f32) {
        self.music_volume = volume;
    }
}

// ------------------------------------------------------------ platform ----

/// A save medium framed exactly like the console backends do it
/// (`header ‖ data`, data written first and the header last), plus blobs.
pub struct MemoryPlatform {
    /// The raw medium (EEPROM image); `0xFF` = blank.
    pub medium: Vec<u8>,
    pub blobs: HashMap<u32, &'static [u8]>,
    pub log: std::cell::RefCell<Vec<String>>,
    pub clock: u64,
    /// Simulate a power cut after this many bytes of the next write.
    pub cut_after: Option<usize>,
}

impl MemoryPlatform {
    pub fn new(medium_bytes: usize) -> Self {
        MemoryPlatform {
            medium: vec![0xFF; medium_bytes],
            blobs: HashMap::new(),
            log: Default::default(),
            clock: 0,
            cut_after: None,
        }
    }

    pub fn insert_blob(&mut self, id: BlobId, bytes: Vec<u8>) {
        self.blobs.insert(id.0, Box::leak(bytes.into_boxed_slice()));
    }

    fn write_at(&mut self, at: usize, bytes: &[u8], budget: &mut Option<usize>) -> bool {
        for (i, b) in bytes.iter().enumerate() {
            if let Some(left) = budget {
                if *left == 0 {
                    return false;
                }
                *left -= 1;
            }
            self.medium[at + i] = *b;
        }
        true
    }
}

impl Platform for MemoryPlatform {
    fn now_micros(&self) -> u64 {
        self.clock
    }

    fn log(&self, message: &str) {
        self.log.borrow_mut().push(message.to_string());
    }

    fn blob(&self, id: BlobId) -> Option<&'static [u8]> {
        self.blobs.get(&id.0).copied()
    }

    fn save_capacity(&self) -> usize {
        self.medium.len().saturating_sub(HEADER)
    }

    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        if self.medium.len() <= HEADER {
            return Err(SaveError::Unsupported);
        }
        let header: [u8; HEADER] = self.medium[..HEADER].try_into().unwrap();
        let total = self.medium.len();
        let Some(len) = save::data_len(&header).filter(|&l| l <= total - HEADER) else {
            return Ok(0);
        };
        let data = &self.medium[HEADER..HEADER + len];
        if !save::verify(&header, data) {
            return Ok(0);
        }
        let n = len.min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(len)
    }

    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        if self.medium.len() <= HEADER {
            return Err(SaveError::Unsupported);
        }
        let capacity = self.save_capacity();
        if data.len() > capacity {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity,
            });
        }
        let mut budget = self.cut_after.take();
        if !self.write_at(HEADER, data, &mut budget) {
            return Err(SaveError::Io);
        }
        let header = save::header(data);
        if !self.write_at(0, &header, &mut budget) {
            return Err(SaveError::Io);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- TMDL ----

pub fn f32s(out: &mut Vec<u8>, vals: &[f32]) {
    for v in vals {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Append a `tag len payload` section.
pub fn section(blob: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    blob.extend_from_slice(tag);
    blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
    blob.extend_from_slice(body);
}

/// The base TMDL layout: header, positions, normals, colors, indices.
pub fn tmdl(positions: &[Vec3], normals: &[Vec3], colors: &[[u8; 4]], indices: &[u16]) -> Vec<u8> {
    assert_eq!(positions.len(), normals.len());
    assert_eq!(positions.len(), colors.len());
    let mut blob = Vec::new();
    blob.extend_from_slice(render3d::TMDL_MAGIC);
    blob.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for p in positions {
        f32s(&mut blob, &[p.x, p.y, p.z]);
    }
    for n in normals {
        f32s(&mut blob, &[n.x, n.y, n.z]);
    }
    for c in colors {
        blob.extend_from_slice(c);
    }
    for i in indices {
        blob.extend_from_slice(&i.to_le_bytes());
    }
    blob
}

/// The six faces of a unit cube: (outward normal, color).
pub const CUBE_FACES: [(Vec3, [u8; 4]); 6] = [
    (Vec3::new(0.0, 0.0, -1.0), [230, 80, 80, 255]),
    (Vec3::new(0.0, 0.0, 1.0), [80, 230, 80, 255]),
    (Vec3::new(-1.0, 0.0, 0.0), [80, 80, 230, 255]),
    (Vec3::new(1.0, 0.0, 0.0), [230, 230, 80, 255]),
    (Vec3::new(0.0, -1.0, 0.0), [230, 80, 230, 255]),
    (Vec3::new(0.0, 1.0, 0.0), [80, 230, 230, 255]),
];

/// Quads (4 vertices, 2 front-facing triangles each) for `faces`, each a
/// `size`-sided square at `size / 2` along its normal.
pub fn faces_tmdl(faces: &[(Vec3, [u8; 4])], size: f32) -> Vec<u8> {
    let (mut pos, mut nrm, mut col, mut idx) = (vec![], vec![], vec![], vec![]);
    for (f, (n, c)) in faces.iter().enumerate() {
        let u = if n.y.abs() > 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0).cross(*n).normalized()
        };
        let v = n.cross(u);
        let base = (f * 4) as u16;
        for (su, sv) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            pos.push((*n * 0.5 + u * su + v * sv) * size);
            nrm.push(*n);
            col.push(*c);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    tmdl(&pos, &nrm, &col, &idx)
}

/// Unit cube: 24 vertices, 12 triangles.
pub fn cube_tmdl() -> Vec<u8> {
    faces_tmdl(&CUBE_FACES, 1.0)
}

/// `BNDS` body: bounding sphere.
pub fn bnds(center: Vec3, radius: f32) -> Vec<u8> {
    let mut b = Vec::new();
    f32s(&mut b, &[center.x, center.y, center.z, radius]);
    b
}

/// Unit cube with a baked bounding sphere.
pub fn bounded_cube() -> Vec<u8> {
    let mut blob = cube_tmdl();
    section(&mut blob, b"BNDS", &bnds(Vec3::ZERO, 0.87));
    blob
}

pub const IDENT34: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
/// Rest pose `t3 q4 s3`: identity.
pub const REST: [f32; 10] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];

/// `t3 q4 s3` sample translated by `t`.
pub fn sample(t: Vec3) -> [f32; 10] {
    [t.x, t.y, t.z, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]
}

/// `SKEL` body: joint per vertex, then (parent, inverse bind, rest) per joint.
pub fn skel(joint_of_vertex: &[u8], joints: &[(i32, [f32; 12], [f32; 10])]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(joints.len() as u32).to_le_bytes());
    b.extend_from_slice(joint_of_vertex);
    for (parent, inv, rest) in joints {
        b.extend_from_slice(&parent.to_le_bytes());
        f32s(&mut b, inv);
        f32s(&mut b, rest);
    }
    b
}

/// One baked clip: name, fps, looping, frames of per-joint samples.
pub struct ClipDef<'a> {
    pub name: &'a str,
    pub fps: f32,
    pub looping: bool,
    pub frames: Vec<Vec<[f32; 10]>>,
}

/// `ANIM` body.
pub fn anim(clips: &[ClipDef]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(clips.len() as u32).to_le_bytes());
    for c in clips {
        b.extend_from_slice(&trino_core::ClipId::from_name(c.name).0.to_le_bytes());
        f32s(&mut b, &[c.fps]);
        b.extend_from_slice(&(c.frames.len() as u32).to_le_bytes());
        b.extend_from_slice(&(c.looping as u32).to_le_bytes());
        for frame in &c.frames {
            for s in frame {
                f32s(&mut b, s);
            }
        }
    }
    b
}

/// Unit cube rigged with two joints: the root owns the bottom vertices,
/// its child the top ones. Clips: `lift` (looping, 2 fps, the child rises
/// 1 unit per frame over 3 frames) and `slide` (once, 1 fps, the child
/// moves +4 X over 2 frames).
pub fn rigged_cube() -> Vec<u8> {
    let mut blob = cube_tmdl();
    let base = cube_tmdl();
    let mesh = Mesh::from_tmdl(&base).unwrap();
    let jov: Vec<u8> = (0..mesh.vertex_count)
        .map(|i| u8::from(mesh.position(i).y > 0.0))
        .collect();
    section(
        &mut blob,
        b"SKEL",
        &skel(&jov, &[(-1, IDENT34, REST), (0, IDENT34, REST)]),
    );
    let lift = ClipDef {
        name: "lift",
        fps: 2.0,
        looping: true,
        frames: (0..3)
            .map(|f| vec![REST, sample(Vec3::new(0.0, f as f32, 0.0))])
            .collect(),
    };
    let slide = ClipDef {
        name: "slide",
        fps: 1.0,
        looping: false,
        frames: (0..2)
            .map(|f| vec![REST, sample(Vec3::new(4.0 * f as f32, 0.0, 0.0))])
            .collect(),
    };
    section(&mut blob, b"ANIM", &anim(&[lift, slide]));
    blob
}

/// The unit cube with planar UVs and a default texture `TXID`.
pub fn textured_cube(texture: SpriteId) -> Vec<u8> {
    let mut blob = cube_tmdl();
    let base = cube_tmdl();
    let mesh = Mesh::from_tmdl(&base).unwrap();
    let mut uv = Vec::new();
    for i in 0..mesh.vertex_count {
        let p = mesh.position(i);
        f32s(&mut uv, &[p.x + 0.5, p.y + 0.5]);
    }
    section(&mut blob, b"TXUV", &uv);
    section(&mut blob, b"TXID", &texture.0.to_le_bytes());
    blob
}

/// Big horizontal quad (normal +Y) at height `y`, `half` units around the
/// origin — a floor that crosses the near plane of a camera standing on it.
pub fn floor_tmdl(y: f32, half: f32) -> Vec<u8> {
    let n = Vec3::new(0.0, 1.0, 0.0);
    let pos = [
        Vec3::new(-half, y, -half),
        Vec3::new(half, y, -half),
        Vec3::new(half, y, half),
        Vec3::new(-half, y, half),
    ];
    // Front-facing seen from above (y-down screen, positive area).
    tmdl(
        &pos,
        &[n; 4],
        &[[200, 200, 200, 255]; 4],
        &[0, 2, 1, 0, 3, 2],
    )
}

// ---------------------------------------------------------------- TFNT ----

pub fn glyph(c: char, x: u16, w: u8, advance: u8) -> Glyph {
    Glyph {
        codepoint: c as u32,
        x,
        y: 0,
        w,
        h: 8,
        x_offset: 0,
        advance,
    }
}

/// TFNT v1 (records sorted by codepoint, as the bake writes them).
pub fn tfnt(line_height: u16, ascent: u16, glyphs: &[Glyph]) -> Vec<u8> {
    let mut sorted = glyphs.to_vec();
    sorted.sort_by_key(|g| g.codepoint);
    let mut out = Vec::new();
    out.extend_from_slice(text::TFNT_MAGIC);
    out.extend_from_slice(&text::TFNT_VERSION.to_le_bytes());
    out.extend_from_slice(&line_height.to_le_bytes());
    out.extend_from_slice(&ascent.to_le_bytes());
    out.extend_from_slice(&(sorted.len() as u16).to_le_bytes());
    for g in &sorted {
        out.extend_from_slice(&g.codepoint.to_le_bytes());
        out.extend_from_slice(&g.x.to_le_bytes());
        out.extend_from_slice(&g.y.to_le_bytes());
        out.extend_from_slice(&[g.w, g.h, g.x_offset as u8, g.advance]);
    }
    out
}

/// A 9-pixel-tall font with `a`, `b`, `?` (5 wide, advance 6), space
/// (advance 4) and `j` (x offset -1).
pub fn sample_font() -> Vec<u8> {
    let mut j = glyph('j', 18, 4, 5);
    j.x_offset = -1;
    tfnt(
        9,
        7,
        &[
            glyph('b', 6, 5, 6),
            glyph('a', 0, 5, 6),
            glyph('?', 12, 5, 6),
            glyph(' ', 0, 0, 4),
            j,
        ],
    )
}

// ---------------------------------------------------------------- TSCN ----

/// One node of a hand-built TSCN blob (the record layout of
/// `trino_core::scene`, ADR 0005 §3).
#[derive(Clone)]
pub struct TscnNode {
    pub name: String,
    pub tags: Vec<String>,
    pub matrix: Mat34,
    pub model: Option<u32>,
    pub double_sided: bool,
    pub texture: Option<u32>,
    /// 0 gouraud, 1 flat, 2 unlit.
    pub shading: u32,
    pub tint: [u8; 4],
    pub emissive: [u8; 4],
    pub anim: Option<(u32, f32)>,
    /// (shape 1 box / 2 sphere / 3 capsule, a b c, offset, trigger)
    pub collider: Option<(u32, [f32; 3], [f32; 3], bool)>,
    pub lods: Vec<(f32, u32)>,
    pub sound: Option<(u32, f32, f32)>,
    pub cell: u8,
    /// (kind 1 cell / 2 portal, a, b, half)
    pub vis: Option<(u8, u8, u8, [f32; 3])>,
    /// (kind 1 blob / 2 mesh / 3 auto, blob radius) (v5)
    pub shadow: Option<(u8, f32)>,
    /// The receiver's top: (center, half x, half z) (v5)
    pub receiver: Option<(Vec3, f32, f32)>,
}

impl TscnNode {
    pub fn new(name: &str) -> Self {
        TscnNode {
            name: name.into(),
            tags: vec![],
            matrix: Mat34::IDENTITY,
            model: None,
            double_sided: false,
            texture: None,
            shading: 0,
            tint: [255; 4],
            emissive: [0, 0, 0, 255],
            anim: None,
            collider: None,
            lods: vec![],
            sound: None,
            cell: trino_core::NO_CELL,
            vis: None,
            shadow: None,
            receiver: None,
        }
    }

    pub fn at(mut self, p: Vec3) -> Self {
        self.matrix.m[0][3] = p.x;
        self.matrix.m[1][3] = p.y;
        self.matrix.m[2][3] = p.z;
        self
    }
}

/// Environment of a hand-built TSCN.
pub struct TscnEnv {
    pub clear: [u8; 3],
    pub camera: Camera3,
    pub light: Light,
}

/// Encode a TSCN blob (the current version, shadows off), leaked to
/// `'static` like `Platform::blob` bytes.
pub fn tscn(env: &TscnEnv, nodes: &[TscnNode]) -> &'static [u8] {
    use trino_core::scene::{
        TSCN_ENV_BYTES, TSCN_HEADER, TSCN_MAGIC, TSCN_NODE_BYTES, TSCN_VERSION,
    };
    let mut b = Vec::new();
    b.extend_from_slice(TSCN_MAGIC);
    b.extend_from_slice(&TSCN_VERSION.to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    b.extend_from_slice(&(nodes.len() as u32).to_le_bytes());
    b.extend_from_slice(&[env.clear[0], env.clear[1], env.clear[2], 0]);
    let c = &env.camera;
    f32s(
        &mut b,
        &[
            c.eye.x, c.eye.y, c.eye.z, c.target.x, c.target.y, c.target.z, c.fov_y,
        ],
    );
    let l = &env.light;
    f32s(&mut b, &[l.dir.x, l.dir.y, l.dir.z, l.ambient]);
    for col in [l.color, l.ambient_color, l.fog.color] {
        b.extend_from_slice(&[col.r, col.g, col.b, col.a]);
    }
    f32s(&mut b, &[l.fog.near, l.fog.far]);
    // Shadows (v5): off.
    b.extend_from_slice(&[0; trino_core::scene::TSCN_ENV_SHADOW_BYTES]);
    assert_eq!(b.len(), TSCN_HEADER + TSCN_ENV_BYTES);

    let mut data: Vec<u8> = Vec::new();
    let mut put = |bytes: &[u8]| {
        let at = data.len() as u32;
        data.extend_from_slice(bytes);
        (at, bytes.len() as u32)
    };
    for n in nodes {
        let start = b.len();
        let (no, nl) = put(n.name.as_bytes());
        let (to, tl) = put(n.tags.join("\n").as_bytes());
        let mut lod = Vec::new();
        for (d, m) in &n.lods {
            lod.extend_from_slice(&d.to_le_bytes());
            lod.extend_from_slice(&m.to_le_bytes());
            // Not an impostor: the rest of the 32-byte record is zero.
            lod.extend_from_slice(&[0; trino_core::scene::LOD_RECORD_BYTES - 8]);
        }
        let (lo, _) = put(&lod);
        for v in [no, nl, to, tl] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for row in n.matrix.m {
            f32s(&mut b, &row);
        }
        let mut flags = n.shading << 8;
        flags |= u32::from(n.model.is_some());
        flags |= u32::from(n.anim.is_some()) << 1;
        flags |= u32::from(n.double_sided) << 2;
        flags |= u32::from(n.texture.is_some()) << 3;
        flags |= u32::from(n.sound.is_some()) << 4;
        if let Some((shape, _, _, trigger)) = n.collider {
            flags |= shape << 12;
            flags |= u32::from(trigger) << 15;
        }
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(&n.model.unwrap_or(0).to_le_bytes());
        b.extend_from_slice(&n.tint);
        b.extend_from_slice(&n.emissive);
        let (clip, speed) = n.anim.unwrap_or((0, 0.0));
        b.extend_from_slice(&clip.to_le_bytes());
        f32s(&mut b, &[speed]);
        let (_, abc, off, _) = n.collider.unwrap_or((0, [0.0; 3], [0.0; 3], false));
        f32s(&mut b, &abc);
        f32s(&mut b, &off);
        b.extend_from_slice(&n.texture.unwrap_or(0).to_le_bytes());
        b.extend_from_slice(&lo.to_le_bytes());
        b.extend_from_slice(&(n.lods.len() as u32).to_le_bytes());
        let (sound, radius, volume) = n.sound.unwrap_or((0, 0.0, 0.0));
        b.extend_from_slice(&sound.to_le_bytes());
        f32s(&mut b, &[radius, volume]);
        let (kind, pa, pb, half) = n.vis.unwrap_or((0, 0, 0, [0.0; 3]));
        b.extend_from_slice(&[n.cell, kind, pa, pb]);
        f32s(&mut b, &half);
        // Surface (v3): matte, the target's filter, no detail layer; no
        // water (v4, flag bit6 off).
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&[0; trino_core::scene::TSCN_WATER_BYTES]);
        // Shadow (v5): the cast, and the receiver's top.
        let (cast, radius) = n.shadow.unwrap_or((0, 0.0));
        b.extend_from_slice(&[cast, u8::from(n.receiver.is_some()), 0, 0]);
        let (c, hx, hz) = n.receiver.unwrap_or((Vec3::ZERO, 0.0, 0.0));
        f32s(&mut b, &[radius, c.x, c.y, c.z, hx, hz]);
        // Glow (v6, flag bit7 off).
        b.extend_from_slice(&[0; trino_core::scene::TSCN_GLOW_BYTES]);
        // Physics (v7): the default layer, no character.
        b.extend_from_slice(&trino_core::physics::LAYER_DEFAULT.to_le_bytes());
        b.extend_from_slice(&[0; trino_core::scene::TSCN_PHYSICS_BYTES - 2]);
        // Access (v8): no semantic slot, palette or outline.
        b.extend_from_slice(&[0; trino_core::scene::TSCN_ACCESS_BYTES]);
        assert_eq!(b.len() - start, TSCN_NODE_BYTES);
    }
    b.extend_from_slice(&data);
    Box::leak(b.into_boxed_slice())
}
