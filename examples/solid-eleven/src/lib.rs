#![cfg_attr(any(target_os = "none", target_os = "horizon"), no_std)]
// Players, commands and per-side tables are parallel fixed-size arrays
// indexed by player or side number: index loops read better than zips.
#![allow(clippy::needless_range_loop)]

pub mod sim;
