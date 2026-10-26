//! WebRTC data channels as a session's socket (the Web, issue #67).
//!
//! [`meet`] joins room `code` of a lobby over a WebSocket
//! (`cargo xtask net lobby`), then opens one unordered, unreliable data
//! channel to every other player (the lower seat offers; offers, answers
//! and ICE candidates travel through the lobby as `SIGNAL` lines). Once
//! every channel is open, [`Meeting::take`] hands back this page's seat and
//! an [`RtcSocket`] for [`crate::NetSession`]. ggrs adds its own
//! reliability and ordering on top, as it does over UDP.
//!
//! Browsers on one machine or a LAN connect with their host candidates and
//! no ICE server; across the Internet a STUN server is needed
//! (`?stun=stun:stun.l.google.com:19302`).

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use ggrs::{Message, NonBlockingSocket};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelEvent, RtcDataChannelInit,
    RtcDataChannelType, RtcIceCandidateInit, RtcPeerConnection, RtcPeerConnectionIceEvent,
    RtcSdpType, RtcSessionDescriptionInit, WebSocket,
};

use crate::session::PeerAddr;

/// Where a meeting is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetStatus {
    /// Opening the lobby's WebSocket.
    Connecting,
    /// In the room, `have` of `need` players.
    Waiting {
        have: usize,
        need: usize,
    },
    /// Everyone is in; `open` of `need` data channels are open.
    Linking {
        open: usize,
        need: usize,
    },
    /// Every channel is open: [`Meeting::take`] the socket.
    Ready,
    Failed(String),
}

#[derive(Default)]
struct Shared {
    status: Option<MeetStatus>,
    seat: usize,
    players: usize,
    peers: BTreeMap<usize, RtcPeerConnection>,
    channels: BTreeMap<usize, RtcDataChannel>,
    open: usize,
    inbox: VecDeque<(usize, Vec<u8>)>,
    /// Kept alive for the page's life (event handlers).
    closures: Vec<Box<dyn std::any::Any>>,
}

/// A meeting in progress (poll it each frame).
pub struct Meeting {
    shared: Rc<RefCell<Shared>>,
    ws: WebSocket,
}

/// The data channels to every other player.
pub struct RtcSocket {
    shared: Rc<RefCell<Shared>>,
    _ws: WebSocket,
}

fn fail(shared: &Rc<RefCell<Shared>>, why: String) {
    shared.borrow_mut().status = Some(MeetStatus::Failed(why));
}

fn js_err(v: JsValue) -> String {
    v.as_string()
        .or_else(|| js_sys::JSON::stringify(&v).ok().and_then(|s| s.as_string()))
        .unwrap_or_else(|| "a JavaScript error".into())
}

fn encode(s: &str) -> String {
    js_sys::encode_uri_component(s).into()
}

fn decode(s: &str) -> String {
    js_sys::decode_uri_component(s)
        .map(String::from)
        .unwrap_or_default()
}

/// Join room `code` (for `players` players) of the lobby at `lobby_url`
/// (`ws://host:7401`); `stun` adds an ICE server.
pub fn meet(
    lobby_url: &str,
    code: &str,
    players: usize,
    stun: Option<&str>,
) -> Result<Meeting, String> {
    let ws = WebSocket::new(lobby_url).map_err(js_err)?;
    let shared = Rc::new(RefCell::new(Shared {
        status: Some(MeetStatus::Connecting),
        players,
        ..Shared::default()
    }));
    let stun = stun.map(str::to_string);
    {
        let ws2 = ws.clone();
        let join = format!("JOIN {code} {players}");
        let onopen = Closure::<dyn FnMut()>::new(move || {
            let _ = ws2.send_with_str(&join);
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        shared.borrow_mut().closures.push(Box::new(onopen));
    }
    {
        let sh = shared.clone();
        let ws2 = ws.clone();
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            if let Some(line) = e.data().as_string() {
                on_line(&sh, &ws2, &line, stun.as_deref());
            }
        });
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        shared.borrow_mut().closures.push(Box::new(onmessage));
    }
    {
        let sh = shared.clone();
        let onerror = Closure::<dyn FnMut()>::new(move || {
            fail(&sh, "the lobby's WebSocket failed".into());
        });
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        shared.borrow_mut().closures.push(Box::new(onerror));
    }
    Ok(Meeting { shared, ws })
}

/// A data channel's handlers: open counts, messages go to the inbox.
fn wire_channel(shared: &Rc<RefCell<Shared>>, seat: usize, dc: RtcDataChannel) {
    dc.set_binary_type(RtcDataChannelType::Arraybuffer);
    let sh = shared.clone();
    let onopen = Closure::<dyn FnMut()>::new(move || {
        let mut s = sh.borrow_mut();
        s.open += 1;
        let need = s.players - 1;
        s.status = Some(if s.open >= need {
            MeetStatus::Ready
        } else {
            MeetStatus::Linking { open: s.open, need }
        });
    });
    dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    let sh = shared.clone();
    let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Ok(buf) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
            let bytes = js_sys::Uint8Array::new(&buf).to_vec();
            sh.borrow_mut().inbox.push_back((seat, bytes));
        }
    });
    dc.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    let mut s = shared.borrow_mut();
    s.closures.push(Box::new(onopen));
    s.closures.push(Box::new(onmessage));
    s.channels.insert(seat, dc);
}

/// A connection to `seat`: its ICE candidates go through the lobby.
fn peer(
    shared: &Rc<RefCell<Shared>>,
    ws: &WebSocket,
    seat: usize,
    stun: Option<&str>,
) -> Result<RtcPeerConnection, String> {
    let cfg = RtcConfiguration::new();
    if let Some(url) = stun {
        let server = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&server, &"urls".into(), &url.into());
        cfg.set_ice_servers(&js_sys::Array::of1(&server));
    }
    let pc = RtcPeerConnection::new_with_configuration(&cfg).map_err(js_err)?;
    let ws2 = ws.clone();
    let onice = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(
        move |e: RtcPeerConnectionIceEvent| {
            if let Some(c) = e.candidate() {
                let json = js_sys::JSON::stringify(&c.to_json())
                    .ok()
                    .and_then(|s| s.as_string())
                    .unwrap_or_default();
                let _ = ws2.send_with_str(&format!("SIGNAL {seat} ice:{}", encode(&json)));
            }
        },
    );
    pc.set_onicecandidate(Some(onice.as_ref().unchecked_ref()));
    let sh = shared.clone();
    let ondc = Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |e: RtcDataChannelEvent| {
        wire_channel(&sh, seat, e.channel());
    });
    pc.set_ondatachannel(Some(ondc.as_ref().unchecked_ref()));
    let mut s = shared.borrow_mut();
    s.closures.push(Box::new(onice));
    s.closures.push(Box::new(ondc));
    s.peers.insert(seat, pc.clone());
    Ok(pc)
}

fn on_line(shared: &Rc<RefCell<Shared>>, ws: &WebSocket, line: &str, stun: Option<&str>) {
    let mut w = line.splitn(2, ' ');
    match w.next() {
        Some("WAIT") => {
            let n: Vec<usize> = line
                .split_whitespace()
                .skip(2)
                .filter_map(|v| v.parse().ok())
                .collect();
            if let [have, need] = n[..] {
                shared.borrow_mut().status = Some(MeetStatus::Waiting { have, need });
            }
        }
        Some("START") => {
            let Some(start) = crate::lobby::parse_start(line) else {
                return fail(shared, format!("lobby: bad `{line}`"));
            };
            {
                let mut s = shared.borrow_mut();
                s.seat = start.seat;
                s.players = start.players;
                s.status = Some(MeetStatus::Linking {
                    open: 0,
                    need: start.players - 1,
                });
                if !s.peers.is_empty() {
                    return; // a repeated START
                }
            }
            for (seat, _) in start.members.iter().filter(|(s, _)| *s != start.seat) {
                let seat = *seat;
                let pc = match peer(shared, ws, seat, stun) {
                    Ok(pc) => pc,
                    Err(e) => return fail(shared, e),
                };
                if start.seat < seat {
                    // The lower seat offers, with the channel.
                    let init = RtcDataChannelInit::new();
                    init.set_ordered(false);
                    init.set_max_retransmits(0);
                    let dc = pc.create_data_channel_with_data_channel_dict("trino", &init);
                    wire_channel(shared, seat, dc);
                    let ws = ws.clone();
                    let sh = shared.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let r: Result<(), JsValue> = async {
                            let offer = JsFuture::from(pc.create_offer()).await?;
                            let sdp = js_sys::Reflect::get(&offer, &"sdp".into())?
                                .as_string()
                                .unwrap_or_default();
                            let desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
                            desc.set_sdp(&sdp);
                            JsFuture::from(pc.set_local_description(&desc)).await?;
                            let _ =
                                ws.send_with_str(&format!("SIGNAL {seat} offer:{}", encode(&sdp)));
                            Ok(())
                        }
                        .await;
                        if let Err(e) = r {
                            fail(&sh, format!("offer: {}", js_err(e)));
                        }
                    });
                }
            }
        }
        Some("FROM") => {
            let mut p = line.splitn(3, ' ');
            let (_, Some(seat), Some(body)) = (p.next(), p.next(), p.next()) else {
                return;
            };
            let Ok(seat) = seat.parse::<usize>() else {
                return;
            };
            let Some(pc) = shared.borrow().peers.get(&seat).cloned() else {
                return;
            };
            let (kind, payload) = body.split_once(':').unwrap_or((body, ""));
            let text = decode(payload);
            let ws = ws.clone();
            let sh = shared.clone();
            let kind = kind.to_string();
            wasm_bindgen_futures::spawn_local(async move {
                let r: Result<(), JsValue> = async {
                    match kind.as_str() {
                        "offer" => {
                            let desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
                            desc.set_sdp(&text);
                            JsFuture::from(pc.set_remote_description(&desc)).await?;
                            let answer = JsFuture::from(pc.create_answer()).await?;
                            let sdp = js_sys::Reflect::get(&answer, &"sdp".into())?
                                .as_string()
                                .unwrap_or_default();
                            let local = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                            local.set_sdp(&sdp);
                            JsFuture::from(pc.set_local_description(&local)).await?;
                            let _ =
                                ws.send_with_str(&format!("SIGNAL {seat} answer:{}", encode(&sdp)));
                        }
                        "answer" => {
                            let desc = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                            desc.set_sdp(&text);
                            JsFuture::from(pc.set_remote_description(&desc)).await?;
                        }
                        "ice" => {
                            let obj = js_sys::JSON::parse(&text)?;
                            let cand = js_sys::Reflect::get(&obj, &"candidate".into())?
                                .as_string()
                                .unwrap_or_default();
                            let init = RtcIceCandidateInit::new(&cand);
                            let mid = js_sys::Reflect::get(&obj, &"sdpMid".into())?.as_string();
                            init.set_sdp_mid(mid.as_deref());
                            let line = js_sys::Reflect::get(&obj, &"sdpMLineIndex".into())?
                                .as_f64()
                                .map(|v| v as u16);
                            init.set_sdp_m_line_index(line);
                            JsFuture::from(
                                pc.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init)),
                            )
                            .await?;
                        }
                        _ => {}
                    }
                    Ok(())
                }
                .await;
                if let Err(e) = r {
                    fail(&sh, format!("signal {kind}: {}", js_err(e)));
                }
            });
        }
        Some("ERROR") => fail(shared, format!("lobby: {line}")),
        _ => {}
    }
}

/// What a finished meeting hands over: this page's seat, the players,
/// the other seats' addresses and the socket.
pub type Met = (usize, usize, Vec<(usize, PeerAddr)>, RtcSocket);

impl Meeting {
    pub fn status(&self) -> MeetStatus {
        self.shared
            .borrow()
            .status
            .clone()
            .unwrap_or(MeetStatus::Connecting)
    }

    /// Once [`MeetStatus::Ready`]: this page's seat, the players, the
    /// other seats' addresses and the socket.
    pub fn take(self) -> Result<Met, Meeting> {
        if self.status() != MeetStatus::Ready {
            return Err(self);
        }
        let (seat, players, peers) = {
            let s = self.shared.borrow();
            let peers = s
                .channels
                .keys()
                .map(|k| (*k, PeerAddr::Rtc(*k as u16)))
                .collect();
            (s.seat, s.players, peers)
        };
        Ok((
            seat,
            players,
            peers,
            RtcSocket {
                shared: self.shared,
                _ws: self.ws,
            },
        ))
    }
}

impl NonBlockingSocket<PeerAddr> for RtcSocket {
    fn send_to(&mut self, msg: &Message, addr: &PeerAddr) {
        let PeerAddr::Rtc(seat) = addr else { return };
        let Ok(bytes) = bincode::serialize(msg) else {
            return;
        };
        if let Some(dc) = self.shared.borrow().channels.get(&(*seat as usize)) {
            let _ = dc.send_with_u8_array(&bytes);
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(PeerAddr, Message)> {
        let mut s = self.shared.borrow_mut();
        s.inbox
            .drain(..)
            .filter_map(|(seat, b)| {
                let m = bincode::deserialize::<Message>(&b).ok()?;
                Some((PeerAddr::Rtc(seat as u16), m))
            })
            .collect()
    }
}
