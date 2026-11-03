//! A simulated Internet of NATs (issue #117): machines behind full-cone,
//! address-restricted, port-restricted or symmetric NATs (RFC 4787's
//! mapping and filtering behaviors), public machines, and a lobby server
//! (the same [`crate::lobby::serve_udp`] as `cargo xtask net lobby`), all
//! in one process. Each machine is a [`Datagrams`] endpoint, so a real
//! [`UdpSocket`](crate::udp::UdpSocket) meets and plays through it: the
//! tests prove which NAT pairs punch through and that the rest fall back
//! to the relay, without root, network namespaces or a second machine.
//!
//! Delivery is immediate and lossless; latency and loss come from wrapping
//! the session's socket in [`crate::udp::Conditioned`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use crate::lobby::Rooms;
use crate::udp::Datagrams;

/// How a NAT maps and filters (RFC 4787).
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum NatKind {
    /// No NAT: the machine has a public address.
    Open,
    /// One public port per private socket; anyone may send to it.
    FullCone,
    /// One public port per private socket; only addresses it sent to.
    Restricted,
    /// One public port per private socket; only address+ports it sent to.
    PortRestricted,
    /// A new public port per destination; only that destination replies.
    Symmetric,
}

impl NatKind {
    pub const ALL: [NatKind; 5] = [
        NatKind::Open,
        NatKind::FullCone,
        NatKind::Restricted,
        NatKind::PortRestricted,
        NatKind::Symmetric,
    ];

    pub fn name(self) -> &'static str {
        match self {
            NatKind::Open => "open",
            NatKind::FullCone => "full cone",
            NatKind::Restricted => "restricted",
            NatKind::PortRestricted => "port-restricted",
            NatKind::Symmetric => "symmetric",
        }
    }

    /// Can two machines behind `a` and `b` (different NATs) reach each
    /// other directly by hole punching? Only a symmetric NAT facing a
    /// port-restricted or symmetric one can't: the symmetric side's port
    /// toward the peer is one the peer never sent to.
    pub fn punches_with(self, b: NatKind) -> bool {
        use NatKind::*;
        !matches!(
            (self, b),
            (Symmetric, PortRestricted | Symmetric) | (PortRestricted, Symmetric)
        )
    }
}

struct Nat {
    kind: NatKind,
    public_ip: Ipv4Addr,
    /// Whether a machine behind it reaches another behind it through its
    /// public address.
    hairpin: bool,
    /// (private socket, destination or none) → public port.
    out: BTreeMap<(SocketAddr, Option<SocketAddr>), u16>,
    /// Public port → (private socket, the destination it was made for).
    back: BTreeMap<u16, (SocketAddr, Option<SocketAddr>)>,
    /// (private socket, destination) pairs sent to.
    sent: BTreeSet<(SocketAddr, SocketAddr)>,
    next_port: u16,
}

#[derive(Default)]
struct World {
    nats: Vec<Nat>,
    /// Private socket → its NAT (None: a public machine).
    hosts: BTreeMap<SocketAddr, Option<usize>>,
    inbox: BTreeMap<SocketAddr, VecDeque<(SocketAddr, Vec<u8>)>>,
    lobby: Option<(SocketAddr, Rooms)>,
    /// Datagrams the lobby relayed, and every datagram routed.
    relayed: u64,
    routed: u64,
    dropped: u64,
}

impl World {
    /// `from` (a private or public socket) sends `d` to `to`.
    fn send(&mut self, from: SocketAddr, to: SocketAddr, d: Vec<u8>) {
        self.routed += 1;
        let nat = self.hosts.get(&from).copied().flatten();
        // Same LAN: straight there, private addresses.
        if let Some(n) = nat
            && self.hosts.get(&to) == Some(&Some(n))
        {
            self.deliver(to, from, d);
            return;
        }
        let public = match nat {
            None => from,
            Some(n) => {
                let nat = &mut self.nats[n];
                let key = match nat.kind {
                    NatKind::Symmetric => (from, Some(to)),
                    _ => (from, None),
                };
                let port = match nat.out.get(&key) {
                    Some(p) => *p,
                    None => {
                        let p = nat.next_port;
                        nat.next_port += 1;
                        nat.out.insert(key, p);
                        nat.back.insert(p, key);
                        p
                    }
                };
                nat.sent.insert((from, to));
                SocketAddr::new(IpAddr::V4(nat.public_ip), port)
            }
        };
        if let Some(n) = nat
            && to.ip() == IpAddr::V4(self.nats[n].public_ip)
            && !self.nats[n].hairpin
        {
            self.dropped += 1;
            return;
        }
        self.arrive(public, to, d);
    }

    /// A datagram from public `from` reaches public `to`.
    fn arrive(&mut self, from: SocketAddr, to: SocketAddr, d: Vec<u8>) {
        if let Some((lobby, rooms)) = &mut self.lobby
            && *lobby == to
        {
            let lobby = *lobby;
            let relay = d.first() == Some(&crate::lobby::RELAY_TAG);
            let out = crate::lobby::serve_udp(rooms, &from.to_string(), &d);
            for (m, bytes) in out {
                if relay {
                    self.relayed += 1;
                }
                if let Ok(a) = m.parse::<SocketAddr>() {
                    self.arrive(lobby, a, bytes);
                }
            }
            return;
        }
        if self.hosts.get(&to) == Some(&None) {
            self.deliver(to, from, d);
            return;
        }
        let Some(n) = self
            .nats
            .iter()
            .position(|n| IpAddr::V4(n.public_ip) == to.ip())
        else {
            self.dropped += 1;
            return;
        };
        let nat = &self.nats[n];
        let Some(&(private, made_for)) = nat.back.get(&to.port()) else {
            self.dropped += 1;
            return;
        };
        let allowed = match nat.kind {
            NatKind::Open | NatKind::FullCone => true,
            NatKind::Restricted => nat
                .sent
                .iter()
                .any(|(p, d)| *p == private && d.ip() == from.ip()),
            NatKind::PortRestricted => nat.sent.contains(&(private, from)),
            NatKind::Symmetric => made_for == Some(from),
        };
        if allowed {
            self.deliver(private, from, d);
        } else {
            self.dropped += 1;
        }
    }

    fn deliver(&mut self, to: SocketAddr, from: SocketAddr, d: Vec<u8>) {
        self.inbox.entry(to).or_default().push_back((from, d));
    }
}

/// The simulated Internet.
#[derive(Clone, Default)]
pub struct Internet(Arc<Mutex<World>>);

/// What the network did (for reports).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct InternetStats {
    pub routed: u64,
    pub relayed: u64,
    pub dropped: u64,
}

impl Internet {
    pub fn new() -> Internet {
        Internet::default()
    }

    /// A lobby server on a public address.
    pub fn lobby(&self) -> SocketAddr {
        let a: SocketAddr = "198.51.100.1:7400".parse().expect("literal");
        if let Ok(mut w) = self.0.lock() {
            w.lobby = Some((a, Rooms::new()));
        }
        a
    }

    /// A NAT of `kind` (its index for [`machine`](Self::machine)).
    pub fn nat(&self, kind: NatKind, hairpin: bool) -> usize {
        let mut w = self.0.lock().expect("world");
        let i = w.nats.len();
        w.nats.push(Nat {
            kind,
            public_ip: Ipv4Addr::new(203, 0, 113, 1 + i as u8),
            hairpin,
            out: BTreeMap::new(),
            back: BTreeMap::new(),
            sent: BTreeSet::new(),
            next_port: 40_000,
        });
        i
    }

    /// A machine behind `nat` (`None`: a public machine), its socket on
    /// `port`.
    pub fn machine(&self, nat: Option<usize>, port: u16) -> SimDatagrams {
        let mut w = self.0.lock().expect("world");
        let n = w.hosts.len() as u8;
        let ip = match nat {
            Some(i) => Ipv4Addr::new(10, 0, i as u8, 2 + n),
            None => Ipv4Addr::new(192, 0, 2, 1 + n),
        };
        let addr = SocketAddr::new(IpAddr::V4(ip), port);
        w.hosts.insert(addr, nat);
        SimDatagrams {
            world: self.0.clone(),
            addr,
        }
    }

    /// A machine behind a NAT of its own.
    pub fn behind(&self, kind: NatKind) -> SimDatagrams {
        match kind {
            NatKind::Open => self.machine(None, 7000),
            k => {
                let n = self.nat(k, false);
                self.machine(Some(n), 7000)
            }
        }
    }

    pub fn stats(&self) -> InternetStats {
        let w = self.0.lock().expect("world");
        InternetStats {
            routed: w.routed,
            relayed: w.relayed,
            dropped: w.dropped,
        }
    }
}

/// One simulated machine's socket.
pub struct SimDatagrams {
    world: Arc<Mutex<World>>,
    addr: SocketAddr,
}

impl Datagrams for SimDatagrams {
    fn send_to(&mut self, datagram: &[u8], to: SocketAddr) -> std::io::Result<()> {
        if let Ok(mut w) = self.world.lock() {
            w.send(self.addr, to, datagram.to_vec());
        }
        Ok(())
    }

    fn recv_from(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        let mut w = self.world.lock().ok()?;
        let (from, d) = w.inbox.get_mut(&self.addr)?.pop_front()?;
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        Some((n, from))
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        Ok(self.addr)
    }

    fn lan_addr(&self, _toward: SocketAddr) -> Option<SocketAddr> {
        Some(self.addr)
    }
}
