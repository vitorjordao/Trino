//! The match simulation: pure, deterministic, fixed point. No text, no
//! rendering, no platform: `xtask/tests` guards that this module never
//! imports `trino_core` (its types are floats and handles).

pub mod ai;
pub mod ball;
pub mod engine;
pub mod fx;
pub mod pitch;
pub mod rules;
pub mod team;
#[cfg(test)]
mod tests;
