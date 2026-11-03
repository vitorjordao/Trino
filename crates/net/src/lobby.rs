//! The lobby: two players meet by a room code (issue #67).
//!
//! A tiny server (`cargo xtask net lobby`) keeps rooms by code. Each player
//! joins with the code; when the room is full every member learns the
//! others' seats and addresses, and from then on they talk directly:
//!
//! - **PC (UDP):** the game's own socket sends `TRINO_LOBBY JOIN <code>
//!   <players>` to the lobby's UDP port every quarter second until the
//!   answer `TRINO_LOBBY START <seat> <players> <seat>=<ip:port> …` comes;
//!   the address the lobby saw is the one a peer behind a NAT is reached at
//!   (both peers' first packets open their NATs).
//! - **Web (WebSocket):** the page sends `JOIN <code> <players>`, gets
//!   `START <seat> <players> <seat>=<n> …` (peer numbers), then
//!   `SIGNAL <to> <text>` lines carry each WebRTC offer, answer and ICE
//!   candidate to the peer (`FROM <seat> <text>` on arrival).
//!
//! [`Rooms`] is the server's state, transport-free: the xtask server feeds
//! it lines and sends what it returns, and the tests drive it directly.
//! Seats go in join order: the first to join is player 1 (seat 0).

use std::collections::BTreeMap;

/// The UDP line prefix (the game socket also carries ggrs datagrams).
pub const UDP_PREFIX: &str = "TRINO_LOBBY ";

/// The first byte of a relayed datagram (issue #117): `0xAA`, the room's
/// id (`u32` LE, [`room_id`]), the sender's seat, the receiver's seat, then
/// the body. A peer sends it to the lobby, which forwards it unchanged to
/// the receiver's address when the sender's address is the room's member
/// at that seat.
pub const RELAY_TAG: u8 = 0xAA;

/// A room's id on the wire: [`trino_core::asset_id`] of its code.
pub fn room_id(code: &str) -> u32 {
    trino_core::asset_id(code)
}

/// A room member: its transport address (UDP `ip:port`, or a WebSocket
/// connection's number).
pub type Member = String;

/// What the server must send after a line: (to, line).
pub type Outbox = Vec<(Member, String)>;

#[derive(Default, Debug)]
struct Room {
    id: u32,
    players: usize,
    /// Players in seat order, then spectators (seats `players..`).
    members: Vec<Member>,
    watchers: Vec<Member>,
    /// Each member's LAN address, as it reported it (`lan=`).
    lans: BTreeMap<Member, String>,
    /// The first joiner's settings fingerprint (`fp=`).
    fp: Option<String>,
    started: bool,
}

impl Room {
    /// Every seat's member: the players, then the spectators.
    fn seats(&self) -> impl Iterator<Item = (usize, &Member)> {
        self.members.iter().chain(self.watchers.iter()).enumerate()
    }

    fn start_lines(&self, prefix: &str) -> Outbox {
        let list: Vec<String> = self
            .seats()
            .map(|(s, m)| match self.lans.get(m) {
                Some(lan) => format!("{s}={m}+{lan}"),
                None => format!("{s}={m}"),
            })
            .collect();
        self.seats()
            .map(|(seat, m)| {
                (
                    m.clone(),
                    format!("{prefix}START {seat} {} {}", self.players, list.join(" ")),
                )
            })
            .collect()
    }
}

/// The lobby server's rooms.
#[derive(Default, Debug)]
pub struct Rooms {
    rooms: BTreeMap<String, Room>,
}

impl Rooms {
    pub fn new() -> Rooms {
        Rooms::default()
    }

    /// `from` sent `line`: what to send back. `prefix` goes in front of
    /// every reply (UDP's [`UDP_PREFIX`], none on a WebSocket). Lines:
    ///
    /// - `JOIN <code> <players> [fp=<hex>] [lan=<ip:port>]` — take the next
    ///   seat; `WAIT <seat> <joined> <players>` until the room is full,
    ///   then `START` to everyone;
    /// - `WATCH <code> [<players>] [fp=<hex>] [lan=<ip:port>]` — spectate it (issue
    ///   #117): seats after the players, before the room starts (`players`
    ///   creates the room if the spectator comes first);
    /// - `SIGNAL <seat> <text>` — to a seat (`FROM <seat> <text>`);
    /// - `LEAVE`.
    ///
    /// A `fp` that differs from the first joiner's is refused with an
    /// `ERROR` naming both ([`crate::NetConfig::fingerprint`]): peers whose
    /// settings differ would only desync later.
    pub fn handle(&mut self, from: &str, line: &str, prefix: &str) -> Outbox {
        let line = line.trim();
        let line = line
            .strip_prefix(UDP_PREFIX.trim_end())
            .unwrap_or(line)
            .trim();
        let err = |m: &str| vec![(from.to_string(), format!("{prefix}ERROR {m}"))];
        let mut words = line.split_whitespace();
        match words.next() {
            Some(verb @ ("JOIN" | "WATCH")) => {
                let watch = verb == "WATCH";
                let code = words.next().unwrap_or("").to_string();
                let mut players = 2usize;
                let (mut fp, mut lan) = (None, None);
                for w in words {
                    if let Some(v) = w.strip_prefix("fp=") {
                        fp = Some(v.to_string());
                    } else if let Some(v) = w.strip_prefix("lan=") {
                        lan = Some(v.to_string());
                    } else if let Ok(n) = w.parse::<usize>() {
                        players = n.clamp(2, trino_core::MAX_PLAYERS);
                    }
                }
                if code.is_empty() {
                    return err("no room code");
                }
                let id = room_id(&code);
                let room = self.rooms.entry(code).or_insert_with(|| Room {
                    id,
                    players,
                    fp: fp.clone(),
                    ..Room::default()
                });
                if let (Some(want), Some(got)) = (&room.fp, &fp)
                    && want != got
                {
                    return err(&format!(
                        "the room's settings differ (room {want}, yours {got}): \
                         [net] mode, rollback, delay, players, fps and pointer_space must match"
                    ));
                }
                if room.fp.is_none() {
                    room.fp = fp;
                }
                let known = room.members.iter().chain(&room.watchers).any(|m| m == from);
                if !known {
                    if room.started {
                        return err(if watch {
                            "the match started (spectators join before it starts)"
                        } else {
                            "the room is full"
                        });
                    }
                    if watch {
                        room.watchers.push(from.into());
                    } else if room.members.len() >= room.players {
                        return err("the room is full");
                    } else {
                        room.members.push(from.into());
                    }
                }
                if let Some(lan) = lan {
                    room.lans.insert(from.into(), lan);
                }
                if room.members.len() < room.players {
                    let seat = room.seats().find(|(_, m)| *m == from).map_or(0, |s| s.0);
                    return vec![(
                        from.into(),
                        format!(
                            "{prefix}WAIT {seat} {} {}",
                            room.members.len(),
                            room.players
                        ),
                    )];
                }
                room.started = true;
                // Everyone (again, for a member whose START was lost).
                room.start_lines(prefix)
            }
            Some("SIGNAL") => {
                let to: Option<usize> = words.next().and_then(|s| s.parse().ok());
                let text = line.splitn(3, ' ').nth(2).unwrap_or("");
                for room in self.rooms.values() {
                    if let Some((me, _)) = room.seats().find(|(_, m)| *m == from)
                        && let Some((_, dest)) = room.seats().find(|(s, _)| Some(*s) == to)
                    {
                        return vec![(dest.clone(), format!("{prefix}FROM {me} {text}"))];
                    }
                }
                err("not in a room")
            }
            Some("LEAVE") => {
                self.leave(from);
                Vec::new()
            }
            _ => err("unknown request"),
        }
    }

    /// A relayed datagram ([`RELAY_TAG`]) from `from`: the member it goes
    /// to, when the room has started, `from` is the sender seat's member
    /// and the receiver exists. Anything else is dropped (`None`).
    pub fn relay(&self, from: &str, datagram: &[u8]) -> Option<Member> {
        let (id, src, dst) = parse_relay(datagram)?;
        let room = self.rooms.values().find(|r| r.id == id && r.started)?;
        let (_, sender) = room.seats().find(|(s, _)| *s == src as usize)?;
        if sender != from {
            return None;
        }
        room.seats()
            .find(|(s, _)| *s == dst as usize)
            .map(|(_, m)| m.clone())
    }

    /// A member is gone (a WebSocket closed): its rooms close.
    pub fn leave(&mut self, member: &str) {
        self.rooms
            .retain(|_, r| !r.members.iter().chain(&r.watchers).any(|m| m == member));
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

/// A [`RELAY_TAG`] datagram's room, sender seat and receiver seat.
pub fn parse_relay(d: &[u8]) -> Option<(u32, u8, u8)> {
    if d.len() < 7 || d[0] != RELAY_TAG {
        return None;
    }
    Some((u32::from_le_bytes([d[1], d[2], d[3], d[4]]), d[5], d[6]))
}

/// One datagram to a UDP lobby server (`cargo xtask net lobby`, a PC host
/// serving its own room): the lines to send back, or the relayed datagram
/// to forward. The xtask server, the PC host and the NAT tests share it.
pub fn serve_udp(rooms: &mut Rooms, from: &str, datagram: &[u8]) -> Vec<(Member, Vec<u8>)> {
    if datagram.first() == Some(&RELAY_TAG) {
        return rooms
            .relay(from, datagram)
            .map(|to| vec![(to, datagram.to_vec())])
            .unwrap_or_default();
    }
    let Ok(line) = std::str::from_utf8(datagram) else {
        return Vec::new();
    };
    rooms
        .handle(from, line, UDP_PREFIX)
        .into_iter()
        .map(|(to, l)| (to, l.into_bytes()))
        .collect()
}

/// A `START` reply: this member's seat, the players, every seat's member
/// (players then spectators) and the LAN addresses members reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub seat: usize,
    pub players: usize,
    pub members: Vec<(usize, Member)>,
    pub lans: Vec<(usize, Member)>,
}

impl Start {
    /// Is this member a spectator (a seat after the players)?
    pub fn spectating(&self) -> bool {
        self.seat >= self.players
    }
}

/// Parse `START <seat> <players> <seat>=<member>[+<lan>] …` (with or
/// without the UDP prefix).
pub fn parse_start(line: &str) -> Option<Start> {
    let line = line.trim();
    let line = line.strip_prefix(UDP_PREFIX).unwrap_or(line);
    let mut w = line.strip_prefix("START ")?.split_whitespace();
    let seat = w.next()?.parse().ok()?;
    let players = w.next()?.parse().ok()?;
    let mut members = Vec::new();
    let mut lans = Vec::new();
    for m in w {
        let (s, a) = m.split_once('=')?;
        let s: usize = s.parse().ok()?;
        let (a, lan) = match a.split_once('+') {
            Some((a, lan)) => (a, Some(lan)),
            None => (a, None),
        };
        members.push((s, a.to_string()));
        if let Some(lan) = lan {
            lans.push((s, lan.to_string()));
        }
    }
    Some(Start {
        seat,
        players,
        members,
        lans,
    })
}

/// Meet the other players of room `code` through the lobby at `lobby`
/// (UDP), on the session's own socket: keeps asking until the room is
/// full or `timeout` passes. Returns this seat and the peers (seat,
/// address).
#[cfg(not(target_arch = "wasm32"))]
pub fn rendezvous(
    socket: &mut crate::udp::UdpSocket,
    lobby: std::net::SocketAddr,
    code: &str,
    players: usize,
    timeout: std::time::Duration,
) -> Result<(usize, Vec<(usize, std::net::SocketAddr)>), String> {
    let t0 = std::time::Instant::now();
    let mut last = None::<std::time::Instant>;
    loop {
        if t0.elapsed() > timeout {
            return Err(format!(
                "lobby {lobby}: room `{code}` didn't fill in {timeout:?}"
            ));
        }
        if last.is_none_or(|l| l.elapsed() > std::time::Duration::from_millis(250)) {
            socket
                .send_text(lobby, &format!("{UDP_PREFIX}JOIN {code} {players}"))
                .map_err(|e| format!("lobby {lobby}: {e}"))?;
            last = Some(std::time::Instant::now());
        }
        while let Some((from, line)) = socket.recv_text() {
            if from != lobby {
                continue;
            }
            if let Some(err) = line
                .strip_prefix(UDP_PREFIX)
                .and_then(|l| l.strip_prefix("ERROR "))
            {
                return Err(format!("lobby: {err}"));
            }
            if let Some(s) = parse_start(&line) {
                let peers = s
                    .members
                    .iter()
                    .filter(|(seat, _)| *seat != s.seat)
                    .map(|(seat, a)| {
                        if a == crate::udp::HOST {
                            // A host serving its own room: the address we
                            // joined it at.
                            return Ok((*seat, lobby));
                        }
                        a.parse()
                            .map(|a| (*seat, a))
                            .map_err(|_| format!("lobby: bad address `{a}`"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok((s.seat, peers));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_starts_when_full_and_relays_signals() {
        let mut r = Rooms::new();
        let wait = r.handle("a", "JOIN abc 2", "");
        assert_eq!(wait, vec![("a".to_string(), "WAIT 0 1 2".to_string())]);
        let start = r.handle("b", "JOIN abc 2", "");
        assert_eq!(start.len(), 2);
        let a = parse_start(&start[0].1).unwrap();
        let b = parse_start(&start[1].1).unwrap();
        assert_eq!((a.seat, b.seat, a.players), (0, 1, 2));
        assert_eq!(a.members, vec![(0, "a".into()), (1, "b".into())]);
        // A third player finds it full; a repeated JOIN gets START again.
        assert!(r.handle("c", "JOIN abc 2", "")[0].1.starts_with("ERROR"));
        assert_eq!(r.handle("a", "JOIN abc 2", "").len(), 2);
        // Signals go to a seat, tagged with the sender's.
        let s = r.handle("a", "SIGNAL 1 offer:v=0 o=- 1", "");
        assert_eq!(
            s,
            vec![("b".to_string(), "FROM 0 offer:v=0 o=- 1".to_string())]
        );
        r.leave("b");
        assert!(r.is_empty());
        // UDP lines carry the prefix both ways.
        let w = r.handle("1.2.3.4:5", "TRINO_LOBBY JOIN x 2", UDP_PREFIX);
        assert_eq!(w[0].1, "TRINO_LOBBY WAIT 0 1 2");
        let st = r.handle("6.7.8.9:10", "TRINO_LOBBY JOIN x 2", UDP_PREFIX);
        assert_eq!(parse_start(&st[1].1).unwrap().members[0].1, "1.2.3.4:5");
    }

    /// Issue #117: spectators take the seats after the players, settings
    /// fingerprints must match, LAN addresses ride along, and the relay
    /// forwards only a member's own datagrams.
    #[test]
    fn spectators_fingerprints_lans_and_the_relay() {
        let mut r = Rooms::new();
        // A spectator may come first (it names the players).
        let w = r.handle("w", "WATCH r 3 fp=aa", "");
        assert_eq!(w[0].1, "WAIT 0 0 3");
        r.handle("a", "JOIN r 3 fp=aa lan=10.0.0.2:7000", "");
        let bad = r.handle("x", "JOIN r 3 fp=bb", "");
        assert!(bad[0].1.contains("room aa, yours bb"), "{bad:?}");
        r.handle("b", "JOIN r 3 fp=aa", "");
        let st = r.handle("c", "JOIN r 3 fp=aa", "");
        assert_eq!(st.len(), 4, "three players and the spectator");
        let by: std::collections::BTreeMap<_, _> = st
            .iter()
            .map(|(m, l)| (m.as_str(), parse_start(l).unwrap()))
            .collect();
        assert_eq!(by["a"].seat, 0);
        assert_eq!(by["w"].seat, 3);
        assert!(by["w"].spectating() && !by["c"].spectating());
        assert_eq!(by["c"].lans, vec![(0, "10.0.0.2:7000".to_string())]);
        assert_eq!(by["c"].members.len(), 4);
        // Too late to watch now.
        assert!(r.handle("v", "WATCH r 3", "")[0].1.contains("started"));
        // The relay: seat 1 → seat 3, only from seat 1's own address.
        let mut d = vec![RELAY_TAG];
        d.extend_from_slice(&room_id("r").to_le_bytes());
        d.extend_from_slice(&[1, 3, 0, 1, 42]);
        assert_eq!(r.relay("b", &d), Some("w".to_string()));
        assert_eq!(r.relay("c", &d), None, "a spoofed sender");
        d[6] = 9;
        assert_eq!(r.relay("b", &d), None, "no seat 9");
        assert_eq!(serve_udp(&mut r, "b", &[RELAY_TAG, 1]), Vec::new());
        let lines = serve_udp(&mut r, "z", b"TRINO_LOBBY JOIN q 2");
        assert_eq!(lines[0].1, b"TRINO_LOBBY WAIT 0 1 2".to_vec());
    }
}
