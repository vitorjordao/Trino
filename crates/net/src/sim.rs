//! An in-process network: every [`SimSocket`] of one [`SimNet`] reaches the
//! others through a queue that adds latency, jitter and loss, on a clock
//! the caller advances ([`SimNet::advance`]). Tests run thousands of frames
//! of a session over it in seconds and get the same packet fate from the
//! same seed; the editor's Net panel shows what a connection would do.

use std::cell::RefCell;
use std::rc::Rc;

use ggrs::{Message, NonBlockingSocket};

use crate::session::PeerAddr;

/// What the network does to each packet.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Conditions {
    /// One-way delay, milliseconds.
    pub latency_ms: u32,
    /// Extra delay up to this many milliseconds, random per packet (so
    /// packets also arrive out of order).
    pub jitter_ms: u32,
    /// The share of packets lost, 0..=1.
    pub loss: f32,
}

impl Conditions {
    /// A perfect network.
    pub const NONE: Conditions = Conditions {
        latency_ms: 0,
        jitter_ms: 0,
        loss: 0.0,
    };

    /// `latency=100,jitter=10,loss=5` (loss in percent): the editor's and
    /// `TRINO_NET_SIM`'s form.
    pub fn parse(text: &str) -> Option<Conditions> {
        let mut c = Conditions::NONE;
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=')?;
            let v = v.trim().trim_end_matches("ms").trim_end_matches('%');
            match k.trim() {
                "latency" => c.latency_ms = v.parse().ok()?,
                "jitter" => c.jitter_ms = v.parse().ok()?,
                "loss" => c.loss = (v.parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0),
                _ => return None,
            }
        }
        Some(c)
    }
}

struct Packet {
    at: u64,
    from: PeerAddr,
    to: PeerAddr,
    msg: Message,
}

struct Hub {
    now_ms: u64,
    cond: Conditions,
    rng: u64,
    queue: Vec<Packet>,
    sent: u64,
    lost: u64,
}

impl Hub {
    fn next(&mut self) -> u64 {
        // xorshift64*: the same fates from the same seed.
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The network all its sockets share.
#[derive(Clone)]
pub struct SimNet(Rc<RefCell<Hub>>);

impl SimNet {
    pub fn new(cond: Conditions, seed: u64) -> SimNet {
        SimNet(Rc::new(RefCell::new(Hub {
            now_ms: 0,
            cond,
            rng: seed | 1,
            queue: Vec::new(),
            sent: 0,
            lost: 0,
        })))
    }

    /// A socket at `addr` (any [`PeerAddr`]; tests use `PeerAddr::Sim`).
    pub fn socket(&self, addr: PeerAddr) -> SimSocket {
        SimSocket {
            hub: self.clone(),
            addr,
        }
    }

    /// Let `ms` milliseconds pass: packets due by then arrive.
    pub fn advance(&self, ms: u64) {
        self.0.borrow_mut().now_ms += ms;
    }

    pub fn set_conditions(&self, cond: Conditions) {
        self.0.borrow_mut().cond = cond;
    }

    /// Packets sent, and lost on the way.
    pub fn counts(&self) -> (u64, u64) {
        let h = self.0.borrow();
        (h.sent, h.lost)
    }
}

/// One endpoint of a [`SimNet`].
pub struct SimSocket {
    hub: SimNet,
    addr: PeerAddr,
}

impl NonBlockingSocket<PeerAddr> for SimSocket {
    fn send_to(&mut self, msg: &Message, addr: &PeerAddr) {
        let mut h = self.hub.0.borrow_mut();
        h.sent += 1;
        let loss = h.cond.loss;
        if loss > 0.0 && h.unit() < loss {
            h.lost += 1;
            return;
        }
        let jitter = h.cond.jitter_ms;
        let extra = if jitter > 0 {
            h.next() % (jitter as u64 + 1)
        } else {
            0
        };
        let at = h.now_ms + h.cond.latency_ms as u64 + extra;
        h.queue.push(Packet {
            at,
            from: self.addr,
            to: *addr,
            msg: msg.clone(),
        });
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerAddr, Message)> {
        let mut h = self.hub.0.borrow_mut();
        let now = h.now_ms;
        let me = self.addr;
        let mut out = Vec::new();
        h.queue.retain(|p| {
            if p.to == me && p.at <= now {
                out.push((p.from, p.msg.clone()));
                false
            } else {
                true
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_parse_like_the_editor_writes_them() {
        let c = Conditions::parse("latency=120ms, jitter=8, loss=5%").unwrap();
        assert_eq!((c.latency_ms, c.jitter_ms), (120, 8));
        assert!((c.loss - 0.05).abs() < 1e-6);
        assert_eq!(Conditions::parse(""), Some(Conditions::NONE));
        assert_eq!(Conditions::parse("speed=1"), None);
    }
}
