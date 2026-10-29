//! Targets over the parsers outside `trino-core`: the PC's and the Web's
//! player files, the Web asset pack, the RON formats agents and players
//! edit, the pipeline's 3DS sound container and the network messages.

use std::path::Path;

fn text(data: &[u8]) -> Option<&str> {
    std::str::from_utf8(data).ok()
}

fn keep<T>(v: T) {
    std::hint::black_box(v);
}

/// `<save>.input`: one player's and every player's bindings, written back
/// and read again to the same bindings.
pub fn input_text(data: &[u8]) {
    use trino_platform_pc::input;
    let Some(t) = text(data) else { return };
    let b = input::bindings_from_text(t);
    let back = input::bindings_from_text(&input::bindings_to_text(&b));
    assert_eq!(input::bindings_to_text(&back), input::bindings_to_text(&b));
    let all = input::players_bindings_from_text(t);
    let again = input::players_bindings_from_text(&input::players_bindings_to_text(&all));
    assert_eq!(
        input::players_bindings_to_text(&again),
        input::players_bindings_to_text(&all)
    );
}

/// `<save>.display`.
pub fn display_text(data: &[u8]) {
    use trino_platform_pc::system::{display_settings_from_text, display_settings_to_text};
    let Some(t) = text(data) else { return };
    let s = display_settings_from_text(t);
    assert_eq!(display_settings_from_text(&display_settings_to_text(&s)), s);
}

/// `<save>.access`, `<save>.audio` and `<save>.quality`.
pub fn settings_text(data: &[u8]) {
    use trino_platform_pc::{audio, system};
    let Some(t) = text(data) else { return };
    let a = system::access_settings_from_text(t);
    assert_eq!(
        system::access_settings_from_text(&system::access_settings_to_text(&a)),
        a
    );
    // The file keeps two decimals: what the game writes reads back as it
    // wrote it (a hand-edited 0.947 settles at 0.95 once).
    let b = audio::settings_from_text(t);
    let written = audio::settings_to_text(&b);
    assert_eq!(
        audio::settings_to_text(&audio::settings_from_text(&written)),
        written
    );
}

/// `<save>.achievements` against the engine's fixture table, `<save>.crash`
/// and `trino-package.txt`.
pub fn services_text(data: &[u8]) {
    let Some(t) = text(data) else { return };
    let fixture = achievements_fixture();
    let table = trino_core::services::table::AchievementTable::parse(&fixture).unwrap();
    let st = trino_platform_pc::system::services_from_text(t, &table);
    let again = trino_platform_pc::system::services_from_text(
        &trino_platform_pc::system::services_to_text(&st, &table),
        &table,
    );
    assert_eq!(again, st, "achievements file round trip");
    keep(trino_runner_pc::crash::consent_from_text(t));
    let p = trino_runner_pc::package::PackageInfo::parse(t, Path::new("pkg"));
    keep(p.to_text());
}

/// The Web: the asset pack (`TPAK`, its `TS16` sounds and `TSPR` sprites),
/// base64 saves and gamepad id strings.
pub fn tpak(data: &[u8]) {
    use trino_platform_web::pack;
    if let Ok((_, entries)) = pack::parse_versioned(data) {
        for e in entries.iter().take(1024) {
            keep(pack::sound_blob(e.kind, e.bytes).map(|b| b.len()));
            keep(pack::read_sprite(e.bytes).map(|(w, h, _)| (w, h)));
            if e.bytes.len() <= 1 << 16 {
                keep(pack::read_sound16(e.bytes).map(|(r, s)| (r, s.len())));
            }
        }
    }
    keep(pack::read_sprite(data).map(|(w, h, _)| (w, h)));
    if data.len() <= 1 << 16 {
        keep(pack::read_sound16(data).map(|(r, s)| (r, s.len())));
    }
    if let Some(t) = text(data) {
        if let Some(bytes) = trino_platform_web::b64::decode(t) {
            assert_eq!(
                trino_platform_web::b64::decode(&trino_platform_web::b64::encode(&bytes)),
                Some(bytes)
            );
        }
        keep(trino_platform_web::keys::parse_pad_id(t));
    }
}

/// A TSN2 sound's payload as the PC and the Web decode it (PCM16, or Ogg
/// Vorbis through lewton).
pub fn tsn2_decode(data: &[u8]) {
    let Ok(blob) = trino_core::SoundBlob::parse(data) else {
        return;
    };
    if blob.frames as usize > crate::core_targets::MAX_DECODE {
        return;
    }
    // A prefix of the audio (issue #121): the setup header and the first
    // packets are where a bad file goes wrong; minutes of valid audio
    // only cost the fuzzer time.
    keep(
        trino_platform_pc::audio::decode_blob_limit(
            blob.codec,
            blob.sample_rate,
            blob.payload,
            TSN2_DECODE_SAMPLES,
        )
        .map(|(r, s)| (r, s.len())),
    );
}

/// The samples `tsn2_decode` decodes of a payload at most.
pub const TSN2_DECODE_SAMPLES: usize = 1 << 14;

/// The 3DS's `T3AU` sound container (`asset-pipeline`, read back by
/// `audio inspect` and the editor).
pub fn t3au(data: &[u8]) {
    let Ok(t) = trino_asset_pipeline::audio::T3au::parse(data) else {
        return;
    };
    if t.samples <= crate::core_targets::MAX_DECODE {
        keep(t.decode().len());
    }
}

/// Scenes and prefabs, the editor's biggest RON files (split out of `ron`
/// in issue #121: each full-document parse of a 20 KB scene costs ~1 ms,
/// and `ron` paid six of them per input). A scene the editor writes back
/// parses to the same scene.
pub fn ron_scene(data: &[u8]) {
    use trino_scene as s;
    let Some(t) = text(data) else { return };
    if let Ok(scene) = s::Scene::from_ron(t) {
        // What the editor writes back must parse to the same scene.
        let out = scene.to_ron();
        let back = s::Scene::from_ron(&out).expect("a written scene parses");
        assert_eq!(back.to_ron(), out, "scene RON round trip");
    }
    keep(s::prefab::Prefab::from_ron(t).is_ok());
}

/// Every other RON format an agent or the editor writes: materials, UI
/// screens, string tables, achievements and its lock, palettes, physics
/// layers, animation graphs (scenes and prefabs: [`ron_scene`]).
pub fn ron(data: &[u8]) {
    use trino_scene as s;
    let Some(t) = text(data) else { return };
    keep(s::material::MaterialDef::from_ron(t).is_ok());
    if let Ok(screen) = s::ui::ScreenDef::from_ron(t) {
        keep(s::ui::validate(&screen).len());
    }
    keep(s::i18n::StringsDef::from_ron(t).is_ok());
    keep(s::services::AchievementsDef::from_ron(t).is_ok());
    keep(s::services::lock_from_text(t));
    keep(s::access::PalettesDef::from_ron(t).is_ok());
    keep(s::physics::PhysicsSettings::from_ron(t).is_ok());
    keep(s::anim::AnimGraphDef::from_ron(t).is_ok());
    keep(s::anim::ScriptDef::from_ron(t).is_ok());
    keep(s::i18n::parse_csv(t).map(|r| r.len()));
}

/// The lobby server's lines (`JOIN`, `SIGNAL`, `LEAVE`, from anyone on the
/// network), the clients' `START`, ggrs messages off a socket, and the
/// network config text.
pub fn net(data: &[u8]) {
    use trino_net::lobby;
    keep(trino_net::decode_message(data).is_some());
    let Some(t) = text(data) else { return };
    let mut rooms = lobby::Rooms::new();
    let who = ["1.2.3.4:5", "6.7.8.9:10", "host"];
    for (i, line) in t.lines().take(256).enumerate() {
        let out = rooms.handle(who[i % 3], line, lobby::UDP_PREFIX);
        keep(out.len());
        keep(lobby::parse_start(line));
        if i % 7 == 6 {
            rooms.leave(who[i % 3]);
        }
    }
    keep(rooms.len());
    keep(trino_net::NetConfig::parse(t));
    keep(trino_net::Conditions::parse(t));
}

/// A small achievements table (two achievements, one on a stat).
fn achievements_fixture() -> Vec<u8> {
    use trino_core::services::table::{AchievementRecord, StatRecord, encode, encoded_len};
    let ach = [
        AchievementRecord {
            api: "FIRST",
            name: "a.n",
            description: "a.d",
            icon: 0,
            hidden: false,
            stat: None,
        },
        AchievementRecord {
            api: "COINS_100",
            name: "b.n",
            description: "b.d",
            icon: 0,
            hidden: true,
            stat: Some((0, 100)),
        },
    ];
    let stats = [StatRecord {
        api: "COINS",
        min: 0,
        max: 9999,
        increment_only: true,
    }];
    let mut out = vec![0u8; encoded_len(&ach, &stats, &["p.menu"])];
    encode(&ach, &stats, &["p.menu"], &mut out).unwrap();
    out
}

/// One baked PC asset file as the PC's loader reads it (what a packaged
/// game ships and a mod can replace): the first byte picks the kind.
pub fn pc_asset(data: &[u8]) {
    const KINDS: [&str; 7] = ["sprite", "sound", "music", "mixer", "model", "font", "blob"];
    let Some((&k, rest)) = data.split_first() else {
        return;
    };
    let kind = KINDS[k as usize % KINDS.len()];
    if let Ok(a) = trino_asset_pipeline::parse_baked(kind, rest.to_vec()) {
        for s in &a.sprites {
            assert_eq!(s.rgba.len() as u64, s.width as u64 * s.height as u64 * 4);
        }
        for m in &a.models {
            assert!(trino_core::render3d::Mesh::from_tmdl(&m.tmdl).is_ok());
        }
    }
}

/// A PC asset group's bundle (`groups/<name>.tbnd`), loaded step by step
/// like the runner's group loader does.
pub fn pc_bundle(data: &[u8]) {
    let Ok(mut load) = trino_asset_pipeline::BundleLoad::from_bytes(
        data.to_vec(),
        "fuzz.tbnd".into(),
        trino_core::HwTier::Base,
    ) else {
        return;
    };
    let mut out = trino_asset_pipeline::LoadedAssets::default();
    let mut steps = 0;
    while !load.is_done() && steps < 4096 {
        if load.step(&mut out).is_err() {
            break;
        }
        steps += 1;
    }
    keep(load.progress());
}

/// The 3DS shim's `T3AU` header check in C (`t3au_header.h`, compiled for
/// the host by build.rs), as the shim's `trino_wav_load` runs it.
pub mod shim {
    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug)]
    pub struct T3auHeader {
        pub adpcm: u8,
        pub bus: u8,
        pub looped: u8,
        pub stream: u8,
        pub sample_rate: u32,
        pub nsamples: u32,
        pub data_bytes: u32,
        pub chunk_samples: u32,
        pub ncontexts: u32,
        pub data_offset: u32,
        pub coefs: [i16; 16],
    }

    unsafe extern "C" {
        fn trino_fuzz_t3au_parse(file: *const u8, len: usize, out: *mut T3auHeader) -> i32;
        fn trino_fuzz_t3au_context(c: *const u8, index: *mut u16, h0: *mut i16, h1: *mut i16);
        fn trino_fuzz_t3au_buses() -> u32;
    }

    /// The header the shim reads from the whole file `file`, `None` when
    /// the shim refuses it.
    pub fn parse(file: &[u8]) -> Option<T3auHeader> {
        let mut out = T3auHeader::default();
        // SAFETY: the C reads `len` bytes of `file` at most and writes `out`.
        let ok = unsafe { trino_fuzz_t3au_parse(file.as_ptr(), file.len(), &mut out) };
        (ok != 0).then_some(out)
    }

    /// The shim's read of one 8-byte context: (index, history0, history1).
    pub fn context(c: &[u8; 8]) -> (u16, i16, i16) {
        let (mut i, mut h0, mut h1) = (0, 0, 0);
        // SAFETY: the C reads the 8 bytes and writes the three outputs.
        unsafe { trino_fuzz_t3au_context(c.as_ptr(), &mut i, &mut h0, &mut h1) };
        (i, h0, h1)
    }

    /// `TRINO_T3AU_BUSES`.
    pub fn buses() -> usize {
        // SAFETY: returns a constant.
        unsafe { trino_fuzz_t3au_buses() as usize }
    }
}

/// Differential (issue #121): the 3DS shim's `T3AU` check (C) against the
/// pipeline's `T3au::parse` (Rust), which claim the same rules. Both accept
/// or both refuse; what both accept they read the same way (every header
/// field, the stream decision, every context).
pub fn t3au_shim(data: &[u8]) {
    use trino_asset_pipeline::audio::{Codec, T3au};
    let c = shim::parse(data);
    let r = T3au::parse(data);
    let (c, r) = match (c, r) {
        (None, Err(_)) => return,
        (Some(c), Ok(r)) => (c, r),
        (c, r) => panic!(
            "T3AU: the shim {} what the pipeline {} ({:?} / {:?})",
            if c.is_some() { "accepts" } else { "refuses" },
            if r.is_ok() { "accepts" } else { "refuses" },
            c,
            r.err()
        ),
    };
    assert_eq!(c.adpcm != 0, r.codec == Codec::DspAdpcm, "codec");
    assert_eq!(c.bus as usize, r.bus.index(), "bus");
    assert_eq!(c.looped != 0, r.looped, "loop");
    assert_eq!(
        c.stream != 0,
        r.stream && r.chunk_samples > 0 && r.samples > 0,
        "stream"
    );
    assert_eq!(c.sample_rate, r.rate, "rate");
    assert_eq!(c.nsamples as usize, r.samples, "samples");
    assert_eq!(c.data_bytes as usize, r.data.len(), "data bytes");
    assert_eq!(c.chunk_samples as usize, r.chunk_samples, "chunk");
    assert_eq!(c.ncontexts as usize, r.contexts.len(), "contexts");
    assert_eq!(
        c.data_offset as usize,
        data.len() - r.data.len(),
        "data offset"
    );
    assert_eq!(c.coefs, r.coefs, "coefficients");
    let raw = &data[60..c.data_offset as usize];
    for (k, (bytes, ctx)) in raw.as_chunks::<8>().0.iter().zip(&r.contexts).enumerate() {
        assert_eq!(
            shim::context(bytes),
            (ctx.header as u16, ctx.hist1, ctx.hist2),
            "context {k}"
        );
    }
    if r.samples <= crate::core_targets::MAX_DECODE {
        keep(r.decode().len());
    }
}
