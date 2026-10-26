//! Engine feature checks shared by every target's self-test: the PC test
//! suite runs them against the wgpu backend, the N64/3DS apps run them in
//! ares/Azahar (`cargo xtask test n64|3ds`) — same code, same expectations.
//!
//! Each check exercises one engine service through the portable traits
//! only, so a pass on a console proves the backend wiring (shim, asset
//! bake, loader) end to end.

use trino_core::HwTier;
use trino_core::PrecompileRequest;
use trino_core::math3d::{cos, sin};
use trino_core::{
    BakedScene, BillboardParams, BlobId, Camera3, Cells, Color, DEFAULT_LIGHT, DetailTexture,
    Effect, Emitter, EntityStore, FontId, Impostor, InputState, Instance, Light, LocalLight,
    Material, ModelId, ModelParams, Palette, ParticleSystem, Platform, QualityPreset,
    QualitySettings, Rect, Renderer, Screen, Shading, ShadowCast, ShadowPlane, ShadowSettings,
    ShadowTier, SpriteId, SpriteParams, TexFilter, TextBuf, TextParams, Transform3, Vec2, Vec3,
};

/// The HUD font (also what the checks measure).
pub const FONT: FontId = FontId::from_path("fonts/trino5x7");
/// `assets/shared/data/selftest.txt`, declared as `[blobs.selftest]`.
pub const BLOB: BlobId = BlobId::from_path("blobs/selftest");
pub const BLOB_CONTENT: &[u8] = b"trino-blob-ok\n";
/// A 1x1 card textured with four colored quadrants (`[models.selftest_card]`).
pub const CARD: ModelId = ModelId::from_path("models/selftest_card");

/// Run every engine check; `Err` names the first failure.
pub fn engine_checks(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<(), &'static str> {
    // Progress markers make an emulator hang/crash point obvious in logs.
    platform.log("TRINO_STEP:text\n");
    text(renderer)?;
    platform.log("TRINO_STEP:blob\n");
    blob(platform)?;
    platform.log("TRINO_STEP:save\n");
    save(platform)?;
    platform.log("TRINO_STEP:textured_3d\n");
    if textured_3d(renderer)? {
        platform.log("TRINO_STEP:textured_3d probed 4 quadrants\n");
    } else {
        platform.log("TRINO_STEP:textured_3d skipped (no read_pixel)\n");
    }
    platform.log("TRINO_STEP:texture filter and detail\n");
    if texture_detail(renderer)? {
        platform.log("TRINO_STEP:texture filter and detail probed\n");
    }
    platform.log("TRINO_STEP:texture formats\n");
    if texture_formats(renderer, platform)? {
        platform.log("TRINO_STEP:texture formats probed 9 formats\n");
    }
    platform.log("TRINO_STEP:billboards\n");
    if billboards(renderer)? {
        platform.log("TRINO_STEP:billboards probed\n");
    }
    platform.log("TRINO_STEP:occlusion\n");
    if occlusion(renderer)? {
        platform.log("TRINO_STEP:occlusion probed\n");
    }
    platform.log("TRINO_STEP:fill rect\n");
    if fill_rect(renderer)? {
        platform.log("TRINO_STEP:fill rect probed\n");
    }
    platform.log("TRINO_STEP:ui layout\n");
    if ui_layout(renderer, platform)? {
        platform.log("TRINO_STEP:ui layout probed focus, pad and touch\n");
    }
    platform.log("TRINO_STEP:i18n\n");
    if i18n(renderer, platform)? {
        platform.log("TRINO_STEP:i18n probed 3 languages\n");
    }
    platform.log("TRINO_STEP:dual screen\n");
    if dual_screen(renderer)? {
        platform.log("TRINO_STEP:dual screen probed both screens\n");
    } else if renderer.screen_size(Screen::Bottom).is_none() {
        platform.log("TRINO_STEP:dual screen single screen (collapsed)\n");
    }
    platform.log("TRINO_STEP:stereo\n");
    match stereo(renderer)? {
        StereoProbe::Eyes { gpu_draws } => platform.log(alloc_free_line(gpu_draws).as_str()),
        StereoProbe::Mono => platform.log("TRINO_STEP:stereo mono target (no stereo screen)\n"),
        StereoProbe::Skipped => platform.log("TRINO_STEP:stereo skipped (no read_pixel)\n"),
    }
    platform.log("TRINO_STEP:smooth skin\n");
    smooth_skin(renderer)?;
    platform.log("TRINO_STEP:palette\n");
    if palette(renderer)? {
        platform.log("TRINO_STEP:palette probed 3 slots\n");
    }
    platform.log("TRINO_STEP:instancing\n");
    if instancing(renderer)? {
        platform.log("TRINO_STEP:instancing probed\n");
    }
    platform.log("TRINO_STEP:local lights\n");
    if local_lights(renderer)? {
        platform.log("TRINO_STEP:local lights probed\n");
    }
    platform.log("TRINO_STEP:ambient occlusion\n");
    if ambient_occlusion(renderer)? {
        platform.log("TRINO_STEP:ambient occlusion probed\n");
    }
    platform.log("TRINO_STEP:post\n");
    if post(renderer)? {
        platform.log("TRINO_STEP:post probed grading and glow\n");
    }
    platform.log("TRINO_STEP:shadows\n");
    if shadows(renderer, platform)? {
        platform.log("TRINO_STEP:shadows probed\n");
    }
    platform.log("TRINO_STEP:water\n");
    if water(renderer)? {
        platform.log("TRINO_STEP:water probed\n");
    }
    platform.log("TRINO_STEP:impostor\n");
    if impostor(renderer, platform)? {
        platform.log("TRINO_STEP:impostor probed\n");
    }
    platform.log("TRINO_STEP:particles\n");
    particles(renderer, platform)?;
    platform.log("TRINO_STEP:portals\n");
    portals(renderer, platform)?;
    platform.log("TRINO_STEP:physics\n");
    let probe = physics(platform)?;
    log_physics(platform, &probe);
    platform.log("TRINO_STEP:accessibility\n");
    if accessibility(renderer, platform)? {
        platform
            .log("TRINO_STEP:accessibility probed palette swap, outline, captions and contrast\n");
    }
    platform.log("TRINO_STEP:services\n");
    if services(renderer, platform)? {
        platform.log("TRINO_STEP:services probed list and toast\n");
    }
    platform.log("TRINO_STEP:hw tier\n");
    let tier = hw_tier(renderer, platform)?;
    log_tier(platform, &tier);
    platform.log("TRINO_STEP:quality\n");
    let q = quality(renderer, platform)?;
    log_quality(platform, &q);
    platform.log("TRINO_STEP:trace\n");
    let events = trace(renderer, platform)?;
    log_trace(platform, events);
    platform.log("TRINO_STEP:shader precompile\n");
    let warm = shader_precompile(renderer, platform)?;
    log_precompile(platform, &warm);
    platform.log("TRINO_STEP:engine ok\n");
    Ok(())
}

/// The session name of the `trace` step's lines (`cargo xtask test`
/// collects it into `target/traces/<platform>-selftest.json`).
pub const TRACE_SESSION: &str = "selftest";

/// Zone and counter names the `trace` step records, which the hosts' tools
/// check in the collected trace.
pub const TRACE_NAMES: [&str; 6] = ["frame", "update", "physics", "render", "draws", "present"];

/// The timeline trace (issue #62, ADR 0030) in its exported form: a local
/// ring (not the global one a `--trace` build installs) records two frame
/// marks, nested zones timed by the platform's clock around real work (a
/// physics sweep, a rendered frame and its present), a counter from the
/// renderer's stats; its packet goes out as `TRINO_TRACE:` lines over
/// `platform.log` (the debug channel) and every line is read back as it is
/// written: sequence, check, base64, then the packet itself (names,
/// nesting, times in order, the platform tag). A second, tiny ring
/// overflows on purpose: it keeps the newest events and counts the rest.
/// Returns the events the packet carried.
#[inline(never)]
pub fn trace(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<u32, &'static str> {
    use trino_core::trace::{EventKind, LINE_BYTES, Packet, TraceLine, TraceRing, track};

    // Wraparound: 6 events in a ring of 4.
    let mut small = TraceRing::<4>::new();
    for i in 0..6 {
        small.counter("n", track::CPU, i, i);
    }
    if small.len() != 4 || small.dropped() != 2 || small.events().next().map(|e| e.value) != Some(2)
    {
        return Err("trace: a full ring doesn't keep the newest events");
    }

    let clock = |p: &dyn Platform| p.now_micros() as u32;
    let mut ring = TraceRing::<32>::new();
    let t_frame = clock(platform);
    ring.frame(t_frame, 1);
    let t_update = clock(platform);
    ring.open(track::CPU);
    let t_physics = clock(platform);
    // Real work for the zone.
    let mut acc = 0.0f32;
    for i in 0..256 {
        acc += sin(i as f32 * 0.01) * cos(i as f32 * 0.02);
    }
    if !acc.is_finite() {
        return Err("trace: the timed work went wrong");
    }
    let t_physics_end = clock(platform);
    ring.zone("physics", track::CPU, t_physics, t_physics_end);
    ring.close(track::CPU);
    ring.zone("update", track::CPU, t_update, clock(platform));
    let t_render = clock(platform);
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    });
    renderer.set_light(&DEFAULT_LIGHT);
    renderer.draw_model(
        CARD,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    ring.open(track::CPU);
    let t_present = clock(platform);
    renderer.end_frame();
    ring.zone("present", track::CPU, t_present, clock(platform));
    ring.close(track::CPU);
    // The finished frame's counters.
    let draws = renderer.stats().models;
    if draws != 1 {
        return Err("trace: the traced frame didn't draw its model");
    }
    ring.zone("render", track::CPU, t_render, clock(platform));
    ring.counter("draws", track::CPU, clock(platform), draws);
    ring.frame(clock(platform), 2);

    // Out over the debug channel, each line read back as written.
    struct Lines<'a> {
        platform: &'a dyn Platform,
        line: [u8; 192],
        len: usize,
        bytes: [u8; 1024],
        at: usize,
        next_seq: u32,
        err: Option<&'static str>,
    }
    impl core::fmt::Write for Lines<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.len == self.line.len() {
                    self.err = Some("trace: a line is too long");
                    return Err(core::fmt::Error);
                }
                self.line[self.len] = b;
                self.len += 1;
                if b != b'\n' {
                    continue;
                }
                let text = core::str::from_utf8(&self.line[..self.len]).unwrap_or("");
                self.platform.log(text);
                let mut chunk = [0u8; LINE_BYTES];
                let decoded = TraceLine::parse(text)
                    .filter(|l| l.session == TRACE_SESSION && l.seq == self.next_seq)
                    .map(|l| l.decode(&mut chunk));
                match decoded {
                    Some(Ok(n)) if self.at + n <= self.bytes.len() => {
                        self.bytes[self.at..self.at + n].copy_from_slice(&chunk[..n]);
                        self.at += n;
                    }
                    _ => {
                        self.err = Some("trace: a line doesn't read back");
                        return Err(core::fmt::Error);
                    }
                }
                self.next_seq += 1;
                self.len = 0;
            }
            Ok(())
        }
    }
    let mut lines = Lines {
        platform: &*platform,
        line: [0; 192],
        len: 0,
        bytes: [0; 1024],
        at: 0,
        next_seq: 0,
        err: None,
    };
    let held = ring.len();
    let want = ring.packet_len();
    let mut seq = 0;
    let target = platform.trace_platform();
    if ring
        .write_lines(target, TRACE_SESSION, &mut seq, &mut lines)
        .is_err()
    {
        return Err(lines.err.unwrap_or("trace: writing the lines failed"));
    }
    if lines.at != want || seq != lines.next_seq || seq as usize != want.div_ceil(LINE_BYTES) {
        return Err("trace: the lines don't carry the whole packet");
    }

    let (p, len) = Packet::parse(&lines.bytes[..lines.at]).map_err(|_| "trace: packet")?;
    if len != want || p.platform != target || p.dropped != 0 || p.event_count as usize != held {
        return Err("trace: the packet's header is wrong");
    }
    let mut names = [""; 8];
    for (i, n) in p.names().enumerate().take(8) {
        names[i] = n;
    }
    if p.name_count as usize != TRACE_NAMES.len() || TRACE_NAMES.iter().any(|n| !names.contains(n))
    {
        return Err("trace: the packet's names are wrong");
    }
    let find = |name: &str| {
        p.events()
            .find(|e| names[e.name as usize] == name && e.kind == EventKind::Zone as u8)
    };
    let (Some(update), Some(physics), Some(render), Some(present)) = (
        find("update"),
        find("physics"),
        find("render"),
        find("present"),
    ) else {
        return Err("trace: a zone is missing");
    };
    let inside = |inner: &trino_core::trace::TraceEvent, outer: &trino_core::trace::TraceEvent| {
        inner.ts >= outer.ts
            && inner.ts.wrapping_add(inner.value) <= outer.ts.wrapping_add(outer.value)
            && inner.depth == outer.depth + 1
    };
    if !inside(&physics, &update) || !inside(&present, &render) || render.ts < update.ts {
        return Err("trace: zones don't nest or follow each other");
    }
    let frames: [u32; 2] = {
        let mut f = p.events().filter(|e| e.kind == EventKind::Frame as u8);
        match (f.next(), f.next()) {
            (Some(a), Some(b)) if (a.value, b.value) == (1, 2) && b.ts >= a.ts => [a.ts, b.ts],
            _ => return Err("trace: the frame marks are wrong"),
        }
    };
    if render.ts.wrapping_add(render.value) > frames[1] {
        return Err("trace: the frame ends before its render");
    }
    let counter = p
        .events()
        .find(|e| e.kind == EventKind::Counter as u8)
        .ok_or("trace: no counter")?;
    if names[counter.name as usize] != "draws" || counter.value != draws {
        return Err("trace: the counter's value is wrong");
    }
    Ok(held as u32)
}

/// `TRINO_STEP:trace events=<n> instrumented=<0|1>` (whether this build
/// compiled the global zones in: `--trace`).
pub fn log_trace(platform: &dyn Platform, events: u32) {
    let mut line = TextBuf::<80>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:trace events={events} instrumented={}\n",
            trino_core::trace::ENABLED as u8
        ),
    );
    platform.log(line.as_str());
}

/// What the `shader precompile` step saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrecompileReport {
    /// Passes the warm-up built (0: nothing compiles at run time).
    pub passes: u32,
    pub threads: u32,
    /// Frames drawn while it ran (a loading screen's).
    pub frames: u32,
}

/// Pipeline precompilation (issue #101): a background
/// `precompile(EVERYTHING)` finishes while frames keep coming (a loading
/// screen), and a second request finds nothing left. The N64 and the 3DS
/// assemble their shaders at build time: 0 passes, done at once. The PC
/// builds its lazily created passes on worker threads, the Web one a frame.
/// The wait is bounded by the platform's clock, never a frame count: a
/// software adapter (CI's WARP) compiles for seconds while empty frames
/// take microseconds.
pub fn shader_precompile(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<PrecompileReport, &'static str> {
    const LIMIT_US: u64 = 300_000_000;
    let start = renderer.precompile(&PrecompileRequest::EVERYTHING);
    let t0 = platform.now_micros();
    let mut frames = 0;
    while !renderer.precompile_status().is_done() {
        renderer.begin_frame(Color::rgb(0, 0, 0));
        renderer.end_frame();
        frames += 1;
        if platform.now_micros().saturating_sub(t0) > LIMIT_US {
            return Err("shader precompile: still running after 300 s");
        }
    }
    let done = renderer.precompile_status();
    if done.done != done.total || done.total != start.total {
        return Err("shader precompile: the plan changed while it ran");
    }
    if done.total > 0 && done.threads == 0 {
        return Err("shader precompile: work without a worker");
    }
    if renderer.precompile(&PrecompileRequest::EVERYTHING).total != 0 {
        return Err("shader precompile: a second request found work left");
    }
    Ok(PrecompileReport {
        passes: done.total,
        threads: done.threads,
        frames,
    })
}

/// `TRINO_STEP:shader precompile passes=14 threads=7 frames=3`, or
/// `... built ahead` where nothing compiles at run time.
pub fn log_precompile(platform: &mut dyn Platform, r: &PrecompileReport) {
    let mut line: TextBuf<96> = TextBuf::new();
    let _ = if r.passes == 0 {
        core::fmt::Write::write_str(&mut line, "TRINO_STEP:shader precompile built ahead\n")
    } else {
        core::fmt::Write::write_fmt(
            &mut line,
            format_args!(
                "TRINO_STEP:shader precompile passes={} threads={} frames={}\n",
                r.passes, r.threads, r.frames
            ),
        )
    };
    platform.log(line.as_str());
}

/// Achievements, stats and presence (issue #61) through the `Platform`
/// defaults every backend serves with its `HostedServices`: castle64's
/// baked table (`assets/achievements.ron`), an unlock and its popup queue,
/// an increment-only stat reaching a threshold, the packed state written to
/// the real save medium and restored quietly (what a console game does),
/// presence kept as text, then the overlay's list and popup probed by
/// pixel. The platform's state is put back at the end. `Ok(false)` where
/// `read_pixel` is unsupported (after the non-pixel checks).
pub fn services(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::services::{AchievementTable, Presence};
    use trino_core::ui::NoStrings;
    use trino_core::{
        AchievementId, AchievementList, AchievementState, AchievementToast, Arg, OverlayStyle,
        StatId, StrKey,
    };

    // 1. The baked table.
    let table = AchievementTable::from_platform(platform)
        .ok_or("services: blob services/achievements missing or broken")?;
    let first = AchievementId::new("FIRST_COIN");
    if table.len() != 5
        || table.stat_count() != 2
        || table.index(first) != Some(0)
        || table
            .find(AchievementId::new("COINS_25"))
            .and_then(|a| a.stat)
            != Some(0)
        || !table
            .find(AchievementId::new("BOAR_STOMP"))
            .is_some_and(|a| a.hidden)
        || table.presence_key(StrKey::new("presence.stage")) != Some("presence.stage")
    {
        return Err("services: the baked table differs from assets/achievements.ron");
    }
    if platform.services().is_none() {
        return Err("services: the platform holds no services");
    }
    let saved = platform.achievements();
    platform.reset_achievements();
    while platform.next_unlock().is_some() {}

    // 2. Unlock once, one popup.
    platform.unlock(first);
    platform.unlock(first);
    platform.unlock(AchievementId::new("NOT_IN_THE_TABLE"));
    if !platform.is_unlocked(first)
        || platform.next_unlock() != Some(0)
        || platform.next_unlock().is_some()
    {
        return Err("services: an unlock is not kept once with one popup");
    }

    // 3. An increment-only stat and its threshold (COINS_25 at 25).
    let coins = StatId::new("COINS");
    platform.set_stat(coins, 24);
    platform.set_stat(coins, 10);
    if platform.stat(coins) != 24 || platform.is_unlocked(AchievementId::new("COINS_25")) {
        return Err("services: an increment-only stat went down or unlocked early");
    }
    platform.set_stat(coins, 25);
    if !platform.is_unlocked(AchievementId::new("COINS_25")) || platform.next_unlock() != Some(1) {
        return Err("services: a stat threshold did not unlock its achievement");
    }

    // 4. The save form through the real medium, restored quietly.
    let state = platform.achievements();
    let mut packed = [0u8; 32];
    let n = state.pack_for(&table, &mut packed);
    if n != AchievementState::packed_len(5, 2) {
        return Err("services: the packed state has the wrong size");
    }
    if platform.save_capacity() >= n {
        platform
            .save_write(&packed[..n])
            .map_err(|_| "services: save write")?;
        let mut back = [0u8; 32];
        let m = platform
            .save_read(&mut back)
            .map_err(|_| "services: save read")?;
        if m != n || back[..n] != packed[..n] {
            return Err("services: the save medium did not keep the packed state");
        }
    }
    let (read, _) = AchievementState::unpack(&packed[..n])
        .map_err(|_| "services: the packed state does not read back")?;
    platform.reset_achievements();
    platform.restore_achievements(&read);
    if platform.achievements() != state || platform.next_unlock().is_some() {
        return Err("services: a restored state differs or shows popups");
    }

    // 5. Presence, kept as text for a store.
    platform.set_presence(StrKey::new("presence.stage"), &[Arg::int("stage", 2)]);
    let want = Presence::new(StrKey::new("presence.stage"), &[Arg::int("stage", 2)]);
    if platform.services().map(|s| *s.presence()) != Some(want) {
        return Err("services: presence was not kept");
    }

    // 6. The overlay: the list over a magenta frame, then the popup.
    let style = OverlayStyle::DEFAULT;
    let mut list = AchievementList::new();
    for _ in 0..3 {
        renderer.begin_frame(Color::rgb(255, 0, 255));
        list.draw(renderer, &NoStrings, &table, &state, &style);
        renderer.end_frame();
    }
    let size = renderer
        .screen_size(Screen::Top)
        .ok_or("services: no screen size")?;
    let px = style.text_px(size.y);
    let safe = renderer.safe_area(Screen::Top);
    let inner = Vec2::new(
        size.x - (safe.left + safe.right) as f32,
        size.y - (safe.top + safe.bottom) as f32,
    );
    let (_, row) = AchievementList::rows(inner, px);
    let top = safe.top as f32 + 8.0 * px + 6.0 * px;
    // Row 0's panel at 70 % of its width, and the dimmed frame below the
    // last row (5 rows fit every target).
    let panel_at = (
        (safe.left as f32 + inner.x * 0.7) as u32,
        (top + row * 0.5) as u32,
    );
    let below = (
        (size.x * 0.5) as u32,
        (top + 5.5 * row).min(size.y - 2.0) as u32,
    );
    let (Some(panel), Some(dim)) = (
        renderer.read_pixel(panel_at.0, panel_at.1),
        renderer.read_pixel(below.0, below.1),
    ) else {
        // Restore before returning: no read-back on this backend.
        platform.reset_achievements();
        platform.restore_achievements(&saved);
        while platform.next_unlock().is_some() {}
        return Ok(false);
    };
    // Dimmed magenta: (51, 0, 51); the panel over it: about (30, 28, 45).
    if dim.g > 16 || dim.r < 25 || dim.r > 90 || dim.b < 25 {
        return Err("services: the list's background is not drawn");
    }
    if panel.g < 16 || panel.r > 60 || panel.b < 25 {
        return Err("services: the list's rows are not drawn");
    }
    let mut toast = AchievementToast::new();
    toast.push(0);
    toast.advance(1.0);
    let (r, shown) = toast.rect(size, px, &style);
    if shown != 1.0 {
        return Err("services: the popup did not slide in");
    }
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        toast.draw(renderer, &NoStrings, &table, &style);
        renderer.end_frame();
    }
    let inside = renderer.read_pixel(
        (r.pos.x + r.size.x * 0.85) as u32,
        (r.pos.y + r.size.y * 0.3) as u32,
    );
    let outside = renderer.read_pixel(
        (r.pos.x + r.size.x * 0.5) as u32,
        (r.pos.y + r.size.y + 6.0 * px) as u32,
    );
    match (inside, outside) {
        (Some(i), Some(o)) if i.b > 25 && i.g > 16 && o.r < 12 && o.g < 12 && o.b < 12 => {}
        _ => return Err("services: the popup is not drawn where it says"),
    }

    platform.reset_achievements();
    platform.restore_achievements(&saved);
    while platform.next_unlock().is_some() {}
    Ok(true)
}

/// The quadrants in 16-bit color (no palette: they share TMEM with a
/// detail layer on the N64) and the detail layer, white left, gray right.
pub const QUADRANTS16: SpriteId = SpriteId::from_path("sprites/selftest_quadrants16");
pub const DETAIL: SpriteId = SpriteId::from_path("sprites/selftest_detail");

/// Texture filtering and the detail layer: the quadrant card magnified
/// (~11 px per texel), probed 2.6 px left of the red/green texel edge —
/// pure red with `TexFilter::Point`, a red/green blend with `Bilinear` (the
/// RDP's 3-point on the N64, whose texel centers sit half a texel off). Then the detail layer: the right half of the
/// card (gray detail) at half the color, the left half (white) at full.
/// `Ok(false)` where `read_pixel` is unsupported.
pub fn texture_detail(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let draw = |r: &mut dyn Renderer, params: &ModelParams| {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            r.set_camera(&camera);
            r.set_light(&DEFAULT_LIGHT);
            r.draw_model(CARD, &Transform3::default(), Material::VertexLit, params);
            r.end_frame();
        }
    };
    // Front face at z = -0.05; planar UVs u = x + 0.5, v = 0.5 - y.
    let probe = |r: &mut dyn Renderer, u: f32, v: f32| {
        let p = view.transform_point(Vec3::new(u - 0.5, 0.5 - v, -0.05));
        let x = screen.x * 0.5 + p.x * focal / p.z;
        let y = screen.y * 0.5 - p.y * focal / p.z;
        r.read_pixel(x as u32, y as u32)
    };
    let base = ModelParams {
        shading: Shading::Unlit,
        texture: Some(QUADRANTS16),
        ..ModelParams::DEFAULT
    };
    // Filter: 0.24 texel left of the edge between texels 7 (red) and 8.
    for (filter, mixed) in [(TexFilter::Point, false), (TexFilter::Bilinear, true)] {
        draw(renderer, &ModelParams { filter, ..base });
        let Some(c) = probe(renderer, 0.485, 0.25) else {
            return Ok(false);
        };
        // The RDP's bilinear centers its texels half a texel off the PC's
        // (ares: 74, 189, 0 here, the PC: ~180, 70, 0): both blend.
        if mixed && (c.r < 40 || c.g < 40) {
            return Err("texture filter: Bilinear did not blend the texel edge");
        }
        if !mixed && (c.r < 150 || c.g > 30) {
            return Err("texture filter: Point blended the texel edge");
        }
    }
    // Detail: halves the right column, keeps the left.
    draw(
        renderer,
        &ModelParams {
            filter: TexFilter::Point,
            detail: Some(DetailTexture::new(DETAIL, 0)),
            ..base
        },
    );
    let near = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 40;
    for (u, v, want) in [
        (0.25, 0.25, [255, 0, 0]),
        (0.75, 0.25, [0, 128, 0]),
        (0.25, 0.75, [0, 0, 255]),
        (0.75, 0.75, [128, 128, 128]),
    ] {
        let Some(c) = probe(renderer, u, v) else {
            return Ok(false);
        };
        if !(near(c.r, want[0]) && near(c.g, want[1]) && near(c.b, want[2])) {
            return Err("texture detail: the detail layer was not multiplied in");
        }
    }
    Ok(true)
}

/// One card per 3DS texture format (`[sprites.selftest_fmt_*]`, issue #52),
/// in manifest order with its TL, TR, BL, BR quadrants (`None` =
/// transparent). The N64 bakes every card as RGBA5551, which stores the same
/// colors, so each target expects this table.
pub const FORMAT_CARDS: [(SpriteId, [Option<[u8; 3]>; 4]); 9] = {
    const R: Option<[u8; 3]> = Some([255, 0, 0]);
    const G: Option<[u8; 3]> = Some([0, 255, 0]);
    const B: Option<[u8; 3]> = Some([0, 0, 255]);
    const W: Option<[u8; 3]> = Some([255, 255, 255]);
    const GRAY: Option<[u8; 3]> = Some([136, 136, 136]);
    const DARK: Option<[u8; 3]> = Some([68, 68, 68]);
    const BLACK: Option<[u8; 3]> = Some([0, 0, 0]);
    const CLEAR: Option<[u8; 3]> = None;
    [
        (
            SpriteId::from_path("sprites/selftest_fmt_rgba8"),
            [R, G, B, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_rgb565"),
            [R, G, B, W],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_rgba5551"),
            [R, G, B, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_rgba4"),
            [R, G, B, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_la8"),
            [W, GRAY, DARK, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_l8"),
            [W, GRAY, DARK, BLACK],
        ),
        // A8 is a white mask on every target (the 3DS shim tints it).
        (
            SpriteId::from_path("sprites/selftest_fmt_a8"),
            [W, CLEAR, CLEAR, W],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_etc1"),
            [R, G, B, W],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_etc1a4"),
            [R, G, B, CLEAR],
        ),
    ]
};

/// [`FORMAT_CARDS`]' 3DS formats, for the failure log.
const FORMAT_NAMES: [&str; 9] = [
    "RGBA8", "RGB565", "RGBA5551", "RGBA4", "LA8", "L8", "A8", "ETC1", "ETC1A4",
];

/// Log a failed probe: format, 2D or 3D, quadrant, the color read and the
/// one expected.
fn log_probe(
    platform: &mut dyn Platform,
    i: usize,
    kind: &str,
    q: usize,
    got: Color,
    want: [u8; 3],
) {
    use core::fmt::Write;
    let mut line: TextBuf<96> = TextBuf::new();
    let _ = writeln!(
        line,
        "TRINO_STEP:texture formats {} {kind} q{q} got {},{},{} want {},{},{}",
        FORMAT_NAMES[i], got.r, got.g, got.b, want[0], want[1], want[2]
    );
    platform.log(line.as_str());
}

/// The texture-formats frame's background (a transparent texel shows it).
const FORMATS_BG: [u8; 3] = [96, 0, 96];
/// Its 3D cards: 0.45 m, 0.5 m apart, centered, 0.8 m below the axis.
const FORMAT_CARD_SCALE: f32 = 0.45;

fn formats_camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 0.0, -3.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    }
}

/// 2D cards: 32 px, from x = 4, 35 px apart (fits the N64's 320).
fn format_sprite_at(i: usize) -> Vec2 {
    Vec2::new(4.0 + 35.0 * i as f32, 8.0)
}

fn format_card_at(i: usize) -> Vec3 {
    Vec3::new((i as f32 - 4.0) * 0.5, -0.8, 0.0)
}

/// The photo-like card of the 3DS golden: master, ETC1, RGBA4.
pub const FORMAT_PHOTOS: [SpriteId; 3] = [
    SpriteId::from_path("sprites/selftest_photo"),
    SpriteId::from_path("sprites/selftest_photo_etc1"),
    SpriteId::from_path("sprites/selftest_photo_rgba4"),
];

/// The texture-formats frame (three identical frames: the 3DS LCD is
/// double-buffered): every [`FORMAT_CARDS`] card as a sprite (top row) and
/// as a textured 3D card (bottom row). With `photos`, the middle row shows
/// [`FORMAT_PHOTOS`] side by side (ETC1 blocks, RGBA4 banding): the 3DS
/// self-test dumps that frame as the `texture_formats.png` golden.
pub fn draw_texture_formats(renderer: &mut dyn Renderer, photos: bool) {
    let camera = formats_camera();
    for _ in 0..3 {
        renderer.begin_frame(Color::rgb(FORMATS_BG[0], FORMATS_BG[1], FORMATS_BG[2]));
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        for (i, (sprite, _)) in FORMAT_CARDS.iter().enumerate() {
            let s = FORMAT_CARD_SCALE;
            let transform = Transform3 {
                position: format_card_at(i),
                scale: Vec3::new(s, s, s),
                ..Transform3::default()
            };
            let params = ModelParams {
                shading: Shading::Unlit,
                texture: Some(*sprite),
                filter: TexFilter::Point,
                ..ModelParams::DEFAULT
            };
            renderer.draw_model(CARD, &transform, Material::VertexLit, &params);
        }
        for (i, (sprite, _)) in FORMAT_CARDS.iter().enumerate() {
            renderer.draw_sprite(*sprite, format_sprite_at(i), &SpriteParams::DEFAULT);
        }
        if photos {
            for (i, sprite) in FORMAT_PHOTOS.iter().enumerate() {
                let pos = Vec2::new(56.0 + 100.0 * i as f32, 56.0);
                renderer.draw_sprite(*sprite, pos, &SpriteParams::DEFAULT);
            }
        }
        renderer.end_frame();
    }
}

/// Texture formats: each [`FORMAT_CARDS`] card as a 2D sprite on a purple
/// background (top row: every quadrant, a transparent one must show the
/// background) and as a textured 3D card (bottom row: the opaque quadrants
/// only, since the 3D paths treat texture alpha differently). Catches a
/// format the shim samples wrong: an ignored ETC1A4 alpha, A8 drawn black,
/// L8 read as alpha, swapped channels. `Ok(false)` where `read_pixel` is
/// unsupported.
pub fn texture_formats(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = formats_camera();
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    draw_texture_formats(renderer, false);
    let near = |c: Color, want: [u8; 3]| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 40;
        d(c.r, want[0]) && d(c.g, want[1]) && d(c.b, want[2])
    };
    // Quadrant centers: (u, v) in the card.
    const CENTERS: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    for (i, (_, quadrants)) in FORMAT_CARDS.iter().enumerate() {
        for (q, want) in quadrants.iter().enumerate() {
            let (u, v) = CENTERS[q];
            let p = format_sprite_at(i);
            let Some(c) = renderer.read_pixel((p.x + 32.0 * u) as u32, (p.y + 32.0 * v) as u32)
            else {
                return Ok(false);
            };
            if !near(c, want.unwrap_or(FORMATS_BG)) {
                log_probe(platform, i, "2D", q, c, want.unwrap_or(FORMATS_BG));
                return Err("texture formats: a 2D sprite sampled its format wrong");
            }
            let Some(want) = want else { continue };
            // Front face at z = -0.05; planar UVs u = x + 0.5, v = 0.5 - y.
            let local = Vec3::new(u - 0.5, 0.5 - v, -0.05);
            let world = Vec3::new(
                format_card_at(i).x + local.x * FORMAT_CARD_SCALE,
                format_card_at(i).y + local.y * FORMAT_CARD_SCALE,
                local.z * FORMAT_CARD_SCALE,
            );
            let p = view.transform_point(world);
            let x = screen.x * 0.5 + p.x * focal / p.z;
            let y = screen.y * 0.5 - p.y * focal / p.z;
            let Some(c) = renderer.read_pixel(x as u32, y as u32) else {
                return Ok(false);
            };
            if !near(c, *want) {
                log_probe(platform, i, "3D", q, c, *want);
                return Err("texture formats: a 3D texture sampled its format wrong");
            }
        }
    }
    Ok(true)
}

/// Textured 3D: draw the quadrant card face-on and read back one pixel per
/// quadrant. Catches unbound textures, flipped/rotated UVs and broken
/// texture paths per backend. Skipped (Ok) where `read_pixel` is
/// unsupported (returns `Ok(false)`; `Ok(true)` = probed). Uses `caps()`
/// for the screen size (console profiles).
pub fn textured_3d(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let params = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(CARD, &Transform3::default(), Material::VertexLit, &params);
        renderer.end_frame();
    }
    // Project probe points exactly like the T&L does.
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    // Front face at z = -0.05; planar UVs u = x + 0.5, v = 0.5 - y.
    let probes = [
        (Vec3::new(-0.25, 0.25, -0.05), [255, 0, 0]), // u,v = .25,.25: TL red
        (Vec3::new(0.25, 0.25, -0.05), [0, 255, 0]),  // TR green
        (Vec3::new(-0.25, -0.25, -0.05), [0, 0, 255]), // BL blue
        (Vec3::new(0.25, -0.25, -0.05), [255, 255, 255]), // BR white
    ];
    for (world, want) in probes {
        let v = view.transform_point(world);
        let x = screen.x * 0.5 + v.x * focal / v.z;
        let y = screen.y * 0.5 - v.y * focal / v.z;
        let Some(got) = renderer.read_pixel(x as u32, y as u32) else {
            return Ok(false); // backend cannot read back
        };
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 48;
        if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
            return Err("textured 3D: wrong color in a texture quadrant");
        }
    }
    Ok(true)
}

/// `Renderer::fill_rect`: an opaque red rectangle with a half-transparent
/// white one over its left half, on black. Probes the opaque part (red),
/// the blend (pink: red + white at 50%) and the outside (black). `Ok(false)`
/// where `read_pixel` is unsupported.
pub fn fill_rect(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    let r = trino_core::Rect::new(w * 0.25, h * 0.25, w * 0.5, h * 0.5);
    let half = trino_core::Rect::new(w * 0.25, h * 0.25, w * 0.25, h * 0.5);
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.fill_rect(r, Color::rgb(255, 0, 0));
        renderer.fill_rect(half, Color::rgba(255, 255, 255, 128));
        renderer.end_frame();
    }
    let probes = [
        ((w * 0.6) as u32, (h * 0.5) as u32, [255, 0, 0]),
        ((w * 0.35) as u32, (h * 0.5) as u32, [255, 128, 128]),
        ((w * 0.1) as u32, (h * 0.1) as u32, [0, 0, 0]),
    ];
    for (x, y, want) in probes {
        let Some(got) = renderer.read_pixel(x, y) else {
            return Ok(false);
        };
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 40;
        if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
            return Err("fill_rect: wrong color (opaque, blended or outside)");
        }
    }
    Ok(true)
}

/// The `stereo` step's probed line (no allocation: a fixed buffer).
fn alloc_free_line(gpu_draws: u32) -> TextBuf<80> {
    let mut b = TextBuf::<80>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut b,
        format_args!("TRINO_STEP:stereo probed both eyes and mono gpu_draws={gpu_draws}\n"),
    );
    b
}

/// What the `stereo` step could check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StereoProbe {
    /// Both eyes and the mono frame read back and checked; `gpu_draws`:
    /// the stereo frame's draws on the hardware T&L path (the 3DS's
    /// PICA200, both eyes).
    Eyes { gpu_draws: u32 },
    /// A target without a stereo screen: one picture, the mono one.
    Mono,
    /// No pixel read-back here; only the counters were checked.
    Skipped,
}

/// The stereo scene's separation and convergence (world units).
pub const STEREO_SEPARATION: f32 = 0.4;
pub const STEREO_CONVERGENCE: f32 = 4.0;

/// Stereoscopic 3D (issue #63): the top screen once per eye, the 3D through
/// an off-axis projection, the 2D floated by its layer depth.
///
/// One frame with three things in it: a cyan square at the convergence
/// depth (zero parallax: the same pixels in both eyes), a magenta square
/// nearer (crossed parallax: the left eye's copy right of the right eye's,
/// by the closed form's amount), and a yellow HUD rectangle at
/// `LayerDepth(16)` (8 px apart each way). The slider is fixed at 1
/// ([`StereoSettings::with_slider`]), so the step never depends on the
/// console's slider. Then the same frame with the slider at 0: one picture,
/// the mono one, and no right eye. Where both eyes read back (the 3DS, the
/// PC's 3DS profile) every probe is checked; on a target without a stereo
/// screen the picture must be the mono one ([`StereoProbe::Mono`]).
pub fn stereo(renderer: &mut dyn Renderer) -> Result<StereoProbe, &'static str> {
    use trino_core::stereo::{Eye, LayerDepth, StereoMono, StereoSettings, project_eye};
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -STEREO_CONVERGENCE),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let flat = |c: Color| ModelParams {
        shading: Shading::Unlit,
        tint: c,
        emissive: c,
        ..ModelParams::DEFAULT
    };
    let (cyan, magenta, yellow) = (
        Color::rgba(0, 255, 255, 255),
        Color::rgba(255, 0, 255, 255),
        Color::rgba(255, 255, 0, 255),
    );
    // Squares facing the camera: (center x, view depth, half size, color).
    let far = (-0.9f32, STEREO_CONVERGENCE, 0.5f32, cyan);
    let near = (0.55f32, 2.0f32, 0.3f32, magenta);
    let hud = Rect::new(screen.x * 0.75, 12.0, 40.0, 16.0);
    let settings = StereoSettings::new(STEREO_SEPARATION, STEREO_CONVERGENCE);
    let draw = |r: &mut dyn Renderer, slider: f32| {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            r.set_stereo(Some(&settings.with_slider(slider)));
            r.set_camera(&camera);
            r.set_light(&DEFAULT_LIGHT);
            for (x, z, half, c) in [far, near] {
                r.draw_model(
                    BLOCK,
                    &Transform3 {
                        position: Vec3::new(x, 0.0, z - STEREO_CONVERGENCE),
                        rotation: Vec3::ZERO,
                        scale: Vec3::new(half * 2.0, half * 2.0, 0.02),
                    },
                    Material::VertexLit,
                    &flat(c),
                );
            }
            r.set_layer_depth(LayerDepth(16));
            r.fill_rect(hud, yellow);
            r.set_layer_depth(LayerDepth::SCREEN);
            if r.stereo_eye() != Eye::Mono {
                return Err("stereo: game code saw an eye (draws are replayed per eye)");
            }
            r.end_frame();
        }
        Ok(r.stats().stereo)
    };
    let stats = draw(renderer, 1.0)?;
    let gpu_draws = renderer.stats().tnl.gpu_draws;
    let two = stats.eyes == 2;
    if caps.stereo != two {
        return Err("stereo: Caps::stereo and the eyes drawn disagree");
    }
    // Where a square's edges land in one eye (front face, z - 0.01).
    let ctx = trino_core::TnlContext::new(&camera, &DEFAULT_LIGHT, screen);
    let shift = |eye: Eye| eye.sign() * STEREO_SEPARATION * 0.5;
    let edges = |sq: (f32, f32, f32, Color), eye: Eye| {
        let z = sq.1 - 0.01;
        let x0 = project_eye(
            Vec3::new(sq.0 - sq.2, 0.0, z),
            ctx.focal(),
            ctx.center(),
            shift(eye),
            STEREO_CONVERGENCE,
        )
        .x;
        let x1 = project_eye(
            Vec3::new(sq.0 + sq.2, 0.0, z),
            ctx.focal(),
            ctx.center(),
            shift(eye),
            STEREO_CONVERGENCE,
        )
        .x;
        (x0, x1)
    };
    let cy = (screen.y * 0.5) as u32;
    let close = |a: Color, b: Color| {
        let d = |x: u8, y: u8| (x as i32 - y as i32).abs() <= 60;
        d(a.r, b.r) && d(a.g, b.g) && d(a.b, b.b)
    };
    // (eye, x, y, want)
    let mut probes: [(Eye, f32, u32, Color); 10] = [(Eye::Mono, 0.0, 0, Color::BLACK); 10];
    let hy = (hud.pos.y + hud.size.y * 0.5) as u32;
    if two {
        let (fl, fr) = edges(far, Eye::Left);
        let (nl0, nl1) = edges(near, Eye::Left);
        let (nr0, nr1) = edges(near, Eye::Right);
        let want = trino_core::stereo::parallax_px(
            near.1 - 0.01,
            STEREO_SEPARATION,
            ctx.focal(),
            STEREO_CONVERGENCE,
        );
        if !(want < -8.0 && ((nr0 - nl0) - want).abs() < 0.5) {
            return Err("stereo: the near square's parallax is not the closed form's");
        }
        let (h0, h1) = (hud.pos.x, hud.pos.x + hud.size.x);
        probes = [
            // Zero parallax: the far square's edges in both eyes.
            (Eye::Left, fl + 3.0, cy, cyan),
            (Eye::Right, fl + 3.0, cy, cyan),
            (Eye::Left, fr + 3.0, cy, Color::BLACK),
            (Eye::Right, fr + 3.0, cy, Color::BLACK),
            // Crossed: inside the right eye's copy, left of the left one's.
            (Eye::Right, nr0 + 3.0, cy, magenta),
            (Eye::Left, nr0 + 3.0, cy, Color::BLACK),
            (Eye::Left, nl1 - 3.0, cy, magenta),
            (Eye::Right, nl1 - 3.0, cy, Color::BLACK),
            // The HUD: the left eye's 8 px right, the right eye's 8 px left.
            (Eye::Right, h0 - 4.0, hy, yellow),
            (Eye::Left, h1 + 4.0, hy, yellow),
        ];
        let _ = nr1;
    }
    let mut read = 0;
    for (eye, x, y, want) in probes.iter().filter(|_| two) {
        let Some(got) = renderer.read_eye_pixel(*eye, *x as u32, *y) else {
            break;
        };
        read += 1;
        if !close(got, *want) {
            return Err(match (eye, *want == Color::BLACK) {
                (Eye::Left, false) => "stereo: the left eye is missing a shape it should show",
                (Eye::Left, true) => "stereo: the left eye shows a shape where it shouldn't",
                (_, false) => "stereo: the right eye is missing a shape it should show",
                (_, true) => "stereo: the right eye shows a shape where it shouldn't",
            });
        }
    }
    // The slider at 0: one picture, the mono one, no extra eye.
    let mono = draw(renderer, 0.0)?;
    if mono.eyes > 1 || (caps.stereo && mono.mono != StereoMono::Slider) {
        return Err("stereo: the slider at 0 still drew two eyes");
    }
    if renderer.read_eye_pixel(Eye::Right, 0, 0).is_some() {
        return Err("stereo: a mono frame has a right eye");
    }
    let (ml0, ml1) = edges(near, Eye::Mono);
    let (fm0, _) = edges(far, Eye::Mono);
    let mono_probes = [
        (ml0 + 3.0, cy, magenta),
        (ml0 - 3.0, cy, Color::BLACK),
        (ml1 - 3.0, cy, magenta),
        (ml1 + 3.0, cy, Color::BLACK),
        (fm0 + 3.0, cy, cyan),
        (hud.pos.x + 3.0, hy, yellow),
        (hud.pos.x - 3.0, hy, Color::BLACK),
    ];
    for (x, y, want) in mono_probes {
        let Some(got) = renderer.read_eye_pixel(Eye::Mono, x as u32, y) else {
            return Ok(StereoProbe::Skipped);
        };
        if !close(got, want) {
            return Err("stereo: the mono frame is not the mono picture");
        }
    }
    Ok(match (two, read) {
        (true, 10) => StereoProbe::Eyes { gpu_draws },
        (true, _) => StereoProbe::Skipped,
        (false, _) => StereoProbe::Mono,
    })
}

/// Two screens: `Renderer::on_screen` / `screen_size` /
/// `read_screen_pixel` and touch routing (`InputState::pointer_on`).
///
/// - A bottom-screen pointer is never reported to top-screen picking.
/// - With a second screen (3DS, the PC's 3DS profile): a strip on each
///   screen and the quadrant card in 3D on the bottom, then both screens
///   read back. The card's quadrants are probed where the *bottom's*
///   320x240 projection puts them (a top-screen projection leaking through
///   misses them), the top keeps its clear color where the card would land
///   there, and neither strip shows on the other screen (no missing or
///   shared framebuffer). Going back to the top after the bottom must be
///   refused.
/// - With one screen (N64, PC native): `on_screen(Bottom)` says no and
///   draws stay on the top: `Ok(false)`, also where read-back is missing.
pub fn dual_screen(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let mut touch = InputState::default();
    touch.pointer = Some(Vec2::new(40.0, 30.0));
    touch.pointer_screen = Screen::Bottom.index();
    if touch.pointer_on(Screen::Top).is_some() {
        return Err("dual screen: a bottom-screen touch reached top-screen picking");
    }
    if touch.pointer_on(Screen::Bottom) != Some(Vec2::new(40.0, 30.0)) {
        return Err("dual screen: a bottom-screen touch is missing on the bottom");
    }
    let top = renderer
        .screen_size(Screen::Top)
        .ok_or("dual screen: no top screen")?;
    let Some(bottom) = renderer.screen_size(Screen::Bottom) else {
        renderer.begin_frame(Color::BLACK);
        let refused = !renderer.on_screen(Screen::Bottom);
        let stayed = renderer.screen() == Screen::Top && renderer.on_screen(Screen::Top);
        renderer.end_frame();
        if !(refused && stayed) {
            return Err("dual screen: a single-screen target left the top screen");
        }
        return Ok(false);
    };
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let clear = [0, 0, 120];
    let (yellow, cyan) = (Color::rgb(255, 255, 0), Color::rgb(0, 255, 255));
    // Several identical frames: the 3DS LCDs are double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::rgb(clear[0], clear[1], clear[2]));
        if !renderer.on_screen(Screen::Top) {
            return Err("dual screen: on_screen(Top) refused at the frame's start");
        }
        renderer.fill_rect(Rect::new(0.0, 0.0, top.x, 16.0), yellow);
        if !renderer.on_screen(Screen::Bottom) {
            return Err("dual screen: on_screen(Bottom) refused with a second screen");
        }
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(CARD, &Transform3::default(), Material::VertexLit, &unlit);
        renderer.fill_rect(Rect::new(0.0, bottom.y - 16.0, bottom.x, 16.0), cyan);
        if renderer.on_screen(Screen::Top) || renderer.screen() != Screen::Bottom {
            return Err("dual screen: went back to the top after the bottom");
        }
        renderer.end_frame();
    }
    let close = |c: Color, want: [u8; 3], tol: i32| {
        (c.r as i32 - want[0] as i32).abs() <= tol
            && (c.g as i32 - want[1] as i32).abs() <= tol
            && (c.b as i32 - want[2] as i32).abs() <= tol
    };
    let Some(strip) = renderer.read_screen_pixel(Screen::Top, 8, 8) else {
        return Ok(false); // backend cannot read back
    };
    if !close(strip, [255, 255, 0], 40) {
        return Err("dual screen: the top strip is missing");
    }
    let mid = renderer.read_screen_pixel(Screen::Top, top.x as u32 / 2, top.y as u32 / 2);
    if !mid.is_some_and(|c| close(c, clear, 40)) {
        return Err("dual screen: bottom-screen 3D landed on the top screen");
    }
    let low = renderer.read_screen_pixel(Screen::Top, 8, top.y as u32 - 8);
    if !low.is_some_and(|c| close(c, clear, 40)) {
        return Err("dual screen: the bottom strip landed on the top screen");
    }
    let at = |r: &mut dyn Renderer, x: f32, y: f32| {
        r.read_screen_pixel(Screen::Bottom, x as u32, y as u32)
    };
    if !at(renderer, 8.0, bottom.y - 8.0).is_some_and(|c| close(c, [0, 255, 255], 40)) {
        return Err("dual screen: the bottom strip is missing (no bottom framebuffer?)");
    }
    if at(renderer, 8.0, 8.0).is_none_or(|c| close(c, [255, 255, 0], 80)) {
        return Err("dual screen: the top strip landed on the bottom screen");
    }
    // The card through the bottom's projection: near its corners and just
    // outside its left and right edges. A 400-wide projection shifts the
    // card 40 px right, enough to move both kinds of probe.
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * bottom.y * 0.5;
    let project = |world: Vec3| {
        let v = view.transform_point(world);
        (
            bottom.x * 0.5 + v.x * focal / v.z,
            bottom.y * 0.5 - v.y * focal / v.z,
        )
    };
    let corners = [
        (Vec3::new(-0.35, 0.35, -0.05), [255, 0, 0]),
        (Vec3::new(0.35, 0.35, -0.05), [0, 255, 0]),
        (Vec3::new(-0.35, -0.35, -0.05), [0, 0, 255]),
        (Vec3::new(0.35, -0.35, -0.05), [255, 255, 255]),
    ];
    for (world, want) in corners {
        let (x, y) = project(world);
        if !at(renderer, x, y).is_some_and(|c| close(c, want, 48)) {
            return Err("dual screen: wrong color in the bottom screen's 3D card");
        }
    }
    for side in [-0.7, 0.7] {
        let (x, y) = project(Vec3::new(side, 0.0, -0.05));
        if !at(renderer, x, y).is_some_and(|c| c.r < 60 && c.g < 60 && c.b < 60) {
            return Err("dual screen: the bottom screen's 3D card is off its projection");
        }
    }
    Ok(true)
}

/// Unit block (1x1x1, centered) used by [`occlusion`].
const BLOCK: ModelId = ModelId::from_path("models/c64_block_stone");

/// Depth occlusion: boxes standing on a large floor, seen from a
/// third-person camera — castle64's player and boars on the ground. Probes
/// each box's front face just above the floor: it must show the box, never
/// the floor. A backend that resolves depth per triangle instead of per
/// pixel paints big floor triangles over whatever stands on them (the
/// original 3DS path: citro2d's one averaged depth per triangle). The PC
/// and N64 z-buffers pass trivially; the check exists so no backend can
/// regress to per-triangle depth. `Ok(false)` where `read_pixel` is
/// unsupported.
pub fn occlusion(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 2.6, -6.0),
        target: Vec3::new(0.0, 0.4, 0.0),
        ..Camera3::default()
    };
    // Flat colors whatever the block's vertex colors: tint black for the
    // floor; the boxes' tint zeroes green and the emissive saturates red
    // and blue — pure magenta.
    let floor = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgba(0, 0, 0, 255),
        ..ModelParams::DEFAULT
    };
    let boxes = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgba(255, 0, 255, 255),
        emissive: Color::rgba(255, 0, 255, 255),
        ..ModelParams::DEFAULT
    };
    let spots = [
        Vec3::new(-2.2, 0.5, -1.0),
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::new(2.0, 0.5, 2.5),
        Vec3::new(-1.0, 0.5, 5.0),
    ];
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::rgba(40, 60, 200, 255));
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(
            BLOCK,
            &Transform3 {
                position: Vec3::new(0.0, -0.5, 4.0),
                rotation: Vec3::ZERO,
                scale: Vec3::new(30.0, 1.0, 30.0),
            },
            Material::VertexLit,
            &floor,
        );
        for at in spots {
            let t = Transform3 {
                position: at,
                ..Transform3::default()
            };
            renderer.draw_model(BLOCK, &t, Material::VertexLit, &boxes);
        }
        renderer.end_frame();
    }
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let pixel = |renderer: &mut dyn Renderer, world: Vec3| {
        let v = view.transform_point(world);
        let x = screen.x * 0.5 + v.x * focal / v.z;
        let y = screen.y * 0.5 - v.y * focal / v.z;
        renderer.read_pixel(x as u32, y as u32)
    };
    // The floor itself rendered (in front of the first box).
    let Some(f) = pixel(renderer, Vec3::new(0.0, 0.0, -2.0)) else {
        return Ok(false); // backend cannot read back
    };
    if f.r > 60 || f.g > 60 || f.b > 60 {
        return Err("occlusion: the floor did not render");
    }
    for at in spots {
        // Front face (z - 0.5), low: 0.08 and 0.25 above the floor.
        for (dx, h) in [(0.0, 0.08), (-0.25, 0.25), (0.25, 0.25)] {
            let p = Vec3::new(at.x + dx, h, at.z - 0.5);
            let Some(c) = pixel(renderer, p) else {
                return Ok(false);
            };
            if !(c.r > 150 && c.g < 100 && c.b > 150) {
                return Err("occlusion: the floor painted over a box standing on it");
            }
        }
    }
    Ok(true)
}

/// Bitmap text: the font's glyph table loaded (metrics) and drawing emits
/// one sprite per visible glyph (the atlas + sprite regions work).
pub fn text(renderer: &mut dyn Renderer) -> Result<(), &'static str> {
    // trino5x7: 'x' and '0' are 5 px wide + 1 px spacing; cells are 10 tall.
    if renderer.measure_text(FONT, "x00", 1.0) != Vec2::new(18.0, 10.0) {
        return Err("font metrics (glyph table missing or wrong)");
    }
    renderer.begin_frame(Color::BLACK);
    renderer.draw_text(FONT, Vec2::new(4.0, 4.0), "TRINO 64", &TextParams::DEFAULT);
    renderer.end_frame();
    // 7 glyphs: the space advances without drawing.
    if renderer.stats().sprites != 7 {
        return Err("draw_text glyph count");
    }
    Ok(())
}

/// Data blobs arrive byte-for-byte.
pub fn blob(platform: &mut dyn Platform) -> Result<(), &'static str> {
    match platform.blob(BLOB) {
        Some(bytes) if bytes == BLOB_CONTENT => Ok(()),
        Some(_) => Err("blob content mismatch"),
        None => Err("blob missing"),
    }
}

/// The localized font (`[fonts.i18n]`: `chars = "auto"`, one atlas per
/// language).
pub const I18N_FONT: FontId = FontId::from_path("fonts/i18n");

/// Localization (issue #59) end to end: every baked language loads from
/// `i18n/languages`, the system language picks one, and in each of en-US,
/// pt-BR and es-ES a key, its fallback, a plural and a price come out in
/// the language's words and number format, drawn with the language's own
/// glyph-subset atlas. Pixels: the price's last glyph columns are lit and
/// the columns just past its measured width are dark, so the drawn text is
/// the language's (the three prices differ in width). `Ok(false)` where
/// `read_pixel` is unsupported (the logic still ran).
#[inline(never)]
pub fn i18n(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::i18n::{Arg, LangId, Localizer, StrKey};
    const HELLO: StrKey = StrKey::new("selftest.hello");
    const PRICE: StrKey = StrKey::new("selftest.price");
    const COINS: StrKey = StrKey::new("selftest.coins");
    const ONLY_EN: StrKey = StrKey::new("selftest.only_en");
    let mut loc: Localizer<'static, 4> = Localizer::new();
    if loc.load(platform) < 3 {
        return Err("i18n: i18n/languages or a language's table is missing");
    }
    if loc.tag() != "en-US" {
        return Err("i18n: the default language is not loaded first");
    }
    // Whatever the system says, the pick is a loaded language.
    let system = platform.system_language();
    let picked = loc.pick(None, system);
    if loc.table(picked).is_none() {
        return Err("i18n: the system language's pick is not a loaded language");
    }
    let cases: [(&str, &str, &str, &str, &str, &str); 3] = [
        (
            "en-US",
            "Hello",
            "Price: $1,200.00",
            "1 coin",
            "0 coins",
            "fonts/i18n",
        ),
        (
            "pt-BR",
            "Olá",
            "Preço: R$ 1.200,00",
            "1 moeda",
            "0 moeda",
            "fonts/i18n@pt-BR",
        ),
        (
            "es-ES",
            "Hola",
            "Precio: 1200,00 €",
            "1 moneda",
            "0 monedas",
            "fonts/i18n@es-ES",
        ),
    ];
    let mut probed = true;
    let mut buf: TextBuf<48> = TextBuf::new();
    for (tag, hello, price, one, zero, atlas) in cases {
        if !loc.set_language(LangId::new(tag)) {
            return Err("i18n: a language did not load");
        }
        if loc.tr(HELLO) != hello {
            return Err("i18n: a key is not in the language's words");
        }
        if loc.tr(ONLY_EN) != "Fallback" {
            return Err("i18n: a missing key did not fall back to en-US");
        }
        if loc.tr_fmt(COINS, &[Arg::int("n", 1)], &mut buf) != one
            || loc.tr_fmt(COINS, &[Arg::int("n", 0)], &mut buf) != zero
        {
            return Err("i18n: a plural picked the wrong form");
        }
        let font = loc.font(I18N_FONT);
        if font != FontId::from_path(atlas) {
            return Err("i18n: the language does not draw with its own atlas");
        }
        let text = loc.tr_fmt(PRICE, &[Arg::money("amount", 120_000)], &mut buf);
        if text != price {
            return Err("i18n: a price is not in the language's number format");
        }
        let size = renderer.measure_text(font, text, 1.0);
        if size.x < 20.0 || size.y < 8.0 {
            return Err("i18n: the language's atlas is not loaded");
        }
        if renderer.measure_text(I18N_FONT, "Hello", 1.0).x < 10.0 {
            return Err("i18n: the default atlas is not loaded");
        }
        // Several identical frames: the 3DS LCDs are double-buffered.
        let at = Vec2::new(8.0, 8.0);
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.draw_text(font, at, text, &TextParams::DEFAULT);
            renderer.end_frame();
        }
        let lit = |r: &mut dyn Renderer, x0: f32, x1: f32| -> Option<bool> {
            let mut any = false;
            let mut x = x0;
            while x < x1 {
                for y in (at.y as u32 + 1..(at.y + size.y) as u32).step_by(2) {
                    let c = r.read_pixel(x as u32, y)?;
                    any |= c.r as u16 + c.g as u16 + c.b as u16 > 240;
                }
                x += 1.0;
            }
            Some(any)
        };
        let end = at.x + size.x;
        let last = lit(renderer, end - 4.0, end);
        match (last, lit(renderer, end + 3.0, end + 14.0)) {
            (Some(true), Some(false)) => {}
            (Some(_), Some(_)) => {
                return Err("i18n: the drawn price does not end where its language's ends");
            }
            _ => probed = false,
        }
    }
    // Back to the language the system picked.
    loc.set_language(picked);
    Ok(probed)
}

/// `ui/selftest.ui.ron`, baked as a blob on every target (issue #58).
pub const UI_SCREEN: BlobId = BlobId::from_path("ui/selftest");

/// The UI system (issue #58) end to end: the baked `ui/selftest` screen
/// parses, lays out inside the target's safe area, draws its focused
/// button in the theme's focus color, moves the focus with the D-pad,
/// clicks with A, and a touch on the bottom screen (the main one where
/// there is one screen) taps the `touch` button. `Ok(false)` where
/// `read_pixel` is unsupported (the logic still ran).
#[inline(never)]
pub fn ui_layout(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::ui::{Theme, Ui, UiEnv, UiScreen};
    let bytes = platform
        .blob(UI_SCREEN)
        .ok_or("ui layout: blob ui/selftest missing")?;
    let screen = UiScreen::parse(bytes).map_err(|_| "ui layout: ui/selftest does not parse")?;
    let mut ui: Ui<16> = Ui::new();
    let env = UiEnv::of(renderer);
    let idle = InputState::default();
    let frame = |r: &mut dyn Renderer, ui: &mut Ui<16>| {
        // Several identical frames: the 3DS LCDs are double-buffered.
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            ui.draw(r, &screen, &env);
            r.end_frame();
        }
    };
    frame(renderer, &mut ui);
    let _ = ui.update(&screen, &idle, &idle, 0.0);
    if ui.focused(&screen).map(|f| f.0) != Some("one") {
        return Err("ui layout: the first button is not focused");
    }
    if ui.unreachable(&screen).is_some() {
        return Err("ui layout: the pad cannot reach every button");
    }
    // The stack sits inside the safe area, centered.
    let (_, one) = ui.rect_of(&screen, "one").ok_or("ui layout: no `one`")?;
    let top = renderer
        .screen_size(Screen::Top)
        .ok_or("ui layout: no screen")?;
    let safe = renderer.safe_area(Screen::Top);
    let mid = one.pos.x + one.size.x * 0.5;
    if (mid - top.x * 0.5).abs() > 1.0 || one.pos.y < safe.top as f32 {
        return Err("ui layout: the stack is not centered in the safe area");
    }
    // D-pad down: `two` takes the focus; A clicks it.
    let mut down = InputState::default();
    down.set(trino_core::Button::DpadDown, true);
    let _ = ui.update(&screen, &down, &idle, 1.0 / 60.0);
    let _ = ui.update(&screen, &idle, &down, 1.0 / 60.0);
    if ui.focused(&screen).map(|f| f.0) != Some("two") {
        return Err("ui layout: D-pad down did not move the focus to `two`");
    }
    let mut a = InputState::default();
    a.set(trino_core::Button::A, true);
    if ui.update(&screen, &a, &idle, 1.0 / 60.0).clicked() != Some("two") {
        return Err("ui layout: A did not click the focused button");
    }
    let _ = ui.update(&screen, &idle, &a, 1.0 / 60.0);
    frame(renderer, &mut ui);
    let (_, two) = ui.rect_of(&screen, "two").ok_or("ui layout: no `two`")?;
    let theme = Theme::DEFAULT;
    // Left of the centered label, inside the focus ring.
    let at = |r: Rect| {
        (
            (r.pos.x + r.size.x * 0.1) as u32,
            (r.pos.y + r.size.y * 0.5) as u32,
        )
    };
    let close = |c: Color, want: Color| {
        (c.r as i32 - want.r as i32).abs() <= 40
            && (c.g as i32 - want.g as i32).abs() <= 40
            && (c.b as i32 - want.b as i32).abs() <= 40
    };
    let (x, y) = at(two);
    let Some(focused) = renderer.read_screen_pixel(Screen::Top, x, y) else {
        return ui_touch(&screen, &mut ui).map(|_| false);
    };
    if !close(focused, theme.focus) {
        return Err("ui layout: the focused button is not in the focus color");
    }
    let (x, y) = at(one);
    if !renderer
        .read_screen_pixel(Screen::Top, x, y)
        .is_some_and(|c| close(c, theme.button))
    {
        return Err("ui layout: an unfocused button is not in the button color");
    }
    ui_touch(&screen, &mut ui)?;
    // The tap focused `touch`: probe it where it is drawn.
    frame(renderer, &mut ui);
    let (on, touch) = ui
        .rect_of(&screen, "touch")
        .ok_or("ui layout: no `touch`")?;
    let (x, y) = at(touch);
    if !renderer
        .read_screen_pixel(on, x, y)
        .is_some_and(|c| close(c, theme.focus))
    {
        return Err("ui layout: the tapped button is not focused on its screen");
    }
    Ok(true)
}

/// A tap (press, then the finger lifts with no position, as on the 3DS)
/// on the `touch` button's screen clicks it.
fn ui_touch(
    screen: &trino_core::ui::UiScreen<'_>,
    ui: &mut trino_core::ui::Ui<16>,
) -> Result<(), &'static str> {
    let (on, touch) = ui.rect_of(screen, "touch").ok_or("ui layout: no `touch`")?;
    let mut press = InputState::default();
    press.pointer = Some(Vec2::new(
        touch.pos.x + touch.size.x * 0.5,
        touch.pos.y + touch.size.y * 0.5,
    ));
    press.pointer_screen = on.index();
    press.set_pointer_button(trino_core::PointerButton::Primary, true);
    let _ = ui.update(screen, &press, &InputState::default(), 1.0 / 60.0);
    let lift = InputState::default();
    if ui.update(screen, &lift, &press, 1.0 / 60.0).clicked() != Some("touch") {
        return Err("ui layout: a tap did not click the touch button");
    }
    if ui.focused(screen).map(|f| f.0) != Some("touch") {
        return Err("ui layout: a tap did not focus the touch button");
    }
    Ok(())
}

/// Save data round-trips through the real medium (EEPROM / SD / file).
pub fn save(platform: &mut dyn Platform) -> Result<(), &'static str> {
    if platform.save_capacity() < 16 {
        return Err("no save medium");
    }
    let data = b"trino-save-v1";
    platform.save_write(data).map_err(|_| "save write")?;
    let mut back = [0u8; 32];
    let n = platform.save_read(&mut back).map_err(|_| "save read")?;
    if n != data.len() || &back[..n] != data {
        return Err("save read-back mismatch");
    }
    Ok(())
}

/// The quadrant texture as a sprite (billboards sample regions of it).
pub const QUADRANTS: SpriteId = SpriteId::from_path("sprites/selftest_quadrants");

/// Billboards + particles: two camera-facing quads showing single texture
/// quadrants (a sprite region, clamped — no bleeding from the neighbour),
/// probed by pixel; then a particle burst must emit one billboard per
/// particle. `Ok(false)` where `read_pixel` is unsupported.
pub fn billboards(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let quad = |region: Rect| BillboardParams {
        size: Vec2::new(1.2, 1.2),
        region: Some(region),
        ..BillboardParams::DEFAULT
    };
    let left = Vec3::new(-1.0, 0.0, 0.0);
    let right = Vec3::new(1.0, 0.0, 0.0);
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        // Top-left quadrant (red) on the left, bottom-right (white) right.
        renderer.draw_billboard(QUADRANTS, left, &quad(Rect::new(0.0, 0.0, 8.0, 8.0)));
        renderer.draw_billboard(QUADRANTS, right, &quad(Rect::new(8.0, 8.0, 8.0, 8.0)));
        renderer.end_frame();
    }
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let mut probed = true;
    for (world, want) in [(left, [255u8, 0, 0]), (right, [255, 255, 255])] {
        let v = view.transform_point(world);
        let x = screen.x * 0.5 + v.x * focal / v.z;
        let y = screen.y * 0.5 - v.y * focal / v.z;
        let Some(got) = renderer.read_pixel(x as u32, y as u32) else {
            probed = false;
            break;
        };
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 48;
        if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
            return Err("billboard: wrong color (region or clamp broken)");
        }
    }
    // Particles: a burst of 12 draws 12 billboards (all in front of the
    // camera: a slow upward puff at the origin).
    let mut puff: ParticleSystem<16> = ParticleSystem::new(
        Emitter {
            sprite: QUADRANTS,
            rate: 0.0,
            life: (1.0, 1.0),
            speed: (0.1, 0.2),
            size: (0.3, 0.3),
            ..Emitter::DEFAULT
        },
        5,
    );
    puff.burst(12);
    puff.update(0.1);
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&camera);
    puff.draw(renderer);
    renderer.end_frame();
    if renderer.stats().sprites != 12 {
        return Err("particles: expected one billboard per particle");
    }
    Ok(probed)
}

/// The looping water ambience (`[sounds.water]`, `loop = true`).
pub const WATER: trino_core::SoundId = trino_core::SoundId::from_path("sounds/water");
/// A stinger track (`[music.fanfare]`).
pub const FANFARE: trino_core::MusicId = trino_core::MusicId::from_path("music/fanfare");

/// The backend's input with nothing pressed (the emulator harnesses press
/// nothing): no button, key, right stick or trigger. On consoles this runs
/// the shims' reads (the N64's C-button bits, the 3DS's ZL/ZR and C-stick),
/// so a bit packed into the wrong place fails here.
pub fn idle_input_checks(input: &mut dyn trino_core::Input) -> Result<(), &'static str> {
    for _ in 0..3 {
        let s = input.poll();
        if s.any_button() {
            return Err("input: a button reads held with nothing pressed");
        }
        if !s.keys.is_empty() {
            return Err("input: a key reads held with nothing pressed");
        }
        if s.right_stick != Vec2::ZERO {
            return Err("input: the right stick is off center with nothing pressed");
        }
        if s.triggers != Vec2::ZERO {
            return Err("input: a trigger reads pulled with nothing pressed");
        }
    }
    Ok(())
}

/// Audio v2 through the real backend: a steerable looping voice (start,
/// steer, stop) and a stinger over the music.
pub fn audio_checks(audio: &mut dyn trino_core::Audio) -> Result<(), &'static str> {
    use trino_core::SoundParams;
    let left = SoundParams {
        volume: 0.5,
        pan: -1.0,
        ..SoundParams::DEFAULT
    };
    let voice = audio
        .play_voice(WATER, &left, true)
        .ok_or("no voice for a looping sound")?;
    if !audio.voice_playing(voice) {
        return Err("voice not playing after play_voice");
    }
    audio.set_voice(voice, &SoundParams::DEFAULT);
    audio.stop_voice(voice);
    if audio.voice_playing(voice) {
        return Err("voice still playing after stop_voice");
    }
    audio.play_stinger(FANFARE);
    if !audio.stinger_playing() {
        return Err("stinger not playing");
    }
    audio.play_sound_with(
        WATER,
        &SoundParams {
            volume: 0.2,
            pan: 0.5,
            ..SoundParams::DEFAULT
        },
    );
    Ok(())
}

/// The `audio codecs` step's sounds: the same beep baked as PCM and as the
/// platform's default codec (VADPCM, DSP-ADPCM, Vorbis), with what each
/// proves (issue #53).
pub const AUDIO_CODEC_SOUNDS: [(trino_core::SoundId, &str); 2] = [
    (
        trino_core::SoundId::from_path("sounds/selftest_pcm"),
        "audio codecs: the PCM sound",
    ),
    (
        trino_core::SoundId::from_path("sounds/selftest_codec"),
        "audio codecs: the platform-codec sound",
    ),
];
/// A short track baked as Opus on the N64 (the platform default elsewhere),
/// played through to its end.
pub const AUDIO_OPUS: trino_core::MusicId = trino_core::MusicId::from_path("music/selftest_opus");
/// The streamed track (`[music.theme]`, music streams by default).
pub const AUDIO_STREAM: trino_core::MusicId = trino_core::MusicId::from_path("music/theme");
/// Frames the stream must play for, and the samples it must deliver in
/// them (half a second at the lowest rate a backend uses).
pub const AUDIO_STREAM_FRAMES: u32 = 60;
pub const AUDIO_STREAM_MIN_SAMPLES: u32 = 4_000;
/// Frames a ramp gets to settle (attack and release are 50 ms).
const AUDIO_DUCK_FRAMES: u32 = 15;
/// Frames a one-shot gets to finish before decoding counts as stalled.
const AUDIO_SOUND_TIMEOUT: u32 = 120;

#[derive(Clone, Copy, PartialEq, Debug)]
enum CodecPhase {
    Start,
    Sound(usize),
    Opus,
    Stream,
    Stopped,
    Duck,
    Release,
    Done,
}

/// Step `audio codecs` (issue #53) as a state machine, one call per frame
/// (the Web's animation frames drive it; [`audio_codecs`] loops it on PC
/// and consoles): the bus API reads back, each codec plays and finishes,
/// an Opus/default-codec track plays to its end, the streamed track
/// delivers samples with no underruns and releases its buffers, and a duck
/// rule ramps the music down and back. Leaves the buses as it found them.
pub struct AudioCodecs {
    phase: CodecPhase,
    frames: u32,
    voice: Option<trino_core::VoiceId>,
    volumes: [f32; trino_core::Bus::COUNT],
    base: trino_core::AudioStats,
    /// The stats after the stream played (what the runners log).
    pub streamed: trino_core::AudioStats,
}

impl Default for AudioCodecs {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCodecs {
    pub const DUCK: trino_core::DuckRule =
        trino_core::DuckRule::new(trino_core::Bus::Sfx, trino_core::Bus::Music, 0.4, 50, 50);

    pub fn new() -> Self {
        AudioCodecs {
            phase: CodecPhase::Start,
            frames: 0,
            voice: None,
            volumes: [1.0; trino_core::Bus::COUNT],
            base: trino_core::AudioStats::default(),
            streamed: trino_core::AudioStats::default(),
        }
    }

    /// Advance by one frame (call it, then let a frame of audio play). True
    /// once the step passed.
    pub fn step(&mut self, audio: &mut dyn trino_core::Audio) -> Result<bool, &'static str> {
        use trino_core::{Bus, SoundParams};
        self.frames += 1;
        match self.phase {
            CodecPhase::Start => {
                for b in Bus::ALL {
                    self.volumes[b.index()] = audio.bus_volume(b);
                }
                audio.set_bus_volume(Bus::Sfx, 0.5);
                if (audio.bus_volume(Bus::Sfx) - 0.5).abs() > 0.01 {
                    return Err("audio codecs: a bus volume doesn't read back");
                }
                audio.set_master_volume(0.75);
                audio.set_music_volume(0.6);
                if (audio.bus_volume(Bus::Master) - 0.75).abs() > 0.01
                    || (audio.bus_volume(Bus::Music) - 0.6).abs() > 0.01
                {
                    return Err("audio codecs: set_master/music_volume don't map to the buses");
                }
                audio.set_bus_muted(Bus::Ui, true);
                if !audio.bus_muted(Bus::Ui) || audio.bus_muted(Bus::Sfx) {
                    return Err("audio codecs: a bus mute doesn't read back");
                }
                audio.set_bus_muted(Bus::Ui, false);
                self.restore(audio);
                if audio.audio_stats().resident_bytes == 0 {
                    return Err("audio codecs: no sound memory reported");
                }
                self.start_sound(audio, 0)?;
            }
            CodecPhase::Sound(i) => {
                let v = self.voice.ok_or("audio codecs: lost the voice")?;
                // `start_sound` saw it playing; how soon it ends depends on
                // the frame time (a slow frame outlasts a 0.15 s beep).
                if !audio.voice_playing(v) {
                    if i + 1 < AUDIO_CODEC_SOUNDS.len() {
                        self.start_sound(audio, i + 1)?;
                    } else {
                        audio.play_music(AUDIO_OPUS, false);
                        self.enter(CodecPhase::Opus);
                    }
                } else if self.frames > AUDIO_SOUND_TIMEOUT {
                    return Err("audio codecs: a sound never finished (decoding stalled)");
                }
            }
            CodecPhase::Opus => {
                let s = audio.audio_stats();
                if self.frames == 2 && s.voices == 0 {
                    return Err("audio codecs: the Opus/codec track isn't playing");
                }
                if self.frames > 2 && s.voices == 0 {
                    audio.play_music(AUDIO_STREAM, false);
                    self.base = audio.audio_stats();
                    self.enter(CodecPhase::Stream);
                } else if self.frames > AUDIO_SOUND_TIMEOUT {
                    return Err("audio codecs: the Opus/codec track never finished");
                }
            }
            CodecPhase::Stream => {
                if self.frames >= AUDIO_STREAM_FRAMES {
                    let s = audio.audio_stats();
                    if s.streams == 0 || s.stream_bytes == 0 {
                        return Err("audio codecs: the music isn't streaming");
                    }
                    let got = s.streamed_frames.wrapping_sub(self.base.streamed_frames);
                    if got < AUDIO_STREAM_MIN_SAMPLES {
                        return Err("audio codecs: the stream delivered too few samples");
                    }
                    if s.underruns != self.base.underruns {
                        return Err("audio codecs: the stream underran");
                    }
                    self.streamed = s;
                    audio.stop_music();
                    self.enter(CodecPhase::Stopped);
                }
            }
            CodecPhase::Stopped => {
                if self.frames >= 2 {
                    if audio.audio_stats().streams != 0 {
                        return Err("audio codecs: a stopped stream kept its buffers");
                    }
                    audio.set_duck(Self::DUCK);
                    audio.play_music(AUDIO_STREAM, true);
                    let v = audio
                        .play_voice(WATER, &SoundParams::DEFAULT, true)
                        .ok_or("audio codecs: no voice for the ducking sound")?;
                    self.voice = Some(v);
                    self.enter(CodecPhase::Duck);
                }
            }
            CodecPhase::Duck => {
                if self.frames >= AUDIO_DUCK_FRAMES {
                    if audio.bus_duck(Bus::Music) > 0.45 {
                        return Err("audio codecs: the music didn't duck under the sfx");
                    }
                    if let Some(v) = self.voice.take() {
                        audio.stop_voice(v);
                    }
                    self.enter(CodecPhase::Release);
                }
            }
            CodecPhase::Release => {
                if self.frames >= AUDIO_DUCK_FRAMES {
                    if audio.bus_duck(Bus::Music) < 0.95 {
                        return Err("audio codecs: the music didn't come back after the duck");
                    }
                    let mut off = Self::DUCK;
                    off.level = 1.0;
                    audio.set_duck(off);
                    audio.stop_music();
                    self.restore(audio);
                    self.enter(CodecPhase::Done);
                }
            }
            CodecPhase::Done => {}
        }
        Ok(self.phase == CodecPhase::Done)
    }

    fn enter(&mut self, phase: CodecPhase) {
        self.phase = phase;
        self.frames = 0;
    }

    fn start_sound(
        &mut self,
        audio: &mut dyn trino_core::Audio,
        i: usize,
    ) -> Result<(), &'static str> {
        let (id, what) = AUDIO_CODEC_SOUNDS[i];
        let v = audio
            .play_voice(id, &trino_core::SoundParams::DEFAULT, false)
            .ok_or(what)?;
        if !audio.voice_playing(v) {
            return Err(what);
        }
        self.voice = Some(v);
        self.enter(CodecPhase::Sound(i));
        Ok(())
    }

    fn restore(&self, audio: &mut dyn trino_core::Audio) {
        for b in trino_core::Bus::ALL {
            audio.set_bus_volume(b, self.volumes[b.index()]);
        }
    }
}

/// [`AudioCodecs`] to the end: `frame` lets one frame of audio play (the
/// consoles poll their audio and wait a vblank; the PC renders 1/60 s
/// offline). The stats after the stream played, for the runners' logs.
pub fn audio_codecs<A: trino_core::Audio>(
    audio: &mut A,
    mut frame: impl FnMut(&mut A),
) -> Result<trino_core::AudioStats, &'static str> {
    let mut step = AudioCodecs::new();
    while !step.step(audio)? {
        frame(audio);
    }
    Ok(step.streamed)
}

/// A smooth-skinned strip (`[models.selftest_strip]`, `skinning = "smooth"`)
/// draws through the two-joint path: its triangle reaches the rasterizer.
pub fn smooth_skin(renderer: &mut dyn Renderer) -> Result<(), &'static str> {
    const STRIP: ModelId = ModelId::from_path("models/selftest_strip");
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.1, 0.5, -2.0),
        target: Vec3::new(0.1, 0.5, 0.0),
        fov_y: 1.0,
    });
    renderer.draw_model(
        STRIP,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams {
            double_sided: true,
            ..ModelParams::DEFAULT
        },
    );
    renderer.end_frame();
    if renderer.stats().tris == 0 {
        return Err("smooth-skinned model drew nothing (SKW2 path)");
    }
    Ok(())
}

/// A white card in color slots 0, 1 and 4 (`[models.selftest_slots]`,
/// glTF `_SLOT`).
pub const SLOTS_CARD: ModelId = ModelId::from_path("models/selftest_slots");

/// Color slots and a per-draw palette: the slotted card drawn face-on with
/// a palette shows white (slot 0), the first entry (slot 1) and the fourth
/// (slot 4); drawn again without one it is white everywhere. On the N64 the
/// palette draw must stay off the RSP overlay (which draws baked colors),
/// and the plain one may take it. `Ok(false)` where `read_pixel` is
/// unsupported.
pub fn palette(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let (red, blue) = ([230, 40, 40], [40, 60, 230]);
    let rgb = |c: [u8; 3]| Color::rgb(c[0], c[1], c[2]);
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    for (palette, want) in [
        (
            Some([rgb(red), Color::WHITE, Color::WHITE, rgb(blue)]),
            [[255, 255, 255], red, blue],
        ),
        (None, [[255, 255, 255]; 3]),
    ] {
        let params = ModelParams {
            shading: Shading::Unlit,
            palette,
            ..ModelParams::DEFAULT
        };
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.draw_model(
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &params,
            );
            renderer.end_frame();
        }
        // Column centers: slot 0 at x = -0.4, slot 1 at 0, slot 4 at 0.4.
        for (x, want) in [-0.4f32, 0.0, 0.4].into_iter().zip(want) {
            let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
            let px = screen.x * 0.5 + v.x * focal / v.z;
            let py = screen.y * 0.5 - v.y * focal / v.z;
            let Some(got) = renderer.read_pixel(px as u32, py as u32) else {
                return Ok(false);
            };
            let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 48;
            if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
                return Err(if palette.is_some() {
                    "palette: a color slot shows the wrong color"
                } else {
                    "palette: without a palette the card is not its baked white"
                });
            }
        }
    }
    Ok(true)
}

/// Copies the `instancing` step draws.
pub const INSTANCES: usize = 6;

/// The `instancing` step's scene: [`INSTANCES`] small slotted cards in a
/// 3 × 2 grid, each its own tint, two with a palette from the table, and
/// the call's own palette for the rest.
pub fn instancing_scene() -> ([Instance; INSTANCES], [Palette; 2], ModelParams) {
    let rgb = Color::rgb;
    let tints = [
        rgb(255, 80, 80),
        rgb(80, 255, 80),
        rgb(80, 80, 255),
        Color::WHITE,
        rgb(255, 255, 80),
        rgb(80, 255, 255),
    ];
    let palette_index = [0, 1, 0, 2, 7, 1];
    let instances = core::array::from_fn(|i| {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        Instance {
            matrix: Transform3 {
                position: Vec3::new(-0.6 + col * 0.6, 0.35 - row * 0.7, 0.0),
                rotation: Vec3::ZERO,
                scale: Vec3::new(0.45, 0.45, 0.45),
            }
            .matrix(),
            tint: tints[i],
            palette_index: palette_index[i],
        }
    });
    let palettes = [
        [
            rgb(230, 40, 40),
            Color::WHITE,
            Color::WHITE,
            rgb(40, 60, 230),
        ],
        [rgb(40, 200, 40); 4],
    ];
    let params = ModelParams {
        shading: Shading::Unlit,
        tint: rgb(255, 255, 200),
        palette: Some([rgb(200, 120, 40); 4]),
        ..ModelParams::DEFAULT
    };
    (instances, palettes, params)
}

/// Instancing ([`Renderer::draw_model_instances_with`], issue #51): the
/// same copies drawn as separate `draw_model_matrix` calls (their tint and
/// palette worked out here, by the rules [`Instance`] documents) and then
/// in one call must give the same pixels — a grid of probes plus each
/// card's center — and the same counters (the `instanced` count aside:
/// zero for the trait's default loop, one per copy where a backend counts
/// them). Then the first card must show its own tint. `Ok(false)` where
/// `read_pixel` is unsupported (the counters are still compared).
#[inline(never)]
pub fn instancing(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let (instances, palettes, params) = instancing_scene();
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let project = |p: Vec3| {
        let v = view.transform_point(p);
        (
            (screen.x * 0.5 + v.x * focal / v.z) as u32,
            (screen.y * 0.5 - v.y * focal / v.z) as u32,
        )
    };
    // 8 × 6 probes over the screen, then each card's center.
    let mut probes = [(0u32, 0u32); 48 + INSTANCES];
    for (k, p) in probes.iter_mut().enumerate().take(48) {
        let (gx, gy) = ((k % 8) as u32, (k / 8) as u32);
        *p = (
            (gx * 2 + 1) * screen.x as u32 / 16,
            (gy * 2 + 1) * screen.y as u32 / 12,
        );
    }
    for (p, i) in probes[48..].iter_mut().zip(&instances) {
        *p = project(i.matrix.transform_point(Vec3::ZERO));
    }
    let mul = |a: u8, b: u8| ((a as u16 * b as u16 + 127) / 255) as u8;
    let mut seen = [[Color::BLACK; 48 + INSTANCES]; 2];
    let mut stats = [trino_core::FrameStats::default(); 2];
    let mut read = true;
    for (pass, (seen, stats)) in seen.iter_mut().zip(&mut stats).enumerate() {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            if pass == 0 {
                for i in &instances {
                    let (a, b) = (params.tint, i.tint);
                    let palette = match i.palette_index {
                        0 => params.palette,
                        k => palettes.get(k as usize - 1).copied().or(params.palette),
                    };
                    let own = ModelParams {
                        tint: Color::rgba(
                            mul(a.r, b.r),
                            mul(a.g, b.g),
                            mul(a.b, b.b),
                            mul(a.a, b.a),
                        ),
                        palette,
                        ..params
                    };
                    renderer.draw_model_matrix(SLOTS_CARD, &i.matrix, Material::VertexLit, &own);
                }
            } else {
                renderer.draw_model_instances_with(SLOTS_CARD, &instances, &palettes, &params);
            }
            renderer.end_frame();
        }
        *stats = renderer.stats();
        for (c, &(x, y)) in seen.iter_mut().zip(&probes) {
            match renderer.read_pixel(x, y) {
                Some(got) => *c = got,
                None => read = false,
            }
        }
    }
    let [separate, one_call] = stats;
    if separate.models != INSTANCES as u32 || separate.tris == 0 {
        return Err("instancing: the separate draws drew nothing");
    }
    let mut tnl = one_call.tnl;
    if tnl.instanced != 0 && tnl.instanced != INSTANCES as u32 {
        return Err("instancing: the instanced count is not one per copy");
    }
    tnl.instanced = separate.tnl.instanced;
    if one_call.models != separate.models || one_call.tris != separate.tris || tnl != separate.tnl {
        return Err("instancing: one call did different work than the separate draws");
    }
    if !read {
        return Ok(false);
    }
    if seen[0] != seen[1] {
        return Err("instancing: one call drew different pixels than the separate draws");
    }
    // The first card: its tint (red) over the call's, slot 0 of the card.
    let c = seen[1][48];
    if !(c.r > 150 && c.g < 120 && c.b < 120) {
        return Err("instancing: the first copy does not show its own tint");
    }
    Ok(true)
}

/// The AO probe panel, with ambient occlusion baked into its vertex colors.
pub const AO_PANEL: ModelId = ModelId::from_path("models/selftest_ao");
/// The same file baked without it.
pub const AO_PANEL_RAW: ModelId = ModelId::from_path("models/selftest_ao_raw");

/// Baked ambient occlusion ([`trino_core::ao`], the consoles' tier): the
/// white panel between two fins, unlit, baked with `ao` and without. With
/// it the panel is darker next to the fins and as white as without in its
/// middle, on every target — the occlusion is plain vertex color, drawn by
/// the RSP, the PICA200 or the PC alike. `Ok(false)` where `read_pixel`
/// is unsupported.
pub fn ambient_occlusion(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let params = ModelParams {
        shading: Shading::Unlit,
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    // [edge, middle] of the panel, for the raw and the baked model.
    let mut seen = [[Color::BLACK; 2]; 2];
    for (k, model) in [AO_PANEL_RAW, AO_PANEL].into_iter().enumerate() {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.draw_model(model, &Transform3::default(), Material::VertexLit, &params);
            renderer.end_frame();
        }
        for (i, x) in [-0.46f32, 0.0].into_iter().enumerate() {
            let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
            let px = screen.x * 0.5 + v.x * focal / v.z;
            let py = screen.y * 0.5 - v.y * focal / v.z;
            let Some(c) = renderer.read_pixel(px as u32, py as u32) else {
                return Ok(false);
            };
            seen[k][i] = c;
        }
    }
    let luma = |c: Color| (c.r as i32 * 3 + c.g as i32 * 6 + c.b as i32) / 10;
    let [[raw_edge, raw_mid], [ao_edge, ao_mid]] = seen;
    if luma(raw_mid) < 200 || (luma(raw_edge) - luma(raw_mid)).abs() > 24 {
        return Err("ambient occlusion: the panel without AO is not evenly white");
    }
    if (luma(ao_mid) - luma(raw_mid)).abs() > 24 {
        return Err("ambient occlusion: the open middle of the panel darkened");
    }
    if luma(raw_edge) - luma(ao_edge) < 48 {
        return Err("ambient occlusion: no baked occlusion next to the fins");
    }
    if (ao_edge.r as i32 - ao_edge.b as i32).abs() > 24 {
        return Err("ambient occlusion: the occlusion tinted the panel");
    }
    Ok(true)
}

/// The post step's card (`[models.selftest_post]`): orange (200, 60, 20),
/// its colors graded through [`POST_LUT`] by the consoles' bake.
pub const POST_CARD: ModelId = ModelId::from_path("models/selftest_post");
/// `luts/selftest`: a channel rotation, `(r, g, b) → (g, b, r)`.
pub const POST_LUT: trino_core::LutId = trino_core::LutId::of("luts/selftest");
/// The card as it should look graded, on every target.
pub const POST_GRADED: Color = Color::rgb(60, 20, 200);

/// Post-processing (issue #64, ADR 0033), each target at its tier
/// (`Caps::post_tier`):
///
/// 1. **Grading.** The orange card drawn unlit with `set_post(grading:
///    luts/selftest)` is blue (60, 20, 200) on every target: the PC and the
///    Web grade the frame through the LUT, the consoles (and the PC's
///    console profiles) draw what their bake graded into its vertex colors.
///    Orange there means the grading never happened.
/// 2. **Bloom or glow.** Beside a small card, the frame is brighter with
///    `set_post(bloom)` plus a [`trino_core::Glow`] halo than without: the
///    Full tier blooms (the halo, `GlowMode::Baked`, draws nothing there),
///    the Baked tier draws the halo.
///
/// `Ok(false)` where `read_pixel` is unsupported (after drawing both).
#[inline(never)]
pub fn post(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    use trino_core::{Bloom, Glow, PostSettings, PostTier};
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let at = |p: Vec3| {
        let v = view.transform_point(p);
        (
            (screen.x * 0.5 + v.x * focal / v.z) as u32,
            (screen.y * 0.5 - v.y * focal / v.z) as u32,
        )
    };
    let unlit = ModelParams {
        shading: Shading::Unlit,
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    let grade = PostSettings {
        grading: Some(POST_LUT),
        ..PostSettings::DEFAULT
    };
    // 1. The graded card, filling the middle.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.set_post(Some(&grade));
        renderer.draw_model(
            POST_CARD,
            &Transform3::default(),
            Material::VertexLit,
            &unlit,
        );
        renderer.end_frame();
    }
    let (cx, cy) = at(Vec3::ZERO);
    let graded = renderer.read_pixel(cx, cy);
    // 2. A small card, with and without bloom + its halo.
    let small = Transform3 {
        scale: Vec3::new(0.3, 0.3, 0.3),
        ..Transform3::default()
    };
    let bloom = PostSettings {
        bloom: Some(Bloom {
            threshold: 0.2,
            intensity: 2.0,
            radius: 0.8,
        }),
        ..grade
    };
    let glow = Glow::new(0.6, Color::WHITE);
    let beside = at(Vec3::new(0.2, 0.0, 0.0));
    let mut seen = [None; 2];
    for (k, on) in [false, true].into_iter().enumerate() {
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.set_post(Some(if on { &bloom } else { &grade }));
            renderer.draw_model(POST_CARD, &small, Material::VertexLit, &unlit);
            if on {
                trino_core::draw_glow(renderer, &glow, Vec3::new(0.0, 0.0, -0.01));
            }
            renderer.end_frame();
        }
        seen[k] = renderer.read_pixel(beside.0, beside.1);
    }
    renderer.begin_frame(Color::BLACK);
    renderer.end_frame();
    let (Some(c), [Some(dark), Some(lit)]) = (graded, seen) else {
        return Ok(false);
    };
    let near = |a: Color, b: Color, tol: u8| {
        a.r.abs_diff(b.r) <= tol && a.g.abs_diff(b.g) <= tol && a.b.abs_diff(b.b) <= tol
    };
    if near(c, Color::rgb(200, 60, 20), 40) {
        return Err(if caps.post_tier == PostTier::Full {
            "post: the card is not graded (the post stack ran no LUT)"
        } else {
            "post: the card is not graded (the console bake ignored `grading`)"
        });
    }
    if !near(c, POST_GRADED, 28) {
        return Err("post: the graded card has the wrong color");
    }
    // The card is blue once graded: weigh the channels alike.
    let level = |c: Color| (c.r as i32 + c.g as i32 + c.b as i32) / 3;
    if level(lit) < level(dark) + 12 {
        return Err(if caps.post_tier == PostTier::Full {
            "post: no bloom beside the card"
        } else {
            "post: no glow beside the card"
        });
    }
    Ok(true)
}

/// The shadows step's floor, post and ball (`[models.selftest_shadow_*]`).
pub const SHADOW_FLOOR: ModelId = ModelId::from_path("models/selftest_shadow_floor");
pub const SHADOW_POST: ModelId = ModelId::from_path("models/selftest_shadow_post");
pub const SHADOW_BALL: ModelId = ModelId::from_path("models/selftest_shadow_ball");
/// Where the post stands and the ball floats.
pub const SHADOW_POST_AT: Vec3 = Vec3::new(-1.2, 0.0, 0.6);
pub const SHADOW_BALL_AT: Vec3 = Vec3::new(0.6, 0.9, -0.6);

/// The shadows step's sun.
pub const SHADOW_SUN: Light = Light {
    dir: Vec3::new(0.45, -0.8, 0.35),
    ..DEFAULT_LIGHT
};

/// The shadows step's camera.
pub fn shadow_camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 4.2, -6.5),
        target: Vec3::new(0.0, 0.2, 0.4),
        fov_y: 50.0 * core::f32::consts::PI / 180.0,
    }
}

/// One frame of the shadows scene: the floor (the receiver), the post
/// (`ShadowCast::Mesh`) and the ball (`ShadowCast::Auto`), with
/// `shadows` (or none).
pub fn shadow_scene(renderer: &mut dyn Renderer, shadows: Option<&ShadowSettings>) {
    renderer.begin_frame(Color::rgb(140, 175, 220));
    renderer.set_camera(&shadow_camera());
    renderer.set_light(&SHADOW_SUN);
    renderer.set_shadows(shadows);
    renderer.set_shadow_receivers(&[ShadowPlane::horizontal(Vec3::ZERO, 4.0, 4.0)]);
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    let cast = |shadow| ModelParams {
        shadow,
        ..ModelParams::DEFAULT
    };
    renderer.draw_model(
        SHADOW_FLOOR,
        &at(Vec3::ZERO),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    renderer.draw_model(
        SHADOW_POST,
        &at(SHADOW_POST_AT),
        Material::VertexLit,
        &cast(ShadowCast::Mesh),
    );
    renderer.draw_model(
        SHADOW_BALL,
        &at(SHADOW_BALL_AT),
        Material::VertexLit,
        &cast(ShadowCast::auto(0.35)),
    );
    renderer.end_frame();
}

/// Shadows ([`trino_core::shadow`], ADR 0025), at each target's tier: the
/// post's shadow along the light (projected on the N64 and the 3DS, the
/// map on the PC) and the ball's (a blob straight down on the N64, along
/// the light elsewhere) darken the floor where they land; the open floor
/// stays as it was. `Ok(false)` where `read_pixel` is unsupported.
pub fn shadows(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = shadow_camera();
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    // Where the light's ray through `p` meets the floor.
    let l = SHADOW_SUN.dir;
    let ground = |p: Vec3| Vec3::new(p.x - l.x * p.y / l.y, 0.0, p.z - l.z * p.y / l.y);
    let post = ground(SHADOW_POST_AT + Vec3::new(0.0, 1.0, 0.0));
    let ball = if caps.shadow_tier == ShadowTier::Blob {
        Vec3::new(SHADOW_BALL_AT.x, 0.0, SHADOW_BALL_AT.z - 0.1)
    } else {
        ground(SHADOW_BALL_AT)
    };
    let open = Vec3::new(-2.6, 0.0, -2.0);
    let points = [post, ball, open];
    let mut seen = [[Color::BLACK; 3]; 2];
    let mut stats = trino_core::ShadowStats::default();
    for (k, on) in [false, true].into_iter().enumerate() {
        let settings = on.then_some(ShadowSettings::DEFAULT);
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            shadow_scene(renderer, settings.as_ref());
        }
        if on {
            stats = renderer.stats().shadows;
        }
        for (i, p) in points.into_iter().enumerate() {
            let v = view.transform_point(p);
            let px = screen.x * 0.5 + v.x * focal / v.z;
            let py = screen.y * 0.5 - v.y * focal / v.z;
            let Some(c) = renderer.read_pixel(px as u32, py as u32) else {
                return Ok(false);
            };
            seen[k][i] = c;
        }
    }
    let luma = |c: Color| (c.r as i32 * 3 + c.g as i32 * 6 + c.b as i32) / 10;
    let [off, on] = seen;
    let mut line = TextBuf::<160>::new();
    let _ = core::fmt::write(
        &mut line,
        format_args!(
            "TRINO_SHADOWS: tier={} blobs={} planar={} mapped={} tris={} post={}->{} ball={}->{} \
             open={}->{}\n",
            caps.shadow_tier.key(),
            stats.blobs,
            stats.planar,
            stats.mapped,
            stats.tris,
            luma(off[0]),
            luma(on[0]),
            luma(off[1]),
            luma(on[1]),
            luma(off[2]),
            luma(on[2]),
        ),
    );
    platform.log(line.as_str());
    if luma(off[0]) - luma(on[0]) < 30 {
        return Err("shadows: no shadow of the post on the floor");
    }
    if luma(off[1]) - luma(on[1]) < 30 {
        return Err("shadows: no shadow of the ball on the floor");
    }
    if (luma(off[2]) - luma(on[2])).abs() > 6 {
        return Err("shadows: the open floor changed");
    }
    if stats.blobs + stats.planar + stats.mapped < 2 {
        return Err("shadows: the frame counted no shadows");
    }
    Ok(true)
}

/// The far-blob golden's distances (`shadow_far_scene`), meters.
pub const SHADOW_FAR_AT: [f32; 6] = [4.0, 9.0, 18.0, 35.0, 60.0, 100.0];

/// The far-blob golden (the N64's `shadows_far`, ADR 0025): blobs on a
/// 256 m floor (the shadows floor 32 times over) out to 100 m from a low
/// camera, where the RDP's compressed z steps are coarsest: each lies on
/// the floor without fighting it. The casters are small balls on the
/// floor, each blob wider the farther it is (a pixel tall or more).
pub fn shadow_far_scene(renderer: &mut dyn Renderer) {
    renderer.begin_frame(Color::rgb(140, 175, 220));
    renderer.set_camera(&shadow_far_camera());
    renderer.set_light(&SHADOW_SUN);
    renderer.set_shadows(Some(&ShadowSettings {
        distance: 200.0,
        ..ShadowSettings::DEFAULT
    }));
    renderer.set_shadow_receivers(&[ShadowPlane::ground(0.0)]);
    renderer.draw_model(
        SHADOW_FLOOR,
        &Transform3 {
            scale: Vec3::new(32.0, 1.0, 32.0),
            ..Default::default()
        },
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    for (k, z) in SHADOW_FAR_AT.iter().enumerate() {
        renderer.draw_model(
            SHADOW_BALL,
            &Transform3 {
                position: shadow_far_ball(k),
                scale: Vec3::new(0.1, 0.1, 0.1),
                ..Default::default()
            },
            Material::VertexLit,
            &ModelParams {
                shadow: ShadowCast::Blob {
                    radius: 0.5 + z * 0.06,
                },
                ..ModelParams::DEFAULT
            },
        );
    }
    renderer.end_frame();
}

/// [`shadow_far_scene`]'s camera: eye height 3 m, looking down the floor.
pub fn shadow_far_camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 3.0, -4.0),
        target: Vec3::new(0.0, 0.6, 40.0),
        fov_y: 50.0 * core::f32::consts::PI / 180.0,
    }
}

/// Where [`shadow_far_scene`]'s `k`th ball sits (alternating sides).
pub fn shadow_far_ball(k: usize) -> Vec3 {
    let z = SHADOW_FAR_AT[k];
    let side = if k.is_multiple_of(2) { -0.3 } else { 0.3 };
    Vec3::new(side * z * 0.2, 0.08, z)
}

/// Blob casters in [`shadow_crowd_scene`]: a football match's players.
pub const SHADOW_CROWD: usize = 22;

/// The blob cost scene (the N64's `shadow cost`): [`SHADOW_CROWD`] small
/// balls in a 6 x 4 grid over the shadows floor, each casting a blob
/// (`shadows` on) or not.
pub fn shadow_crowd_scene(renderer: &mut dyn Renderer, shadows: bool) {
    renderer.begin_frame(Color::rgb(140, 175, 220));
    renderer.set_camera(&shadow_camera());
    renderer.set_light(&SHADOW_SUN);
    renderer.set_shadows(shadows.then_some(&ShadowSettings::DEFAULT));
    renderer.set_shadow_receivers(&[ShadowPlane::horizontal(Vec3::ZERO, 4.0, 4.0)]);
    renderer.draw_model(
        SHADOW_FLOOR,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    let cast = ModelParams {
        shadow: ShadowCast::Blob { radius: 0.3 },
        ..ModelParams::DEFAULT
    };
    for i in 0..SHADOW_CROWD {
        let (x, z) = ((i % 6) as f32 - 2.5, (i / 6) as f32 - 1.5);
        renderer.draw_model(
            SHADOW_BALL,
            &Transform3 {
                position: Vec3::new(x * 1.1, 0.3, z * 1.2),
                scale: Vec3::new(0.5, 0.5, 0.5),
                ..Default::default()
            },
            Material::VertexLit,
            &cast,
        );
    }
    renderer.end_frame();
}

/// One frame of the water test scene: the sheet (`params`) over a red
/// floor from `camera`, at `time` with `ripples` (the `water` step, and the
/// 3DS's GPU-vs-CPU water frame).
pub fn water_scene(
    r: &mut dyn Renderer,
    camera: &Camera3,
    params: &ModelParams,
    time: f32,
    ripples: &[trino_core::Ripple],
) {
    let floor = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgba(255, 0, 0, 255),
        emissive: Color::rgba(255, 0, 0, 255),
        ..ModelParams::DEFAULT
    };
    r.begin_frame(Color::BLACK);
    r.set_camera(camera);
    r.set_light(&DEFAULT_LIGHT);
    r.set_water(time, ripples);
    r.draw_model(
        BLOCK,
        &Transform3 {
            position: Vec3::new(0.0, -1.0, 0.0),
            rotation: Vec3::ZERO,
            scale: Vec3::new(6.0, 1.0, 6.0),
        },
        Material::VertexLit,
        &floor,
    );
    r.draw_model(
        WATER_SHEET,
        &Transform3::default(),
        Material::VertexLit,
        params,
    );
    r.end_frame();
}

/// The water scene as the 3DS's GPU water path takes it (the whole sheet
/// past the near plane): a swell, a ripple, the two layers flowing, a
/// glint. `(camera, params, time, ripples)`.
pub fn gpu_water_scene() -> (Camera3, ModelParams, f32, [trino_core::Ripple; 1]) {
    let camera = Camera3 {
        eye: Vec3::new(0.0, 5.0, -5.5),
        target: Vec3::new(0.0, 0.0, 0.3),
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let params = ModelParams {
        texture: Some(WATER_TEX),
        detail: Some(DetailTexture::new(WATER_DETAIL, 1)),
        gloss: 200,
        water: Some(trino_core::Water {
            flow_speed: Vec2::new(0.2, 0.7),
            detail_flow: Vec2::new(0.5, -0.6),
            wave_amplitude: 0.15,
            wave_frequency: 0.7,
            ripple_responsiveness: 1.0,
        }),
        ..ModelParams::DEFAULT
    };
    let ripple = trino_core::Ripple {
        pos: Vec2::new(0.4, -0.3),
        age: 0.5,
        strength: 0.12,
    };
    (camera, params, 1.3, [ripple])
}

/// The water self-test's sheet: 4 × 4 m, 8 × 8 quads at y 0, translucent
/// blue (`[models.selftest_water]`).
pub const WATER_SHEET: ModelId = ModelId::from_path("models/selftest_water");
pub const WATER_TEX: SpriteId = SpriteId::from_path("sprites/tex_water");
pub const WATER_DETAIL: SpriteId = SpriteId::from_path("sprites/tex_ripples");

/// Water surfaces ([`trino_core::water`], ADR 0015): the sheet over a red
/// floor, seen from above at an angle, on every tier.
/// - **Translucent**: the floor shows through (red and blue both).
/// - **Swell**: a tall swell moves the surface (and its light).
/// - **Ripples**: a ripple's ring shows around its center (the consoles
///   draw it as a ring on the surface, the PC per pixel).
/// - **Flow**: later in time the layers have moved.
///
/// Each is two frames that must differ at probes over the sheet (or, for
/// the blend, one in between the two colors). `Ok(false)` where
/// `read_pixel` is unsupported.
pub fn water(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    use trino_core::{Ripple, Water};
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 3.0, -2.2),
        target: Vec3::new(0.0, 0.0, 0.2),
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let calm = Water {
        flow_speed: Vec2::new(0.2, 0.7),
        detail_flow: Vec2::new(0.5, -0.6),
        wave_amplitude: 0.0,
        wave_frequency: 0.5,
        ripple_responsiveness: 1.0,
    };
    let base = ModelParams {
        shading: Shading::Unlit,
        texture: Some(WATER_TEX),
        detail: Some(DetailTexture::new(WATER_DETAIL, 1)),
        water: Some(calm),
        ..ModelParams::DEFAULT
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let pixel = |world: Vec3| {
        let v = view.transform_point(world);
        (
            (screen.x * 0.5 + v.x * focal / v.z) as u32,
            (screen.y * 0.5 - v.y * focal / v.z) as u32,
        )
    };
    // Probes: a ring of 8 at 0.64 m around the middle (where the ripple
    // below spreads to), and its center.
    let mut probes = [(0u32, 0u32); 9];
    for (k, p) in probes.iter_mut().enumerate().take(8) {
        let a = k as f32 * core::f32::consts::PI / 4.0;
        *p = pixel(Vec3::new(0.64 * cos(a), 0.0, 0.64 * sin(a)));
    }
    probes[8] = pixel(Vec3::ZERO);
    let ripple = Ripple {
        pos: Vec2::new(0.0, 0.0),
        age: 0.4,
        strength: 0.12,
    };
    // One frame of the sheet over the floor; the probes' colors.
    let frame = |r: &mut dyn Renderer,
                 params: &ModelParams,
                 time: f32,
                 ripples: &[Ripple]|
     -> Option<[Color; 9]> {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            water_scene(r, &camera, params, time, ripples);
        }
        let mut out = [Color::BLACK; 9];
        for (o, &(x, y)) in out.iter_mut().zip(&probes) {
            *o = r.read_pixel(x, y)?;
        }
        Some(out)
    };
    let Some(still) = frame(renderer, &base, 0.0, &[]) else {
        return Ok(false);
    };
    // Translucent: the red floor through the blue water, in the middle.
    let c = still[8];
    if !(c.r > 40 && c.r < 230 && c.b > 50) {
        return Err("water: the floor does not show through the water");
    }
    let differ = |a: &[Color; 9], b: &[Color; 9]| {
        a.iter()
            .zip(b)
            .filter(|(x, y)| {
                let d = |p: u8, q: u8| (p as i32 - q as i32).abs();
                d(x.r, y.r) + d(x.g, y.g) + d(x.b, y.b) > 24
            })
            .count()
    };
    let lit = ModelParams {
        shading: Shading::Gouraud,
        ..base
    };
    let flat_lit = frame(renderer, &lit, 0.0, &[]).ok_or("water: read-back failed")?;
    let swell = Water {
        wave_amplitude: 0.4,
        wave_frequency: 0.7,
        ..calm
    };
    let swelling = frame(
        renderer,
        &ModelParams {
            water: Some(swell),
            ..lit
        },
        0.0,
        &[],
    )
    .ok_or("water: read-back failed")?;
    if differ(&flat_lit, &swelling) < 3 {
        return Err("water: the swell did not move the surface");
    }
    let rippled = frame(renderer, &base, 0.0, &[ripple]).ok_or("water: read-back failed")?;
    if differ(&still, &rippled) < 3 {
        return Err("water: a ripple left no ring");
    }
    // The flow on high-contrast layers (the quadrants and the white/gray
    // detail): the texture half a repeat on, then the detail layer alone.
    let quads = ModelParams {
        texture: Some(QUADRANTS16),
        detail: Some(DetailTexture::new(DETAIL, 0)),
        water: Some(Water {
            flow_speed: Vec2::new(0.0, 0.5),
            detail_flow: Vec2::new(0.0, 0.0),
            ..calm
        }),
        ..base
    };
    let start = frame(renderer, &quads, 0.0, &[]).ok_or("water: read-back failed")?;
    let later = frame(renderer, &quads, 1.0, &[]).ok_or("water: read-back failed")?;
    if differ(&start, &later) < 3 {
        return Err("water: the texture did not flow");
    }
    let detail_flows = ModelParams {
        water: Some(Water {
            flow_speed: Vec2::new(0.0, 0.0),
            detail_flow: Vec2::new(0.5, 0.0),
            ..calm
        }),
        ..quads
    };
    let start = frame(renderer, &detail_flows, 0.0, &[]).ok_or("water: read-back failed")?;
    let later = frame(renderer, &detail_flows, 1.0, &[]).ok_or("water: read-back failed")?;
    if differ(&start, &later) < 3 {
        return Err("water: the detail layer did not flow");
    }
    Ok(true)
}

/// Local lights ([`trino_core::light`]): the white slot card lit only by
/// a dim ambient, then with a red point light in front of its left column.
/// The left column turns red, the right one (out of reach) stays gray, on
/// every tier (per pixel on PC, per vertex on the consoles); the lit frame
/// draws nothing on the N64's RSP (`RspReject::LocalLight`), and the next
/// frame without lights is the unlit one again (cleared at
/// `begin_frame`). `Ok(false)` where `read_pixel` is unsupported.
pub fn local_lights(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    // Ambient only: the global light travels away from the viewer.
    let dim = Light {
        dir: Vec3::new(0.0, 0.0, -1.0),
        ambient: 0.35,
        ..DEFAULT_LIGHT
    };
    let lamp = LocalLight::point(Vec3::new(-0.4, 0.0, -0.25), Color::rgb(255, 0, 0), 0.6)
        .with_intensity(3.0);
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let probe = |r: &mut dyn Renderer, x: f32| {
        let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
        let px = screen.x * 0.5 + v.x * focal / v.z;
        let py = screen.y * 0.5 - v.y * focal / v.z;
        r.read_pixel(px as u32, py as u32)
    };
    let mut rsp_lit = 0;
    let mut seen = [[Color::BLACK; 2]; 3];
    for (pass, lights) in [&[][..], &[lamp][..], &[][..]].into_iter().enumerate() {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&dim);
            renderer.set_local_lights(lights);
            renderer.draw_model(
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &ModelParams::DEFAULT,
            );
            renderer.end_frame();
        }
        if pass == 1 {
            rsp_lit = renderer.stats().tnl.rsp_draws;
        }
        for (k, x) in [-0.4f32, 0.4].into_iter().enumerate() {
            let Some(c) = probe(renderer, x) else {
                return Ok(false);
            };
            seen[pass][k] = c;
        }
    }
    let gray = |c: Color| c.r.abs_diff(c.g) <= 24 && c.g.abs_diff(c.b) <= 24 && c.g > 40;
    let [off, on, after] = seen;
    if !(gray(off[0]) && gray(off[1])) {
        return Err("local lights: the unlit card is not gray");
    }
    if !(on[0].r as i32 >= on[0].g as i32 + 40 && on[0].r as i32 >= off[0].r as i32 + 30) {
        return Err("local lights: the lit column did not turn red");
    }
    if !gray(on[1]) {
        return Err("local lights: a light reached past its radius");
    }
    if rsp_lit != 0 {
        return Err("local lights: a lit draw stayed on the N64 RSP");
    }
    if after != off {
        return Err("local lights: lights survived begin_frame");
    }
    Ok(true)
}

/// The boar pre-rendered from 8 yaws (`cargo xtask impostor bake
/// selftest-boar`): its `TIMP` table; the atlas is
/// `sprites/impostor-selftest-boar` (CI4 within TMEM on the N64).
pub const BOAR_IMPOSTOR: BlobId = BlobId::from_path("blobs/impostor-selftest-boar");

/// Impostors: the baked table parses on the target, the view seen from the
/// side is the side view on every target (the core's deterministic yaw),
/// the draw is one billboard of exactly 2 triangles, and the boar's body
/// shows at its center. `Ok(false)` where `read_pixel` is unsupported.
pub fn impostor(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    let bytes = platform
        .blob(BOAR_IMPOSTOR)
        .ok_or("impostor: table blob missing")?;
    let imp = Impostor::parse(bytes).map_err(|_| "impostor: table does not parse")?;
    if imp.views != 8 {
        return Err("impostor: expected 8 views");
    }
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    // From the boar's right (+X), 6 m away, level with its center.
    let center = imp.center;
    let eye = Vec3::new(center.x + 6.0, center.y, center.z);
    if imp.view_for(&trino_core::Mat34::IDENTITY, eye) != 2 {
        return Err("impostor: the side camera picked the wrong view");
    }
    let camera = Camera3 {
        eye,
        target: center,
        fov_y: 0.6,
    };
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        imp.draw(renderer, &trino_core::Mat34::IDENTITY, eye, Color::WHITE);
        renderer.end_frame();
    }
    let stats = renderer.stats();
    if stats.tris != 2 || stats.sprites != 1 {
        return Err("impostor: not one billboard of 2 triangles");
    }
    let Some(got) = renderer.read_pixel(screen.x as u32 / 2, screen.y as u32 / 2) else {
        return Ok(false);
    };
    // The boar is brown: lit, and redder than it is blue.
    if (got.r as u32 + got.g as u32 + got.b as u32) < 60 || got.r <= got.b {
        return Err("impostor: the center pixel is not the boar");
    }
    Ok(true)
}

/// Sparks baked per platform (`[particles.selftest-sparks]`): the cap is
/// 24 on PC, 8 on the N64 and 16 on the 3DS.
pub const SPARKS: BlobId = BlobId::from_path("particles/selftest-sparks");

/// FNV-1a over the particles' exact positions.
fn particle_hash<const N: usize>(s: &ParticleSystem<N>) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for p in s.particles() {
        for v in [p.pos.x, p.pos.y, p.pos.z] {
            for b in v.to_bits().to_le_bytes() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
    }
    h
}

/// Particle effects: the baked effect parses on the target with this
/// platform's cap, a burst never passes it, the same seed gives the same
/// particles (and another seed others), and drawing them is one billboard
/// per particle. Logs the cap (`TRINO_STEP:particles cap N`).
#[inline(never)]
pub fn particles(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<(), &'static str> {
    let bytes = platform
        .blob(SPARKS)
        .ok_or("particles: effect blob missing")?;
    let fx = Effect::parse(bytes).map_err(|_| "particles: effect does not parse")?;
    if fx.max == 0 || fx.max > 24 {
        return Err("particles: the platform cap is not baked in");
    }
    let run = |seed: u32| {
        let mut s: ParticleSystem<32> = ParticleSystem::from_effect(&fx, seed);
        s.burst(100);
        let first = s.len();
        let mut most = first;
        for _ in 0..45 {
            s.update(1.0 / 30.0);
            most = most.max(s.len());
        }
        (s, first, most)
    };
    let (a, first, most) = run(7);
    let cap = fx.max as usize;
    if first != cap || most > cap {
        return Err("particles: the cap does not hold");
    }
    let (b, ..) = run(7);
    let (c, ..) = run(8);
    if particle_hash(&a) != particle_hash(&b) {
        return Err("particles: the same seed gave different particles");
    }
    if particle_hash(&a) == particle_hash(&c) {
        return Err("particles: another seed gave the same particles");
    }
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 1.0, -6.0),
        target: Vec3::new(0.0, 1.0, 0.0),
        fov_y: 1.0,
    });
    renderer.set_light(&DEFAULT_LIGHT);
    a.draw(renderer);
    renderer.end_frame();
    if renderer.stats().sprites as usize != a.len() {
        return Err("particles: not one billboard per particle");
    }
    let mut line: TextBuf<48> = TextBuf::new();
    let _ =
        core::fmt::Write::write_fmt(&mut line, format_args!("TRINO_STEP:particles cap {cap}\n"));
    platform.log(line.as_str());
    Ok(())
}

/// The strider's animation graph (`assets/anim/strider.anim.ron`, issue #65).
pub const STRIDER_GRAPH: BlobId = BlobId::from_path("anim/strider");
/// Its model (`[models.strider]`).
pub const STRIDER: ModelId = ModelId::from_path("models/strider");

/// The pose hash every target must reach on [`anim_graph`]'s script: the
/// graph's f32 math is the same additions and products everywhere, and the
/// hash quantizes to 1/4096 (computed on the PC; `xtask/tests/engine_checks.rs`
/// prints it when it moves).
pub const ANIM_GRAPH_HASH: u32 = 0x5dd6_83cb;

/// What the `anim graph` step saw.
#[derive(Debug)]
pub struct AnimGraphReport {
    pub hash: u32,
    pub transitions: u32,
    pub events: u32,
    /// Root motion forward over the script (mm).
    pub root_mm: i32,
    /// Whether the posed strider's pixels were read back.
    pub probed: bool,
}

/// The graph's script: 300 updates at 60 Hz — idle, a walk, a run, a hop
/// on the trigger (refused once while airborne), back to idle. Returns the
/// hash of every update's pose (clips, times, weights), events and root
/// motion, and the instance at the update with the most clips in its pose.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn anim_graph_run(
    g: &trino_core::AnimGraph,
) -> Result<(u32, trino_core::AnimGraphInstance, AnimGraphReport), &'static str> {
    use trino_core::anim::EventId;
    let speed = g
        .param_named("speed")
        .ok_or("anim graph: no `speed` parameter")?;
    let hop = g.param_named("hop").ok_or("anim graph: no `hop` trigger")?;
    let grounded = g
        .param_named("grounded")
        .ok_or("anim graph: no `grounded`")?;
    let mut i = trino_core::AnimGraphInstance::new(g);
    let mut h: u32 = 0x811c_9dc5;
    let mut mix = |v: u32| {
        for b in v.to_le_bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
    };
    let q = |v: f32| trino_core::math3d::to_i32(v * 4096.0) as u32;
    let mut busiest = (0u8, i);
    let (mut events, mut takeoffs, mut z) = (0u32, 0u32, 0.0f32);
    for u in 0..300u32 {
        let v = match u {
            0..=39 => 0.0,
            40..=99 => 0.8,
            100..=139 => 2.0,
            _ => (300 - u) as f32 * 0.005,
        };
        i.set_float(speed, v);
        // The first hop is refused: the strider is not grounded.
        i.set_bool(grounded, u != 120);
        if u == 120 || u == 150 {
            i.set_trigger(hop);
        }
        i.update(g, 1.0 / 60.0);
        let a = i.anim(g);
        mix(a.clip.0);
        mix(q(a.time));
        let mut clips = 1u8;
        if let Some(b) = a.blend {
            mix(b.clip.0);
            mix(q(b.time));
            mix(q(b.weight));
            clips += 1;
        }
        if a.over.weight > 0.0 {
            mix(a.over.clip.0);
            mix(q(a.over.time));
            mix(q(a.over.weight));
            clips += 1;
        }
        if clips > busiest.0 {
            busiest = (clips, i);
        }
        for e in i.events().iter() {
            mix(e.id.0);
            events += 1;
            takeoffs += (e.id == EventId::from_name("sfx:jump")) as u32;
        }
        let d = i.root_motion();
        mix(q(d.translation.x));
        mix(q(d.translation.z));
        z += d.translation.z;
    }
    mix(i.digest());
    if busiest.0 < 3 {
        return Err("anim graph: no pose mixed three clips (blend space into the hop)");
    }
    if takeoffs != 1 {
        return Err("anim graph: the hop's sfx:jump event did not fire exactly once");
    }
    if i.transitions != 2 {
        return Err("anim graph: expected locomotion -> hop -> locomotion");
    }
    if z < 1.0 {
        return Err("anim graph: the walk and run carried the strider less than 1 m");
    }
    let report = AnimGraphReport {
        hash: h,
        transitions: i.transitions,
        events,
        root_mm: trino_core::math3d::to_i32(z * 1000.0),
        probed: false,
    };
    Ok((h, busiest.1, report))
}

/// Animation graphs end to end (issue #65): the baked `anim/strider` graph
/// parses on the target, the script plays the same states, events and root
/// motion on every target (the hash equals [`ANIM_GRAPH_HASH`], the PC's),
/// and the busiest pose — three clips, the blend space fading into the hop
/// (`AnimRef::over`) — draws: the strider covers the screen's center.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn anim_graph(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<AnimGraphReport, &'static str> {
    let bytes = platform
        .blob(STRIDER_GRAPH)
        .ok_or("anim graph: anim/strider blob missing")?;
    let g = trino_core::AnimGraph::parse(bytes)
        .map_err(|_| "anim graph: anim/strider does not parse")?;
    if g.model() != STRIDER.0 {
        return Err("anim graph: anim/strider names another model");
    }
    let (hash, busiest, mut report) = anim_graph_run(&g)?;
    let (again, ..) = anim_graph_run(&g)?;
    if hash != again {
        return Err("anim graph: the same script gave another pose hash");
    }
    if hash != ANIM_GRAPH_HASH {
        return Err("anim graph: pose hash differs from the PC's (ANIM_GRAPH_HASH)");
    }
    let clear = Color::rgb(10, 12, 30);
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(clear);
        renderer.set_camera(&Camera3 {
            eye: Vec3::new(0.0, 1.0, 3.2),
            target: Vec3::new(0.0, 0.9, 0.0),
            fov_y: 1.0,
        });
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(
            STRIDER,
            &Transform3 {
                position: Vec3::ZERO,
                rotation: Vec3::new(0.0, core::f32::consts::PI, 0.0),
                scale: Vec3::ONE,
            },
            Material::VertexLit,
            &ModelParams {
                anim: Some(busiest.anim(&g)),
                ..ModelParams::DEFAULT
            },
        );
        renderer.end_frame();
    }
    // Its triangles on whichever path drew them (CPU, the N64's RSP, the
    // PICA200, the PC GPU: exclusive counters).
    let st = renderer.stats();
    let t = &st.tnl;
    if st.not_loaded > 0 {
        return Err("anim graph: models/strider is not loaded");
    }
    if st.tris + t.rsp_tris + t.gpu_tris + t.pc_gpu_tris == 0 {
        return Err("anim graph: the posed strider drew nothing");
    }
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as u32, caps.screen_height as u32);
    if let Some(px) = renderer.read_pixel(w / 2, h / 2) {
        if px == clear {
            return Err("anim graph: nothing at the screen's center (the strider's torso)");
        }
        report.probed = true;
    }
    Ok(report)
}

/// What the `foot ik` step saw.
#[derive(Debug)]
pub struct FootIkReport {
    /// The tier's pose (the caps' `anim_ik`) and the forced one.
    pub auto: trino_core::anim::IkPose,
    pub forced: trino_core::anim::IkPose,
    /// Whether the IK frame's pixels were compared with the plain one's.
    pub probed: bool,
    /// The CPU time of 16 IK draws less 16 plain ones (µs, per draw).
    pub us_per_draw: i32,
}

/// The strider standing on a step: its graph's idle feet (`feet()`), the
/// ground under them (the left foot's 0.2 m higher), and the pose's IK.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn ik_scene() -> (trino_core::Mat34, impl Fn(Vec3) -> Option<f32>) {
    let m = trino_core::Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, core::f32::consts::PI, 0.0),
        Vec3::ONE,
        Vec3::ZERO,
    );
    // Facing the camera, the strider's left (+X) is the screen's... left
    // after the turn: world x < 0. The step is under it.
    (m, |p: Vec3| Some(if p.x < 0.0 { 0.2 } else { 0.0 }))
}

/// Foot IK by tier (issue #65): `FootIk::pose` is the 0.2 m step under the
/// left foot where `Caps::anim_ik` is on (PC, Web, 3DS) and nothing on the
/// N64 unless forced; forced, the frame differs from the plain one (the
/// leg bent onto the step) and the tier's frame equals the plain one on the
/// N64 and the forced one elsewhere. The step also times 16 IK draws
/// against 16 plain ones (`TRINO_IK_COST:`, `perf::N64_COST.us_per_ik_draw`).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn foot_ik(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<FootIkReport, &'static str> {
    use trino_core::anim::{FootIk, IkPose};
    let bytes = platform
        .blob(STRIDER_GRAPH)
        .ok_or("foot ik: anim/strider blob missing")?;
    let g =
        trino_core::AnimGraph::parse(bytes).map_err(|_| "foot ik: anim/strider does not parse")?;
    let mut inst = trino_core::AnimGraphInstance::new(&g);
    for _ in 0..10 {
        inst.update(&g, 1.0 / 60.0);
    }
    let feet = inst
        .feet(&g)
        .ok_or("foot ik: the strider's graph has no feet (IKCH)")?;
    let caps = *renderer.caps();
    let (m, ground) = ik_scene();
    let auto = FootIk::default().pose(&caps, &m, feet, &ground);
    let forced = FootIk {
        force: true,
        ..FootIk::default()
    }
    .pose(&caps, &m, feet, &ground);
    if forced
        != (IkPose {
            left: 200,
            right: 0,
        })
    {
        return Err("foot ik: the forced pose is not the 0.2 m step under the left foot");
    }
    if caps.anim_ik != (auto == forced) || (!caps.anim_ik && !auto.is_none()) {
        return Err("foot ik: the tier's pose does not follow Caps::anim_ik");
    }
    let base = inst.anim(&g);
    let clear = Color::rgb(12, 16, 28);
    let frame_once = |renderer: &mut dyn Renderer, ik: IkPose| {
        renderer.begin_frame(clear);
        renderer.set_camera(&Camera3 {
            eye: Vec3::new(0.0, 0.6, 2.4),
            target: Vec3::new(0.0, 0.5, 0.0),
            fov_y: 1.0,
        });
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model_matrix(
            STRIDER,
            &m,
            Material::VertexLit,
            &ModelParams {
                anim: Some(base.with_ik(ik)),
                ..ModelParams::DEFAULT
            },
        );
        renderer.end_frame();
    };
    // Each frame three times: the 3DS LCD is double-buffered.
    let frame = |renderer: &mut dyn Renderer, ik: IkPose| {
        for _ in 0..3 {
            frame_once(renderer, ik);
        }
    };
    // A coarse grid over the lower half of the screen: the legs.
    let (w, h) = (caps.screen_width as u32, caps.screen_height as u32);
    let grid = |renderer: &mut dyn Renderer| -> Option<u32> {
        let mut hsh: u32 = 0x811c_9dc5;
        for y in 0..12 {
            for x in 0..16 {
                let c = renderer.read_pixel(w * (2 * x + 1) / 32, h / 2 + h * (2 * y + 1) / 48)?;
                for b in [c.r, c.g, c.b] {
                    hsh ^= b as u32;
                    hsh = hsh.wrapping_mul(0x0100_0193);
                }
            }
        }
        Some(hsh)
    };
    frame(renderer, IkPose::NONE);
    let plain = grid(renderer);
    frame(renderer, forced);
    let bent = grid(renderer);
    frame(renderer, auto);
    let tier = grid(renderer);
    let probed = match (plain, bent, tier) {
        (Some(p), Some(b), Some(t)) => {
            if p == b {
                return Err("foot ik: the forced IK frame is the plain one (no leg moved)");
            }
            if (t == b) != caps.anim_ik || (t == p) == caps.anim_ik {
                return Err("foot ik: the tier's frame is not the plain one (N64) or the IK one");
            }
            true
        }
        _ => false,
    };
    // The cost: 16 draws with both legs solved against 16 without, one a
    // frame: more striders a frame grow the N64 renderer's batch buffers
    // for good (16 at once kept 24 KB the base console's heap can't spare;
    // one is 11 parts, fewer than the showcase's two robots).
    let time = |renderer: &mut dyn Renderer, ik: IkPose| {
        let mut total = 0i64;
        for k in 0..16 {
            renderer.begin_frame(clear);
            renderer.set_light(&DEFAULT_LIGHT);
            let mm = trino_core::Mat34::from_rotation_scale_translation(
                Vec3::new(0.0, k as f32 * 0.3, 0.0),
                Vec3::ONE,
                Vec3::new(k as f32 * 0.01, 0.0, 0.0),
            );
            let t0 = platform.now_micros();
            renderer.draw_model_matrix(
                STRIDER,
                &mm,
                Material::VertexLit,
                &ModelParams {
                    anim: Some(base.with_ik(ik)),
                    ..ModelParams::DEFAULT
                },
            );
            total += platform.now_micros().saturating_sub(t0) as i64;
            renderer.end_frame();
        }
        total
    };
    let both = IkPose {
        left: 200,
        right: -100,
    };
    let plain_us = time(renderer, IkPose::NONE);
    let ik_us = time(renderer, both);
    Ok(FootIkReport {
        auto,
        forced,
        probed,
        us_per_draw: ((ik_us - plain_us) / 16) as i32,
    })
}

#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn log_foot_ik(platform: &mut dyn Platform, r: &FootIkReport) {
    let mut line: TextBuf<128> = TextBuf::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:foot ik tier={} forced={},{} {}\nTRINO_IK_COST: us_per_draw={}\n",
            if r.auto.is_none() { "off" } else { "on" },
            r.forced.left,
            r.forced.right,
            if r.probed { "probed" } else { "drawn" },
            r.us_per_draw
        ),
    );
    platform.log(line.as_str());
}

#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn log_anim_graph(platform: &mut dyn Platform, r: &AnimGraphReport) {
    let mut line: TextBuf<128> = TextBuf::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:anim graph data={:#010x} transitions={} events={} root_mm={} {}\n",
            r.hash,
            r.transitions,
            r.events,
            r.root_mm,
            if r.probed { "probed" } else { "drawn" }
        ),
    );
    platform.log(line.as_str());
}

/// The baked portal/cell demo (`scenes/interior.scene.ron`).
pub const INTERIOR: BlobId = BlobId::from_path("scenes/interior");

/// Portal visibility end to end: the interior scene baked by the pipeline,
/// parsed on the target, cells and portals rebuilt, masks computed from
/// two cameras in the first room, and the draw counts the backend saw.
pub fn portals(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<(), &'static str> {
    let bytes = platform
        .blob(INTERIOR)
        .ok_or("interior scene blob missing")?;
    let baked = BakedScene::parse(bytes).map_err(|_| "interior scene does not parse")?;
    let cells: Cells<4> = Cells::from_nodes(baked.nodes());
    if (cells.cell_count(), cells.portal_count()) != (3, 2) {
        return Err("interior: expected 3 cells and 2 portals");
    }
    let mut world: EntityStore<32> = EntityStore::new();
    world.spawn_iter(baked.nodes());
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let eye = Vec3::new(1.0, 1.6, 4.0);
    let mut models = |target: Vec3, cull: bool| {
        let camera = Camera3 {
            eye,
            target,
            fov_y: 1.05,
        };
        let mask = if cull {
            cells.visible(&camera, screen)
        } else {
            u64::MAX
        };
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        world.draw_in(renderer, &trino_core::Mat34::IDENTITY, 0.0, eye, mask);
        renderer.end_frame();
        (mask & 0b111, renderer.stats().models)
    };
    // Down the doorways: every room is visible, nothing is culled.
    let east = Vec3::new(23.0, 1.4, 4.0);
    let (mask, through) = models(east, true);
    if mask != 0b111 || through != models(east, false).1 {
        return Err("portals: the doorway chain should show all three rooms");
    }
    // Facing the back wall: rooms B and C (3 models each) are skipped.
    let west = Vec3::new(-10.0, 1.6, 4.0);
    let (mask, away) = models(west, true);
    let all = models(west, false).1;
    if mask != 0b001 || all - away != 6 {
        return Err("portals: facing away should cull the two far rooms");
    }
    Ok(())
}

/// The hardware-tier probe scene (`[scenes.selftest-tier]`, issue #54): one
/// card, drawn with [`TIER_CARD`] in the expanded tiers' variant.
pub const TIER_SCENE: BlobId = BlobId::from_path("scenes/selftest-tier");
/// A model only the expanded tiers' data has (`platforms = ["n64_expanded",
/// "n3ds_new"]`): in the ROM / app, loaded on that hardware only.
pub const TIER_CARD: ModelId = ModelId::from_path("models/selftest_tier_card");
/// Sparks capped 4, and 6 on the expanded tiers
/// (`[particles.selftest-tier-sparks]`).
pub const TIER_SPARKS: BlobId = BlobId::from_path("particles/selftest-tier-sparks");

/// What [`hw_tier`] found: the hardware's tier and the tier the loaded data
/// was resolved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierReport {
    /// [`Platform::hw_tier`] (the renderer's caps agree).
    pub tier: HwTier,
    /// The tier the probe scene, the sparks and the tier-only model were
    /// loaded for: the hardware's on a console, the base one on the PC and
    /// the Web, whose one bake serves every simulated tier.
    pub data: HwTier,
    pub ram_bytes: u32,
    pub cpu_cores: u8,
    /// The tier sparks' baked cap.
    pub sparks: u16,
}

/// The hardware tier (issue #54, ADR 0020): the platform's answer and the
/// renderer's caps agree and are that tier's table (an expanded tier adds
/// RAM or cores, never takes them); the data the build loaded is one
/// tier's throughout — the probe scene's card, the tier sparks' cap and the
/// tier-only model are all base or all expanded — and never the expanded
/// tier's on base hardware. `cargo xtask test n64|3ds` then holds `data` to
/// what the emulator ran (the `TRINO_STEP:hw tier` line).
pub fn hw_tier(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<TierReport, &'static str> {
    let caps = *renderer.caps();
    let tier = platform.hw_tier();
    if caps.hw_tier != tier {
        return Err("hw tier: the renderer's caps and the platform disagree");
    }
    let table = caps.with_tier(tier);
    if (table.ram_bytes, table.cpu_cores, table.cpu_mhz)
        != (caps.ram_bytes, caps.cpu_cores, caps.cpu_mhz)
        || (table.crowd_percent, table.particle_percent)
            != (caps.crowd_percent, caps.particle_percent)
    {
        return Err("hw tier: the caps are not the tier's table");
    }
    let base = caps.base();
    if caps.ram_bytes < base.ram_bytes || caps.cpu_cores < base.cpu_cores {
        return Err("hw tier: an expanded tier with less than the base console");
    }
    // The probe scene: the card it draws says which tier it resolved for.
    let bytes = platform
        .blob(TIER_SCENE)
        .ok_or("hw tier: probe scene blob missing")?;
    let scene = BakedScene::parse(bytes).map_err(|_| "hw tier: probe scene does not parse")?;
    let data = match scene.find("probe").and_then(|n| n.model) {
        Some(CARD) => HwTier::Base,
        Some(TIER_CARD) => HwTier::Expanded,
        _ => return Err("hw tier: the probe draws neither card"),
    };
    if data.is_expanded() && !tier.is_expanded() {
        return Err("hw tier: the expanded tier's data on base hardware");
    }
    let sparks = platform
        .blob(TIER_SPARKS)
        .ok_or("hw tier: tier sparks blob missing")?;
    let fx = Effect::parse(sparks).map_err(|_| "hw tier: tier sparks do not parse")?;
    if fx.max != if data.is_expanded() { 6 } else { 4 } {
        return Err("hw tier: the sparks' cap is another tier's");
    }
    // The tier-only model draws exactly where the expanded data is loaded.
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    });
    renderer.set_light(&DEFAULT_LIGHT);
    renderer.draw_model(
        TIER_CARD,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    renderer.end_frame();
    if (renderer.stats().models == 1) != data.is_expanded() {
        return Err("hw tier: the tier-only model is loaded on the wrong tier");
    }
    Ok(TierReport {
        tier,
        data,
        ram_bytes: caps.ram_bytes,
        cpu_cores: caps.cpu_cores,
        sparks: fx.max,
    })
}

/// `TRINO_STEP:hw tier expanded ram=8M cores=1 data=expanded sparks=6`.
pub fn log_tier(platform: &mut dyn Platform, t: &TierReport) {
    let mut line: TextBuf<96> = TextBuf::new();
    let ram = if t.ram_bytes == u32::MAX {
        0
    } else {
        t.ram_bytes >> 20
    };
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:hw tier {} ram={ram}M cores={} data={} sparks={}\n",
            t.tier.key(),
            t.cpu_cores,
            t.data.key(),
            t.sparks
        ),
    );
    platform.log(line.as_str());
}

/// What [`quality`] found (issue #55).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityReport {
    pub preset: QualityPreset,
    /// The console's own preset: no host reports one
    /// ([`Platform::quality_info`] is `None`).
    pub fixed: bool,
    /// [`Renderer::lod_bias`] × 100.
    pub lod_pct: u16,
    /// The sparks' cap after the quality's particle scale.
    pub particles: usize,
}

/// The quality preset (issue #55). A console's is fixed and scales nothing:
/// `Low` on the N64 (with or without the Expansion Pak) and the 3DS,
/// `Medium` on the New 3DS ([`QualitySettings::console`]); `set_quality`
/// changes nothing, no preset table is offered, the renderer's LOD bias is
/// 1 and the particle caps stay the baked ones. A host that manages the
/// quality (the PC's native profile, the Web) reports it and offers its
/// presets; its renderer applies the LOD bias, its particle scale holds.
pub fn quality(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<QualityReport, &'static str> {
    let q = platform.quality();
    if q.clamped() != q {
        return Err("quality: settings out of their ranges");
    }
    let bytes = platform
        .blob(SPARKS)
        .ok_or("quality: effect blob missing")?;
    let fx = Effect::parse(bytes).map_err(|_| "quality: effect does not parse")?;
    let mut sparks: ParticleSystem<32> = ParticleSystem::from_effect(&fx, 3);
    sparks.scale_limit(q.particle_scale);
    sparks.burst(100);
    if sparks.len() != q.particle_cap(fx.max as usize).min(32) {
        return Err("quality: the particle scale does not hold");
    }
    let fixed = platform.quality_info().is_none();
    let bias = renderer.lod_bias();
    if fixed {
        if q != QualitySettings::console(renderer.caps()) {
            return Err("quality: not the console's fixed preset");
        }
        if q.render_scale != 1.0 || q.dynamic_resolution.is_some() || q.particle_scale != 1.0 {
            return Err("quality: a console's preset scales something");
        }
        platform.set_quality(QualitySettings::ULTRA);
        if platform.quality() != q {
            return Err("quality: set_quality changed a console's preset");
        }
        if platform.quality_preset(QualityPreset::Low).is_some() {
            return Err("quality: a console offers presets");
        }
        if bias != 1.0 {
            return Err("quality: a console's renderer biases its LODs");
        }
    } else {
        if platform.quality_preset(QualityPreset::Low).is_none() {
            return Err("quality: the host offers no presets");
        }
        if (bias - q.lod_bias).abs() > 1e-3 {
            return Err("quality: the renderer's LOD bias is not the quality's");
        }
    }
    Ok(QualityReport {
        preset: q.preset,
        fixed,
        lod_pct: (bias * 100.0 + 0.5) as u16,
        particles: sparks.len(),
    })
}

/// `TRINO_STEP:quality low fixed lod=100 particles=4`.
pub fn log_quality(platform: &mut dyn Platform, q: &QualityReport) {
    let mut line: TextBuf<96> = TextBuf::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:quality {} {} lod={} particles={}\n",
            q.preset.key(),
            if q.fixed { "fixed" } else { "hosted" },
            q.lod_pct,
            q.particles
        ),
    );
    platform.log(line.as_str());
}

// ---------------------------------------------------------------------------
// Asset groups (issue #56): `Platform::load_group` / `unload_group`.

/// `[groups.selftest_level]`: the card, its texture and a blob.
pub const LEVEL: trino_core::GroupId = trino_core::GroupId::from_name("selftest_level");
/// `[groups.selftest_level2]`: the same texture (shared), a sound and a
/// blob of its own (the N64 compresses it at level 2: aPLib).
pub const LEVEL2: trino_core::GroupId = trino_core::GroupId::from_name("selftest_level2");
pub const LEVEL_TILE: SpriteId = SpriteId::from_path("sprites/selftest_level_tile");
pub const LEVEL_CARD: ModelId = ModelId::from_path("models/selftest_level_card");
pub const LEVEL_BLOB: BlobId = BlobId::from_path("blobs/selftest_level");
pub const LEVEL_BLOB_CONTENT: &[u8] = b"trino-group-ok\n";
pub const LEVEL2_BLOB: BlobId = BlobId::from_path("blobs/selftest_level2");

/// What the `asset groups` check drives: the backends and the host's loader
/// (`trino_runner_pc::GroupLoader::pump` on the PC, the console hosts' own
/// pump), one call per frame, before the game's update.
pub trait GroupHost {
    fn parts(&mut self) -> (&mut dyn Renderer, &mut dyn Platform);
    fn pump(&mut self);
    /// How far the heap may sit above its baseline after an unload: what
    /// the backend's own bookkeeping grows by while frames go by (the
    /// consoles: none to speak of; wgpu's trackers grow a few KB per
    /// thousand frames even without loads).
    fn heap_tolerance(&self) -> u32 {
        8 * 1024
    }
}

/// Frames a group may take to load before the check gives up.
const LOAD_FRAMES: u32 = 600;

/// One frame as a game's loading screen draws it: a bar of `percent`, the
/// host's loader step first.
fn loading_frame(host: &mut dyn GroupHost, percent: u8) {
    host.pump();
    let (renderer, _) = host.parts();
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    renderer.begin_frame(Color::BLACK);
    let full = w * 0.6;
    renderer.fill_rect(
        Rect::new(w * 0.2, h * 0.5, full, 8.0),
        Color::rgb(60, 60, 60),
    );
    let done = full * percent as f32 / 100.0;
    renderer.fill_rect(
        Rect::new(w * 0.2, h * 0.5, done, 8.0),
        Color::rgb(80, 220, 120),
    );
    renderer.end_frame();
}

/// Load `group` behind a loading bar: progress never goes back, and it ends
/// `Ready` within [`LOAD_FRAMES`]. Returns the frames it took.
fn load_until_ready(
    host: &mut dyn GroupHost,
    group: trino_core::GroupId,
) -> Result<u32, &'static str> {
    let mut last = 0u8;
    for frame in 0..LOAD_FRAMES {
        let state = host.parts().1.load_group(group);
        match state {
            trino_core::LoadState::Ready => return Ok(frame),
            trino_core::LoadState::Failed => return Err("asset groups: a group failed to load"),
            trino_core::LoadState::Loading { progress } => {
                if progress < last {
                    return Err("asset groups: load progress went back");
                }
                last = progress;
                loading_frame(host, progress);
            }
            trino_core::LoadState::Unloaded => {
                return Err("asset groups: load_group left the group unloaded");
            }
        }
    }
    Err("asset groups: a group never got ready")
}

/// Draw the level's tile (8x: 128x128, its four quadrants) and card; the
/// frame's `not_loaded` count and, where `read_pixel` works, the tile's
/// top-left quadrant (red) or `None`.
fn draw_level(host: &mut dyn GroupHost) -> (u32, Option<Color>) {
    let (renderer, _) = host.parts();
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    let tile = SpriteParams {
        scale: Vec2::new(8.0, 8.0),
        ..SpriteParams::DEFAULT
    };
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -3.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let mut not_loaded = 0;
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(
            LEVEL_CARD,
            &Transform3 {
                position: Vec3::new(1.2, 0.0, 0.0),
                ..Transform3::default()
            },
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        renderer.draw_sprite(LEVEL_TILE, Vec2::new(w * 0.5 - 64.0, h * 0.5 - 64.0), &tile);
        renderer.end_frame();
        not_loaded = renderer.stats().not_loaded;
    }
    let probe = renderer.read_pixel((w * 0.5 - 32.0) as u32, (h * 0.5 - 32.0) as u32);
    (not_loaded, probe)
}

fn is_red(c: Option<Color>) -> bool {
    c.is_none_or(|c| c.r > 200 && c.g < 60 && c.b < 60)
}

/// The heap a backend reports (0 where it doesn't count).
fn heap_used(host: &mut dyn GroupHost) -> u32 {
    host.parts().1.memory().heap_used
}

/// What the `asset groups` step measured, for its log line.
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupReport {
    /// Frames the first load took (behind the loading bar).
    pub load_frames: u32,
    /// Bytes the level's assets took (`MemoryReport::assets`).
    pub level_bytes: u32,
    /// Heap after the warm-up unload, and the most it was above that after
    /// any later unload.
    pub heap_baseline: u32,
    pub heap_drift: u32,
    /// Load/unload cycles of the soak, and its frames.
    pub cycles: u32,
    pub soak_frames: u32,
}

/// Asset groups end to end: an unloaded group's assets don't draw (counted
/// as `not_loaded`, never a crash) and its blob isn't there; `load_group`
/// reaches `Ready` behind a loading bar with progress that never goes back;
/// its sprite, card and blob are there and its bytes count in `memory()`;
/// a texture two groups share stays while either is loaded; an unloaded
/// group frees its memory (the heap back within the host's tolerance of the
/// warm-up baseline); an unknown group fails. Then `soak_frames` frames of
/// load/unload cycles (fragmentation): the heap never drifts from the
/// baseline and the group still loads at the end.
pub fn asset_groups(
    host: &mut dyn GroupHost,
    soak_frames: u32,
) -> Result<GroupReport, &'static str> {
    let tolerance = host.heap_tolerance();
    let mut report = GroupReport::default();
    let unknown = trino_core::GroupId::from_name("selftest_no_such_group");
    {
        let (_, platform) = host.parts();
        if platform.load_group(trino_core::GroupId::BOOT) != trino_core::LoadState::Ready {
            return Err("asset groups: boot is not ready");
        }
        if platform.group_state(LEVEL) != trino_core::LoadState::Unloaded {
            return Err("asset groups: the level group is not in this build (or loaded at boot)");
        }
        if platform.group_blob(LEVEL_BLOB).is_some() {
            return Err("asset groups: an unloaded group's blob is there");
        }
        if platform.load_group(unknown) != trino_core::LoadState::Failed {
            return Err("asset groups: an unknown group did not fail");
        }
    }
    let (missing, _) = draw_level(host);
    if missing < 2 {
        return Err("asset groups: an unloaded group's draws were not counted as not_loaded");
    }
    let before = host.parts().1.memory();
    report.load_frames = load_until_ready(host, LEVEL)?;
    let after = host.parts().1.memory();
    report.level_bytes = after.assets().saturating_sub(before.assets());
    if report.level_bytes == 0 {
        return Err("asset groups: the loaded group's assets are not counted in memory()");
    }
    if host.parts().1.group_blob(LEVEL_BLOB) != Some(LEVEL_BLOB_CONTENT) {
        return Err("asset groups: the group's blob is missing or wrong");
    }
    let (missing, probe) = draw_level(host);
    if missing != 0 {
        return Err("asset groups: a ready group's assets did not draw");
    }
    if !is_red(probe) {
        return Err("asset groups: the group's sprite drew the wrong color");
    }
    // The shared texture survives the first group's unload.
    load_until_ready(host, LEVEL2)?;
    if host.parts().1.group_blob(LEVEL2_BLOB) != Some(LEVEL_BLOB_CONTENT) {
        return Err("asset groups: the second group's blob is missing or wrong");
    }
    host.parts().1.unload_group(LEVEL);
    if host.parts().1.group_state(LEVEL) != trino_core::LoadState::Unloaded {
        return Err("asset groups: unload_group left the group loaded");
    }
    host.pump();
    if host.parts().1.group_blob(LEVEL_BLOB).is_some() {
        return Err("asset groups: an unloaded group's blob is still there");
    }
    let (missing, probe) = draw_level(host);
    if missing != 1 || !is_red(probe) {
        return Err("asset groups: a texture both groups hold went with the first unload");
    }
    host.parts().1.unload_group(LEVEL2);
    host.pump();
    let (missing, _) = draw_level(host);
    if missing < 2 {
        return Err("asset groups: the last unload left the shared texture loaded");
    }
    let freed = host.parts().1.memory();
    if freed.assets() != before.assets() {
        return Err("asset groups: memory() still counts an unloaded group's assets");
    }
    // Warm-up done: everything the backends keep around exists now.
    loading_frame(host, 100);
    report.heap_baseline = heap_used(host);
    // The soak: what a game does between levels, one frame at a time (each
    // frame pumps the loader and draws, as a loading screen would).
    let mut cycle_frames = 0u32;
    while cycle_frames < soak_frames {
        let state = host.parts().1.load_group(LEVEL);
        match state {
            trino_core::LoadState::Ready => {
                host.parts().1.unload_group(LEVEL);
                loading_frame(host, 100);
                report.cycles += 1;
                let drift = heap_used(host).saturating_sub(report.heap_baseline);
                report.heap_drift = report.heap_drift.max(drift);
                if drift > tolerance {
                    return Err("asset groups: the heap grows over load/unload cycles");
                }
            }
            trino_core::LoadState::Loading { progress } => loading_frame(host, progress),
            _ => return Err("asset groups: a group failed during the soak"),
        }
        cycle_frames += 1;
    }
    report.soak_frames = cycle_frames;
    // After the soak (a fragmented heap would fail here) the group loads.
    load_until_ready(host, LEVEL)?;
    host.parts().1.unload_group(LEVEL);
    loading_frame(host, 100);
    if heap_used(host).saturating_sub(report.heap_baseline) > tolerance {
        return Err("asset groups: the heap did not return to its baseline after unload");
    }
    Ok(report)
}

/// The strider's group (`[groups.strider]`): the animation steps load it.
pub const STRIDER_GROUP: trino_core::GroupId = trino_core::GroupId::from_name("strider");

/// The animation steps (issue #65), with the strider's group loaded:
/// `anim graph` and `foot ik`. Returns the frames the load took. Hosts run
/// it last: the strider's load and unload in the middle of the run left the
/// base N64's heap too fragmented for the perf scenes' 45 KB copies.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn anim_steps(host: &mut dyn GroupHost) -> Result<u32, &'static str> {
    let before = heap_used(host);
    let frames = load_until_ready(host, STRIDER_GROUP)?;
    let loaded = heap_used(host);
    {
        let (renderer, platform) = host.parts();
        platform.log("TRINO_STEP:anim graph\n");
        let graph = anim_graph(renderer, platform)?;
        log_anim_graph(platform, &graph);
        platform.log("TRINO_STEP:foot ik\n");
        let ik = foot_ik(renderer, platform)?;
        log_foot_ik(platform, &ik);
        platform.unload_group(STRIDER_GROUP);
    }
    // Frames for the backends to free what the RSP or the GPU may still
    // read (the N64's grave keeps an unloaded mesh's copies 4 frames).
    for _ in 0..8 {
        loading_frame(host, 100);
    }
    let after = heap_used(host);
    let mut line = TextBuf::<128>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!("TRINO_STEP:anim steps heap before={before} loaded={loaded} after={after}\n"),
    );
    host.parts().1.log(line.as_str());
    Ok(frames)
}

/// The `asset groups` step's log line.
pub fn log_groups(platform: &mut dyn Platform, r: &GroupReport) {
    let mut line = TextBuf::<160>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:asset groups ok load_frames={} level_bytes={} heap_baseline={} \
             heap_drift={} cycles={} soak_frames={}\n",
            r.load_frames, r.level_bytes, r.heap_baseline, r.heap_drift, r.cycles, r.soak_frames
        ),
    );
    platform.log(line.as_str());
}

/// The stripe pattern `sprites/selftest_quadrants` shows under a
/// color-vision mode (`cvd_icon` in the manifest, and on `ui/selftest`'s
/// first button).
pub const CVD_STRIPES: SpriteId = SpriteId::from_path("sprites/cvd_stripes");

/// Audio that only counts what reaches it: the caption wrapper must pass
/// every call through.
struct CountingAudio {
    sounds: u32,
    music: u32,
}

impl trino_core::Audio for CountingAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {
        self.sounds += 1;
    }
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {
        self.music += 1;
    }
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Accessibility (issue #60): the platform keeps the player's settings and
/// the 3-byte save form round-trips; the baked `access/data` holds the
/// semantic colors, the captions and the icons; a slotted card drawn with
/// the deuteranopia palette shows the alternatives (the same draw as the
/// base palette: no shader); the caption queue fills through the audio
/// wrapper and draws a dark box with text over the scene (on the touch
/// screen with `SecondScreen`, where there is one); the UI at 200 % text
/// keeps its texts in their boxes, the high-contrast theme fills the
/// focused button with its yellow and a color-vision mode adds the
/// button's icon; a high-contrast outline rings a card in its color; the
/// flash guard keeps a 10 Hz strobe to 3 flashes a second, reduced motion
/// damps a shake, hold-to-toggle latches. `Ok(false)`
/// after the non-pixel checks where `read_pixel` is unsupported.
#[inline(never)]
pub fn accessibility(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::access::{CaptionMode, MAX_FLASHES_PER_SECOND};
    use trino_core::ui::{NoStrings, Ui, UiEnv, UiScreen};
    use trino_core::{
        AccessBlob, AccessSettings, Audio, CameraFx, CaptionPlace, CaptionStyle, Captions, CvdMode,
        FlashMonitor, HoldToggle, ScreenFlash,
    };

    // 1. Settings: the save form, the platform round trip.
    let want = AccessSettings {
        cvd: CvdMode::Deuteranopia,
        text_scale: 200,
        high_contrast: true,
        captions: CaptionMode::All,
        reduce_motion: true,
        flash_guard: true,
        hold_to_toggle: true,
        cvd_filter: true,
    };
    if AccessSettings::unpack(want.pack()) != Some(want) {
        return Err("accessibility: the packed settings do not round-trip");
    }
    let saved = platform.access();
    platform.set_access(want);
    let kept = platform.access();
    platform.set_access(saved);
    if kept != want || platform.access() != saved {
        return Err("accessibility: the platform does not keep its settings");
    }

    // 2. The baked data.
    let mut blob = AccessBlob::new();
    let data = *blob
        .get(platform)
        .ok_or("accessibility: blob access/data missing or broken")?;
    let rgb = |r, g, b| Color::rgb(r, g, b);
    let (a_base, b_base) = (rgb(220, 40, 40), rgb(40, 160, 60));
    let (a_deut, b_deut) = (rgb(240, 160, 0), rgb(40, 90, 220));
    if data.color("selftest.team_a", CvdMode::Off) != Some(a_base)
        || data.color("selftest.team_b", CvdMode::Deuteranopia) != Some(b_deut)
        || data.color("selftest.team_a", CvdMode::Tritanopia) != Some(a_base)
    {
        return Err("accessibility: a semantic color has the wrong value");
    }
    if data.icon(QUADRANTS) != Some(CVD_STRIPES) {
        return Err("accessibility: the quadrants' color-vision icon is missing");
    }
    let coin = trino_core::SoundId::from_path("sounds/coin");
    let win = trino_core::SoundId::from_path("sounds/win");
    match (data.caption(coin.0), data.caption(win.0)) {
        (Some(c), Some(w)) if c.key == "cap.coin" && !c.important && w.important => {}
        _ => return Err("accessibility: the sounds' captions are wrong"),
    }

    // 3. Motion, flashes, held input (no renderer).
    let mut flash = ScreenFlash::new();
    let guarded = AccessSettings::DEFAULT;
    let open = AccessSettings {
        flash_guard: false,
        ..guarded
    };
    let mut shown = 0;
    for _ in 0..10 {
        shown += flash.trigger(Color::WHITE, 0.05, &guarded) as usize;
        flash.update(0.1);
    }
    if shown != MAX_FLASHES_PER_SECOND || flash.dropped != 7 {
        return Err("accessibility: the flash guard let a 10 Hz strobe through");
    }
    let mut free = ScreenFlash::new();
    let free_shown = (0..10)
        .map(|_| {
            let s = free.trigger(Color::WHITE, 0.05, &open);
            free.update(0.1);
            s as usize
        })
        .sum::<usize>();
    if free_shown != 10 || flash.current().a > trino_core::access::motion::GUARDED_ALPHA {
        return Err("accessibility: flashes without the guard were dropped");
    }
    let mut monitor = FlashMonitor::new();
    let strobe = (0..60).any(|f| monitor.frame(f as f32 / 60.0, ((f / 3) % 2) as f32));
    let mut calm = FlashMonitor::new();
    let slow = (0..120).any(|f| calm.frame(f as f32 / 60.0, ((f / 30) % 2) as f32));
    if !strobe || slow {
        return Err("accessibility: the strobe monitor is wrong");
    }
    let mut fx = CameraFx::new();
    fx.shake(1.0, 1.0);
    fx.kick_fov(0.3, 1.0);
    fx.update(0.05);
    let full = fx.offset(&guarded);
    let damped = fx.offset(&want);
    let (full2, damped2) = (full.dot(full), damped.dot(damped));
    // Squared lengths: the damped shake is at most a fifth of the full one.
    if damped2 > full2 * 0.21 * 0.21 + 1e-8
        || full2 <= 0.0
        || fx.fov_delta(&want) != 0.0
        || fx.fov_delta(&guarded) <= 0.0
    {
        return Err("accessibility: reduced motion does not damp the camera");
    }
    let mut press = InputState::default();
    press.set(trino_core::Button::B, true);
    let idle = InputState::default();
    let (mut hold, mut toggle) = (HoldToggle::new(), HoldToggle::new());
    hold.update(&press, &idle, trino_core::Button::B, &guarded);
    toggle.update(&press, &idle, trino_core::Button::B, &want);
    let held = hold.update(&idle, &press, trino_core::Button::B, &guarded);
    let latched = toggle.update(&idle, &press, trino_core::Button::B, &want);
    if held || !latched {
        return Err("accessibility: hold-to-toggle does not latch");
    }

    // 4. Captions through the audio wrapper.
    let mut captions = Captions::new();
    captions.configure(Some(&data), &want);
    let mut sink = CountingAudio {
        sounds: 0,
        music: 0,
    };
    {
        let mut audio = captions.wrap(&mut sink);
        audio.play_sound(coin);
        audio.play_sound(trino_core::SoundId::from_path("sounds/jump"));
        audio.play_music(trino_core::MusicId::from_path("music/theme"), true);
    }
    if sink.sounds != 2 || sink.music != 1 || captions.len() != 1 {
        return Err("accessibility: the caption wrapper lost a sound or a caption");
    }
    captions.configure(
        Some(&data),
        &AccessSettings {
            captions: CaptionMode::Important,
            ..want
        },
    );
    if captions.on_sound(coin.0, None) || !captions.on_sound(win.0, None) {
        return Err("accessibility: the important mode shows the wrong sounds");
    }

    // 5. The UI at 200 % text, in high contrast, with the icons.
    let bytes = platform
        .blob(UI_SCREEN)
        .ok_or("accessibility: blob ui/selftest missing")?;
    let screen = UiScreen::parse(bytes).map_err(|_| "accessibility: ui/selftest does not parse")?;
    let mut ui: Ui<16> = Ui::new();
    let draw_ui = |r: &mut dyn Renderer, env: &UiEnv<'_>, ui: &mut Ui<16>| {
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            ui.draw(r, &screen, env);
            r.end_frame();
        }
        ui.stats()
    };
    let env = UiEnv::of(renderer);
    let plain = draw_ui(renderer, &env, &mut ui);
    let big = draw_ui(
        renderer,
        &env.with_access(&AccessSettings {
            text_scale: 200,
            ..guarded
        }),
        &mut ui,
    );
    if big.overflow != 0 || plain.overflow != 0 {
        return Err("accessibility: a text overflows its button at 200 %");
    }
    let icons = draw_ui(
        renderer,
        &env.with_access(&AccessSettings {
            cvd: CvdMode::Deuteranopia,
            ..guarded
        }),
        &mut ui,
    );
    if icons.draws != plain.draws + 1 {
        return Err("accessibility: the color-vision icon is not drawn");
    }
    let contrast_env = env.with_access(&AccessSettings {
        high_contrast: true,
        ..guarded
    });
    // The first button takes the focus on the first update.
    let _ = ui.update(&screen, &idle, &idle, 0.0);
    draw_ui(renderer, &contrast_env, &mut ui);
    let (_, one) = ui
        .rect_of(&screen, "one")
        .ok_or("accessibility: no `one`")?;
    let probe = (one.pos.x + 4.0, one.pos.y + 3.0);
    let Some(focus) = renderer.read_pixel(probe.0 as u32, probe.1 as u32) else {
        return Ok(false);
    };
    let near = |c: Color, w: Color, t: i32| {
        (c.r as i32 - w.r as i32).abs() <= t
            && (c.g as i32 - w.g as i32).abs() <= t
            && (c.b as i32 - w.b as i32).abs() <= t
    };
    if !near(focus, trino_core::ui::Theme::HIGH_CONTRAST.focus, 40) {
        return Err("accessibility: the high-contrast focus is not its yellow");
    }

    // 6. The palette swap on a slotted card: the same draw, the colors of
    //    the mode.
    let caps = *renderer.caps();
    let size = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * size.y * 0.5;
    let names = ["selftest.team_a", "", "", "selftest.team_b"];
    for (mode, want_cols) in [
        (CvdMode::Off, [Color::WHITE, a_base, b_base]),
        (CvdMode::Deuteranopia, [Color::WHITE, a_deut, b_deut]),
    ] {
        let params = ModelParams {
            shading: Shading::Unlit,
            palette: Some(data.palette(names, mode)),
            ..ModelParams::DEFAULT
        };
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.draw_model(
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &params,
            );
            renderer.end_frame();
        }
        for (x, w) in [-0.4f32, 0.0, 0.4].into_iter().zip(want_cols) {
            let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
            let px = size.x * 0.5 + v.x * focal / v.z;
            let py = size.y * 0.5 - v.y * focal / v.z;
            let got = renderer
                .read_pixel(px as u32, py as u32)
                .ok_or("accessibility: read-back stopped")?;
            if !near(got, w, 48) {
                return Err(if mode == CvdMode::Off {
                    "accessibility: the base palette shows the wrong colors"
                } else {
                    "accessibility: the deuteranopia palette shows the wrong colors"
                });
            }
        }
    }

    // 7. A high-contrast outline: the white card ringed in yellow (a
    //    second, unlit draw pushed behind it), black there without one.
    let ring = trino_core::Outline::new(trino_core::access::OUTLINE_COLOR, Vec3::ZERO, 0.68)
        .with_pixels(6.0);
    let white = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let edge = view.transform_point(Vec3::new(0.6, 0.0, 0.0));
    let edge_x = size.x * 0.5 + edge.x * focal / edge.z;
    for with in [true, false] {
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            trino_core::access::draw_outlined(
                renderer,
                &camera,
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &white,
                with.then_some(&ring),
            );
            renderer.end_frame();
        }
        let out = renderer
            .read_pixel((edge_x + 2.0) as u32, (size.y * 0.5) as u32)
            .ok_or("accessibility: read-back stopped")?;
        let mid = renderer
            .read_pixel((size.x * 0.5) as u32, (size.y * 0.5) as u32)
            .ok_or("accessibility: read-back stopped")?;
        let want_out = if with {
            trino_core::access::OUTLINE_COLOR
        } else {
            Color::BLACK
        };
        if !near(out, want_out, 48) || !near(mid, Color::WHITE, 48) {
            return Err(if with {
                "accessibility: the outline does not ring the card"
            } else {
                "accessibility: without an outline the card is not alone"
            });
        }
    }

    //    At a gameplay distance (a boar-sized card 15 m away), where the
    //    N64's 16-bit z has about 7 cm a step against the copy's 18 cm gap:
    //    the card stays white, the ring still shows.
    let far = Camera3 {
        eye: camera.eye * 10.0,
        ..camera
    };
    let small = Transform3 {
        scale: Vec3::new(0.9, 0.9, 0.9),
        ..Transform3::default()
    };
    let ring = trino_core::Outline::new(trino_core::access::OUTLINE_COLOR, Vec3::ZERO, 0.6)
        .with_pixels(4.0);
    let far_view = far.view();
    let edge = far_view.transform_point(Vec3::new(0.54, 0.0, 0.0));
    let edge_x = size.x * 0.5 + edge.x * focal / edge.z;
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&far);
        renderer.set_light(&DEFAULT_LIGHT);
        trino_core::access::draw_outlined(
            renderer,
            &far,
            SLOTS_CARD,
            &small,
            Material::VertexLit,
            &white,
            Some(&ring),
        );
        renderer.end_frame();
    }
    let out = renderer
        .read_pixel((edge_x + 2.0) as u32, (size.y * 0.5) as u32)
        .ok_or("accessibility: read-back stopped")?;
    let mid = renderer
        .read_pixel((size.x * 0.5) as u32, (size.y * 0.5) as u32)
        .ok_or("accessibility: read-back stopped")?;
    if !near(mid, Color::WHITE, 48) {
        return Err("accessibility: a far outline shows through its object");
    }
    if !near(out, trino_core::access::OUTLINE_COLOR, 48) {
        return Err("accessibility: a far outline does not ring its object");
    }

    // 8. The caption box: dark under the text, the scene around it, a
    //    glyph inside; then on the touch screen where there is one.
    let mut captions = Captions::new();
    captions.configure(Some(&data), &want);
    captions.on_sound(coin.0, None);
    let gray = Color::rgb(200, 200, 200);
    for place in [CaptionPlace::Bottom, CaptionPlace::SecondScreen] {
        let style = CaptionStyle {
            font: FONT,
            place,
            ..CaptionStyle::DEFAULT
        };
        let on = match place {
            CaptionPlace::SecondScreen => match renderer.screen_size(Screen::Bottom) {
                Some(_) => Screen::Bottom,
                None => continue,
            },
            _ => Screen::Top,
        };
        for _ in 0..3 {
            renderer.begin_frame(gray);
            if on == Screen::Bottom && renderer.on_screen(Screen::Bottom) {
                // The touch screen's own background, gray like the top's.
                renderer.fill_rect(Rect::new(0.0, 0.0, 320.0, 240.0), gray);
            }
            captions.draw(renderer, &NoStrings, &style);
            renderer.end_frame();
        }
        let screen_px = renderer
            .screen_size(on)
            .ok_or("accessibility: no caption screen")?;
        let boxes = captions.boxes(
            renderer,
            &NoStrings,
            &style,
            screen_px,
            CaptionPlace::Bottom,
        );
        let r = boxes[0].ok_or("accessibility: no caption box")?;
        let read = |r: &mut dyn Renderer, x: f32, y: f32| {
            r.read_screen_pixel(on, x as u32, y as u32)
                .ok_or("accessibility: read-back stopped")
        };
        // The box's left padding, half-way down (clear of the text and of
        // the high-contrast top and bottom rules).
        let inside = read(renderer, r.pos.x + 1.0, r.pos.y + r.size.y * 0.5)?;
        let outside = read(renderer, r.pos.x + 1.0, r.pos.y - 3.0)?;
        if inside.r > 90 || outside.r < 150 {
            return Err("accessibility: the caption box is not where it should be");
        }
        let px = captions.text_px(screen_px.y);
        let y = r.pos.y + 2.0 * px + 3.0 * px;
        let mut glyph = false;
        let mut x = r.pos.x + 2.0 * px;
        while x < r.pos.x + r.size.x - 2.0 * px && !glyph {
            glyph = read(renderer, x, y)?.r > 150;
            x += 1.0;
        }
        if !glyph {
            return Err("accessibility: the caption box has no text");
        }
    }
    Ok(true)
}

/// Rows of [`UI_COST_SCREEN`]'s list.
pub const UI_COST_ROWS: u16 = 41;

/// A screen of 126 laid-out nodes (a 41-row list of a label and a number
/// in a scroll box, issue #95): what the N64 pays to lay out and draw a
/// menu at the UI budget (`TRINO_UI_COST:` in the ares self-test).
pub static UI_COST_SCREEN: trino_core::ui::UiScreen<'static> = {
    use trino_core::ui::{
        Align, Dir, Justify, NONE, NodeKind, Nodes, Scaling, Size, Theme, UiNode, UiScreen,
    };
    static NODES: [UiNode<'static>; 6] = [
        UiNode {
            kind: NodeKind::Overlay,
            end: 6,
            size: (Size::Fill, Size::Fill),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Scroll {
                dir: Dir::Vertical,
                gap: 0,
            },
            parent: 0,
            end: 6,
            size: (Size::Fill, Size::Fill),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::List {
                dir: Dir::Vertical,
                gap: 1,
                max: 64,
            },
            parent: 1,
            end: 6,
            bind: "rows",
            size: (Size::Fill, Size::Fit),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Stack {
                dir: Dir::Horizontal,
                gap: 4,
                align: Align::Center,
                justify: Justify::SpaceBetween,
                wrap: false,
            },
            parent: 2,
            end: 6,
            size: (Size::Fill, Size::Fit),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Text,
            parent: 3,
            end: 5,
            text: "{}",
            bind: "name",
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Text,
            parent: 3,
            end: 6,
            text: "{}",
            bind: "n",
            ..UiNode::DEFAULT
        },
    ];
    UiScreen {
        name: "ui_cost",
        reference: (320, 240),
        scaling: Scaling::Integer,
        wrap: false,
        focus: NONE,
        on_back: "",
        theme: Theme::DEFAULT,
        nodes: Nodes::Slice(&NODES),
    }
};

/// [`UI_COST_SCREEN`]'s values, from the game on demand ([`UiData`]): a
/// new revision lays the screen out again.
///
/// [`UiData`]: trino_core::ui::UiData
pub struct UiCostData {
    pub revision: u32,
}

impl trino_core::ui::UiData for UiCostData {
    fn value(&self, key: &str, item: u16) -> Option<trino_core::ui::UiValue> {
        use trino_core::ui::{UiValue, text_value};
        match key {
            "rows" => Some(UiValue::Int(UI_COST_ROWS as i32)),
            "name" => Some(text_value("PLAYER NAME")),
            "n" => Some(UiValue::Int(item as i32 * 7 + self.revision as i32 % 3)),
            _ => None,
        }
    }

    fn revision(&self) -> Option<u32> {
        Some(self.revision)
    }
}

#[cfg(test)]
mod ui_cost_tests {
    use super::*;

    /// The N64's cost screen sits at the 128-node UI budget and lays out
    /// again on each new revision only.
    #[test]
    fn the_ui_cost_screen_lays_out_126_nodes() {
        use trino_core::ui::{CellMeasure, UiEnv};
        let mut ui = trino_core::Ui::<128>::new();
        let data = UiCostData { revision: 1 };
        let env = UiEnv::of_caps(&trino_core::Caps::N64).with_data(&data);
        ui.layout(&UI_COST_SCREEN, &env, &CellMeasure);
        assert_eq!(ui.stats().nodes, 3 + 3 * UI_COST_ROWS);
        assert!(ui.stats().nodes <= trino_core::Caps::N64.max_ui_nodes);
        assert_eq!(ui.stats().dropped, 0);
    }
}

#[cfg(test)]
mod ui_cost_bench {
    use super::*;
    #[test]
    #[ignore]
    fn bench_layout() {
        use trino_core::ui::{CellMeasure, UiEnv};
        let mut ui = trino_core::Ui::<128>::new();
        let n = 2000;
        let t = std::time::Instant::now();
        for k in 0..n {
            let data = UiCostData { revision: k };
            let env = UiEnv::of_caps(&trino_core::Caps::N64).with_data(&data);
            ui.layout(&UI_COST_SCREEN, &env, &CellMeasure);
        }
        println!(
            "layout {:.1} us",
            t.elapsed().as_secs_f64() * 1e6 / n as f64
        );
    }
}

// ---------------------------------------------------------------- physics --

/// The `physics` step's course hashes on the PC (issue #66): every target
/// must reproduce them bit for bit — the f32 controller (no FMA, the core's
/// own sqrt) and the fixed-point one.
pub const PHYSICS_HASH: u32 = 0xba58_1be1;
pub const PHYSICS_FX_HASH: u32 = 0xffa3_22c7;
/// Solids in the step's venue (the issue's 500-solid scene).
pub const VENUE_SOLIDS: usize = 500;
/// Rays and controller steps the venue times, scanned and gridded.
pub const VENUE_RAYS: usize = 64;
pub const VENUE_STEPS: usize = 16;

/// What the `physics` step measured.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhysicsProbe {
    pub hash: u32,
    pub fx_hash: u32,
    /// The venue: [`VENUE_RAYS`] rays, then [`VENUE_STEPS`] controller
    /// steps, through a linear scan and through the grid (µs).
    pub rays_scan_us: u32,
    pub rays_grid_us: u32,
    pub steps_scan_us: u32,
    pub steps_grid_us: u32,
    /// castle64's hub frame (its camera ray and player step), the same.
    pub hub_scan_us: u32,
    pub hub_grid_us: u32,
    pub hub_solids: u32,
    /// The `collision/rock` mesh's triangles and where a body dropped on
    /// it stood (millimetres; 0 without the blob).
    pub mesh_tris: u32,
    pub mesh_feet_mm: i32,
}

fn fnv(h: &mut u32, words: &[u32]) {
    for w in words {
        for b in w.to_le_bytes() {
            *h = (*h ^ b as u32).wrapping_mul(0x0100_0193);
        }
    }
}

/// The course every target drives (no allocation): a 30° ramp to a deck,
/// a 0.25 m ledge climbed as a step, a one-way deck jumped up through, a
/// 50° ramp slid off, and a lift carrying the body. Returns the hash of
/// every position and which of those it did (bits: ramp, step, one-way,
/// slide, lift).
#[inline(never)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn physics_course<S: trino_core::physics::Scalar>() -> (u32, u8) {
    use trino_core::physics::{Box3, CharacterController, ControllerConfig, RampDir, Surface, V3};
    let v = |x: f32, y: f32, z: f32| V3::new(S::from_f32(x), S::from_f32(y), S::from_f32(z));
    let b = |a: V3<S>, c: V3<S>| Box3::new(a, c);
    let fixed = [
        Surface::solid(b(v(-20.0, -1.0, -20.0), v(20.0, 0.0, 20.0))),
        Surface::ramp(b(v(2.0, 0.0, -2.0), v(6.0, 1.5, 2.0)), RampDir::PosX),
        Surface::solid(b(v(6.0, 0.0, -2.0), v(10.0, 1.5, 2.0))),
        Surface::solid(b(v(10.0, 1.5, -2.0), v(12.0, 1.75, 2.0))),
        Surface::one_way(b(v(12.0, 3.0, -2.0), v(16.0, 3.2, 2.0))),
        Surface::ramp(b(v(-6.0, 0.0, 4.0), v(-3.0, 3.6, 8.0)), RampDir::NegX),
    ];
    let cfg = ControllerConfig {
        step_height: S::from_f32(0.3),
        snap_down: S::from_f32(0.1),
        ..ControllerConfig::new(v(0.3, 0.55, 0.3), S::from_int(25), S::from_int(20))
            .with_max_slope_deg(45.0)
    };
    let dt = S::from_f32(1.0 / 60.0);
    let lift = |f: i32| {
        // A lift at x -10, rising 0.01 per frame for 100 frames.
        let y = S::from_f32(0.5) + S::from_int(f.min(100)) * S::from_f32(0.01);
        let up = V3::new(S::ZERO, y, S::ZERO);
        b(v(-11.0, 0.0, -1.0) + up, v(-9.0, 0.2, 1.0) + up)
    };
    let mut world = [Surface::solid(b(V3::ZERO, V3::ZERO)); 7];
    world[..6].copy_from_slice(&fixed);
    let mut h = 0x811c_9dc5u32;
    let mut did = 0u8;
    // Five short runs: (start, walk velocity x z, jump frame).
    type Run = ((f32, f32, f32), (f32, f32), i32);
    let runs: [Run; 5] = [
        ((0.0, 0.56, 0.0), (3.0, 0.0), -1),
        ((8.0, 2.06, 0.0), (3.0, 0.0), -1),
        ((11.0, 2.31, 0.0), (2.0, 0.0), 5),
        ((-4.5, 6.0, 6.0), (0.0, 0.0), -1),
        ((-10.0, 1.26, 0.0), (0.0, 0.0), -1),
    ];
    for (k, (at, (vx, vz), jump)) in runs.into_iter().enumerate() {
        let mut c = CharacterController::new(cfg, v(at.0, at.1, at.2));
        for f in 0..120i32 {
            c.vel.x = S::from_f32(vx);
            c.vel.z = S::from_f32(vz);
            c.jump(f == jump, S::from_int(9));
            world[6] = Surface::solid(lift(f));
            if c.standing_on == Some(6) && (1..=100).contains(&f) {
                c.carry(V3::new(S::ZERO, S::from_f32(0.01), S::ZERO));
            }
            let rep = c.step(dt, &world, &[]);
            did |= (rep.standing_on == Some(1) && rep.on_ground) as u8
                | (rep.stepped as u8) << 1
                | ((rep.standing_on == Some(4)) as u8) << 2
                | (rep.sliding as u8) << 3
                | ((rep.standing_on == Some(6) && f > 50) as u8) << 4;
            let p = c.pos;
            let bits = |x: S| (x.to_f32()).to_bits();
            fnv(
                &mut h,
                &[
                    k as u32,
                    bits(p.x),
                    bits(p.y),
                    bits(p.z),
                    rep.on_ground as u32 | (rep.sliding as u32) << 1 | (rep.stepped as u32) << 2,
                ],
            );
        }
    }
    (h, did)
}

/// A deterministic 500-solid venue (400 × 400 m) on four layers.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn venue(out: &mut [trino_core::physics::Solid; VENUE_SOLIDS]) {
    use trino_core::Aabb;
    let mut r = 0x1234_5678u32;
    let mut next = || {
        r ^= r << 13;
        r ^= r >> 17;
        r ^= r << 5;
        (r >> 8) as f32 / (1u32 << 24) as f32
    };
    for (i, s) in out.iter_mut().enumerate() {
        let c = Vec3::new(next() * 400.0 - 200.0, next() * 8.0, next() * 400.0 - 200.0);
        let h = Vec3::new(0.5 + next() * 5.5, 0.5 + next() * 3.5, 0.5 + next() * 5.5);
        *s = trino_core::physics::Solid::new(Aabb::from_center_half(c, h), 1 << (i % 4));
    }
    // A floor under it all, last (so it is not the first hit of a scan).
    out[VENUE_SOLIDS - 1] = trino_core::physics::Solid::new(
        Aabb::new(
            Vec3::new(-210.0, -1.0, -210.0),
            Vec3::new(210.0, 0.0, 210.0),
        ),
        1,
    );
}

/// The physics step (issue #66): the controller course on f32 and on fixed
/// point (hashes pinned to the PC's), the grid answering exactly what a
/// scan answers on a 500-solid venue (rays and overlaps), the cost of
/// rays and controller steps there and of castle64's own hub frame, both
/// ways, triggers, and a body standing on the `collision/rock` mesh.
#[inline(never)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn physics(platform: &mut dyn Platform) -> Result<PhysicsProbe, &'static str> {
    use trino_core::physics::{
        self, CharacterController, LAYER_ALL, MeshCollider, MeshInstance, Solid, StaticWorld,
        TriggerEvent, TriggerPhase, TriggerTracker, V3, World,
    };
    use trino_core::{Aabb, Mat34};
    let ((hash, did), (fx_hash, fx_did)) =
        (physics_course::<f32>(), physics_course::<physics::Fx>());
    let mut p = PhysicsProbe {
        hash,
        fx_hash,
        ..PhysicsProbe::default()
    };
    for (d, what) in [(did, "f32"), (fx_did, "fixed-point")] {
        let _ = what;
        if d & 1 == 0 {
            return Err("physics: the course never stood on the 30° ramp");
        }
        if d & 2 == 0 {
            return Err("physics: the course never climbed the step");
        }
        if d & 4 == 0 {
            return Err("physics: the course never landed on the one-way deck");
        }
        if d & 8 == 0 {
            return Err("physics: the course never slid on the 50° ramp");
        }
        if d & 16 == 0 {
            return Err("physics: the course never rode the lift");
        }
    }
    // Say what this target computed before judging it (a mismatch names
    // the hashes to compare with the PC's).
    let mut line = TextBuf::<96>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!("TRINO_STEP:physics course hash={hash:08x} fx={fx_hash:08x}\n"),
    );
    platform.log(line.as_str());
    if p.hash != PHYSICS_HASH {
        return Err("physics: the f32 course moved differently from the PC (PHYSICS_HASH)");
    }
    if p.fx_hash != PHYSICS_FX_HASH {
        return Err("physics: the fixed-point course differs from the PC (PHYSICS_FX_HASH)");
    }

    // The venue: grid == scan, then the cost of each.
    let mut solids = [Solid::new(Aabb::EMPTY, 0); VENUE_SOLIDS];
    venue(&mut solids);
    const CELL: f32 = 24.0;
    let mut starts = [0u16; 20 * 20];
    let mut refs = [0u16; 1600];
    let need = StaticWorld::size(&solids, CELL).ok_or("physics: venue grid size")?;
    if need.starts > starts.len() || need.refs > refs.len() {
        return Err("physics: venue grid buffers too small");
    }
    let grid = StaticWorld::build(&solids, CELL, &mut starts, &mut refs)
        .ok_or("physics: venue grid did not build")?;
    let ray = |i: usize| {
        let a = i as f32 * 0.37;
        let o = Vec3::new(sin(a) * 180.0, 2.0 + (i % 5) as f32, cos(a * 1.3) * 180.0);
        let d = Vec3::new(cos(a * 2.1), -0.05, sin(a * 1.7)).normalized();
        (o, d)
    };
    for i in 0..VENUE_RAYS {
        let (o, d) = ray(i);
        let mask = [LAYER_ALL, 1, 6][i % 3];
        if grid.raycast(o, d, 120.0, mask) != physics::raycast_solids(o, d, 120.0, mask, &solids) {
            return Err("physics: a grid ray differs from the scan");
        }
        let q = Aabb::from_center_half(o, Vec3::new(8.0, 3.0, 8.0));
        let (mut a, mut b) = ([0u16; 16], [0u16; 16]);
        let (na, nb) = (
            grid.overlap_aabb(&q, mask, &mut a),
            physics::overlap_solids(&q, mask, &solids, &mut b),
        );
        if na != nb || a[..na.min(16)] != b[..nb.min(16)] {
            return Err("physics: a grid overlap differs from the scan");
        }
    }
    let clock = |pl: &dyn Platform| pl.now_micros() as u32;
    let t0 = clock(platform);
    let mut sink = 0.0f32;
    for i in 0..VENUE_RAYS {
        let (o, d) = ray(i);
        sink += physics::raycast_solids(o, d, 120.0, LAYER_ALL, &solids).map_or(0.0, |h| h.t);
    }
    let t1 = clock(platform);
    for i in 0..VENUE_RAYS {
        let (o, d) = ray(i);
        sink += grid.raycast(o, d, 120.0, LAYER_ALL).map_or(0.0, |h| h.t);
    }
    let t2 = clock(platform);
    p.rays_scan_us = t1.wrapping_sub(t0);
    p.rays_grid_us = t2.wrapping_sub(t1);
    // Controller steps: the venue as a scanned list (dynamics) or gridded.
    let body = physics::ControllerConfig::new(V3::new(0.3, 0.55, 0.3), 25.0, 20.0);
    let walk = |world: &World, pl: &dyn Platform| {
        let mut c = CharacterController::new(body, V3::new(3.0, 0.56, 3.0));
        let t = clock(pl);
        for k in 0..VENUE_STEPS {
            c.vel.x = 4.0;
            c.vel.z = if k % 2 == 0 { 2.0 } else { -2.0 };
            c.step_world(1.0 / 60.0, world, &[]);
        }
        (clock(pl).wrapping_sub(t), c.pos)
    };
    let empty: [Solid; 1] = [Solid::new(
        Aabb::new(Vec3::new(1e4, 1e4, 1e4), Vec3::new(1e4, 1e4, 1e4)),
        0,
    )];
    let (mut es, mut er) = ([0u16; 4], [0u16; 4]);
    let none = StaticWorld::build(&empty, 64.0, &mut es, &mut er).ok_or("physics: empty grid")?;
    let (scan_us, scan_pos) = walk(
        &World {
            statics: none,
            dynamics: &solids,
        },
        platform,
    );
    let (grid_us, grid_pos) = walk(
        &World {
            statics: grid,
            dynamics: &[],
        },
        platform,
    );
    if scan_pos != grid_pos {
        return Err("physics: the controller moved differently on the grid");
    }
    p.steps_scan_us = scan_us;
    p.steps_grid_us = grid_us;

    // castle64's hub frame: its ray and its player step, scanned (what the
    // game does) and with its blocks gridded.
    let mut game = crate::Castle64Game::new(Vec2::new(320.0, 240.0));
    game.goto_level(0);
    let mut hub = [Solid::new(Aabb::EMPTY, 0); 64];
    let (n_static, n) = game.frame_solids(&mut hub);
    p.hub_solids = n as u32;
    let (cfg, pos, target, dir, len) = game.frame_queries();
    let mut boxes = [Aabb::EMPTY; 64];
    for (b, s) in boxes.iter_mut().zip(&hub[..n]) {
        *b = s.aabb;
    }
    let (mut hs, mut hr) = ([0u16; 512], [0u16; 1024]);
    let hub_grid = StaticWorld::build(&hub[..n_static], 8.0, &mut hs, &mut hr)
        .ok_or("physics: hub grid buffers too small")?;
    let reps = 32;
    let t0 = clock(platform);
    for _ in 0..reps {
        sink += physics::raycast_distance(target, dir, len, &boxes[..n]);
        let mut c = CharacterController::new(cfg, pos.into());
        let mut surf = [physics::Surface::solid(physics::Box3::new(V3::ZERO, V3::ZERO)); 64];
        for (o, s) in surf.iter_mut().zip(&hub[..n]) {
            *o = s.into();
        }
        c.step(1.0 / 60.0, &surf[..n], &[]);
        sink += c.pos.y;
    }
    let t1 = clock(platform);
    let hub_world = World {
        statics: hub_grid,
        dynamics: &hub[n_static..n],
    };
    for _ in 0..reps {
        sink += hub_world
            .raycast(target, dir, len, LAYER_ALL)
            .map_or(len, |h| h.t);
        let mut c = CharacterController::new(cfg, pos.into());
        c.step_world(1.0 / 60.0, &hub_world, &[]);
        sink += c.pos.y;
    }
    let t2 = clock(platform);
    p.hub_scan_us = t1.wrapping_sub(t0) / reps;
    p.hub_grid_us = t2.wrapping_sub(t1) / reps;
    if !sink.is_finite() {
        return Err("physics: the timed queries went wrong");
    }

    // Triggers: enter, stay, exit, in order.
    let zones = [Solid::new(
        Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 2.0, 2.0)),
        1,
    )];
    let mut tracker = TriggerTracker::<4>::new();
    let mut ev = [TriggerEvent {
        body: 0,
        trigger: 0,
        phase: TriggerPhase::Stay,
    }; 4];
    let mut phases = [TriggerPhase::Stay; 3];
    for (k, x) in [1.0f32, 1.5, 5.0].iter().enumerate() {
        tracker.begin_frame();
        tracker.track(
            7,
            &Aabb::from_center_half(Vec3::new(*x, 1.0, 1.0), Vec3::new(0.2, 0.2, 0.2)),
            &zones,
            LAYER_ALL,
        );
        if tracker.end_frame(&mut ev) != 1 {
            return Err("physics: a trigger frame reported no event");
        }
        phases[k] = ev[0].phase;
    }
    if phases != [TriggerPhase::Enter, TriggerPhase::Stay, TriggerPhase::Exit] {
        return Err("physics: triggers did not enter, stay and exit");
    }

    // The baked mesh collider (`collision/rock`, every target's bake).
    if let Some(blob) = platform.blob(BlobId::from_path("collision/rock")) {
        let mesh =
            MeshCollider::parse(blob).map_err(|_| "physics: collision/rock does not parse")?;
        p.mesh_tris = mesh.triangle_count() as u32;
        let inst = MeshInstance::new(
            mesh,
            Mat34::from_rotation_scale_translation(
                Vec3::ZERO,
                Vec3::new(5.0, 2.2, 5.0),
                Vec3::ZERO,
            ),
        );
        let top = inst
            .raycast(Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 20.0)
            .ok_or("physics: a ray missed the rock")?;
        let cfg = physics::ControllerConfig::new(V3::new(0.3, 0.55, 0.3), 25.0, 20.0);
        let mut c = CharacterController::new(cfg, V3::new(0.0, 4.0, 0.0));
        for _ in 0..120 {
            c.step(1.0 / 60.0, &[], &[&inst]);
        }
        let feet = c.pos.y - 0.55;
        if !c.on_ground || (feet - (10.0 - top.t)).abs() > 0.05 {
            return Err("physics: a body did not land on the rock's mesh");
        }
        p.mesh_feet_mm = trino_core::math3d::to_i32(feet * 1000.0);
    } else {
        return Err("physics: collision/rock blob missing");
    }
    Ok(p)
}

/// `TRINO_STEP:physics …` and the `TRINO_PHYSICS_COST:` line the hosts'
/// tools read (`cargo xtask test n64` prints it).
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn log_physics(platform: &mut dyn Platform, p: &PhysicsProbe) {
    let mut line = TextBuf::<192>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:physics hash={:08x} fx={:08x} mesh_tris={} feet_mm={}\n",
            p.hash, p.fx_hash, p.mesh_tris, p.mesh_feet_mm
        ),
    );
    platform.log(line.as_str());
    let mut cost = TextBuf::<192>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut cost,
        format_args!(
            "TRINO_PHYSICS_COST: solids={} rays={} rays_scan_us={} rays_grid_us={} steps={} \
             steps_scan_us={} steps_grid_us={} hub_solids={} hub_scan_us={} hub_grid_us={}\n",
            VENUE_SOLIDS,
            VENUE_RAYS,
            p.rays_scan_us,
            p.rays_grid_us,
            VENUE_STEPS,
            p.steps_scan_us,
            p.steps_grid_us,
            p.hub_solids,
            p.hub_scan_us,
            p.hub_grid_us
        ),
    );
    platform.log(cost.as_str());
}

// ---- issue #67: local players and the cross-platform replay ----

/// What the `players` step saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayersReport {
    /// The host's connected mask (the N64 in ares: four ports, `0b1111`).
    pub connected: u8,
    /// Devices the host reported.
    pub devices: u32,
}

/// Local players (issue #67) on this target: the host's own controllers
/// read idle and player 0 is connected where a controller is plugged; then
/// the seat rules on synthetic devices (four ports, a keyboard and pads),
/// so every CPU runs the same fold the PC and the Web use.
pub fn players_checks(input: &mut dyn trino_core::Input) -> Result<PlayersReport, &'static str> {
    use trino_core::{Button, DeviceKind, MAX_PLAYERS, PlayerSetup, Seats};
    let mut last = trino_core::PlayersInput::NONE;
    for _ in 0..3 {
        last = input.poll_players();
        for p in &last.players {
            if p.any_button() || !p.keys.is_empty() || p.triggers != Vec2::ZERO {
                return Err("players: a player reads input with nothing pressed");
            }
        }
        if last.connected >> MAX_PLAYERS != 0 {
            return Err("players: a player past the fourth is connected");
        }
    }
    let press = |b: Button| {
        let mut s = InputState::default();
        s.set(b, true);
        s
    };
    // Four console ports: port k is player k; an empty port reads
    // disconnected.
    let mut seats = Seats::new();
    seats.begin();
    seats.report(DeviceKind::Port, 0, None, press(Button::A));
    seats.report(DeviceKind::Port, 1, None, press(Button::B));
    seats.report(DeviceKind::Port, 3, None, press(Button::Start));
    let p = *seats.finish();
    if p.connected != 0b1011
        || !p.players[0].is_down(Button::A)
        || !p.players[1].is_down(Button::B)
        || p.players[2] != InputState::default()
        || !p.players[3].is_down(Button::Start)
    {
        return Err("players: four ports didn't map to four players");
    }
    // A keyboard and two pads: one player folds them all; two players seat
    // the second pad apart.
    let mut seats = Seats::new();
    let frame = |seats: &mut Seats| {
        seats.begin();
        seats.report(DeviceKind::Keyboard, 0, None, press(Button::A));
        seats.report(DeviceKind::Gamepad, 7, None, press(Button::X));
        seats.report(DeviceKind::Gamepad, 9, None, press(Button::B));
        *seats.finish()
    };
    let one = frame(&mut seats);
    if one.connected != 1
        || !(one.players[0].is_down(Button::A)
            && one.players[0].is_down(Button::X)
            && one.players[0].is_down(Button::B))
    {
        return Err("players: one player didn't fold every device");
    }
    seats.set_setup(PlayerSetup::up_to(2));
    let two = frame(&mut seats);
    if two.connected != 0b11
        || two.players[0].is_down(Button::B)
        || !two.players[1].is_down(Button::B)
    {
        return Err("players: the second pad didn't take player 2");
    }
    Ok(PlayersReport {
        connected: last.connected,
        devices: last.count(),
    })
}

/// `TRINO_STEP:players connected=<mask> players=<n>`.
pub fn log_players(platform: &dyn Platform, r: &PlayersReport) {
    let mut line = TextBuf::<96>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:players connected={:04b} players={}\n",
            r.connected, r.devices
        ),
    );
    platform.log(line.as_str());
}

/// The recording's group and blob (`[groups.replay]`).
pub const REPLAY_GROUP: trino_core::GroupId = trino_core::GroupId::from_name("replay");
pub const REPLAY_BLOB: BlobId = BlobId::from_path("blobs/replay_castle64");

/// What the `replay` step played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    pub frames: u32,
    /// Hashes compared with the recording's (all of them matched).
    pub checked: u32,
    /// The game's hash after the last frame.
    pub hash: u64,
    pub load_frames: u32,
}

/// Audio that plays nothing (the replay checks the sim, not the sound).
struct Silent;

impl trino_core::Audio for Silent {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// The cross-platform determinism gate (issue #67): castle64's bot run as
/// the PC recorded it (`assets/shared/replays/castle64_green.trec`: wall-
/// clock `dt`s, a state hash every frame), played on this target from a
/// reset game through `update_with`, every hash compared. The recording's
/// group loads first and unloads after. `tamper` flips player 1's A on
/// frame 100: the mutation the step must catch (PC tests).
///
/// With `rerecord`, the target records what it fed the game, with its own
/// hashes, into that sink (the consoles: their debug channel through
/// [`trino_core::replay::LineSink`]): `cargo xtask test` reassembles it and
/// holds it byte for byte to the PC's file — the console's recorder and
/// hashes, end to end.
pub fn replay(
    host: &mut dyn GroupHost,
    game: &mut crate::Castle64Game,
    tamper: bool,
    mut rerecord: Option<&mut dyn trino_core::replay::Sink>,
) -> Result<ReplayReport, &'static str> {
    use trino_core::Game as _;
    use trino_core::replay::{Reader, Recorder};
    let load_frames = load_until_ready(host, REPLAY_GROUP)?;
    game.reset();
    let mut cursor = {
        let platform = host.parts().1;
        let bytes = platform
            .group_blob(REPLAY_BLOB)
            .ok_or("replay: the recording's blob is missing")?;
        let r = Reader::new(bytes).map_err(|e| e.message())?;
        if r.header().game != trino_core::asset_id("castle64") {
            return Err("replay: the recording is another game's");
        }
        if game.state_hash() != Some(r.header().start_hash) {
            return Err("replay: the reset game isn't the recording's start state");
        }
        r.cursor()
    };
    let mut recorder = {
        let platform = host.parts().1;
        let bytes = platform
            .group_blob(REPLAY_BLOB)
            .ok_or("replay: the recording's blob is missing")?;
        let header = *Reader::new(bytes).map_err(|e| e.message())?.header();
        rerecord
            .as_deref_mut()
            .map(|sink| Recorder::begin(header, sink))
    };
    let mut report = ReplayReport {
        frames: 0,
        checked: 0,
        hash: 0,
        load_frames,
    };
    loop {
        let platform = host.parts().1;
        let frame = {
            let bytes = platform
                .group_blob(REPLAY_BLOB)
                .ok_or("replay: the recording's blob went away")?;
            let mut r = Reader::resume(bytes, cursor);
            let f = r.next_frame();
            cursor = r.cursor();
            f
        };
        let Some(frame) = frame else { break };
        let mut frame = frame.map_err(|e| e.message())?;
        if tamper && frame.index == 100 {
            let a = frame.players.players[0].is_down(trino_core::Button::A);
            frame.players.players[0].set(trino_core::Button::A, !a);
        }
        if let Some(h) = platform.hosted_players_mut() {
            h.publish_players(&frame.players);
        }
        game.update_with(platform, &frame.players.players[0], &mut Silent, frame.dt);
        report.frames += 1;
        if let (Some(rec), Some(sink)) = (recorder.as_mut(), rerecord.as_deref_mut()) {
            let hash = frame.hash.map(|_| game.state_hash().unwrap_or(0));
            rec.frame(sink, &frame.players, frame.dt, hash);
        }
        if let Some(want) = frame.hash {
            let got = game.state_hash().unwrap_or(0);
            report.checked += 1;
            report.hash = got;
            if got != want {
                let mut line = TextBuf::<160>::new();
                let _ = core::fmt::Write::write_fmt(
                    &mut line,
                    format_args!(
                        "TRINO_REPLAY: diverged at frame {} (expected {want:#018x}, got {got:#018x})\n",
                        report.frames
                    ),
                );
                platform.log(line.as_str());
                platform.unload_group(REPLAY_GROUP);
                host.pump();
                return Err("replay: castle64 diverged from the PC recording");
            }
        }
    }
    // An unload frees on the host's next pump: the base N64's 4 MB heap
    // can't keep 15 KB of recording through the rest of the self-test.
    host.parts().1.unload_group(REPLAY_GROUP);
    host.pump();
    if host.parts().1.group_blob(REPLAY_BLOB).is_some() {
        return Err("replay: the unloaded recording is still in memory");
    }
    if report.checked == 0 {
        return Err("replay: the recording has no hashes");
    }
    // The rest of the self-test starts castle64 fresh.
    game.reset();
    Ok(report)
}

/// `TRINO_STEP:replay ok frames=731 checked=731 hash=0x…`.
pub fn log_replay(platform: &dyn Platform, r: &ReplayReport) {
    let mut line = TextBuf::<160>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:replay ok frames={} checked={} hash={:#018x} load_frames={}\n",
            r.frames, r.checked, r.hash, r.load_frames
        ),
    );
    platform.log(line.as_str());
}
