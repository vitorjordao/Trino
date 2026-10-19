//! Drawing the match: the stadium, the players (rigid parts posed by
//! [`anim`]), the ball, the officials and the 3D markers (cursor, switch
//! target, pass lane, shot arc). The sim is read-only here.
//!
//! **Scale:** the world is drawn at [`WORLD`] (1/64) so the N64's 16-bit
//! depth buffer resolves 2 cm line strips at 60 m; the camera is scaled the
//! same way, so the picture is identical.
//!
//! **Tiers:** the PC's native profile (per-pixel lights) draws 11 parts per
//! player with knees and elbows, palette kits and a dense crowd (PC-only
//! models and overrides); consoles draw 7 tinted parts that all stay on the
//! N64's RSP, and fewer for distant players.

pub mod anim;
#[cfg(test)]
pub(crate) mod fit;
pub mod menu;
pub mod studio;
#[rustfmt::skip]
#[allow(clippy::all)]
pub mod clips_gen;
pub mod hud;

use crate::sim::engine::{Act, EventKind, Match, N, Phase, SIDE, side_of};
use crate::sim::fx::Fx;
use crate::sim::pitch::{HALF_LEN, HALF_WID};
use crate::sim::team::{CLUBS, Rgb, Role};
use anim::{AnimState, Pose, Tint, body_matrix, joint, joint_matrices, mask};
use clips_gen::{ClipName, JOINTS};
use trino_core::{
    BillboardParams, Camera3, Color, DEFAULT_LIGHT, Light, LightTier, Mat34, Material, ModelId,
    ModelParams, Renderer, Shading, SpriteId, Vec2, Vec3,
};

/// World units per meter when drawn.
pub const WORLD: f32 = 1.0 / 64.0;

const fn model(path: &str) -> ModelId {
    ModelId::from_path(path)
}

pub const M_GOAL: ModelId = model("models/se_goal");
pub const M_GOAL_HD: ModelId = model("models/se_goal_hd");
pub const M_BALL_HD: ModelId = model("models/se_ball_hd");
pub const M_HEAD_HD: ModelId = model("models/se_head_hd");
pub const M_TORSO_HD: ModelId = model("models/se_torso_hd");
pub const M_SHORTS_HD: ModelId = model("models/se_shorts_hd");
pub const M_STAND_LONG_HD: [ModelId; 2] = [
    model("models/se_stand_long_a_hd"),
    model("models/se_stand_long_b_hd"),
];
pub const M_STAND_END_HD: [ModelId; 2] = [
    model("models/se_stand_end_a_hd"),
    model("models/se_stand_end_b_hd"),
];
pub const M_FLAG: ModelId = model("models/se_corner_flag");
pub const M_BALL: ModelId = model("models/se_ball");
pub const M_FLOOD: ModelId = model("models/se_flood_tower");
pub const M_STAND_LONG: [ModelId; 2] = [
    model("models/se_stand_long_a"),
    model("models/se_stand_long_b"),
];
pub const M_STAND_END: [ModelId; 2] = [
    model("models/se_stand_end_a"),
    model("models/se_stand_end_b"),
];
pub const M_HEAD: ModelId = model("models/se_head");
/// The console heads by [`Hair::console_head`]: dark hair, light hair,
/// shaved (the hair is baked into the one wedge, so the head stays one
/// draw on the RSP).
pub const M_CONSOLE_HEADS: [ModelId; 3] = [
    M_HEAD,
    model("models/se_head_light"),
    model("models/se_head_shaved"),
];
pub const M_TORSO: ModelId = model("models/se_torso");
pub const M_SHORTS: ModelId = model("models/se_shorts");
pub const M_ARM: ModelId = model("models/se_arm");
pub const M_LEG: ModelId = model("models/se_leg");
pub const M_UPPER_ARM: ModelId = model("models/se_upper_arm");
pub const M_FOREARM: ModelId = model("models/se_forearm");
pub const M_THIGH: ModelId = model("models/se_thigh");
pub const M_SHIN: ModelId = model("models/se_shin");
pub const M_FLAG_HAND: ModelId = model("models/se_flag_hand");
pub const M_RING: ModelId = model("models/se_ring");
pub const M_RING_SMALL: ModelId = model("models/se_ring_small");
pub const M_ARROW: ModelId = model("models/se_arrow");
pub const M_DASH: ModelId = model("models/se_dash");
pub const M_DOT: ModelId = model("models/se_dot");
pub const M_PEDESTAL: ModelId = model("models/se_pedestal");
pub const M_BLOB: ModelId = model("models/se_blob");
pub const M_PLAYER_SHADOW: ModelId = model("models/se_player_shadow");
pub const M_TROPHY: ModelId = model("models/se_trophy");

/// The 6 x 4 pitch tiles, row-major by x then z.
pub const M_PITCH: [ModelId; 24] = [
    model("models/se_pitch_0_0"),
    model("models/se_pitch_0_1"),
    model("models/se_pitch_0_2"),
    model("models/se_pitch_0_3"),
    model("models/se_pitch_1_0"),
    model("models/se_pitch_1_1"),
    model("models/se_pitch_1_2"),
    model("models/se_pitch_1_3"),
    model("models/se_pitch_2_0"),
    model("models/se_pitch_2_1"),
    model("models/se_pitch_2_2"),
    model("models/se_pitch_2_3"),
    model("models/se_pitch_3_0"),
    model("models/se_pitch_3_1"),
    model("models/se_pitch_3_2"),
    model("models/se_pitch_3_3"),
    model("models/se_pitch_4_0"),
    model("models/se_pitch_4_1"),
    model("models/se_pitch_4_2"),
    model("models/se_pitch_4_3"),
    model("models/se_pitch_5_0"),
    model("models/se_pitch_5_1"),
    model("models/se_pitch_5_2"),
    model("models/se_pitch_5_3"),
];
const TILE_W: f32 = 117.0 / 6.0;
const TILE_D: f32 = 80.0 / 4.0;

/// The 12 x 8 fine tiles (`models/se_pitch_f_<ix>_<iz>`), by `ix * 8 + iz`.
pub const M_PITCH_FINE: [ModelId; 96] = {
    let mut out = [ModelId(0); 96];
    let mut i = 0;
    while i < 96 {
        let (ix, iz) = (i / 8, i % 8);
        let mut path = [0u8; 24];
        let head = b"models/se_pitch_f_";
        let mut len = 0;
        while len < head.len() {
            path[len] = head[len];
            len += 1;
        }
        if ix >= 10 {
            path[len] = b'0' + (ix / 10) as u8;
            len += 1;
        }
        path[len] = b'0' + (ix % 10) as u8;
        path[len + 1] = b'_';
        path[len + 2] = b'0' + iz as u8;
        out[i] = ModelId(fnv1a(&path, len + 3));
        i += 1;
    }
    out
};

/// `trino_core::asset_id` over the first `len` bytes (a const path built
/// from numbers).
const fn fnv1a(bytes: &[u8], len: usize) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < len {
        h ^= bytes[i] as u32;
        h = h.wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

/// Beyond this view depth a coarse tile is drawn whole.
const FINE_DEPTH: f32 = 30.0;

/// Skin tones a player's `skin` index picks.
pub const SKIN_TONES: [Color; 8] = [
    Color::rgb(255, 224, 196),
    Color::rgb(241, 194, 155),
    Color::rgb(224, 172, 125),
    Color::rgb(198, 140, 95),
    Color::rgb(166, 110, 72),
    Color::rgb(134, 86, 56),
    Color::rgb(104, 66, 44),
    Color::rgb(78, 50, 34),
];

/// Hair colors a player's `hair` index picks: black, dark brown, brown,
/// blond, red.
pub const HAIR_COLORS: [Color; 5] = [
    Color::rgb(34, 28, 26),
    Color::rgb(72, 48, 32),
    Color::rgb(122, 84, 50),
    Color::rgb(214, 178, 106),
    Color::rgb(156, 70, 36),
];

/// Hair styles: the PC's [`clips_gen::HAIR`] parts in order, then bald.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HairStyle {
    Crop,
    Buzz,
    Curly,
    Long,
    Mohawk,
    Bald,
}

impl HairStyle {
    pub const ALL: [HairStyle; 6] = [
        HairStyle::Crop,
        HairStyle::Buzz,
        HairStyle::Curly,
        HairStyle::Long,
        HairStyle::Mohawk,
        HairStyle::Bald,
    ];

    pub fn from_index(i: u8) -> HairStyle {
        HairStyle::ALL[i as usize % HairStyle::ALL.len()]
    }

    /// The PC's hair part (none for bald).
    pub fn part(self) -> Option<&'static anim::Part> {
        clips_gen::HAIR.get(self as usize)
    }
}

/// A player's hair: a style and a color.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hair {
    pub style: HairStyle,
    pub color: Color,
}

impl Hair {
    /// Short dark hair (officials, and looks made without a player).
    pub const DEFAULT: Hair = Hair {
        style: HairStyle::Crop,
        color: HAIR_COLORS[0],
    };

    /// A player's hair from the database's indices.
    pub fn of(style: u8, color: u8) -> Hair {
        Hair {
            style: HairStyle::from_index(style),
            color: HAIR_COLORS[color as usize % HAIR_COLORS.len()],
        }
    }

    /// Which [`M_CONSOLE_HEADS`] head the consoles draw: shaved for the
    /// bald, light for blond and red hair, dark otherwise.
    pub fn console_head(self) -> usize {
        if self.style == HairStyle::Bald {
            2
        } else if self.color == HAIR_COLORS[3] || self.color == HAIR_COLORS[4] {
            1
        } else {
            0
        }
    }
}

pub fn rgb(c: Rgb) -> Color {
    Color::rgb(c.0, c.1, c.2)
}

/// The grounds a match can be played in: the same stand models, tinted,
/// and more or fewer of them (a tint keeps every draw on the N64's RSP; a
/// smaller ground draws fewer).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stadium {
    /// Four full stands.
    #[default]
    Arena,
    /// A small ground: sun-bleached stands along the middle of the sides,
    /// open ends.
    Harbour,
    /// Cool painted stands all round, a lusher pitch.
    Park,
}

impl Stadium {
    pub const ALL: [Stadium; 3] = [Stadium::Arena, Stadium::Harbour, Stadium::Park];

    pub fn from_index(i: u8) -> Stadium {
        Stadium::ALL[i as usize % Stadium::ALL.len()]
    }

    /// Club `c`'s home ground (career matches).
    pub fn of_club(c: u8) -> Stadium {
        Stadium::from_index(c)
    }

    /// The stands' tint and the pitch's (times the sky's).
    fn tints(self) -> (Color, Color) {
        match self {
            Stadium::Arena => (Color::WHITE, Color::WHITE),
            Stadium::Harbour => (Color::rgb(255, 196, 140), Color::rgb(250, 244, 200)),
            Stadium::Park => (Color::rgb(150, 186, 255), Color::rgb(214, 255, 214)),
        }
    }

    fn end_stands(self) -> bool {
        self != Stadium::Harbour
    }

    /// The side stands' sections (of six along each touchline).
    fn side_sections(self) -> core::ops::Range<usize> {
        if self == Stadium::Harbour { 1..5 } else { 0..6 }
    }
}

/// Lighting and sky by time of day.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Sky {
    #[default]
    Day,
    Dusk,
    Night,
}

impl Sky {
    pub fn clear(self) -> Color {
        match self {
            Sky::Day => Color::rgb(126, 178, 232),
            Sky::Dusk => Color::rgb(232, 150, 110),
            Sky::Night => Color::rgb(16, 22, 48),
        }
    }

    /// Multiplied into every draw's tint: dusk warms the scene, night
    /// darkens and cools it. (A tint stays on the N64's RSP; a dimmed or
    /// colored global light would not.)
    pub fn tint(self) -> Color {
        match self {
            Sky::Day => Color::WHITE,
            Sky::Dusk => Color::rgb(250, 214, 186),
            Sky::Night => Color::rgb(150, 162, 205),
        }
    }

    /// White light only: a colored light sends every N64 draw to the CPU
    /// (`RspReject::ColoredLight`). Dusk and night are a lower, grazing
    /// light and less ambient.
    pub fn light(self) -> Light {
        match self {
            Sky::Day => Light {
                dir: Vec3::new(0.35, -1.0, -0.45),
                ambient: 0.48,
                ..DEFAULT_LIGHT
            },
            Sky::Dusk => Light {
                dir: Vec3::new(0.8, -0.55, -0.3),
                ambient: 0.42,
                ..DEFAULT_LIGHT
            },
            Sky::Night => Light {
                dir: Vec3::new(-0.2, -1.0, 0.25),
                ambient: 0.4,
                ..DEFAULT_LIGHT
            },
        }
    }
}

/// How a player looks (kit tints, skin).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Look {
    pub shirt: Color,
    pub shorts: Color,
    pub socks: Color,
    pub skin: Color,
    pub hair: Hair,
    /// The consoles' baked leg for this shorts and socks pair
    /// ([`crate::sim::team::LEG_KITS`]), or [`NO_KIT_LEGS`]: then the legs
    /// are tinted parts.
    pub legs: u8,
}

/// [`Look::legs`] with no baked leg.
pub const NO_KIT_LEGS: u8 = u8::MAX;

impl Look {
    /// A look from kit colors: the baked console legs when the pair has one.
    pub fn of(shirt: Rgb, shorts: Rgb, socks: Rgb, skin: Color) -> Look {
        Look {
            shirt: rgb(shirt),
            shorts: rgb(shorts),
            socks: rgb(socks),
            skin,
            hair: Hair::DEFAULT,
            legs: crate::sim::team::leg_kit(shorts, socks).unwrap_or(NO_KIT_LEGS),
        }
    }

    pub fn with_hair(self, hair: Hair) -> Look {
        Look { hair, ..self }
    }
}

/// Visual state the game keeps for the match (never read by the sim).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MatchView {
    pub anims: [AnimState; N + 3],
    pub looks: [Look; N],
    /// Ball spin, as a rotation.
    pub ball_rot: trino_core::Quat,
    pub cam: CamState,
    pub sky: Sky,
    pub stadium: Stadium,
    pub rain: bool,
    /// Seconds since the match view began (marker pulses).
    pub time: f32,
    /// What the HUD draws this frame (the game computes it in update).
    pub guides: Guides,
    /// The settings' retro graphics: the console models on the PC too.
    pub retro: bool,
}

/// The PC tier (15-part players, dense crowd, effects): a per-pixel
/// renderer, unless the settings ask for the retro look.
pub fn hifi(r: &dyn Renderer, retro: bool) -> bool {
    !retro && r.caps().light_tier == LightTier::PerPixel
}

/// The PC tier's screen-space ambient occlusion (contact shadows under
/// players, the goal's frame, the stands' rows), in meters scaled to the
/// drawing scale. Consoles bake theirs into the stands' vertex colors.
const AO: trino_core::AmbientOcclusion = trino_core::AmbientOcclusion {
    radius: 0.55 * WORLD,
    intensity: 1.0,
    bias: 0.03 * WORLD,
    quality: trino_core::AoQuality::Low,
};

/// Declare the frame's ambient occlusion (cleared by `begin_frame`): on
/// for the PC tier, a no-op where the renderer bakes it.
pub fn set_ao(r: &mut dyn Renderer, hifi: bool) {
    if hifi {
        r.set_ambient_occlusion(Some(&AO));
    }
}

/// The broadcast camera: a focus that trails the ball, an eye above the
/// south touchline.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CamState {
    pub focus: Vec3,
    /// Height and distance back (m), and the field of view.
    pub height: f32,
    pub back: f32,
    pub fov: f32,
    /// Widescreen (PC/browser) tracks deeper into lateral touchlines.
    pub wide: bool,
}

impl CamState {
    pub const BROADCAST: CamState = CamState {
        focus: Vec3::ZERO,
        height: 18.0,
        back: 17.0,
        fov: 0.66,
        wide: false,
    };

    pub fn eye(&self) -> Vec3 {
        Vec3::new(
            self.focus.x,
            self.focus.y + self.height,
            self.focus.z + self.back,
        )
    }

    /// The camera as the renderer takes it (scaled world). The focus is
    /// snapped to 1/4096 of a world unit and the eye sits at an exactly
    /// representable offset from it, so `target - eye` — the view's
    /// rotation — is bit-identical from frame to frame while the camera
    /// pans: the N64's draw memo then patches the stadium's draws instead
    /// of rebuilding them (n64-performance skill).
    pub fn camera(&self) -> Camera3 {
        let q = |v: f32| trino_core::math3d::to_i32(v * WORLD * 4096.0) as f32 * (1.0 / 4096.0);
        let target = Vec3::new(q(self.focus.x), q(self.focus.y), q(self.focus.z));
        let offset = Vec3::new(0.0, q(self.height), q(self.back));
        Camera3 {
            eye: target + offset,
            target,
            fov_y: self.fov,
        }
    }

    /// Ease the focus toward the ball (and a little ahead of its run).
    pub fn follow(&mut self, ball: Vec3, vel: Vec3, dt: f32) {
        let ahead = Vec3::new(vel.x * 0.35, 0.0, vel.z * 0.25);
        let (z_scale, min_z, max_z) = if self.wide {
            (0.92, -HALF_WID.to_f32() + 4.0, HALF_WID.to_f32() - 4.0)
        } else {
            (0.55, -HALF_WID.to_f32() + 12.0, HALF_WID.to_f32() - 8.0)
        };
        let want = Vec3::new(
            (ball.x + ahead.x).clamp(-HALF_LEN.to_f32() + 14.0, HALF_LEN.to_f32() - 14.0),
            0.0,
            ((ball.z + ahead.z) * z_scale).clamp(min_z, max_z),
        );
        let k = (dt * 3.2).min(1.0);
        self.focus = self.focus + (want - self.focus) * k;
    }
}

/// HUD guides the game computed this frame.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Guides {
    /// The human's controlled player and the next one a switch would pick.
    pub active: Option<u8>,
    pub next: Option<u8>,
    /// The pass a button press would play now: from (ball) to (target).
    pub pass_lane: Option<(Vec3, Vec3)>,
    /// Shot preview: up to 10 points of the ball's path.
    pub arc: [Vec3; 10],
    pub arc_len: u8,
    /// Charge 0..1 of the kick button held (0 = none).
    pub charge: f32,
}

/// Which parts a player gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Detail {
    /// PC native: 11 parts.
    Full,
    /// Consoles near the camera: 7 parts.
    Near,
    /// Consoles far away: no arms (5 parts).
    Far,
}

impl Detail {
    /// Its bit in [`anim::Part::tiers`].
    pub fn tier(self) -> u8 {
        match self {
            Detail::Full => anim::tier::FULL,
            Detail::Near => anim::tier::NEAR,
            Detail::Far => anim::tier::FAR,
        }
    }

    pub fn mask(self) -> u16 {
        match self {
            Detail::Full => mask::ALL,
            Detail::Near => mask::LIMBS,
            Detail::Far => mask::NO_ARMS,
        }
    }
}

/// A frame's view volume for game-side culling (cheaper than letting the
/// engine cull: off-screen draws still cost T&L time on the N64).
pub(crate) struct Frustum {
    eye: Vec3,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
    tan_y: f32,
    tan_x: f32,
    hyp_x: f32,
    hyp_y: f32,
}

impl Frustum {
    pub(crate) fn new(cam: &CamState, aspect: f32) -> Frustum {
        let eye = cam.eye();
        let fwd = (cam.focus - eye).normalized();
        let right = Vec3::new(0.0, 1.0, 0.0).cross(fwd).normalized();
        let up = fwd.cross(right);
        let tan_y = trino_core::math3d::sin(cam.fov * 0.5) / trino_core::math3d::cos(cam.fov * 0.5);
        let tan_x = tan_y * aspect;
        let hyp_x = trino_core::math3d::sqrt(1.0 + tan_x * tan_x);
        let hyp_y = trino_core::math3d::sqrt(1.0 + tan_y * tan_y);
        Frustum {
            eye,
            fwd,
            right,
            up,
            tan_y,
            tan_x,
            hyp_x,
            hyp_y,
        }
    }

    /// A sphere at `p` (meters) of radius `r` may be on screen.
    fn sees(&self, p: Vec3, r: f32) -> bool {
        let d = p - self.eye;
        let z = d.dot(self.fwd);
        if z < -r {
            return false;
        }
        let x = d.dot(self.right).abs();
        let y = d.dot(self.up).abs();
        // Distance past each side plane: exact sphere-plane test.
        x - z * self.tan_x < r * self.hyp_x && y - z * self.tan_y < r * self.hyp_y
    }

    /// A ground rectangle (y = 0) may be on screen: exact for a flat tile,
    /// where a bounding sphere keeps tiles that only graze the frame's edge
    /// (and those, mostly off screen, don't fit the N64 RSP's clip band).
    fn sees_ground(&self, x0: f32, z0: f32, x1: f32, z1: f32) -> bool {
        let corners = [
            Vec3::new(x0, 0.0, z0),
            Vec3::new(x1, 0.0, z0),
            Vec3::new(x0, 0.0, z1),
            Vec3::new(x1, 0.0, z1),
        ];
        let rel: [(f32, f32, f32); 4] = core::array::from_fn(|k| {
            let d = corners[k] - self.eye;
            (d.dot(self.right), d.dot(self.up), d.dot(self.fwd))
        });
        // Outside one plane with all four corners: culled.
        let all = |f: &dyn Fn(&(f32, f32, f32)) -> bool| rel.iter().all(f);
        !(all(&|v| v.2 < 0.0)
            || all(&|v| v.0 > v.2 * self.tan_x)
            || all(&|v| -v.0 > v.2 * self.tan_x)
            || all(&|v| v.1 > v.2 * self.tan_y)
            || all(&|v| -v.1 > v.2 * self.tan_y))
    }

    fn depth(&self, p: Vec3) -> f32 {
        (p - self.eye).dot(self.fwd)
    }
}

/// `a × b` per channel (tints stacking, the engine's rounding).
pub fn mul(a: Color, b: Color) -> Color {
    let m = |x: u8, y: u8| ((x as u32 * y as u32 + 127) / 255) as u8;
    Color::rgba(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b), a.a)
}

pub(crate) fn plain(tint: Color) -> ModelParams {
    ModelParams::tinted(tint)
}

pub(crate) fn unlit(tint: Color) -> ModelParams {
    ModelParams {
        tint,
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    }
}

/// A placement matrix: `pos` (m) and a ground facing (unit fx, fz).
pub(crate) fn place(pos: Vec3, fx: f32, fz: f32, scale: f32) -> Mat34 {
    let s = scale * WORLD;
    Mat34 {
        m: [
            [fz * s, 0.0, fx * s, pos.x * WORLD],
            [0.0, s, 0.0, pos.y * WORLD],
            [-fx * s, 0.0, fz * s, pos.z * WORLD],
        ],
    }
}

/// The yaw of the four stands' sides: (facing x, facing z).
const NORTH: (f32, f32) = (0.0, 1.0);
const SOUTH: (f32, f32) = (0.0, -1.0);
const WEST: (f32, f32) = (1.0, 0.0);
const EAST: (f32, f32) = (-1.0, 0.0);

/// Draw the match (top screen). `aspect` is the picture's width / height.
pub fn draw_match(r: &mut dyn Renderer, m: &Match, v: &MatchView, aspect: f32) {
    let hifi = hifi(r, v.retro);
    r.begin_frame(v.sky.clear());
    r.set_camera(&v.cam.camera());
    // Declared every frame: renderer state outlives frames (AGENTS.md).
    r.set_light(&v.sky.light());
    set_ao(r, hifi);
    if hifi && v.rain {
        r.set_wetness(0.55);
    }
    let fr = Frustum::new(&v.cam, aspect);
    let env = v.sky.tint();
    draw_stadium(r, &fr, hifi, env, v.stadium);
    draw_people(r, m, v, &fr, hifi);
    draw_ball(r, m, v, &fr, hifi);
    draw_guides(r, m, v);
    if hifi {
        draw_effects(r, v);
    }
}

pub const S_RAIN: SpriteId = SpriteId::from_path("sprites/se_rain");
pub const S_GLOW: SpriteId = SpriteId::from_path("sprites/se_glow");
pub const S_CONFETTI: SpriteId = SpriteId::from_path("sprites/se_confetti");

/// The PC tier's extras: rain falling around the camera's focus, and the
/// floodlights' glow at night (billboards: z-tested, batched with the 3D).
fn draw_effects(r: &mut dyn Renderer, v: &MatchView) {
    if v.rain {
        let f = v.cam.focus;
        for i in 0..220u32 {
            // A fixed scatter per drop (no RNG: visual only), each falling
            // at 15 m/s through a 14 m column.
            let hx = (i.wrapping_mul(2_654_435_761) >> 8) as f32 / 16_777_216.0;
            let hz = (i.wrapping_mul(2_246_822_519) >> 8) as f32 / 16_777_216.0;
            let ph = (i.wrapping_mul(3_266_489_917) >> 8) as f32 / 16_777_216.0;
            let fall = v.time * 15.0 + ph * 14.0;
            let y = 14.0 - (fall - (fall / 14.0) as i32 as f32 * 14.0);
            let p = Vec3::new(f.x + (hx - 0.5) * 60.0, y, f.z + (hz - 0.5) * 44.0);
            r.draw_billboard(
                S_RAIN,
                p * WORLD,
                &BillboardParams {
                    size: Vec2::new(0.025 * WORLD, 0.55 * WORLD),
                    tint: Color::rgba(200, 215, 235, 150),
                    ..BillboardParams::DEFAULT
                },
            );
        }
    }
    if v.sky == Sky::Night {
        for (x, z) in [
            (-61.0f32, -44.0f32),
            (61.0, -44.0),
            (-61.0, 44.0),
            (61.0, 44.0),
        ] {
            r.draw_billboard(
                S_GLOW,
                Vec3::new(x - x.signum() * 0.2, 29.0, z - z.signum() * 0.4) * WORLD,
                &BillboardParams {
                    size: Vec2::new(14.0 * WORLD, 14.0 * WORLD),
                    tint: Color::rgba(255, 245, 210, 210),
                    ..BillboardParams::DEFAULT
                },
            );
        }
    }
}

pub(crate) fn draw_stadium(
    r: &mut dyn Renderer,
    fr: &Frustum,
    hifi: bool,
    env: Color,
    stadium: Stadium,
) {
    let (stands, pitch) = stadium.tints();
    let grass = mul(env, pitch);
    let seats = mul(env, stands);
    for ix in 0..6 {
        for iz in 0..4 {
            let c = Vec3::new(
                -58.5 + (ix as f32 + 0.5) * TILE_W,
                0.0,
                -40.0 + (iz as f32 + 0.5) * TILE_D,
            );
            let (x0, z0) = (-58.5 + ix as f32 * TILE_W, -40.0 + iz as f32 * TILE_D);
            if !fr.sees_ground(x0, z0, x0 + TILE_W, z0 + TILE_D) {
                continue;
            }
            if fr.depth(c) > FINE_DEPTH {
                r.draw_model_matrix(
                    M_PITCH[ix * 4 + iz],
                    &place(Vec3::ZERO, 0.0, 1.0, 1.0),
                    Material::VertexLit,
                    &plain(grass),
                );
                continue;
            }
            for (fx, fz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (gx, gz) = (ix * 2 + fx, iz * 2 + fz);
                let (fx0, fz0) = (
                    -58.5 + gx as f32 * TILE_W * 0.5,
                    -40.0 + gz as f32 * TILE_D * 0.5,
                );
                if fr.sees_ground(fx0, fz0, fx0 + TILE_W * 0.5, fz0 + TILE_D * 0.5) {
                    r.draw_model_matrix(
                        M_PITCH_FINE[gx * 8 + gz],
                        &place(Vec3::ZERO, 0.0, 1.0, 1.0),
                        Material::VertexLit,
                        &plain(grass),
                    );
                }
            }
        }
    }
    // Stands: six sections a side, four an end; two crowd variants
    // alternate.
    for i in stadium.side_sections() {
        let x = -48.75 + 19.5 * i as f32;
        for (z, (fx, fz)) in [(-41.5f32, NORTH), (41.5, SOUTH)] {
            let p = Vec3::new(x, 0.0, z);
            if fr.sees(p + Vec3::new(0.0, 5.0, 0.0), 14.0) {
                let mdl = if hifi {
                    M_STAND_LONG_HD[i % 2]
                } else {
                    M_STAND_LONG[i % 2]
                };
                r.draw_model_matrix(
                    mdl,
                    &place(p, fx, fz, 1.0),
                    Material::VertexLit,
                    &plain(seats),
                );
            }
        }
    }
    for i in 0..if stadium.end_stands() { 4 } else { 0 } {
        let z = -30.0 + 20.0 * i as f32;
        for (x, (fx, fz)) in [(-60.5f32, WEST), (60.5, EAST)] {
            let p = Vec3::new(x, 0.0, z);
            let c = p + Vec3::new(fx * -1.55, 4.0, fz * -1.55);
            let rad = if hifi { 12.5 } else { 11.4 };
            if fr.sees(c, rad) {
                let mdl = if hifi {
                    M_STAND_END_HD[i % 2]
                } else {
                    M_STAND_END[i % 2]
                };
                r.draw_model_matrix(
                    mdl,
                    &place(p, fx, fz, 1.0),
                    Material::VertexLit,
                    &plain(seats),
                );
            }
        }
    }
    for (x, z) in [
        (-61.0f32, -44.0f32),
        (61.0, -44.0),
        (-61.0, 44.0),
        (61.0, 44.0),
    ] {
        let p = Vec3::new(x, 0.0, z);
        if fr.sees(p + Vec3::new(0.0, 14.0, 0.0), 16.0) {
            let f = (-x.signum() * 0.6, -z.signum() * 0.8);
            r.draw_model_matrix(
                M_FLOOD,
                &place(p, f.0, f.1, 1.0),
                Material::VertexLit,
                &plain(env),
            );
        }
    }
    // Goals face the pitch; corner flags.
    for (x, (fx, fz)) in [(-HALF_LEN.to_f32(), WEST), (HALF_LEN.to_f32(), EAST)] {
        let p = Vec3::new(x, 0.0, 0.0);
        if fr.sees(p, 5.0) {
            let mdl = if hifi { M_GOAL_HD } else { M_GOAL };
            r.draw_model_matrix(
                mdl,
                &place(p, fx, fz, 1.0),
                Material::VertexLit,
                &plain(env),
            );
        }
        for z in [-HALF_WID.to_f32(), HALF_WID.to_f32()] {
            let q = Vec3::new(x, 0.0, z);
            if fr.sees(q, 1.5) {
                r.draw_model_matrix(
                    M_FLAG,
                    &place(q, 0.0, 1.0, 1.0),
                    Material::VertexLit,
                    &plain(env),
                );
            }
        }
    }
}

/// Players and officials, nearest first is not needed (z-buffer).
fn draw_people(r: &mut dyn Renderer, m: &Match, v: &MatchView, fr: &Frustum, hifi: bool) {
    let mut joints = [Mat34::IDENTITY; JOINTS];
    for i in 0..N {
        let p = &m.players[i];
        if p.off {
            continue;
        }
        let pos = Vec3::new(p.pos.x.to_f32(), 0.0, p.pos.y.to_f32());
        if !fr.sees(pos + Vec3::new(0.0, 0.9, 0.0), 1.3) {
            continue;
        }
        let detail = if hifi {
            Detail::Full
        } else if fr.depth(pos) > FAR_DEPTH {
            Detail::Far
        } else {
            Detail::Near
        };
        let (fx, fz) = (p.face.x.to_f32(), p.face.y.to_f32());
        let body = body_matrix(pos, fx, fz, WORLD);
        let m = detail.mask();
        let pose = v.anims[i].pose_masked(m);
        joint_matrices(&body, &pose, &mut joints, m);
        anim::plant(&mut joints, m, v.anims[i].grounded(), body.m[1][3]);
        draw_body(r, &body, &joints, &v.looks[i], detail, v.sky.tint());
    }
    // Officials: the referee in black, linesmen in yellow with flags.
    use crate::sim::team::OFFICIAL_BLACK as BLACK;
    let ref_look = Look::of(BLACK, BLACK, BLACK, SKIN_TONES[2]);
    let lm_look = Look::of(Rgb(250, 220, 40), BLACK, BLACK, SKIN_TONES[4]);
    for k in 0..3 {
        let o = m.officials[k];
        let pos = Vec3::new(o.x.to_f32(), 0.0, o.y.to_f32());
        if !fr.sees(pos + Vec3::new(0.0, 0.9, 0.0), 1.3) {
            continue;
        }
        // Officials face the ball.
        let b = m.ball.pos.ground();
        let d = (b - o).norm();
        let (fx, fz) = if d.x.0 == 0 && d.y.0 == 0 {
            (0.0, 1.0)
        } else {
            (d.x.to_f32(), d.y.to_f32())
        };
        let body = body_matrix(pos, fx, fz, WORLD);
        // The referee goes far like a player; a linesman keeps his arms
        // (his flag is in his hand).
        let detail = if hifi {
            Detail::Full
        } else if k == 0 && fr.depth(pos) > FAR_DEPTH {
            Detail::Far
        } else {
            Detail::Near
        };
        let pose = v.anims[N + k].pose_masked(detail.mask());
        joint_matrices(&body, &pose, &mut joints, detail.mask());
        anim::plant(
            &mut joints,
            detail.mask(),
            v.anims[N + k].grounded(),
            body.m[1][3],
        );
        draw_body(
            r,
            &body,
            &joints,
            if k == 0 { &ref_look } else { &lm_look },
            detail,
            v.sky.tint(),
        );
        if k > 0 {
            let hand = if hifi {
                joints[joint::FOREARM_R].mul(&Mat34::from_trs(
                    Vec3::new(0.0, -0.3, 0.0),
                    trino_core::Quat::IDENTITY,
                    Vec3::ONE,
                ))
            } else {
                joints[joint::ARM_R].mul(&Mat34::from_trs(
                    Vec3::new(0.0, -0.6, 0.0),
                    trino_core::Quat::IDENTITY,
                    Vec3::ONE,
                ))
            };
            r.draw_model_matrix(
                M_FLAG_HAND,
                &hand,
                Material::VertexLit,
                &plain(Color::WHITE),
            );
        }
    }
}

/// Draw a posed body: its blob shadow on the turf under the pelvis (the
/// turf is `body`'s height), then the parts of `detail`'s tier.
pub(crate) fn draw_body(
    r: &mut dyn Renderer,
    body: &Mat34,
    j: &[Mat34; JOINTS],
    look: &Look,
    detail: Detail,
    env: Color,
) {
    if detail != Detail::Far {
        let pelvis = &j[joint::PELVIS];
        let mut shadow = *body;
        shadow.m[0][3] = pelvis.m[0][3];
        shadow.m[2][3] = pelvis.m[2][3];
        shadow.m[1][3] = body.m[1][3] + SHADOW_LIFT * body.m[1][1];
        r.draw_model_matrix(M_PLAYER_SHADOW, &shadow, Material::VertexLit, &plain(env));
    }
    let colors = [
        plain(mul(look.skin, env)),
        plain(mul(look.shirt, env)),
        plain(mul(look.shorts, env)),
        plain(mul(look.socks, env)),
        plain(mul(look.hair.color, env)),
    ];
    let bit = detail.tier();
    // Consoles draw a leg as one mesh with the kit's colors baked in.
    let kit_legs = detail != Detail::Full && (look.legs as usize) < M_LEG_KIT.len();
    for p in clips_gen::PARTS.iter() {
        if p.tiers & bit == 0 {
            continue;
        }
        let leg = p.joint == joint::THIGH_L || p.joint == joint::THIGH_R;
        if kit_legs && leg {
            continue;
        }
        // Consoles bake the hair into the head: one of three heads.
        let model = if p.model == M_HEAD {
            M_CONSOLE_HEADS[look.hair.console_head()]
        } else {
            p.model
        };
        r.draw_model_matrix(
            model,
            &j[p.joint],
            Material::VertexLit,
            &colors[p.tint as usize],
        );
    }
    if detail == Detail::Full
        && let Some(h) = look.hair.style.part()
    {
        r.draw_model_matrix(
            h.model,
            &j[h.joint],
            Material::VertexLit,
            &colors[Tint::Hair as usize],
        );
    }
    if kit_legs {
        let m = M_LEG_KIT[look.legs as usize];
        for t in [joint::THIGH_L, joint::THIGH_R] {
            r.draw_model_matrix(m, &j[t], Material::VertexLit, &plain(env));
        }
    }
}

/// The consoles' kit legs (`models/se_leg_k<NN>`, by
/// [`crate::sim::team::LEG_KITS`] index).
pub const M_LEG_KIT: [ModelId; crate::sim::team::LEG_KITS.1] = {
    let mut out = [ModelId(0); crate::sim::team::LEG_KITS.1];
    let mut i = 0;
    while i < out.len() {
        let mut path = [0u8; 24];
        let head = b"models/se_leg_k";
        let mut len = 0;
        while len < head.len() {
            path[len] = head[len];
            len += 1;
        }
        path[len] = b'0' + (i / 10) as u8;
        path[len + 1] = b'0' + (i % 10) as u8;
        out[i] = ModelId(fnv1a(&path, len + 2));
        i += 1;
    }
    out
};

/// The ball speed (m/s, along the turf) from which a shot plays the
/// power clip: shots leave at 17..33 m/s.
pub const POWER_SHOT: f32 = 30.0;

/// Beyond this view depth (m) a console player is drawn far: no arms, no
/// blob shadow (each RSP draw costs the N64 about 0.2 ms of CPU; at
/// 320x240 a far player's arms and shadow are a pixel or two).
pub const FAR_DEPTH: f32 = 24.0;

/// How high the shadow floats over the turf (m): the N64's 16-bit depth
/// resolves 4 cm at the drawing scale.
const SHADOW_LIFT: f32 = 0.04;

fn draw_ball(r: &mut dyn Renderer, m: &Match, v: &MatchView, fr: &Frustum, hifi: bool) {
    let b = m.ball.pos;
    let p = Vec3::new(b.x.to_f32(), b.y.to_f32(), b.z.to_f32());
    if !fr.sees(p, 0.5) {
        return;
    }
    let rot = Mat34::from_trs(p * WORLD, v.ball_rot, Vec3::new(WORLD, WORLD, WORLD));
    let mdl = if hifi { M_BALL_HD } else { M_BALL };
    r.draw_model_matrix(mdl, &rot, Material::VertexLit, &plain(v.sky.tint()));
    // Its shadow: a flattened dark bead on the turf.
    let shadow = Mat34 {
        m: [
            [1.6 * WORLD, 0.0, 0.0, p.x * WORLD],
            [0.0, 0.05 * WORLD, 0.0, 0.035 * WORLD],
            [0.0, 0.0, 1.6 * WORLD, p.z * WORLD],
        ],
    };
    r.draw_model_matrix(
        M_DOT,
        &shadow,
        Material::VertexLit,
        &unlit(Color::rgb(34, 66, 28)),
    );
}

/// The 3D markers: active cursor, switch target, P1 arrow, pass lane, shot arc.
fn draw_guides(r: &mut dyn Renderer, m: &Match, v: &MatchView) {
    let g = &v.guides;
    let pulse = 1.0 + 0.08 * trino_core::math3d::sin(v.time * 6.0);
    if let Some(a) = g.active {
        let p = &m.players[a as usize];
        let pos = Vec3::new(p.pos.x.to_f32(), 0.04, p.pos.y.to_f32());
        r.draw_model_matrix(
            M_RING,
            &place(pos, 0.0, 1.0, pulse),
            Material::VertexLit,
            &unlit(Color::rgb(40, 220, 255)),
        );
        let bob = 0.04 * trino_core::math3d::sin(v.time * 4.0);
        let top = Vec3::new(pos.x, 1.75 + bob, pos.z);
        r.draw_model_matrix(
            M_ARROW,
            &place(top, 0.0, 1.0, 1.0),
            Material::VertexLit,
            &unlit(Color::rgb(40, 220, 255)),
        );
    }
    if let Some(n) = g.next {
        let p = &m.players[n as usize];
        let pos = Vec3::new(p.pos.x.to_f32(), 0.04, p.pos.y.to_f32());
        r.draw_model_matrix(
            M_RING_SMALL,
            &place(pos, 0.0, 1.0, 1.0),
            Material::VertexLit,
            &unlit(Color::rgb(120, 230, 90)),
        );
    }
    if let Some((from, to)) = g.pass_lane {
        let d = to - from;
        let len = trino_core::math3d::sqrt(d.x * d.x + d.z * d.z);
        if len > 1.0 {
            let (fx, fz) = (d.x / len, d.z / len);
            let n = ((len / 1.7) as usize).min(14);
            for k in 1..n {
                let t = k as f32 / n as f32;
                let at = Vec3::new(from.x + d.x * t, 0.05, from.z + d.z * t);
                r.draw_model_matrix(
                    M_DASH,
                    &place(at, fx, fz, 1.0),
                    Material::VertexLit,
                    &unlit(Color::rgb(235, 255, 235)),
                );
            }
        }
    }
    for k in 0..g.arc_len as usize {
        let t = k as f32 / 10.0;
        let c = Color::rgb(255, (230.0 - 170.0 * t) as u8, (60.0 - 40.0 * t) as u8);
        r.draw_model_matrix(
            M_DOT,
            &place(g.arc[k], 0.0, 1.0, 1.0),
            Material::VertexLit,
            &unlit(c),
        );
    }
    let _ = (
        SIDE,
        side_of(0),
        Role::Gk,
        Fx::ZERO,
        Act::Free,
        Phase::Play,
        CLUBS.len(),
    );
}

/// The margin around each locomotion speed edge (m/s).
const HYST: f32 = 0.35;

/// The animation a player's sim state calls for, and how fast it plays.
pub fn pick_clip(m: &Match, i: usize, dt: f32, st: &mut AnimState) {
    let p = &m.players[i];
    // The gait follows a smoothed speed (a fifth of a second): the sim's
    // small steps and stops would flip walk and stand every few frames.
    let raw = p.vel.len().to_f32();
    st.speed += (raw - st.speed) * (dt / 0.2).min(1.0);
    let speed = st.speed;
    let owner = m.owner == Some(i as u8);
    let keeper = p.role == Role::Gk;
    // Cues the sim's state alone doesn't hold: a header just made, a
    // throw-in waiting to be taken.
    let headed = p.headed != 0 && m.tick.saturating_sub(p.headed) < 26;
    if headed {
        show(st, ClipName::Header, false, false, 1.0, dt);
        return;
    }
    let throwing = matches!(m.phase, Phase::Ready { .. } | Phase::Setup { .. })
        && m.taker as usize == i
        && m.restart.kind == crate::sim::rules::RestartKind::ThrowIn;
    if throwing {
        // The wind-up, held until the throw.
        show(st, ClipName::Throw, false, false, 0.0, dt);
        if st.clip == ClipName::Throw {
            st.time = 0.0;
        }
        return;
    }
    let (clip, mirror, rate, restart) = match p.act {
        Act::Kick { plan, t } => {
            let c = match plan.kind {
                // A full-power strike (the fastest fifth of shots) swings
                // through harder than a placed one.
                crate::sim::engine::KickKind::Shot => {
                    let (vx, vz) = (plan.vel.x.to_f32(), plan.vel.z.to_f32());
                    if vx * vx + vz * vz >= POWER_SHOT * POWER_SHOT {
                        ClipName::PowerShot
                    } else {
                        ClipName::Shot
                    }
                }
                crate::sim::engine::KickKind::Throw => ClipName::Throw,
                crate::sim::engine::KickKind::Cross
                | crate::sim::engine::KickKind::Through
                | crate::sim::engine::KickKind::Clear => {
                    if plan.vel.y > Fx::int(3) {
                        ClipName::Lob
                    } else {
                        ClipName::Pass
                    }
                }
                _ => ClipName::Pass,
            };
            let _ = t;
            (c, false, 1.0, false)
        }
        Act::Tackle { .. } => (ClipName::Tackle, false, 1.0, false),
        Act::Slide { .. } => (ClipName::Slide, false, 1.0, false),
        Act::Down { .. } => (ClipName::Down, false, 1.0, false),
        Act::Dive { vel, .. } => {
            // Diving to his left (+x of the model) plays the clip as made.
            let left = p.face.perp().scale(-Fx::ONE);
            let to_left = vel.dot(left) > Fx::ZERO;
            (ClipName::GkDive, !to_left, 1.0, false)
        }
        Act::Hold { .. } => (ClipName::GkHold, false, 1.0, false),
        Act::Celebrate => (ClipName::Celebrate, false, 1.0, false),
        Act::Free => {
            // Speed bands with a margin around each edge, so a speed
            // hovering at one doesn't flip the clip every frame.
            let cur = st.clip;
            let moving = !matches!(cur, ClipName::Idle | ClipName::GkReady);
            let edge = |at: f32, above: bool| if above { at - HYST } else { at + HYST };
            let walk_from = edge(0.6, moving);
            let jog_from = edge(2.6, matches!(cur, ClipName::Jog | ClipName::Sprint));
            let sprint_from = edge(6.2, cur == ClipName::Sprint);
            if matches!(m.phase, Phase::Goal { side, .. } if side as usize == side_of(i))
                && speed < 1.5
            {
                (ClipName::Celebrate, i.is_multiple_of(2), 1.0, false)
            } else if speed < walk_from {
                if keeper {
                    (ClipName::GkReady, false, 1.0, false)
                } else {
                    (ClipName::Idle, false, 1.0, false)
                }
            } else if owner {
                (
                    ClipName::Dribble,
                    false,
                    (speed / 6.0).clamp(0.6, 1.4),
                    false,
                )
            } else if speed < jog_from {
                (ClipName::Walk, false, (speed / 1.6).clamp(0.6, 1.5), false)
            } else if speed < sprint_from {
                (ClipName::Jog, false, (speed / 4.6).clamp(0.7, 1.4), false)
            } else {
                (
                    ClipName::Sprint,
                    false,
                    (speed / 7.6).clamp(0.8, 1.3),
                    false,
                )
            }
        }
    };
    show(st, clip, mirror, restart, rate, dt);
}

/// Play `clip` unless the body must get up first ([`anim::route`]).
fn show(st: &mut AnimState, clip: ClipName, mirror: bool, restart: bool, rate: f32, dt: f32) {
    let shown = anim::route(st, clip);
    if shown == clip {
        anim::switch(st, clip, mirror, restart);
        st.advance(dt, rate);
    } else {
        anim::switch(st, shown, st.mirror, false);
        st.advance(dt, 1.0);
    }
}

/// The officials' clips: the referee walks or jogs with the play; a
/// linesman raises his flag while `m.flag` says so.
pub fn pick_official_clip(m: &Match, k: usize, prev: (f32, f32), dt: f32, st: &mut AnimState) {
    let o = m.officials[k];
    // His speed from his positions, smoothed over 0.4 s (he
    // steps toward a target that jumps with the ball).
    let (dx, dz) = (o.x.to_f32() - prev.0, o.y.to_f32() - prev.1);
    let dist = trino_core::math3d::sqrt(dx * dx + dz * dz);
    let raw = dist / dt.max(1e-3);
    st.speed += (raw - st.speed) * (dt / 0.4).min(1.0);
    let speed = st.speed;
    // Officials face the ball. Check if the referee is backing away from play.
    let b = m.ball.pos.ground();
    let d = (b - o).norm();
    let (fx, fz) = (d.x.to_f32(), d.y.to_f32());
    let dot = if dist > 1e-4 {
        (dx * fx + dz * fz) / dist
    } else {
        1.0
    };
    let backing = k == 0 && dot < -0.15;
    // The referee shows a card for two seconds after one, and points the
    // restart after a foul while the ball is dead.
    let recent = |kinds: &[EventKind], within: u32| {
        m.events_since(m.event_seq.saturating_sub(4))
            .any(|(_, e)| kinds.contains(&e.kind) && m.tick.saturating_sub(e.tick) < within)
    };
    let clip = if k > 0 && m.flag.0 as usize == k && m.flag.1 > 0 {
        ClipName::FlagUp
    } else if k == 0 && recent(&[EventKind::Yellow, EventKind::Red], 120) {
        ClipName::RefCard
    } else if k == 0
        && matches!(m.phase, Phase::Dead { .. } | Phase::Setup { .. })
        && recent(
            &[EventKind::Foul, EventKind::FreeKick, EventKind::Penalty],
            150,
        )
    {
        ClipName::RefPoint
    } else if speed > 3.0 + if st.clip == ClipName::Jog { -0.6 } else { 0.6 } {
        ClipName::Jog
    } else if speed > 0.6 + if st.clip == ClipName::Idle { 0.4 } else { -0.4 } {
        ClipName::Walk
    } else {
        ClipName::Idle
    };
    st.play(clip, false, false);
    let rate = if backing {
        -(speed / 1.5).clamp(0.7, 1.4)
    } else if matches!(clip, ClipName::Walk | ClipName::Jog) {
        (speed / if clip == ClipName::Jog { 4.0 } else { 1.5 }).clamp(0.6, 1.4)
    } else {
        1.0
    };
    st.advance(dt, rate);
}

/// Keep the unused-import list honest for the pose type.
pub type PoseOf = Pose;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fine_tile_handles_match_their_paths() {
        assert_eq!(M_PITCH_FINE[0], ModelId::from_path("models/se_pitch_f_0_0"));
        assert_eq!(
            M_PITCH_FINE[5 * 8 + 3],
            ModelId::from_path("models/se_pitch_f_5_3")
        );
        assert_eq!(
            M_PITCH_FINE[11 * 8 + 7],
            ModelId::from_path("models/se_pitch_f_11_7")
        );
    }

    /// The sim keeps a player `Down` after a slide or a dive, longer than
    /// the clip: the body plays the fall out and gets up all the way, never
    /// the fouled-fall clip in between, and runs only once up.
    #[test]
    fn a_slide_or_a_dive_ends_in_one_whole_get_up() {
        let dt = 1.0 / 60.0;
        for fall in [ClipName::Slide, ClipName::GkDive] {
            let mut st = anim::AnimState::new(ClipName::Jog);
            let mut seen = std::vec::Vec::new();
            // The fall, then the sim's `Down`, then free to run.
            let plan = [(fall, 50), (ClipName::Down, 24), (ClipName::Jog, 90)];
            for (want, ticks) in plan {
                for _ in 0..ticks {
                    show(&mut st, want, false, false, 1.0, dt);
                    if seen.last() != Some(&st.clip) {
                        seen.push(st.clip);
                    }
                }
            }
            assert_eq!(
                seen,
                [fall, ClipName::GetUp, ClipName::Jog],
                "{fall:?}: clips shown in turn"
            );
        }
    }

    /// The camera's `target - eye` doesn't move by a bit while it pans (the
    /// N64 draw memo's key).
    #[test]
    fn a_panning_camera_keeps_its_rotation_bit_exact() {
        let mut c = CamState::BROADCAST;
        let base = c.camera();
        let d0 = base.target - base.eye;
        for k in 0..200 {
            c.focus = Vec3::new(k as f32 * 0.173 - 17.0, 0.0, (k as f32 * 0.071).min(9.0));
            let cam = c.camera();
            let d = cam.target - cam.eye;
            assert_eq!(
                (d.x.to_bits(), d.y.to_bits(), d.z.to_bits()),
                (d0.x.to_bits(), d0.y.to_bits(), d0.z.to_bits())
            );
        }
    }
}
