//! Seeds for the targets whose inputs no bake writes (`Seeds::Committed`):
//! each made by the format's own writer, so a seed is always a valid input
//! the fuzzer mutates from.

/// `(target, file name, bytes)` for every generated seed.
pub fn generated() -> Vec<(&'static str, String, Vec<u8>)> {
    use trino_core::save;
    let mut out = Vec::new();
    let mut put =
        |t: &'static str, name: &str, bytes: Vec<u8>| out.push((t, name.to_string(), bytes));

    // Save media: blank, one save, two saves (both slots), a wide save, the
    // legacy single frame.
    for size in [512usize, 2048] {
        let mut m = crate::core_targets::Ram(vec![0xFF; size]);
        put("save_frame", &format!("blank-{size}"), m.0.clone());
        let mut body = vec![0u8; size];
        let mut scratch = vec![0u8; size];
        let mut lz = save::Lz::default();
        save::write_now(
            &mut m,
            b"stars=3;coins=42;stage=2",
            &mut body,
            &mut scratch,
            &mut lz,
        )
        .unwrap();
        put("save_frame", &format!("one-{size}"), m.0.clone());
        let long: Vec<u8> = (0..size / 3).map(|i| (i % 7) as u8).collect();
        save::write_now(&mut m, &long, &mut body, &mut scratch, &mut lz).unwrap();
        put("save_frame", &format!("two-{size}"), m.0.clone());
        let wide: Vec<u8> = (0..size * 3 / 4).map(|i| (i * 31 % 251) as u8).collect();
        save::write_now(&mut m, &wide, &mut body, &mut scratch, &mut lz).unwrap();
        put("save_frame", &format!("wide-{size}"), m.0.clone());
    }
    let data = b"trino-save-v1";
    let mut legacy = save::header(data).to_vec();
    legacy.extend_from_slice(data);
    put("save_frame", "legacy", legacy);
    let mut slot = vec![0u8; 256];
    let n = save::encode_slot(5, b"slot file save", &mut slot).unwrap();
    put("save_frame", "slot-file", slot[..n].to_vec());

    // The save compressor.
    let mut lz = save::Lz::default();
    let src: Vec<u8> = (0..1500)
        .map(|i| if i % 7 == 0 { (i / 7) as u8 } else { 0 })
        .collect();
    let mut packed = vec![0u8; 2048];
    let n = save::compress(&src, &mut packed, &mut lz).unwrap();
    put("save_lz", "repetitive", packed[..n].to_vec());
    put(
        "save_lz",
        "text",
        b"stars=3;coins=42;stars=3;coins=42;".to_vec(),
    );

    // Small save forms.
    put(
        "bus_settings",
        "default",
        trino_core::BusSettings::default().to_bytes().to_vec(),
    );
    put(
        "access_pack",
        "default",
        trino_core::AccessSettings::DEFAULT.pack().to_vec(),
    );
    let mut st = trino_core::AchievementState::EMPTY;
    st.set_unlocked(0, true);
    st.set_unlocked(2, true);
    st.set_stat(0, 42);
    let mut p = [0u8; 64];
    let n = st.pack(3, 2, &mut p);
    put("achievement_state", "three-two", p[..n].to_vec());
    let pad = trino_core::NetPad::from_bytes([3; trino_core::NET_PAD_LEN]);
    put("netpad", "pad", pad.to_bytes().to_vec());

    // A trace packet.
    let mut ring = trino_core::trace::TraceRing::<64>::default();
    ring.zone("frame", 0, 10, 200);
    ring.counter("tris", 0, 20, 512);
    ring.frame(30, 1);
    let mut pkt = Vec::new();
    ring.encode(trino_core::trace::TracePlatform::from_u8(0), &mut |b| {
        pkt.push(b)
    });
    put("ttrc", "packet", pkt);

    out
}

/// Host-side writers: LZ4 blocks, a 3DS T3AU sound, lobby lines, a ggrs
/// message.
#[cfg(feature = "host")]
pub fn generated_host() -> Vec<(&'static str, String, Vec<u8>)> {
    let mut out = Vec::new();
    let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".repeat(20);
    out.push((
        "lz4",
        "text".to_string(),
        trino_asset_pipeline::lz4::compress(&text),
    ));
    out.push((
        "lz4",
        "zeros".to_string(),
        trino_asset_pipeline::lz4::compress(&[0u8; 2000]),
    ));
    use trino_asset_pipeline::audio as a;
    let master = a::Master {
        rate: 16000,
        samples: (0..1600).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect(),
    };
    let settings = a::AudioSettings {
        codec: a::Codec::auto(trino_asset_pipeline::Platform::N3ds),
        auto: true,
        quality: 6.0,
        sample_rate: None,
        stream: false,
        looped: true,
        bus: trino_core::Bus::Sfx,
    };
    if let Ok(e) = a::encode(trino_asset_pipeline::Platform::N3ds, &settings, &master, 1)
        && let Some(blob) = e.blob
    {
        out.push(("t3au", "tone".to_string(), blob));
    }
    // The PC's player files, as the runner writes them.
    use trino_platform_pc::{audio, input, system};
    let b = input::Bindings::default();
    out.push((
        "input_text",
        "default".to_string(),
        input::bindings_to_text(&b).into_bytes(),
    ));
    let all: [input::Bindings; trino_core::MAX_PLAYERS] =
        core::array::from_fn(|_| input::Bindings::default());
    out.push((
        "input_text",
        "players".to_string(),
        input::players_bindings_to_text(&all).into_bytes(),
    ));
    out.push((
        "display_text",
        "default".to_string(),
        system::display_settings_to_text(&trino_core::DisplaySettings::DEFAULT).into_bytes(),
    ));
    let access = system::access_settings_to_text(&trino_core::AccessSettings::DEFAULT);
    let mix = audio::settings_to_text(&trino_core::BusSettings::default());
    out.push(("settings_text", "access".to_string(), access.into_bytes()));
    out.push(("settings_text", "audio".to_string(), mix.into_bytes()));
    out.push((
        "settings_text",
        "quality".to_string(),
        b"render_scale=0.75\nmsaa=4\nframe_limit=60\nshadows=high\n".to_vec(),
    ));
    out.push((
        "services_text",
        "achievements".to_string(),
        b"# Trino achievements and stats (issue #61)\nunlocked=FIRST\nstat COINS=12\n".to_vec(),
    ));
    out.push((
        "services_text",
        "crash".to_string(),
        b"upload=yes\n".to_vec(),
    ));
    out.push((
        "services_text",
        "package".to_string(),
        b"name=castle64\ntitle=Castle 64\nversion=1.0.0\nassets=assets/pc\nprofile=native\n"
            .to_vec(),
    ));
    // The Web's pack, a base64 save and a pad id.
    use trino_platform_web::pack;
    let sprite = {
        let mut v = b"TSPR".to_vec();
        v.extend_from_slice(&2u32.to_le_bytes());
        v.extend_from_slice(&2u32.to_le_bytes());
        v.extend_from_slice(&[255u8; 16]);
        v
    };
    let tone: Vec<u8> = {
        let mut v = b"TS16".to_vec();
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&[0, 0, 0xFF, 0x7F, 0, 0x80, 0, 0]);
        v
    };
    out.push((
        "tpak",
        "small".to_string(),
        pack::write(&[
            (pack::Kind::Sprite, 1, sprite.clone()),
            (pack::Kind::Sound, 2, tone.clone()),
        ]),
    ));
    out.push(("tpak", "sprite".to_string(), sprite));
    out.push((
        "tpak",
        "base64".to_string(),
        trino_platform_web::b64::encode(b"save bytes!").into_bytes(),
    ));
    out.push((
        "tpak",
        "pad".to_string(),
        b"045e-028e-Xbox 360 Controller".to_vec(),
    ));
    out.push((
        "net",
        "lobby".to_string(),
        b"JOIN ABCD 2\nJOIN ABCD 2\nSIGNAL 1 hello\nLEAVE\nSTART 0 2 1.2.3.4:5 6.7.8.9:10\n"
            .to_vec(),
    ));
    out
}
