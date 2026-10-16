//! # Solid Eleven
//!
//! A low-poly 3D football game for Trino (issue #39), on PC, Nintendo 64
//! and Nintendo 3DS from one crate, and the engine's `football` project
//! template (`cargo xtask new <name> --standalone <dir> --template
//! football`).
//!
//! The crate is four layers, one module each (a game crate may depend only
//! on `trino-core`, so they are modules, not crates; `xtask/tests/
//! solid_eleven_layers.rs` keeps them apart):
//!
//! - [`sim`]: the match and the league as pure, deterministic fixed-point
//!   code — rules, ball physics, tactical AI, attributes, economy. No text,
//!   no rendering, no platform: it never imports `trino_core`.
//! - [`game`]: screens, controls, sessions, career flow.
//! - [`render`]: the 3D match (stadium, posed players, markers) and HUD.
//! - [`ui`]: the immediate-mode interface and the strings (EN / PT-BR).

#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]
// Players, commands and per-side tables are parallel fixed-size arrays
// indexed by player or side number: index loops read better than zips.
#![allow(clippy::needless_range_loop)]
// Draw calls and match setups take their context as plain arguments.
#![allow(clippy::too_many_arguments)]

pub mod game;
pub mod render;
pub mod selftest;
pub mod sim;
pub mod ui;

use game::session::{Session, dress};
use render::Sky;
use sim::engine::{N, SIDE, Setup, SideSetup};
use sim::team::{self, CLUBS};
use trino_core::{Audio, Game, InputState, Platform, Renderer, Vec2};
use ui::i18n::Lang;

/// A quick match between two clubs with their generated squads.
pub fn quick_setup(
    home: usize,
    away: usize,
    seed: u32,
    half_secs: u32,
    humans: [bool; 2],
) -> (Setup, [u8; N], [u8; N]) {
    let mut names = [0u8; N];
    let mut skins = [0u8; N];
    let side = |c: usize, names: &mut [u8; N], skins: &mut [u8; N], s: usize| SideSetup {
        club: c as u8,
        tactics: CLUBS[c].tactics,
        players: core::array::from_fn(|k| {
            let p = team::generate_player(c, k);
            names[s * SIDE + k] = p.surname;
            skins[s * SIDE + k] = p.skin;
            ((c * team::SQUAD + k) as u16, p.number, p.attrs)
        }),
        morale: 50,
    };
    let a = side(home, &mut names, &mut skins, 0);
    let b = side(away, &mut names, &mut skins, 1);
    (
        Setup {
            sides: [a, b],
            half_ticks: half_secs * 60,
            seed,
            wet: false,
            humans,
            difficulty: 1,
        },
        names,
        skins,
    )
}

/// The game.
pub struct SolidElevenGame {
    pub screen: Vec2,
    pub session: Session,
    pub lang: Lang,
    prev: InputState,
}

impl SolidElevenGame {
    pub fn new(screen: Vec2) -> SolidElevenGame {
        SolidElevenGame::exhibition(screen, 0, 1, 0x5E11, [true, false], Sky::Day)
    }

    /// A match between clubs `home` and `away` (indices into
    /// [`CLUBS`]), straight to kick-off: tests, perf scenes, screenshots.
    pub fn exhibition(
        screen: Vec2,
        home: usize,
        away: usize,
        seed: u32,
        humans: [bool; 2],
        sky: Sky,
    ) -> SolidElevenGame {
        let (setup, names, skins) = quick_setup(home, away, seed, 180, humans);
        let d = dress([setup.sides[0].club, setup.sides[1].club], &skins);
        SolidElevenGame {
            screen,
            session: Session::new(&setup, &d, names, sky),
            lang: Lang::En,
            prev: InputState::default(),
        }
    }
}

impl Game for SolidElevenGame {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        self.session.update(input, &self.prev, audio, dt);
        self.prev = *input;
    }

    fn update_with(
        &mut self,
        platform: &mut dyn Platform,
        input: &InputState,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        let _ = platform;
        self.update(input, audio, dt);
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        self.session.render(renderer, self.screen, self.lang);
        renderer.end_frame();
    }

    fn resize(&mut self, screen: Vec2) {
        self.screen = screen;
    }
}

// Hot-reload exports (trino_game_update / trino_game_render / version).
trino_game_api::export_game!(SolidElevenGame);
