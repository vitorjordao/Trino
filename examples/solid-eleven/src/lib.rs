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

pub use game::app::SolidElevenGame;
use sim::engine::{N, SIDE, Setup, SideSetup};
use sim::team::{self, Appearance, CLUBS};

/// A quick match between two clubs with their generated squads.
pub fn quick_setup(
    home: usize,
    away: usize,
    seed: u32,
    half_secs: u32,
    humans: [bool; 2],
) -> (Setup, [u8; N], [Appearance; N]) {
    let mut names = [0u8; N];
    let mut looks = [Appearance::default(); N];
    let side = |c: usize, names: &mut [u8; N], looks: &mut [Appearance; N], s: usize| SideSetup {
        club: c as u8,
        tactics: CLUBS[c].tactics,
        players: core::array::from_fn(|k| {
            let p = team::generate_player(c, k);
            names[s * SIDE + k] = p.surname;
            looks[s * SIDE + k] = p.appearance();
            ((c * team::SQUAD + k) as u16, p.number, p.attrs)
        }),
        morale: 50,
        bench: core::array::from_fn(|b| {
            let p = team::generate_player(c, SIDE + b);
            (
                (c * team::SQUAD + SIDE + b) as u16,
                p.number,
                p.attrs,
                p.pos == team::Pos::Gk,
            )
        }),
        bench_n: 5,
    };
    let a = side(home, &mut names, &mut looks, 0);
    let b = side(away, &mut names, &mut looks, 1);
    (
        Setup {
            sides: [a, b],
            half_ticks: half_secs * 60,
            seed,
            wet: false,
            humans,
            keeper_ctl: [false, false],
            difficulty: 1,
        },
        names,
        looks,
    )
}

// Hot-reload exports (trino_game_update / trino_game_render / version),
// behind the `hot-reload` feature (see Cargo.toml).
#[cfg(feature = "hot-reload")]
trino_game_api::export_game!(SolidElevenGame);
