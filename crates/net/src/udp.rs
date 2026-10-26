//! UDP on the PC: a non-blocking socket ggrs sends its messages through
//! ([`UdpSocket`]), the raw datagrams the lobby's rendezvous needs on the
//! same port, and [`Conditioned`]: latency, jitter and loss added to any
//! socket on the wall clock (two local instances under the editor's Net
//! panel, `TRINO_NET_SIM`).

use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use ggrs::{Message, NonBlockingSocket};

use crate::session::PeerAddr;
use crate::sim::Conditions;

/// Bytes a datagram may have (ggrs's messages are far smaller).
const MAX_DATAGRAM: usize = 4096;

/// The first byte of every ggrs datagram, so the lobby's text lines on the
/// same port are told apart.
const GGRS_TAG: u8 = 0xA7;

/// The host's own member name in its room (a guest reads it as "the address
/// I sent my JOIN to").
pub const HOST: &str = "host";

/// A non-blocking UDP socket for a session (and its rendezvous).
pub struct UdpSocket {
    socket: std::net::UdpSocket,
    buf: Vec<u8>,
    /// Text datagrams that arrived while the session ran (a lobby's late
    /// reply), kept for [`recv_text`](Self::recv_text).
    text: VecDeque<(SocketAddr, String)>,
    /// A host's own room ([`host_room`](Self::host_room)): lobby lines are
    /// answered as they arrive, during the session too (a guest whose
    /// `START` was lost asks again).
    room: Option<crate::lobby::Rooms>,
}

impl UdpSocket {
    /// Bind `addr` (`0.0.0.0:7000`, or port 0 for any).
    pub fn bind(addr: impl ToSocketAddrs) -> std::io::Result<UdpSocket> {
        let socket = std::net::UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(UdpSocket {
            socket,
            buf: vec![0; MAX_DATAGRAM],
            text: VecDeque::new(),
            room: None,
        })
    }

    /// Serve room `code` for `players` players on this socket, this machine
    /// in seat 0 (`TRINO_NET=host:<port>`): guests join it as they would a
    /// lobby. Returns once it is full (after `timeout`, an error), with the
    /// guests' seats and addresses.
    pub fn host_room(
        &mut self,
        code: &str,
        players: usize,
        timeout: Duration,
    ) -> Result<Vec<(usize, SocketAddr)>, String> {
        let mut rooms = crate::lobby::Rooms::new();
        rooms.handle(HOST, &format!("JOIN {code} {players}"), "");
        self.room = Some(rooms);
        let t0 = Instant::now();
        loop {
            self.pump();
            while let Some((_, line)) = self.text.pop_front() {
                if let Some(start) = crate::lobby::parse_start(&line) {
                    return start
                        .members
                        .iter()
                        .filter(|(seat, _)| *seat != 0)
                        .map(|(seat, a)| {
                            a.parse()
                                .map(|a| (*seat, a))
                                .map_err(|_| format!("room: bad address `{a}`"))
                        })
                        .collect();
                }
            }
            if t0.elapsed() > timeout {
                return Err(format!("nobody joined room `{code}` in {timeout:?}"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// A text datagram (the lobby protocol).
    pub fn send_text(&self, to: SocketAddr, line: &str) -> std::io::Result<()> {
        self.socket.send_to(line.as_bytes(), to).map(|_| ())
    }

    /// The next text datagram, if one is waiting.
    pub fn recv_text(&mut self) -> Option<(SocketAddr, String)> {
        self.pump();
        self.text.pop_front()
    }

    /// Read what arrived: ggrs datagrams out, text kept.
    fn pump(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out = Vec::new();
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, from)) => {
                    let d = &self.buf[..n];
                    if d.first() == Some(&GGRS_TAG) {
                        out.push((from, d[1..].to_vec()));
                    } else if let Ok(t) = std::str::from_utf8(d) {
                        let t = t.to_string();
                        match &mut self.room {
                            Some(rooms) if t.starts_with(crate::lobby::UDP_PREFIX) => {
                                let who = from.to_string();
                                let prefix = crate::lobby::UDP_PREFIX;
                                for (to, line) in rooms.handle(&who, &t, prefix) {
                                    if to == HOST {
                                        // The host's own START: the room is full.
                                        self.text.push_back((from, line));
                                    } else if let Ok(a) = to.parse::<SocketAddr>() {
                                        let _ = self.socket.send_to(line.as_bytes(), a);
                                    }
                                }
                            }
                            _ => self.text.push_back((from, t)),
                        }
                    }
                }
                // Would block, or an ICMP error from a closed peer port
                // (Windows reports those on the next receive): read on.
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => continue,
            }
        }
        out
    }
}

impl NonBlockingSocket<PeerAddr> for UdpSocket {
    fn send_to(&mut self, msg: &Message, addr: &PeerAddr) {
        let PeerAddr::Udp(to) = addr else { return };
        let Ok(body) = bincode::serialize(msg) else {
            return;
        };
        let mut d = Vec::with_capacity(body.len() + 1);
        d.push(GGRS_TAG);
        d.extend_from_slice(&body);
        let _ = self.socket.send_to(&d, to);
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerAddr, Message)> {
        self.pump()
            .into_iter()
            .filter_map(|(from, body)| {
                let m = bincode::deserialize::<Message>(&body).ok()?;
                Some((PeerAddr::Udp(from), m))
            })
            .collect()
    }
}

/// Any socket with a worse network on its way in: each received message
/// waits its latency (plus jitter) on the wall clock, or is lost.
pub struct Conditioned<S> {
    inner: S,
    cond: Conditions,
    held: Vec<(Instant, PeerAddr, Message)>,
    rng: u64,
}

impl<S> Conditioned<S> {
    pub fn new(inner: S, cond: Conditions, seed: u64) -> Self {
        Conditioned {
            inner,
            cond,
            held: Vec::new(),
            rng: seed | 1,
        }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

impl<S: NonBlockingSocket<PeerAddr>> NonBlockingSocket<PeerAddr> for Conditioned<S> {
    fn send_to(&mut self, msg: &Message, addr: &PeerAddr) {
        self.inner.send_to(msg, addr);
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerAddr, Message)> {
        let now = Instant::now();
        for (from, m) in self.inner.receive_all_messages() {
            let lost = self.cond.loss > 0.0
                && ((self.next() >> 40) as f32 / (1u64 << 24) as f32) < self.cond.loss;
            if lost {
                continue;
            }
            let jitter = if self.cond.jitter_ms > 0 {
                self.next() % (self.cond.jitter_ms as u64 + 1)
            } else {
                0
            };
            let at = now + Duration::from_millis(self.cond.latency_ms as u64 + jitter);
            self.held.push((at, from, m));
        }
        let mut out = Vec::new();
        self.held.retain(|(at, from, m)| {
            if *at <= now {
                out.push((*from, m.clone()));
                false
            } else {
                true
            }
        });
        out
    }
}
