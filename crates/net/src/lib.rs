//! # trino-net
//!
//! Network play for Trino games (issue #67, ADR 0036). A game needs no
//! network code: it reads every player from
//! [`Platform::players`](trino_core::Platform::players), as it does for
//! local multiplayer, and implements
//! [`Game::save_state`](trino_core::Game::save_state) /
//! [`load_state`](trino_core::Game::load_state) /
//! [`state_hash`](trino_core::Game::state_hash). The host runs the whole
//! session — menus included — under a [`NetSession`]: each peer's local
//! input goes to the others, late inputs are predicted, and a wrong
//! prediction rolls the game back and simulates it again (GGPO's model,
//! through [`ggrs`]). Remote players sit in their seats of
//! [`PlayersInput`](trino_core::PlayersInput), so a game that takes two
//! local players plays online as is.
//!
//! - [`config`]: the session's settings (`trino.toml` `[net]`, the
//!   `TRINO_NET*` variables): rollback or delay-based lockstep, the
//!   rollback window, the input delay.
//! - [`session`]: [`NetSession`], one per peer; [`PeerAddr`] names peers on
//!   any transport.
//! - [`sim`]: an in-process network with latency, jitter and loss, on a
//!   clock the caller advances — the soak tests run 10 000 frames of it in
//!   seconds.
//! - `udp` (not on wasm): UDP sockets for the PC and a [`udp::Conditioned`]
//!   wrapper that adds latency and loss to a real socket (the editor's Net
//!   panel, two local instances).
//! - [`lobby`]: the rendezvous protocol (a room code; the PC's UDP
//!   addresses and the Web's WebRTC offers go through it).
//! - `rtc` (wasm only): WebRTC data channels as a socket.

pub mod config;
pub mod lobby;
pub mod session;
pub mod sim;

#[cfg(target_arch = "wasm32")]
pub mod rtc;
#[cfg(not(target_arch = "wasm32"))]
pub mod udp;

pub use config::{NetConfig, NetMode};
pub use session::{NetSession, NetState, NetStats, PeerAddr, SyncTest};
pub use sim::{Conditions, SimNet, SimSocket};
