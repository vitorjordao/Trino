//! A network session's settings: `trino.toml`'s `[net]` (baked into the
//! `net/config` blob as `key=value` lines) under the host's `TRINO_NET*`
//! variables.
//!
//! ```toml
//! [players]
//! max = 4            # local seats (Platform::player_setup)
//! [net]
//! mode = "rollback"  # or "delay": lockstep, no prediction
//! max_rollback = 8   # frames a late input may roll back
//! input_delay = 2    # frames each local input waits (hides latency)
//! keys = false       # share the PC keyboard (issue #117)
//! pointer = false    # share the pointer, its buttons and the wheel
//! pointer_space = "320x240"  # the units a shared pointer arrives in
//! ```

/// How a session hides latency.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetMode {
    /// Predict late inputs and roll back when a prediction was wrong
    /// (GGPO). Needs `save_state`/`load_state`.
    Rollback,
    /// Wait for every input (lockstep with an input delay): no snapshots,
    /// the fallback for a sim too heavy to simulate again.
    Delay,
}

impl NetMode {
    pub fn parse(s: &str) -> Option<NetMode> {
        match s.trim().trim_matches('"') {
            "rollback" => Some(NetMode::Rollback),
            "delay" | "lockstep" => Some(NetMode::Delay),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            NetMode::Rollback => "rollback",
            NetMode::Delay => "delay",
        }
    }
}

/// A session's settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NetConfig {
    pub mode: NetMode,
    /// Frames a late input may roll the game back (the prediction window);
    /// 0 in [`NetMode::Delay`].
    pub max_rollback: u8,
    /// Frames each local input is held back before it plays.
    pub input_delay: u8,
    /// Players in the session (2..=4).
    pub players: u8,
    /// Frames per second of the simulation (`dt` = 1 / fps).
    pub fps: u8,
    /// Exchange state hashes every this many frames (0: never) and report
    /// a desync on the first that differs.
    pub desync_every: u16,
    /// A peer silent this long is dropped (its seat reads disconnected).
    pub disconnect_ms: u32,
    /// Share the keyboard and the pointer too (issue #117; the pad always
    /// crosses).
    pub extras: trino_core::NetExtras,
    /// The units a shared pointer arrives in on every peer (a fraction of
    /// the sender's screen times this).
    pub pointer_space: (u16, u16),
}

impl NetConfig {
    pub const DEFAULT: NetConfig = NetConfig {
        mode: NetMode::Rollback,
        max_rollback: 8,
        input_delay: 2,
        players: 2,
        fps: 60,
        desync_every: 10,
        disconnect_ms: 3000,
        extras: trino_core::NetExtras::PAD,
        pointer_space: (320, 240),
    };

    /// The prediction window the session runs with (0 for delay mode).
    pub fn prediction(&self) -> usize {
        match self.mode {
            NetMode::Rollback => self.max_rollback.max(1) as usize,
            NetMode::Delay => 0,
        }
    }

    /// Read `key = value` lines (the baked `net/config`, a `TRINO_NET_*`
    /// override): `mode`, `max_rollback`, `input_delay`, `players`, `fps`,
    /// `desync_every`, `disconnect_ms`. Unknown keys and bad values keep
    /// the current setting; `#` starts a comment.
    pub fn apply_text(&mut self, text: &str) -> Vec<String> {
        let mut problems = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=').or_else(|| line.split_once(':')) else {
                problems.push(format!("`{line}`: not key = value"));
                continue;
            };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            let num = |lo: u32, hi: u32| v.parse::<u32>().ok().filter(|n| (lo..=hi).contains(n));
            let flag = || match v {
                "true" | "on" | "1" => Some(true),
                "false" | "off" | "0" => Some(false),
                _ => None,
            };
            let ok = match k {
                "mode" => NetMode::parse(v).map(|m| self.mode = m).is_some(),
                "max_rollback" => num(1, 16).map(|n| self.max_rollback = n as u8).is_some(),
                "input_delay" => num(0, 10).map(|n| self.input_delay = n as u8).is_some(),
                "players" => num(2, 4).map(|n| self.players = n as u8).is_some(),
                "fps" => num(10, 120).map(|n| self.fps = n as u8).is_some(),
                "desync_every" => num(0, 600).map(|n| self.desync_every = n as u16).is_some(),
                "disconnect_ms" => num(500, 60_000).map(|n| self.disconnect_ms = n).is_some(),
                "keys" => flag().map(|b| self.extras.keys = b).is_some(),
                "pointer" => flag().map(|b| self.extras.pointer = b).is_some(),
                "pointer_space" => v
                    .split_once('x')
                    .and_then(|(w, h)| {
                        Some((w.trim().parse::<u16>().ok()?, h.trim().parse::<u16>().ok()?))
                    })
                    .filter(|(w, h)| *w > 0 && *h > 0)
                    .map(|wh| self.pointer_space = wh)
                    .is_some(),
                _ => {
                    problems.push(format!("`{k}`: unknown key"));
                    continue;
                }
            };
            if !ok {
                problems.push(format!("`{k} = {v}`: out of range"));
            }
        }
        problems
    }

    pub fn parse(text: &str) -> (NetConfig, Vec<String>) {
        let mut c = NetConfig::DEFAULT;
        let p = c.apply_text(text);
        (c, p)
    }

    /// The baked `net/config` blob (`trino.toml`'s `[net]`; its
    /// `max_players` line is the local seats' business and skipped).
    pub fn from_baked(bytes: Option<&[u8]>) -> (NetConfig, Vec<String>) {
        let text: String = bytes
            .and_then(|b| core::str::from_utf8(b).ok())
            .unwrap_or("")
            .lines()
            .filter(|l| !l.trim_start().starts_with("max_players"))
            .map(|l| format!("{l}\n"))
            .collect();
        NetConfig::parse(&text)
    }

    /// The `key = value` text [`apply_text`](Self::apply_text) reads.
    pub fn to_text(&self) -> String {
        format!(
            "mode = {}\nmax_rollback = {}\ninput_delay = {}\nplayers = {}\nfps = {}\n\
             desync_every = {}\ndisconnect_ms = {}\nkeys = {}\npointer = {}\n\
             pointer_space = {}x{}\n",
            self.mode.key(),
            self.max_rollback,
            self.input_delay,
            self.players,
            self.fps,
            self.desync_every,
            self.disconnect_ms,
            self.extras.keys,
            self.extras.pointer,
            self.pointer_space.0,
            self.pointer_space.1
        )
    }

    /// What every peer of a session must agree on, as 8 hex digits: the
    /// mode, the prediction window, the input delay, the players, the frame
    /// rate, the desync interval and the pointer space (issue #117). The
    /// lobby refuses a peer whose fingerprint differs from the room's, so a
    /// mismatch fails at the door instead of desyncing (ggrs itself checks
    /// none of it). `keys`/`pointer` are left out: receivers decode
    /// whatever a sender shares.
    pub fn fingerprint(&self) -> String {
        let text = format!(
            "{} {} {} {} {} {} {}x{}",
            self.mode.key(),
            self.prediction(),
            self.input_delay,
            self.players,
            self.fps,
            self.desync_every,
            self.pointer_space.0,
            self.pointer_space.1
        );
        format!("{:08x}", trino_core::asset_id(&text))
    }

    /// Seconds per frame.
    pub fn dt(&self) -> f32 {
        1.0 / self.fps.max(1) as f32
    }
}

impl Default for NetConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_bad_values_are_named() {
        let (c, p) = NetConfig::parse(
            "# [net]\nmode = \"delay\"\nmax_rollback = 4\ninput_delay=3\nplayers: 3\n",
        );
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(
            (c.mode, c.max_rollback, c.input_delay, c.players),
            (NetMode::Delay, 4, 3, 3)
        );
        assert_eq!(c.prediction(), 0);
        assert_eq!(NetConfig::parse(&c.to_text()).0, c);
        let (d, p) = NetConfig::parse("max_rollback = 99\nspeed = 2\nnonsense");
        assert_eq!(d.max_rollback, NetConfig::DEFAULT.max_rollback);
        assert_eq!(p.len(), 3, "{p:?}");
        let (e, p) = NetConfig::parse("keys = true\npointer = on\npointer_space = \"640x360\"\n");
        assert!(p.is_empty(), "{p:?}");
        assert!(e.extras.keys && e.extras.pointer && e.pointer_space == (640, 360));
        assert_eq!(NetConfig::parse(&e.to_text()).0, e);
        assert_eq!(NetConfig::parse("pointer_space = 0x9\n").1.len(), 1);
    }

    #[test]
    fn the_fingerprint_names_what_peers_must_share() {
        let a = NetConfig::DEFAULT;
        let shared = NetConfig {
            extras: trino_core::NetExtras {
                keys: true,
                pointer: true,
            },
            disconnect_ms: 9000,
            ..a
        };
        assert_eq!(a.fingerprint(), shared.fingerprint());
        for b in [
            NetConfig {
                input_delay: 3,
                ..a
            },
            NetConfig { players: 3, ..a },
            NetConfig {
                mode: NetMode::Delay,
                ..a
            },
            NetConfig {
                pointer_space: (640, 480),
                ..a
            },
        ] {
            assert_ne!(a.fingerprint(), b.fingerprint(), "{b:?}");
        }
        assert_eq!(a.fingerprint().len(), 8);
    }
}
