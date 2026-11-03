//! Local wireless between 3DS consoles (issue #117): libctru's `uds`
//! (NWM) through the shim, carrying the delay-based lockstep
//! ([`trino_core::lockstep`]) the host runs a whole session under.
//!
//! The session is asked for by a `/uds` file in the RomFS (`cargo xtask
//! run 3ds --uds host|join`, the football template's two-player builds):
//!
//! ```text
//! host 2 delay=4 hash_at=760
//! ```
//!
//! `host <players>` creates the network (this console is player 1), `join
//! <players>` scans for one and joins it (the seat follows the join order:
//! node 2 is player 2). `delay` is the input delay in frames (default 4);
//! `hash_at` logs `TRINO_NET_HASH:<frame>:<hash>` on that frame (the
//! two-console check: both must print the same).

/// What the `/uds` file asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UdsPlan {
    pub host: bool,
    pub players: u8,
    pub delay: u8,
    pub hash_at: Option<u32>,
}

impl UdsPlan {
    /// Parse the file's text; `None` when it is neither `host` nor `join`.
    pub fn parse(text: &str) -> Option<UdsPlan> {
        let mut w = text.split_whitespace();
        let host = match w.next()? {
            "host" => true,
            "join" => false,
            _ => return None,
        };
        let mut plan = UdsPlan {
            host,
            players: 2,
            delay: 4,
            hash_at: None,
        };
        for word in w {
            if let Some(v) = word.strip_prefix("delay=") {
                plan.delay = v.parse::<u8>().ok()?.clamp(1, 16);
            } else if let Some(v) = word.strip_prefix("hash_at=") {
                plan.hash_at = Some(v.parse().ok()?);
            } else {
                plan.players = word.parse::<u8>().ok()?.clamp(2, 4);
            }
        }
        Some(plan)
    }
}

#[cfg(target_os = "horizon")]
mod link {
    use crate::ffi;

    /// Start the service.
    pub fn init() -> Result<(), i32> {
        // SAFETY: plain shim call.
        match unsafe { ffi::trino_uds_init() } {
            r if r < 0 => Err(r),
            _ => Ok(()),
        }
    }

    /// Create the network for `players` consoles.
    pub fn host(players: u8) -> Result<(), i32> {
        // SAFETY: plain shim call.
        match unsafe { ffi::trino_uds_host(players as u32) } {
            r if r < 0 => Err(r),
            _ => Ok(()),
        }
    }

    /// One scan: joined (`true`) or nothing found yet.
    pub fn try_join() -> Result<bool, i32> {
        // SAFETY: plain shim call.
        match unsafe { ffi::trino_uds_join() } {
            r if r < 0 => Err(r),
            r => Ok(r == 1),
        }
    }

    /// This console's node and the nodes connected.
    pub fn status() -> (u16, u8) {
        // SAFETY: plain shim call.
        let s = unsafe { ffi::trino_uds_status() };
        if s <= 0 {
            return (0, 0);
        }
        ((s as u32 >> 16) as u16, (s & 0xff) as u8)
    }

    pub fn send(node: u16, bytes: &[u8]) {
        // SAFETY: the slice outlives the call (the shim copies it out).
        unsafe {
            ffi::trino_uds_send(node as u32, bytes.as_ptr(), bytes.len() as u32);
        }
    }

    /// The next datagram: its length and sender's node.
    pub fn recv(buf: &mut [u8]) -> Option<(usize, u16)> {
        let mut from = 0u16;
        // SAFETY: `buf` and `from` are valid for the call.
        let n = unsafe { ffi::trino_uds_recv(buf.as_mut_ptr(), buf.len() as u32, &mut from) };
        (n > 0).then_some((n as usize, from))
    }

    pub fn exit() {
        // SAFETY: plain shim call.
        unsafe { ffi::trino_uds_exit() }
    }
}

#[cfg(target_os = "horizon")]
pub use link::{exit, host, init, recv, send, status, try_join};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_parse() {
        assert_eq!(
            UdsPlan::parse("host 2 delay=6 hash_at=760\n"),
            Some(UdsPlan {
                host: true,
                players: 2,
                delay: 6,
                hash_at: Some(760)
            })
        );
        let j = UdsPlan::parse("join 4").unwrap();
        assert!(!j.host && j.players == 4 && j.delay == 4 && j.hash_at.is_none());
        assert_eq!(UdsPlan::parse("join 9").unwrap().players, 4);
        assert!(UdsPlan::parse("lobby").is_none());
        assert!(UdsPlan::parse("host delay=x").is_none());
        assert!(UdsPlan::parse("").is_none());
    }
}
