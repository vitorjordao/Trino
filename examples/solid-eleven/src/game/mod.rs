//! The game layer: screens, the human's controls, sessions of play, the
//! career. Reads and drives [`crate::sim`]; draws through
//! [`crate::render`] and [`crate::ui`].

pub mod app;
#[cfg(test)]
mod e2e;
pub mod input;
pub mod menu;
pub mod session;
pub mod sound;
