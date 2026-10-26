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

/// A room member: its transport address (UDP `ip:port`, or a WebSocket
/// connection's number).
pub type Member = String;

/// What the server must send after a line: (to, line).
pub type Outbox = Vec<(Member, String)>;

#[derive(Default, Debug)]
struct Room {
    players: usize,
    members: Vec<Member>,
    started: bool,
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

    /// `from` sent `line` (`JOIN <code> <players>`, `SIGNAL <seat> <text>`,
    /// `LEAVE`): what to send back. `prefix` goes in front of every reply
    /// (UDP's [`UDP_PREFIX`], none on a WebSocket).
    pub fn handle(&mut self, from: &str, line: &str, prefix: &str) -> Outbox {
        let line = line.trim();
        let line = line
            .strip_prefix(UDP_PREFIX.trim_end())
            .unwrap_or(line)
            .trim();
        let mut words = line.splitn(3, ' ');
        match words.next() {
            Some("JOIN") => {
                let code = words.next().unwrap_or("").to_string();
                let players = words
                    .next()
                    .and_then(|p| p.trim().parse::<usize>().ok())
                    .unwrap_or(2)
                    .clamp(2, trino_core::MAX_PLAYERS);
                if code.is_empty() {
                    return vec![(from.into(), format!("{prefix}ERROR no room code"))];
                }
                let room = self.rooms.entry(code).or_insert_with(|| Room {
                    players,
                    ..Room::default()
                });
                if !room.members.iter().any(|m| m == from) {
                    if room.started || room.members.len() >= room.players {
                        return vec![(from.into(), format!("{prefix}ERROR the room is full"))];
                    }
                    room.members.push(from.into());
                }
                if room.members.len() < room.players {
                    let seat = room.members.iter().position(|m| m == from).unwrap_or(0);
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
                let list: Vec<String> = room
                    .members
                    .iter()
                    .enumerate()
                    .map(|(s, m)| format!("{s}={m}"))
                    .collect();
                room.members
                    .iter()
                    .enumerate()
                    .map(|(seat, m)| {
                        (
                            m.clone(),
                            format!("{prefix}START {seat} {} {}", room.players, list.join(" ")),
                        )
                    })
                    .collect()
            }
            Some("SIGNAL") => {
                let to: Option<usize> = words.next().and_then(|s| s.parse().ok());
                let text = words.next().unwrap_or("");
                for room in self.rooms.values() {
                    if let Some(me) = room.members.iter().position(|m| m == from)
                        && let Some(dest) = to.and_then(|t| room.members.get(t))
                    {
                        return vec![(dest.clone(), format!("{prefix}FROM {me} {text}"))];
                    }
                }
                vec![(from.into(), format!("{prefix}ERROR not in a room"))]
            }
            Some("LEAVE") => {
                self.leave(from);
                Vec::new()
            }
            _ => vec![(from.into(), format!("{prefix}ERROR unknown request"))],
        }
    }

    /// A member is gone (a WebSocket closed): its rooms close.
    pub fn leave(&mut self, member: &str) {
        self.rooms
            .retain(|_, r| !r.members.iter().any(|m| m == member));
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }
}

/// A `START` reply: this member's seat, the players, every seat's member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    pub seat: usize,
    pub players: usize,
    pub members: Vec<(usize, Member)>,
}

/// Parse `START <seat> <players> <seat>=<member> …` (with or without the
/// UDP prefix).
pub fn parse_start(line: &str) -> Option<Start> {
    let line = line.trim();
    let line = line.strip_prefix(UDP_PREFIX).unwrap_or(line);
    let mut w = line.strip_prefix("START ")?.split_whitespace();
    let seat = w.next()?.parse().ok()?;
    let players = w.next()?.parse().ok()?;
    let members = w
        .map(|m| {
            let (s, a) = m.split_once('=')?;
            Some((s.parse().ok()?, a.to_string()))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Start {
        seat,
        players,
        members,
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
}
