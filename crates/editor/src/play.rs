//! **Play in editor**: the open scene runs inside the Game view, in the
//! editor process — no build, no window switch, unsaved edits included.
//!
//! The scene (as currently edited, prefabs and materials resolved) is baked
//! to a `TSCN` blob in memory and handed to the scene runtime that ships
//! with the engine examples (`showcase::ShowcaseGame::from_baked`): rules by
//! **tag** — a `player`-tagged entity walks with physics against the
//! scene's colliders, `coin` triggers are collected, a `goal` trigger wins;
//! without a player the camera simply frames the scene. Real game crates
//! still run in their own process (Play ▸ Play the game) so a crash in game
//! code never takes the editor down.

use std::time::Instant;

use eframe::egui;
use trino_core::{BakedScene, Button, InputState, Vec2};

/// The editor plays silently for now (no audio device from the editor).
pub struct NullAudio;

impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// The platform Play here's game runs on: the players (issue #67), nothing
/// else (no blobs or saves: the scene runtime needs none).
#[derive(Default)]
pub struct EditorPlatform {
    pub players: trino_core::HostedPlayers,
}

impl trino_core::Platform for EditorPlatform {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn hosted_players(&self) -> Option<&trino_core::HostedPlayers> {
        Some(&self.players)
    }
    fn hosted_players_mut(&mut self) -> Option<&mut trino_core::HostedPlayers> {
        Some(&mut self.players)
    }
}

/// What a simulated seat plays (the split-input simulator, issue #67).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeatMode {
    /// The keyboard's left half: WASD, F (A), G (B), R (X), T (Y), Q/E
    /// (L/R), Tab (Start). With one player, the whole keyboard as the PC
    /// app maps it.
    KeysLeft,
    /// The keyboard's right half: arrows, K (A), L (B), I (X), O (Y), U/P
    /// (L/R), Enter (Start).
    KeysRight,
    /// Plugged in, nothing pressed.
    Idle,
    /// Player 1's input, a second pad held the same way.
    CopyP1,
    /// Walks in circles and presses A now and then (seeded per seat).
    Wander,
}

impl SeatMode {
    pub const ALL: [SeatMode; 5] = [
        SeatMode::KeysLeft,
        SeatMode::KeysRight,
        SeatMode::Idle,
        SeatMode::CopyP1,
        SeatMode::Wander,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SeatMode::KeysLeft => "Keys: WASD + F G R T",
            SeatMode::KeysRight => "Keys: arrows + K L I O",
            SeatMode::Idle => "Idle pad",
            SeatMode::CopyP1 => "Copy player 1",
            SeatMode::Wander => "Wander (bot)",
        }
    }
}

/// The local seats Play here simulates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayersSim {
    /// 1..=4 players.
    pub count: u8,
    pub seats: [SeatMode; 4],
}

impl Default for PlayersSim {
    fn default() -> Self {
        PlayersSim {
            count: 1,
            seats: [
                SeatMode::KeysLeft,
                SeatMode::KeysRight,
                SeatMode::Wander,
                SeatMode::Idle,
            ],
        }
    }
}

impl PlayersSim {
    /// `TRINO_EDITOR_PLAYERS=2` (1..=4).
    pub fn from_env() -> PlayersSim {
        let count = std::env::var("TRINO_EDITOR_PLAYERS")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(1)
            .clamp(1, 4);
        PlayersSim {
            count,
            ..PlayersSim::default()
        }
    }

    /// Every seat's input this frame (`frame` seeds the bots).
    pub fn poll(&self, down: impl Fn(egui::Key) -> bool, frame: u64) -> trino_core::PlayersInput {
        let mut p = trino_core::PlayersInput::NONE;
        if self.count <= 1 {
            return trino_core::PlayersInput::single(input_from_keys(down));
        }
        for seat in 0..self.count as usize {
            p.connected |= 1 << seat;
            p.players[seat] = match self.seats[seat] {
                SeatMode::KeysLeft => half_keys(&down, false),
                SeatMode::KeysRight => half_keys(&down, true),
                SeatMode::Idle => InputState::default(),
                SeatMode::CopyP1 => p.players[0],
                SeatMode::Wander => wander(seat, frame),
            };
        }
        p
    }
}

/// One half of the keyboard as a pad.
fn half_keys(down: &impl Fn(egui::Key) -> bool, right: bool) -> InputState {
    use egui::Key;
    let mut s = InputState::default();
    let map: [(Key, Button); 7] = if right {
        [
            (Key::K, Button::A),
            (Key::L, Button::B),
            (Key::I, Button::X),
            (Key::O, Button::Y),
            (Key::U, Button::L),
            (Key::P, Button::R),
            (Key::Enter, Button::Start),
        ]
    } else {
        [
            (Key::F, Button::A),
            (Key::G, Button::B),
            (Key::R, Button::X),
            (Key::T, Button::Y),
            (Key::Q, Button::L),
            (Key::E, Button::R),
            (Key::Tab, Button::Start),
        ]
    };
    for (key, button) in map {
        if down(key) {
            s.set(button, true);
        }
    }
    let (l, r, u, d) = if right {
        (
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::ArrowUp,
            Key::ArrowDown,
        )
    } else {
        (Key::A, Key::D, Key::W, Key::S)
    };
    let axis = |neg: Key, pos: Key| (down(pos) as i32 - down(neg) as i32) as f32;
    s.stick = Vec2::new(axis(l, r), axis(d, u));
    s
}

/// A bot seat: a slow circle, A every two seconds.
fn wander(seat: usize, frame: u64) -> InputState {
    let mut s = InputState::default();
    let t = (frame as f32 / 60.0) * (0.7 + seat as f32 * 0.2);
    s.stick = Vec2::new(trino_core::math3d::cos(t), trino_core::math3d::sin(t));
    s.set(Button::A, (frame + seat as u64 * 31) % 120 < 6);
    s
}

/// A recording in progress (`● Rec`).
pub struct Recording {
    session: trino_core::replay::Session<'static>,
    pub bytes: Vec<u8>,
}

/// A recording played back in the Game view, with its scrubber.
pub struct ReplayView {
    /// Every recorded frame.
    pub frames: Vec<trino_core::replay::Frame>,
    pub header: trino_core::replay::Header,
    /// Frames played so far (the scrubber's position).
    pub pos: u32,
    pub playing: bool,
    /// The first frame whose hash differed (frames run when seen).
    pub diverged: Option<trino_core::replay::Divergence>,
    pub checked: u32,
    /// Snapshots to seek from: (frames run, the game's state).
    keys: Vec<(u32, Vec<u8>)>,
    /// The recording's start state differs from this scene's.
    pub other_start: bool,
    pub name: String,
}

/// Frames between the scrubber's snapshots.
pub const KEY_EVERY: u32 = 60;

/// One running in-editor play session.
pub struct PlaySession {
    pub game: Box<showcase::ShowcaseGame>,
    pub last: Instant,
    pub frames: u64,
    pub platform: EditorPlatform,
    pub players: PlayersSim,
    pub recording: Option<Recording>,
    pub replay: Option<ReplayView>,
    /// The scene's name (recordings carry it: `scene/<name>`).
    pub scene_name: String,
}

impl PlaySession {
    /// Bake `scene` against `lib` for `target` (the Game view's platform:
    /// its variants) and start the scene runtime at the Game view's
    /// internal resolution. The blob is leaked (a few KB per Play
    /// press — the runtime keeps `'static` names and tags, like consoles).
    #[cfg(test)]
    pub fn start(
        scene: &trino_scene::Scene,
        lib: &trino_scene::Library,
        target: trino_scene::Target,
        screen: (u32, u32),
    ) -> Result<Self, String> {
        Self::start_with(scene, lib, target, screen, &Default::default(), false)
    }

    /// [`Self::start`] with the project's collision meshes (`TCOL` blobs
    /// by id, for `Mesh` colliders) and, with `simulate`, the hero on the
    /// engine's character controller even without a `character` body.
    pub fn start_with(
        scene: &trino_scene::Scene,
        lib: &trino_scene::Library,
        target: trino_scene::Target,
        screen: (u32, u32),
        collision: &std::collections::BTreeMap<u32, Vec<u8>>,
        simulate: bool,
    ) -> Result<Self, String> {
        let (blob, warnings) = trino_scene::bake::to_tscn_for(scene, lib, target);
        if let Some(w) = warnings.first() {
            return Err(format!("cannot play: {w}"));
        }
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let baked = BakedScene::parse(blob).map_err(|e| format!("baked scene: {e:?}"))?;
        let mut game = Box::new(showcase::ShowcaseGame::from_baked(
            &baked,
            Vec2::new(screen.0 as f32, screen.1 as f32),
        ));
        // TSCN carries no AO (consoles bake theirs): the scene's own.
        game.ao = scene.environment.core_ao();
        game.stereo = scene.environment.core_stereo();
        game.post = scene.environment.core_post();
        // Mesh colliders: their baked triangles (leaked like the scene).
        for (&id, bytes) in collision {
            if game.attach_mesh(id, Box::leak(bytes.clone().into_boxed_slice())) == 0 {
                continue;
            }
        }
        if simulate {
            game.use_controller(trino_core::physics::Character::DEFAULT);
        }
        Ok(PlaySession {
            game,
            last: Instant::now(),
            frames: 0,
            platform: EditorPlatform::default(),
            players: PlayersSim::from_env(),
            recording: None,
            replay: None,
            scene_name: scene.name.clone(),
        })
    }

    /// Seconds since the previous step, clamped like the PC app does.
    pub fn tick(&mut self) -> f32 {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.frames += 1;
        dt
    }

    /// The name a recording of this scene carries (`scene/<name>`).
    pub fn game_name(&self) -> String {
        format!("scene/{}", self.scene_name)
    }

    /// One frame of play: the simulated seats (or the replay's next frame),
    /// published as the platform's players, then the game's update. A
    /// recording takes the frame with its hash.
    pub fn step(&mut self, down: impl Fn(egui::Key) -> bool, dt: f32) {
        use trino_core::Game as _;
        if let Some(r) = &mut self.replay {
            if r.playing && (r.pos as usize) < r.frames.len() {
                let f = r.frames[r.pos as usize];
                Self::advance(&mut self.game, &mut self.platform, &f.players, f.dt);
                r.pos += 1;
                r.check(&f, self.game.as_ref());
                if r.pos.is_multiple_of(KEY_EVERY) && !r.keys.iter().any(|k| k.0 == r.pos) {
                    r.keys.push((r.pos, snapshot(self.game.as_ref())));
                }
            }
            return;
        }
        let players = self.players.poll(down, self.frames);
        Self::advance(&mut self.game, &mut self.platform, &players, dt);
        if let Some(rec) = &mut self.recording {
            let out = &mut rec.bytes;
            rec.session
                .after(self.game.as_ref(), &players, dt, &mut |b: &[u8]| {
                    out.extend_from_slice(b)
                });
        }
        let _ = self.game.state_hash();
    }

    fn advance(
        game: &mut showcase::ShowcaseGame,
        platform: &mut EditorPlatform,
        players: &trino_core::PlayersInput,
        dt: f32,
    ) {
        use trino_core::Game as _;
        platform.players.publish_players(players);
        game.update_with(platform, &players.players[0], &mut NullAudio, dt);
    }

    /// Start recording from this frame (the app restarts Play here first,
    /// so a recording begins where a replay begins: the scene's start).
    pub fn begin_recording(&mut self) {
        use trino_core::replay::{Header, Session};
        let (w, h) = self.screen();
        let header = Header::new(&self.game_name(), (w, h), self.players.count.max(1), 1);
        let mut bytes = Vec::new();
        let mut session = Session::record(header, &mut |b: &[u8]| bytes.extend_from_slice(b));
        if let Some(head) = session.start(self.game.as_ref()) {
            bytes[..trino_core::replay::HEADER_LEN].copy_from_slice(&head);
        }
        self.recording = Some(Recording { session, bytes });
    }

    /// Frames recorded so far.
    pub fn recorded(&self) -> Option<u32> {
        self.recording.as_ref().map(|r| r.session.frames())
    }

    /// End the recording: its bytes (header final), to save.
    pub fn end_recording(&mut self) -> Option<Vec<u8>> {
        let mut r = self.recording.take()?;
        if let Some(head) = r.session.finish_header() {
            r.bytes[..trino_core::replay::HEADER_LEN].copy_from_slice(&head);
        }
        Some(r.bytes)
    }

    fn screen(&self) -> (u16, u16) {
        let s = self.game.screen();
        (s.x as u16, s.y as u16)
    }

    /// Play `bytes` back from this (fresh) game: decoded at once, the start
    /// state kept as the scrubber's first snapshot.
    pub fn load_replay(&mut self, bytes: &[u8], name: &str) -> Result<(), String> {
        use trino_core::Game as _;
        let mut reader =
            trino_core::replay::Reader::new(bytes).map_err(|e| e.message().to_string())?;
        let header = *reader.header();
        let mut frames = Vec::new();
        while let Some(f) = reader.next_frame() {
            frames.push(f.map_err(|e| e.message().to_string())?);
        }
        let other_start =
            header.start_hash != 0 && Some(header.start_hash) != self.game.state_hash();
        self.recording = None;
        self.replay = Some(ReplayView {
            frames,
            header,
            pos: 0,
            playing: true,
            diverged: None,
            checked: 0,
            keys: vec![(0, snapshot(self.game.as_ref()))],
            other_start,
            name: name.to_string(),
        });
        Ok(())
    }

    /// Jump the replay to `target` frames played: the nearest snapshot at or
    /// before it, then the frames up to it simulated again (no drawing).
    pub fn seek(&mut self, target: u32) {
        use trino_core::Game as _;
        let Some(r) = &mut self.replay else { return };
        let target = target.min(r.frames.len() as u32);
        let Some((at, bytes)) = r
            .keys
            .iter()
            .filter(|k| k.0 <= target)
            .max_by_key(|k| k.0)
            .cloned()
        else {
            return;
        };
        self.game.load_state(&bytes);
        r.pos = at;
        while r.pos < target {
            let f = r.frames[r.pos as usize];
            Self::advance(&mut self.game, &mut self.platform, &f.players, f.dt);
            r.pos += 1;
            r.check(&f, self.game.as_ref());
            if r.pos.is_multiple_of(KEY_EVERY) && !r.keys.iter().any(|k| k.0 == r.pos) {
                r.keys.push((r.pos, snapshot(self.game.as_ref())));
            }
        }
    }
}

impl ReplayView {
    fn check(&mut self, f: &trino_core::replay::Frame, game: &dyn trino_core::Game) {
        let (Some(want), Some(got)) = (f.hash, game.state_hash()) else {
            return;
        };
        self.checked += 1;
        if want != got && self.diverged.is_none_or(|d| d.frame > f.index + 1) {
            self.diverged = Some(trino_core::replay::Divergence {
                frame: f.index + 1,
                expected: want,
                got,
            });
        }
    }

    /// The players of the frame the scrubber is on.
    pub fn players(&self) -> Option<&trino_core::PlayersInput> {
        let i = self.pos.checked_sub(1)?;
        self.frames.get(i as usize).map(|f| &f.players)
    }
}

/// The game's state, for a scrubber snapshot.
fn snapshot(game: &dyn trino_core::Game) -> Vec<u8> {
    let mut buf = vec![0u8; game.state_size()];
    let n = game.save_state(&mut buf).unwrap_or(0);
    buf.truncate(n);
    buf
}

/// Where Play here's recordings go: `<project>/recordings/<scene>-<n>.trec`
/// (the first free `n`).
pub fn recording_path(root: &std::path::Path, scene: &str) -> std::path::PathBuf {
    let dir = root.join("recordings");
    let stem: String = scene
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    (1..)
        .map(|n| dir.join(format!("{stem}-{n}.trec")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem}.trec")))
}

/// The recordings of `root/recordings`, newest first.
pub fn recordings(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<(std::time::SystemTime, std::path::PathBuf)> =
        std::fs::read_dir(root.join("recordings"))
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "trec"))
                    .map(|e| {
                        (
                            e.metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::UNIX_EPOCH),
                            e.path(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
    v.sort_by_key(|e| std::cmp::Reverse(e.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// The PC keyboard mapping (`trino_platform_pc::PcInput`), from egui keys:
/// A/B = Z/X, X/Y = C/V, L/R = Q/E, Start = Enter, D-pad = arrows,
/// stick = WASD. Space doubles as A (jumping without hunting for Z).
pub fn input_from_keys(down: impl Fn(egui::Key) -> bool) -> InputState {
    use egui::Key;
    let mut s = InputState::default();
    for (key, button) in [
        (Key::Z, Button::A),
        (Key::Space, Button::A),
        (Key::X, Button::B),
        (Key::C, Button::X),
        (Key::V, Button::Y),
        (Key::Q, Button::L),
        (Key::E, Button::R),
        (Key::Enter, Button::Start),
        (Key::ArrowUp, Button::DpadUp),
        (Key::ArrowDown, Button::DpadDown),
        (Key::ArrowLeft, Button::DpadLeft),
        (Key::ArrowRight, Button::DpadRight),
    ] {
        if down(key) {
            s.set(button, true);
        }
    }
    let axis = |neg: Key, pos: Key| (down(pos) as i32 - down(neg) as i32) as f32;
    // Stick is Y-up like real hardware.
    s.stick = Vec2::new(axis(Key::A, Key::D), axis(Key::S, Key::W));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_like_the_pc_app() {
        let held = [egui::Key::Space, egui::Key::W, egui::Key::D, egui::Key::Q];
        let s = input_from_keys(|k| held.contains(&k));
        assert!(s.is_down(Button::A));
        assert!(s.is_down(Button::L));
        assert!(!s.is_down(Button::B));
        assert_eq!(s.stick, Vec2::new(1.0, 1.0));
    }

    fn showcase_session() -> PlaySession {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scene = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
        let mut lib = trino_scene::Library::load(&root);
        lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
        PlaySession::start(&scene, &lib, trino_scene::Target::Pc, (320, 240)).unwrap()
    }

    /// Record a run with two simulated players, replay it on a fresh
    /// session hash for hash, then seek back and forth with the scrubber.
    #[test]
    fn records_replays_and_scrubs_a_scene() {
        let mut s = showcase_session();
        s.players.count = 2;
        s.players.seats[1] = SeatMode::Wander;
        s.begin_recording();
        let held = [egui::Key::W, egui::Key::D];
        for f in 0..300u32 {
            let keys = |k: egui::Key| {
                held.contains(&k) && f % 90 < 60 || (k == egui::Key::F && f % 70 == 0)
            };
            s.step(keys, 1.0 / 60.0);
        }
        assert_eq!(s.recorded(), Some(300));
        let end = trino_core::Game::state_hash(s.game.as_ref());
        let bytes = s.end_recording().unwrap();
        let header = trino_core::replay::Header::parse(&bytes).unwrap();
        assert_eq!(header.players, 2);
        assert_eq!(header.game, trino_core::asset_id("scene/showcase"));

        let mut r = showcase_session();
        r.load_replay(&bytes, "test").unwrap();
        assert!(!r.replay.as_ref().unwrap().other_start);
        for _ in 0..300 {
            r.step(|_| false, 0.0);
        }
        let v = r.replay.as_ref().unwrap();
        assert_eq!((v.pos, v.checked, v.diverged), (300, 300, None));
        assert_eq!(trino_core::Game::state_hash(r.game.as_ref()), end);
        // Player 2's bot reached the recording.
        assert!(v.players().unwrap().is_connected(1));
        // Back to frame 75 (from the snapshot at 60) and forward again.
        r.seek(75);
        assert_eq!(r.replay.as_ref().unwrap().pos, 75);
        let at_75 = trino_core::Game::state_hash(r.game.as_ref());
        r.seek(299);
        r.seek(75);
        assert_eq!(trino_core::Game::state_hash(r.game.as_ref()), at_75);
        r.seek(300);
        assert_eq!(trino_core::Game::state_hash(r.game.as_ref()), end);
        assert_eq!(r.replay.as_ref().unwrap().diverged, None);
    }

    /// A two-player recording of the showcase scene for screenshots and
    /// the docs (`target/recordings/showcase-demo.trec`).
    #[test]
    #[ignore = "writes target/recordings/showcase-demo.trec"]
    fn write_demo_recording() {
        let mut s = showcase_session();
        s.players.count = 2;
        s.players.seats[1] = SeatMode::Wander;
        s.begin_recording();
        for f in 0..360u32 {
            let keys = |k: egui::Key| match k {
                egui::Key::W => f % 120 < 90,
                egui::Key::D => f % 200 < 70,
                egui::Key::F => f % 80 == 0,
                _ => false,
            };
            s.step(keys, 1.0 / 60.0);
        }
        let bytes = s.end_recording().unwrap();
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/recordings");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("showcase-demo.trec"), bytes).unwrap();
    }

    #[test]
    fn the_split_keyboard_seats_two_players() {
        let sim = PlayersSim {
            count: 2,
            ..PlayersSim::default()
        };
        let held = [
            egui::Key::W,
            egui::Key::F,
            egui::Key::ArrowLeft,
            egui::Key::K,
        ];
        let p = sim.poll(|k| held.contains(&k), 0);
        assert_eq!(p.connected, 0b11);
        assert_eq!(p.players[0].stick, Vec2::new(0.0, 1.0));
        assert!(p.players[0].is_down(Button::A) && !p.players[0].is_down(Button::B));
        assert_eq!(p.players[1].stick, Vec2::new(-1.0, 0.0));
        assert!(p.players[1].is_down(Button::A));
        // One player: the whole keyboard, as before.
        let one = PlayersSim::default().poll(|k| k == egui::Key::Space, 0);
        assert_eq!(one.connected, 1);
        assert!(one.players[0].is_down(Button::A));
    }

    #[test]
    fn plays_the_showcase_scene_in_memory() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scene = trino_scene::Scene::load(&root.join("scenes/showcase.scene.ron")).unwrap();
        let mut lib = trino_scene::Library::load(&root);
        lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&root.join("assets"));
        let mut s = PlaySession::start(&scene, &lib, trino_scene::Target::Pc, (320, 240)).unwrap();
        assert_eq!(s.game.total_coins, 5);
        let r = showcase::run_demo(&mut s.game, |g, input| {
            trino_core::Game::update(g, input, &mut NullAudio, 1.0 / 60.0)
        });
        assert!(r.is_ok(), "{r:?}");
    }
}
