//! UDP on the PC: a non-blocking socket ggrs sends its messages through
//! ([`UdpSocket`]), the raw datagrams the lobby's rendezvous needs on the
//! same port, and [`Conditioned`]: latency, jitter and loss added to any
//! socket on the wall clock (two local instances under the editor's Net
//! panel, `TRINO_NET_SIM`).
//!
//! **Rooms and NAT traversal (issue #117).** After [`meet`], peers are
//! addressed by seat ([`PeerAddr::Room`]) and the socket picks the route
//! per peer:
//!
//! - **direct** once both sides hear each other: every room datagram
//!   (`0xA9`) carries a "I hear you" flag, and hellos go to each peer's
//!   public address (as the lobby saw it) and LAN address every 100 ms
//!   until then. Two cone NATs, or one symmetric NAT facing a cone that
//!   filters by address only, open this way (UDP hole punching);
//! - **through the lobby** otherwise (`0xAA`, [`crate::lobby::RELAY_TAG`]):
//!   two symmetric NATs, a symmetric one facing a port-restricted one, or
//!   a network that drops UDP between peers. The lobby forwards the
//!   datagram to the member at the receiving seat; the hellos go on, so a
//!   relayed pair turns direct when a path opens, and a direct one falls
//!   back to the relay after 1.5 s of silence.
//!
//! [`Datagrams`] is the raw transport underneath: a real
//! `std::net::UdpSocket`, or the NAT simulator ([`crate::natsim`]) the
//! tests meet through.

use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ggrs::{Message, NonBlockingSocket};

use crate::lobby::{RELAY_TAG, UDP_PREFIX, parse_relay, parse_start, room_id};
use crate::session::PeerAddr;
use crate::sim::Conditions;

/// Bytes a datagram may have (ggrs's messages are far smaller).
const MAX_DATAGRAM: usize = 4096;

/// The first byte of every ggrs datagram sent to a fixed address, so the
/// lobby's text lines on the same port are told apart.
const GGRS_TAG: u8 = 0xA7;

/// The first byte of a room datagram sent straight to a peer: `0xA9`, the
/// room id (`u32` LE), the sender's seat, flags (bit 0: the sender hears
/// the receiver directly), the kind (0 hello, 1 a ggrs message), the body.
/// A relayed one ([`RELAY_TAG`]) has the receiver's seat after the
/// sender's, then the same flags, kind and body.
pub const ROOM_TAG: u8 = 0xA9;

/// How often an unconfirmed peer gets hellos.
const HELLO_EVERY: Duration = Duration::from_millis(100);
/// Silence after which a direct route counts as broken.
const DIRECT_TIMEOUT: Duration = Duration::from_millis(1500);

/// The host's own member name in its room (a guest reads it as "the address
/// I sent my JOIN to").
pub const HOST: &str = "host";

/// Raw datagrams: a real socket or a simulated one.
pub trait Datagrams: Send {
    fn send_to(&mut self, datagram: &[u8], to: SocketAddr) -> std::io::Result<()>;
    /// The next datagram waiting, without blocking.
    fn recv_from(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)>;
    fn local_addr(&self) -> std::io::Result<SocketAddr>;
    /// This machine's address on its LAN as seen when talking to `toward`
    /// (what a peer behind the same NAT reaches it at).
    fn lan_addr(&self, toward: SocketAddr) -> Option<SocketAddr>;
}

impl Datagrams for std::net::UdpSocket {
    fn send_to(&mut self, datagram: &[u8], to: SocketAddr) -> std::io::Result<()> {
        std::net::UdpSocket::send_to(self, datagram, to).map(|_| ())
    }

    fn recv_from(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddr)> {
        loop {
            match std::net::UdpSocket::recv_from(self, buf) {
                Ok(r) => return Some(r),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return None,
                // An ICMP error from a closed peer port (Windows reports
                // those on the next receive): read on.
                Err(_) => continue,
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        std::net::UdpSocket::local_addr(self)
    }

    fn lan_addr(&self, toward: SocketAddr) -> Option<SocketAddr> {
        // The interface the system routes `toward` through (a connected UDP
        // socket sends nothing).
        let probe = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
        probe.connect(toward).ok()?;
        let ip = probe.local_addr().ok()?.ip();
        Some(SocketAddr::new(ip, self.local_addr().ok()?.port()))
    }
}

/// How one room peer is reached right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    /// Hellos out, nothing confirmed yet.
    Opening,
    /// Straight to this address.
    Direct(SocketAddr),
    /// Through the lobby.
    Relay,
}

/// One room peer's link, as [`Routes`] reports it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LinkReport {
    pub seat: u8,
    pub route: Route,
    /// Datagrams sent and received each way.
    pub direct_tx: u64,
    pub relay_tx: u64,
    pub direct_rx: u64,
    pub relay_rx: u64,
}

/// A handle on a room socket's links that outlives handing the socket to a
/// session (the `TRINO_NET:` line, the window title, the tests).
#[derive(Clone, Default)]
pub struct Routes(Arc<Mutex<Vec<LinkReport>>>);

impl Routes {
    pub fn get(&self) -> Vec<LinkReport> {
        self.0.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// `1:direct 2:relay` for logs (seats counted from 1, like players).
    pub fn summary(&self) -> String {
        self.get()
            .iter()
            .map(|l| {
                let r = match l.route {
                    Route::Opening => "opening",
                    Route::Direct(_) => "direct",
                    Route::Relay => "relay",
                };
                format!("{}:{r}", l.seat + 1)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

struct PeerLink {
    seat: u8,
    /// Where to say hello: the address the lobby saw, the LAN address.
    candidates: Vec<SocketAddr>,
    /// The address its direct datagrams last came from, and when.
    direct: Option<(SocketAddr, Instant)>,
    /// Its last direct datagram said it hears us.
    hears_us: bool,
    last_hello: Option<Instant>,
    report: LinkReport,
}

impl PeerLink {
    /// We hear it directly (recently).
    fn hear(&self, now: Instant) -> bool {
        self.direct
            .is_some_and(|(_, at)| now.duration_since(at) < DIRECT_TIMEOUT)
    }

    /// Both ways work: where to send.
    fn direct_now(&self, now: Instant) -> Option<SocketAddr> {
        match self.direct {
            Some((a, _)) if self.hears_us && self.hear(now) => Some(a),
            _ => None,
        }
    }
}

/// What may carry a room's datagrams (`TRINO_NET_ROUTE`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RoutePolicy {
    pub direct: bool,
    pub relay: bool,
}

impl RoutePolicy {
    pub const BOTH: RoutePolicy = RoutePolicy {
        direct: true,
        relay: true,
    };

    /// `direct`, `relay` or `auto` (both, the default).
    pub fn parse(v: &str) -> Option<RoutePolicy> {
        match v.trim() {
            "" | "auto" => Some(RoutePolicy::BOTH),
            "direct" => Some(RoutePolicy {
                direct: true,
                relay: false,
            }),
            "relay" => Some(RoutePolicy {
                direct: false,
                relay: true,
            }),
            _ => None,
        }
    }
}

struct RoomLink {
    id: u32,
    me: u8,
    /// The relay (the lobby); `None` on the machine that serves the room.
    lobby: Option<SocketAddr>,
    policy: RoutePolicy,
    peers: Vec<PeerLink>,
    routes: Routes,
}

impl RoomLink {
    fn peer(&mut self, seat: u8) -> Option<&mut PeerLink> {
        self.peers.iter_mut().find(|p| p.seat == seat)
    }

    fn publish(&mut self, now: Instant) {
        let relay = self.policy.relay && self.lobby.is_some();
        for p in &mut self.peers {
            p.report.route = match p.direct_now(now) {
                Some(a) if self.policy.direct => Route::Direct(a),
                _ if relay => Route::Relay,
                _ if self.lobby.is_none() => {
                    match p.direct.or(p.candidates.first().map(|a| (*a, now))) {
                        Some((a, _)) => Route::Direct(a),
                        None => Route::Opening,
                    }
                }
                _ => Route::Opening,
            };
        }
        if let Ok(mut r) = self.routes.0.lock() {
            *r = self.peers.iter().map(|p| p.report).collect();
        }
    }
}

/// A non-blocking UDP socket for a session (and its rendezvous).
pub struct UdpSocket {
    io: Box<dyn Datagrams>,
    buf: Vec<u8>,
    /// Text datagrams that arrived while the session ran (a lobby's late
    /// reply), kept for [`recv_text`](Self::recv_text).
    text: VecDeque<(SocketAddr, String)>,
    /// A host's own room ([`host_room`](Self::host_room), [`meet`]): lobby
    /// lines and relayed datagrams are answered as they arrive, during the
    /// session too (a guest whose `START` was lost asks again).
    room: Option<crate::lobby::Rooms>,
    /// The room this socket plays in, once met ([`meet`]).
    link: Option<RoomLink>,
}

impl UdpSocket {
    /// Bind `addr` (`0.0.0.0:7000`, or port 0 for any).
    pub fn bind(addr: impl ToSocketAddrs) -> std::io::Result<UdpSocket> {
        let socket = std::net::UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(UdpSocket::over(socket))
    }

    /// A socket over any datagram transport (the NAT simulator).
    pub fn over(io: impl Datagrams + 'static) -> UdpSocket {
        UdpSocket {
            io: Box::new(io),
            buf: vec![0; MAX_DATAGRAM],
            text: VecDeque::new(),
            room: None,
            link: None,
        }
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
                if let Some(start) = parse_start(&line) {
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
        self.io.local_addr()
    }

    /// A text datagram (the lobby protocol).
    pub fn send_text(&mut self, to: SocketAddr, line: &str) -> std::io::Result<()> {
        self.io.send_to(line.as_bytes(), to)
    }

    /// The next text datagram, if one is waiting.
    pub fn recv_text(&mut self) -> Option<(SocketAddr, String)> {
        self.pump();
        self.text.pop_front()
    }

    /// The room links' handle (empty before [`meet`]).
    pub fn routes(&self) -> Routes {
        self.link
            .as_ref()
            .map(|l| l.routes.clone())
            .unwrap_or_default()
    }

    fn send_room(&mut self, seat: u8, kind: u8, body: &[u8]) {
        let now = Instant::now();
        let Some(link) = self.link.as_mut() else {
            return;
        };
        let (id, me, lobby, policy) = (link.id, link.me, link.lobby, link.policy);
        let Some(p) = link.peer(seat) else { return };
        let flags = p.hear(now) as u8;
        let direct = p.direct_now(now).filter(|_| policy.direct);
        let room = |d: &mut Vec<u8>| {
            d.push(ROOM_TAG);
            d.extend_from_slice(&id.to_le_bytes());
            d.extend_from_slice(&[me, flags, kind]);
            d.extend_from_slice(body);
        };
        let mut d = Vec::with_capacity(body.len() + 9);
        match (direct, lobby) {
            (Some(a), _) => {
                room(&mut d);
                p.report.direct_tx += 1;
                let _ = self.io.send_to(&d, a);
            }
            (None, Some(relay)) if policy.relay => {
                d.push(RELAY_TAG);
                d.extend_from_slice(&id.to_le_bytes());
                d.extend_from_slice(&[me, seat, flags, kind]);
                d.extend_from_slice(body);
                p.report.relay_tx += 1;
                let _ = self.io.send_to(&d, relay);
            }
            _ => {
                // No relay (this machine serves the room, or relaying is
                // off): straight to where it was last heard, else to the
                // address the room knows.
                let to = p.direct.map(|d| d.0).or(p.candidates.first().copied());
                if let Some(to) = to {
                    room(&mut d);
                    p.report.direct_tx += 1;
                    let _ = self.io.send_to(&d, to);
                }
            }
        }
    }

    /// Hellos to every peer not yet direct (each candidate, every 100 ms).
    fn hellos(&mut self) {
        let now = Instant::now();
        let Some(link) = self.link.as_mut() else {
            return;
        };
        if link.policy.direct {
            let (id, me) = (link.id, link.me);
            for p in &mut link.peers {
                let due = p
                    .last_hello
                    .is_none_or(|t| now.duration_since(t) >= HELLO_EVERY);
                if p.direct_now(now).is_some() || !due {
                    continue;
                }
                p.last_hello = Some(now);
                let mut d = vec![ROOM_TAG];
                d.extend_from_slice(&id.to_le_bytes());
                d.extend_from_slice(&[me, p.hear(now) as u8, 0]);
                let mut to: Vec<SocketAddr> = p.candidates.clone();
                if let Some((a, _)) = p.direct
                    && !to.contains(&a)
                {
                    to.push(a);
                }
                for a in to {
                    let _ = self.io.send_to(&d, a);
                }
            }
        }
        link.publish(now);
    }

    /// Read what arrived: ggrs messages out (by sender), text kept, room
    /// datagrams routed, a served room's lines and relays answered.
    fn pump(&mut self) -> Vec<(PeerAddr, Vec<u8>)> {
        let mut out = Vec::new();
        let now = Instant::now();
        while let Some((n, from)) = self.io.recv_from(&mut self.buf) {
            let d = self.buf[..n].to_vec();
            match d.first() {
                Some(&GGRS_TAG) => out.push((PeerAddr::Udp(from), d[1..].to_vec())),
                Some(&ROOM_TAG) if d.len() >= 8 => {
                    let id = u32::from_le_bytes([d[1], d[2], d[3], d[4]]);
                    let (seat, flags, kind) = (d[5], d[6], d[7]);
                    let Some(link) = self.link.as_mut().filter(|l| l.id == id) else {
                        continue;
                    };
                    let Some(p) = link.peer(seat) else { continue };
                    p.direct = Some((from, now));
                    p.hears_us = flags & 1 != 0;
                    p.report.direct_rx += 1;
                    if kind == 1 {
                        out.push((PeerAddr::Room(seat as u16), d[8..].to_vec()));
                    }
                }
                Some(&RELAY_TAG) => {
                    let Some((id, src, dst)) = parse_relay(&d) else {
                        continue;
                    };
                    let mine = self
                        .link
                        .as_ref()
                        .is_some_and(|l| l.id == id && l.me == dst);
                    if mine {
                        let link = self.link.as_mut().expect("checked");
                        let Some(p) = link.peer(src) else { continue };
                        p.report.relay_rx += 1;
                        if d.len() >= 9 && d[8] == 1 {
                            out.push((PeerAddr::Room(src as u16), d[9..].to_vec()));
                        }
                    } else if let Some(rooms) = &mut self.room {
                        // This machine serves the room: forward it.
                        for (to, bytes) in crate::lobby::serve_udp(rooms, &from.to_string(), &d) {
                            if let Ok(a) = to.parse::<SocketAddr>() {
                                let _ = self.io.send_to(&bytes, a);
                            }
                        }
                    }
                }
                _ => {
                    let Ok(t) = std::str::from_utf8(&d) else {
                        continue;
                    };
                    let t = t.to_string();
                    match &mut self.room {
                        Some(rooms) if t.starts_with(UDP_PREFIX) => {
                            let who = from.to_string();
                            for (to, line) in rooms.handle(&who, &t, UDP_PREFIX) {
                                if to == HOST {
                                    // The host's own START: the room is full.
                                    self.text.push_back((from, line));
                                } else if let Ok(a) = to.parse::<SocketAddr>() {
                                    let _ = self.io.send_to(line.as_bytes(), a);
                                }
                            }
                        }
                        _ => self.text.push_back((from, t)),
                    }
                }
            }
        }
        self.hellos();
        out
    }
}

impl NonBlockingSocket<PeerAddr> for UdpSocket {
    fn send_to(&mut self, msg: &Message, addr: &PeerAddr) {
        let Some(body) = crate::encode_message(msg) else {
            return;
        };
        match addr {
            PeerAddr::Udp(to) => {
                let mut d = Vec::with_capacity(body.len() + 1);
                d.push(GGRS_TAG);
                d.extend_from_slice(&body);
                let _ = self.io.send_to(&d, *to);
            }
            PeerAddr::Room(seat) => self.send_room(*seat as u8, 1, &body),
            _ => {}
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerAddr, Message)> {
        self.pump()
            .into_iter()
            .filter_map(|(from, body)| Some((from, crate::decode_message(&body)?)))
            .collect()
    }
}

/// Where to meet ([`meet`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetAt {
    /// Serve room `direct` on this socket (this machine is player 1).
    Host,
    /// Join the room a host serves at this address.
    Join(SocketAddr),
    /// Room `code` of a lobby server.
    Lobby { code: String, server: SocketAddr },
}

/// What a finished meeting hands over.
#[derive(Clone)]
pub struct Meeting {
    /// This machine's seat (a spectator's is past the players).
    pub seat: usize,
    pub players: usize,
    /// The other players (seat, address).
    pub peers: Vec<(usize, PeerAddr)>,
    /// The spectators this machine serves (seat 0 only).
    pub spectators: Vec<PeerAddr>,
    /// For a spectator: the host it watches.
    pub watching: Option<PeerAddr>,
    pub routes: Routes,
}

/// Meet the room's other members on `socket` (issue #117): join (or
/// `watch`) through a lobby or a host, then set the socket up to route
/// each peer directly or through the relay. `cfg`'s fingerprint goes with
/// the join, so a peer with other settings is refused at the door.
pub fn meet(
    socket: &mut UdpSocket,
    at: &MeetAt,
    cfg: &crate::NetConfig,
    watch: bool,
    policy: RoutePolicy,
    timeout: Duration,
) -> Result<Meeting, String> {
    let players = cfg.players.clamp(2, trino_core::MAX_PLAYERS as u8) as usize;
    let fp = cfg.fingerprint();
    let (code, server) = match at {
        MeetAt::Host => ("direct".to_string(), None),
        MeetAt::Join(a) => ("direct".to_string(), Some(*a)),
        MeetAt::Lobby { code, server } => (code.clone(), Some(*server)),
    };
    let start = match server {
        None => {
            if watch {
                return Err("a host plays seat 1; spectators join a host or a lobby".into());
            }
            let mut rooms = crate::lobby::Rooms::new();
            rooms.handle(HOST, &format!("JOIN {code} {players} fp={fp}"), "");
            socket.room = Some(rooms);
            let t0 = Instant::now();
            loop {
                socket.pump();
                if let Some(s) = socket.text.iter().find_map(|(_, l)| parse_start(l)) {
                    socket.text.clear();
                    break s;
                }
                if t0.elapsed() > timeout {
                    return Err(format!("nobody joined in {timeout:?}"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Some(server) => {
            let lan = socket
                .io
                .lan_addr(server)
                .map(|a| format!(" lan={a}"))
                .unwrap_or_default();
            let verb = if watch {
                format!("WATCH {code} {players}")
            } else {
                format!("JOIN {code} {players}")
            };
            let line = format!("{UDP_PREFIX}{verb} fp={fp}{lan}");
            let t0 = Instant::now();
            let mut last = None::<Instant>;
            loop {
                if t0.elapsed() > timeout {
                    return Err(format!(
                        "{server}: room `{code}` didn't fill in {timeout:?}"
                    ));
                }
                if last.is_none_or(|l| l.elapsed() > Duration::from_millis(250)) {
                    socket
                        .send_text(server, &line)
                        .map_err(|e| format!("{server}: {e}"))?;
                    last = Some(Instant::now());
                }
                let mut got = None;
                while let Some((from, l)) = socket.recv_text() {
                    if from != server {
                        continue;
                    }
                    if let Some(err) = l
                        .strip_prefix(UDP_PREFIX)
                        .and_then(|l| l.strip_prefix("ERROR "))
                    {
                        return Err(format!("lobby: {err}"));
                    }
                    if let Some(s) = parse_start(&l) {
                        got = Some(s);
                    }
                }
                if let Some(s) = got {
                    break s;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    if start.players != players {
        return Err(format!(
            "the room is for {} players, this game for {players}",
            start.players
        ));
    }
    // Who this machine talks to: every other player, and seat 0 serves the
    // spectators; a spectator talks to its host only.
    let me = start.seat;
    let talks_to = |s: usize| {
        if me >= players {
            s == 0
        } else if s >= players {
            me == 0
        } else {
            s != me
        }
    };
    let mut peers = Vec::new();
    for (seat, member) in start.members.iter().filter(|(s, _)| talks_to(*s)) {
        let public = if member == HOST {
            server.ok_or("a host can't be its own peer")?
        } else {
            member
                .parse::<SocketAddr>()
                .map_err(|_| format!("lobby: bad address `{member}`"))?
        };
        let mut candidates = vec![public];
        for (_, lan) in start.lans.iter().filter(|(s, _)| s == seat) {
            if let Ok(a) = lan.parse::<SocketAddr>()
                && a != public
            {
                candidates.push(a);
            }
        }
        peers.push(PeerLink {
            seat: *seat as u8,
            candidates,
            direct: None,
            hears_us: false,
            last_hello: None,
            report: LinkReport {
                seat: *seat as u8,
                route: Route::Opening,
                direct_tx: 0,
                relay_tx: 0,
                direct_rx: 0,
                relay_rx: 0,
            },
        });
    }
    let routes = Routes::default();
    socket.link = Some(RoomLink {
        id: room_id(&code),
        me: me as u8,
        lobby: server,
        policy,
        peers,
        routes: routes.clone(),
    });
    socket.hellos();
    let addr = |s: usize| PeerAddr::Room(s as u16);
    Ok(Meeting {
        seat: me,
        players,
        peers: (0..players)
            .filter(|s| *s != me && me < players)
            .map(|s| (s, addr(s)))
            .collect(),
        spectators: if me == 0 {
            start
                .members
                .iter()
                .filter(|(s, _)| *s >= players)
                .map(|(s, _)| addr(*s))
                .collect()
        } else {
            Vec::new()
        },
        watching: (me >= players).then(|| addr(0)),
        routes,
    })
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
