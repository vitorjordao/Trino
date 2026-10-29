//! Targets over `trino-core`'s parsers (`no_std` code the consoles run).
//!
//! Each body parses the input, then does what a caller does after `Ok`:
//! reads every record, poses and draws a model, lays out a screen, looks a
//! string up. The panics that matter are "validated, then an accessor
//! indexed out of range", and only these second halves reach them.

use trino_core::render3d::{self, Camera3, DEFAULT_LIGHT, Mesh};
use trino_core::{
    AnimRef, Caps, Color, InputState, Mat34, ModelId, ModelParams, Rect, Renderer, SpriteId,
    SpriteParams, Transform3, Vec2, Vec3,
};

/// A renderer that draws nothing and counts what reached it.
#[derive(Default)]
pub struct Null {
    pub calls: u64,
}

impl Renderer for Null {
    fn caps(&self) -> &Caps {
        &Caps::N64
    }
    fn begin_frame(&mut self, _: Color) {}
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {
        self.calls += 1;
    }
    fn draw_model(&mut self, _: ModelId, _: &Transform3, _: trino_core::Material, _: &ModelParams) {
        self.calls += 1;
    }
    fn fill_rect(&mut self, _: Rect, _: Color) {
        self.calls += 1;
    }
    fn draw_billboard(&mut self, _: SpriteId, _: Vec3, _: &trino_core::BillboardParams) {
        self.calls += 1;
    }
    fn draw_text(&mut self, _: trino_core::FontId, _: Vec2, _: &str, _: &trino_core::TextParams) {
        self.calls += 1;
    }
    fn end_frame(&mut self) {}
}

/// Keep a value alive so the optimizer can't drop the work.
#[inline]
fn keep<T>(v: T) {
    core::hint::black_box(v);
}

// ------------------------------------------------------------------ TMDL

/// TMDL and every section the runtime reads (`SKEL`, `ANIM`/`ANMC`, `SKW2`,
/// `SLOT`, `TXUV`, `BNDS`, `IKCH`, `ROOT`, `AEVT`): parse, read every
/// vertex and index, pose each clip, and run one draw through the CPU T&L.
pub fn tmdl(data: &[u8]) {
    let Ok(mesh) = Mesh::from_tmdl(data) else {
        // Section readers that tools call on their own.
        keep(trino_core::anim::ClipMeta::from_tmdl(data));
        return;
    };
    let layout = mesh.layout(data);
    let again = Mesh::from_layout(data, &layout);
    keep(layout.bounds());
    let v = again.vertex_count;
    for i in 0..v.min(4096) {
        keep(mesh.position(i));
        keep(mesh.normal(i));
        keep(mesh.color(i));
        keep(mesh.slot(i));
        keep(mesh.joint_of_vertex(i));
        keep(mesh.blend_of(i));
        if mesh.has_uvs() {
            keep(mesh.uv(i));
        }
    }
    for i in 0..mesh.index_count.min(12288) {
        keep(mesh.index(i));
    }
    keep((
        mesh.aabb(),
        mesh.bounds(),
        mesh.max_edge2(),
        mesh.opaque_colors(),
    ));
    keep(mesh.default_texture());
    keep(mesh.ik_chains());
    keep(trino_core::anim::ClipMeta::from_tmdl(data));
    keep(render3d::rsp_mesh_check(&mesh));
    keep(render3d::rsp_flat_segments(&mesh));
    let mut joints = [Mat34::IDENTITY; trino_core::render3d::MAX_JOINTS];
    keep(mesh.pose(None, &mut joints));
    for c in 0..mesh.clip_count().min(8) {
        let Some(clip) = mesh.clip(c) else { continue };
        for t in [0.0, 0.37, clip.duration(), clip.duration() * 1.5 + 0.1] {
            let n = mesh.pose(Some(AnimRef::new(clip.id, t)), &mut joints);
            keep(mesh.pose_world(Some(AnimRef::new(clip.id, t)), &mut joints));
            for i in 0..v.min(256) {
                keep(mesh.posed_position(i, &joints, n));
                keep(mesh.posed_normal(i, &joints, n));
            }
        }
        if c + 1 < mesh.clip_count()
            && let Some(next) = mesh.clip(c + 1)
        {
            let blend = AnimRef::new(clip.id, 0.2).blend_to(next.id, 0.1, 0.5);
            keep(mesh.pose(Some(blend), &mut joints));
        }
    }
    draw_mesh(&mesh);
}

/// One draw of `mesh` through the CPU T&L, the camera backed off its bounds.
fn draw_mesh(mesh: &Mesh) {
    let (center, radius) = mesh.bounds().unwrap_or((Vec3::ZERO, 1.0));
    let radius = if radius.is_finite() && radius > 0.0 {
        radius.min(1.0e6)
    } else {
        1.0
    };
    let center = if center.x.is_finite() && center.y.is_finite() && center.z.is_finite() {
        center
    } else {
        Vec3::ZERO
    };
    let camera = Camera3 {
        eye: center + Vec3::new(0.0, radius, radius * 3.0),
        target: center,
        ..Camera3::default()
    };
    let mut tris = 0u32;
    let anim = mesh.clip(0).map(|c| AnimRef::new(c.id, 0.25));
    for params in [
        ModelParams::DEFAULT,
        ModelParams {
            anim,
            double_sided: true,
            tint: Color::rgba(255, 255, 255, 128),
            shading: trino_core::Shading::Flat,
            ..ModelParams::DEFAULT
        },
    ] {
        render3d::tessellate(
            mesh,
            &Mat34::IDENTITY,
            &camera,
            &DEFAULT_LIGHT,
            &params,
            Vec2::new(320.0, 240.0),
            &mut |_| tris += 1,
        );
    }
    keep(tris);
}

// ------------------------------------------------------------- scenes etc.

/// TSCN: every node, its LOD chain and tags, the environment, shadows and
/// receivers. `BakedScene` borrows `'static` (console blobs are), so the
/// input is leaked for the call and freed after every view is gone.
pub fn tscn(data: &[u8]) {
    let leaked: &'static mut [u8] = Box::leak(data.to_vec().into_boxed_slice());
    let ptr: *mut [u8] = leaked;
    {
        let bytes: &'static [u8] = unsafe { &*ptr };
        if let Ok(scene) = trino_core::scene::BakedScene::parse(bytes) {
            keep((
                scene.clear(),
                scene.camera(),
                scene.light(),
                scene.shadows(),
            ));
            for r in scene.receivers() {
                keep(r);
            }
            for node in scene.nodes().take(4096) {
                keep(node.position());
                keep(node.world_aabb());
                keep(node.has_tag("player"));
                for t in node.tags.iter() {
                    keep(t);
                }
                for l in node.lod.iter() {
                    keep(l);
                }
                keep(node.lod_from(Vec3::new(1.0, 2.0, 3.0)));
                keep(node.lod_from_biased(Vec3::ZERO, 0.5));
            }
            keep(scene.find("player"));
        }
    }
    // Every borrow of `bytes` ended with the block above.
    drop(unsafe { Box::from_raw(ptr) });
}

/// TPFX: the effect, a system built from it, a burst and a few updates.
pub fn tpfx(data: &[u8]) {
    let Ok(effect) = trino_core::Effect::parse(data) else {
        return;
    };
    let mut out = [0u8; trino_core::particles::TPFX_BYTES];
    effect.write(&mut out);
    let back = trino_core::Effect::parse(&out).expect("a written TPFX parses");
    let mut again = [0u8; trino_core::particles::TPFX_BYTES];
    back.write(&mut again);
    assert_eq!(out, again, "TPFX write → parse → write is stable");
    let mut sys = trino_core::ParticleSystem::<64>::from_effect(&effect, 7);
    sys.scale_limit(0.5);
    sys.burst(80);
    for _ in 0..4 {
        sys.update(1.0 / 30.0);
    }
    sys.draw(&mut Null::default());
    keep(sys.len());
}

/// TIMP: every region, the view picked from a few angles, a draw.
pub fn timp(data: &[u8]) {
    let Ok(imp) = trino_core::Impostor::parse(data) else {
        return;
    };
    keep(imp.atlas_size());
    keep(trino_core::Impostor::model_hash(data));
    keep(trino_core::Impostor::pitch(data));
    for i in 0..=255u8 {
        keep(imp.region(i));
    }
    let mut r = Null::default();
    for eye in [
        Vec3::new(3.0, 1.0, 0.0),
        Vec3::new(-2.0, 0.5, -2.0),
        Vec3::ZERO,
    ] {
        keep(imp.view_for(&Mat34::IDENTITY, eye));
        imp.draw(&mut r, &Mat34::IDENTITY, eye, Color::WHITE);
    }
    let mut out = [0u8; 4096];
    let n = imp.write(1, 15.0, &mut out);
    if n > 0 {
        let back = trino_core::Impostor::parse(&out[..n]).expect("a written TIMP parses");
        keep(back);
    }
}

/// TFNT: glyphs for every byte value and a few non-ASCII chars, measure,
/// layout and draw through the glyph index.
pub fn tfnt(data: &[u8]) {
    use trino_core::text::{self, BitmapFont, GlyphIndex};
    let Ok(font) = BitmapFont::parse(data) else {
        return;
    };
    let index = GlyphIndex::build(&font);
    let font = font.with_index(&index);
    for c in (0u32..256)
        .filter_map(char::from_u32)
        .chain(['ç', 'é', 'ß', '中', '\u{FFFD}'])
    {
        keep(font.glyph(c));
        keep(font.glyph_or_fallback(c));
    }
    let sample = "Olá, mundo! 0123\nçãõ ?";
    keep(font.line_width(sample));
    keep(font.measure(sample, 2.0));
    let params = trino_core::TextParams::default();
    let mut n = 0;
    text::layout(&font, Vec2::ZERO, sample, &params, &mut |_, _| n += 1);
    keep(n);
}

/// TSN2 sound blobs (the runtime's header view; the PC's decode is the
/// `tsn2_decode` host target).
pub fn tsn2(data: &[u8]) {
    if let Ok(blob) = trino_core::SoundBlob::parse(data) {
        let h = blob.header();
        keep(h);
        keep(blob);
    }
}

/// TMIX: the project mix, applied to a bus mixer.
pub fn tmix(data: &[u8]) {
    let Ok(mix) = trino_core::MixerConfig::parse(data) else {
        return;
    };
    let mut out = vec![0u8; mix.encoded_len()];
    let n = mix.encode(&mut out);
    let back = trino_core::MixerConfig::parse(&out[..n]).expect("a written TMIX parses");
    let mut again = vec![0u8; back.encoded_len()];
    let m = back.encode(&mut again);
    assert_eq!(
        out[..n],
        again[..m],
        "TMIX encode → parse → encode is stable"
    );
}

/// The 4-byte bus settings a console save stores.
pub fn bus_settings(data: &[u8]) {
    let mut b = [0u8; 4];
    let n = data.len().min(4);
    b[..n].copy_from_slice(&data[..n]);
    let s = trino_core::BusSettings::from_bytes(b);
    let back = trino_core::BusSettings::from_bytes(s.to_bytes());
    assert_eq!(
        back.to_bytes(),
        s.to_bytes(),
        "bus settings repack is stable"
    );
}

/// TUIL: the screen, laid out headless and driven by a few inputs, then
/// drawn on the N64's caps.
pub fn tuil(data: &[u8]) {
    use trino_core::ui::{Ui, UiEnv, UiScreen};
    let Ok(screen) = UiScreen::parse(data) else {
        return;
    };
    let mut ui = Ui::<64>::new();
    let idle = InputState::default();
    let mut down = InputState::default();
    down.set(trino_core::Button::DpadDown, true);
    let mut a = InputState::default();
    a.set(trino_core::Button::A, true);
    for (now, prev) in [(&idle, &idle), (&down, &idle), (&idle, &down), (&a, &idle)] {
        for e in ui.update(&screen, now, prev, 1.0 / 60.0).iter() {
            keep(e);
        }
    }
    let mut r = Null::default();
    let env = UiEnv::of_caps(&Caps::N64);
    ui.draw(&mut r, &screen, &env);
    keep(ui.stats());
}

/// TSTR: every entry, the locale, font remaps and plural forms, and the
/// lookups through a localizer.
pub fn tstr(data: &[u8]) {
    use trino_core::i18n::{Localizer, StrKey, table::StringTable};
    let Ok(table) = StringTable::parse(data) else {
        return;
    };
    keep((
        table.version(),
        table.lang(),
        table.fallback(),
        table.name(),
    ));
    keep(table.locale());
    keep(table.group());
    keep(table.font(trino_core::FontId(1)));
    for r in table.font_remaps() {
        keep(r);
    }
    for h in table.hashes().take(4096) {
        if let Some(e) = table.entry_hash(h) {
            keep(e.text());
        }
    }
    keep(table.lookup("menu.start"));
    let mut loc = Localizer::<'_, 2>::default();
    if loc.add(table).is_ok() {
        let k = StrKey::new("menu.start");
        keep(loc.tr(k));
        for n in [0, 1, 2, 5, 21, -1, i64::MAX] {
            keep(loc.plural(k, n));
        }
        let mut buf = trino_core::TextBuf::<64>::new();
        keep(loc.tr_fmt(k, &[trino_core::Arg::int("n", 3)], &mut buf));
    }
}

/// TACH plus the save form games keep in their EEPROM/SD save.
pub fn tach(data: &[u8]) {
    use trino_core::services::table::AchievementTable;
    let Ok(t) = AchievementTable::parse(data) else {
        return;
    };
    keep((t.len(), t.stat_count(), t.presence_count()));
    for a in t.iter() {
        keep(a);
        keep(t.index(a.id));
    }
    for s in t.stats() {
        keep(s);
        keep(t.stat_index(s.id));
    }
    for k in t.presence_keys() {
        keep(k);
    }
    let mut st = trino_core::AchievementState::EMPTY;
    for i in 0..t.len() {
        st.set_unlocked(i, i % 2 == 0);
    }
    let mut out = [0u8; 512];
    let n = st.pack_for(&t, &mut out);
    if n > 0 {
        let (back, used) = trino_core::AchievementState::unpack(&out[..n]).expect("pack → unpack");
        assert_eq!(used, n);
        keep(back);
    }
}

/// `AchievementState::unpack`: bytes from a console save.
pub fn achievement_state(data: &[u8]) {
    let Ok((st, used)) = trino_core::AchievementState::unpack(data) else {
        return;
    };
    assert!(used <= data.len());
    keep(st.unlocked_count());
}

/// TACC: palettes per color-vision mode, captions, icons, the theme.
pub fn tacc(data: &[u8]) {
    use trino_core::access::{CvdMode, data::AccessData};
    let Ok(d) = AccessData::parse(data) else {
        return;
    };
    keep((d.color_count(), d.caption_count(), d.icon_count()));
    for mode in CvdMode::ALL {
        keep(d.color("danger", mode));
        for id in 0..d.color_count().min(64) as u32 {
            keep(d.color_id(id, mode));
        }
        keep(d.palette(["a", "b", "c", "d"], mode));
    }
    keep(d.caption(1));
    keep(d.icon(SpriteId(1)));
    keep(d.contrast_theme());
}

/// TANG: the graph, an instance driven through every state.
pub fn tang(data: &[u8]) {
    use trino_core::{AnimGraph, AnimGraphInstance};
    let Ok(g) = AnimGraph::parse(data) else {
        return;
    };
    keep((
        g.model(),
        g.param_count(),
        g.state_count(),
        g.transition_count(),
        g.clip_count(),
    ));
    for i in 0..g.param_count() {
        keep(g.param_info(i));
    }
    for i in 0..g.clip_count() {
        keep(g.clip(i));
    }
    for t in 0..g.transition_count() {
        keep(g.transition(t));
    }
    let mut inst = AnimGraphInstance::new(&g);
    for s in 0..g.state_count().min(32) {
        let st = g.state(s);
        for p in 0..4 {
            keep(g.point(&st, p));
        }
        for p in 0..g.param_count() {
            inst.set_float(p, 0.7);
            inst.set_bool(p, true);
            inst.set_trigger(p);
        }
        inst.play(s, 0.1);
        for _ in 0..3 {
            inst.update(&g, 1.0 / 30.0);
        }
        let mut c = [trino_core::anim::graph::Contribution {
            clip: 0,
            time: 0.0,
            weight: 0.0,
        }; 4];
        keep(inst.contributions(&g, &mut c));
        keep(inst.feet(&g));
        keep(inst.anim(&g));
        keep(inst.root_motion());
        keep(inst.digest());
    }
}

/// TLUT: the grade, applied to a few colors.
pub fn tlut(data: &[u8]) {
    let Ok(lut) = trino_core::post::Lut::parse(data) else {
        return;
    };
    let s = lut.size() as u32;
    keep(lut.texels());
    keep(lut.texel(0, 0, 0));
    keep(lut.texel(s.saturating_sub(1), s / 2, s.saturating_sub(1)));
    for c in [[0.0, 0.0, 0.0], [1.0, 0.5, 0.25], [2.0, -1.0, f32::NAN]] {
        keep(lut.apply(c));
    }
    keep(lut.apply_color(Color::rgba(10, 200, 90, 255)));
}

/// TCOL: the collider, queries through its BVH.
pub fn tcol(data: &[u8]) {
    use trino_core::physics::Aabb;
    use trino_core::physics::{MeshCollider, MeshInstance};
    let Ok(m) = MeshCollider::parse(data) else {
        return;
    };
    keep((
        m.vertex_count(),
        m.triangle_count(),
        m.node_count(),
        m.bounds(),
    ));
    for i in 0..m.vertex_count().min(2048) {
        keep(m.vertex(i));
    }
    for t in 0..m.triangle_count().min(2048) {
        keep(m.tri(t));
    }
    let b = m.bounds();
    let mut hits = 0;
    m.for_each_tri(&b, |_| {
        hits += 1;
        hits < 4096
    });
    let inst = MeshInstance::new(m, Mat34::IDENTITY);
    for t in 0..m.triangle_count().min(256) {
        keep(inst.triangle(t));
    }
    keep(inst.raycast(Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.0, -1.0, 0.0), 100.0));
    let mut out = [0u32; 64];
    keep(inst.tris_in(
        &Aabb::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
        &mut out,
    ));
}

/// TPHY: the layer matrix.
pub fn tphy(data: &[u8]) {
    let Some(m) = trino_core::physics::LayerMatrix::parse(data) else {
        return;
    };
    for i in 0..m.len().min(64) {
        keep(m.mask(i));
    }
    keep(m.mask_for(0xFFFF));
}

/// TBND: the table, every entry's path and payload, each decoded into a
/// buffer of its announced size (capped: a loader caps it too).
pub fn tbnd(data: &[u8]) {
    use trino_core::bundle::{self, Bundle};
    keep(Bundle::prefix_len(data));
    let Ok(b) = Bundle::parse(data) else { return };
    keep(b.read_total(trino_core::HwTier::Base));
    keep(b.read_total(trino_core::HwTier::Expanded));
    for e in b.entries().take(1024) {
        keep(b.path(&e));
        keep(b.payload_range(&e));
        keep(e.loads_on(trino_core::HwTier::Base));
        if let Some(stored) = b.stored(&e)
            && (e.raw as usize) <= MAX_DECODE
        {
            let mut out = vec![0u8; e.raw as usize];
            if bundle::decode(&e, stored, &mut out).is_ok() {
                keep(bundle::hash(&out));
            }
        }
    }
}

/// Largest buffer a target allocates from a size an input announces.
pub const MAX_DECODE: usize = 1 << 20;

/// LZ4 block decode (bundles, the Web pack).
pub fn lz4(data: &[u8]) {
    let mut out = [0u8; 8192];
    if let Ok(n) = trino_core::lz4::decompress(data, &mut out) {
        assert!(n <= out.len());
    }
    let mut exact = [0u8; 300];
    keep(trino_core::lz4::decompress_exact(data, &mut exact));
}

/// TREC: every frame of a recording.
pub fn trec(data: &[u8]) {
    use trino_core::replay::{Header, Reader};
    if let Ok(h) = Header::parse(data) {
        let back = Header::parse(&h.to_bytes()).expect("a written TREC header parses");
        assert_eq!(back.to_bytes(), h.to_bytes(), "TREC header round trip");
    }
    let Ok(mut r) = Reader::new(data) else { return };
    let mut frames = 0;
    while let Some(f) = r.next_frame() {
        frames += 1;
        if f.is_err() || frames > 100_000 {
            break;
        }
    }
    let cur = r.cursor();
    let mut again = Reader::resume(data, cur);
    keep(again.next_frame());
    r.rewind();
    keep(r.next_frame());
}

/// TTRC: trace packets, and the log lines consoles print them as.
pub fn ttrc(data: &[u8]) {
    use trino_core::trace::{Packet, TraceLine};
    let mut at = 0;
    while at < data.len() {
        match Packet::parse(&data[at..]) {
            Ok((p, len)) => {
                keep(p);
                assert!(len > 0);
                at += len;
            }
            Err(_) => break,
        }
    }
    if let Ok(text) = core::str::from_utf8(data) {
        let mut out = [0u8; 4096];
        for line in text.lines().take(256) {
            if let Some(l) = TraceLine::parse(line) {
                keep(l.decode(&mut out));
            }
            keep(trino_core::trace::base64_decode(line, &mut out));
        }
    }
}

/// The 24-byte network pad and the input sets games exchange (#67).
pub fn netpad(data: &[u8]) {
    let mut b = [0u8; trino_core::NET_PAD_LEN];
    let n = data.len().min(b.len());
    b[..n].copy_from_slice(&data[..n]);
    let pad = trino_core::NetPad::from_bytes(b);
    let back = trino_core::NetPad::from_bytes(pad.to_bytes());
    assert_eq!(back.to_bytes(), pad.to_bytes(), "NetPad repack is stable");
    let mut k = [0u8; 16];
    let m = data.len().min(16);
    k[..m].copy_from_slice(&data[..m]);
    keep(trino_core::KeySet::from_bytes(k));
}

// ------------------------------------------------------------------- saves

/// A save medium in RAM (an EEPROM image).
pub struct Ram(pub Vec<u8>);

impl trino_core::save::Medium for Ram {
    fn size(&self) -> usize {
        self.0.len()
    }
    fn read(&mut self, at: usize, buf: &mut [u8]) {
        buf.copy_from_slice(&self.0[at..at + buf.len()]);
    }
    fn write(&mut self, at: usize, data: &[u8]) {
        self.0[at..at + data.len()].copy_from_slice(data);
    }
}

/// The save frame on a raw medium (the N64 EEPROM's A/B slots) and on a
/// slot file (PC, Web, 3DS): the input is the medium's bytes. Whatever the
/// medium holds, reading never panics, and a save written next reads back.
pub fn save_frame(data: &[u8]) {
    use trino_core::save;
    // 512 B and 2 KB EEPROMs, and whatever size the input is.
    for size in [
        512usize,
        2048,
        data.len().clamp(2 * save::SLOT_HEADER + 64, 4096),
    ] {
        let mut img = vec![0xFFu8; size];
        let n = data.len().min(size);
        img[..n].copy_from_slice(&data[..n]);
        let mut m = Ram(img);
        let mut scratch = vec![0u8; size];
        let mut out = vec![0u8; size];
        let found = save::find_newest(&mut m, &mut scratch);
        let len = save::read_newest(&mut m, &mut out, &mut scratch);
        if let Some(f) = found {
            assert!(f.offset + f.span <= size, "a found slot lies on the medium");
        }
        keep(len);
        let mut body = vec![0u8; size];
        let mut lz = save::Lz::default();
        let new = b"after-damage-save";
        save::write_now(&mut m, new, &mut body, &mut scratch, &mut lz).expect("a small save fits");
        let got = save::read_newest(&mut m, &mut out, &mut scratch);
        assert_eq!(
            &out[..got],
            new,
            "a save written over any medium reads back"
        );
    }
    // A slot file as the PC, the Web and the 3DS keep them, and two of
    // them as a store.
    if let Ok(v) = save::parse_slot(data) {
        assert!(v.data.len() + save::SLOT_HEADER == data.len());
    }
    let half = data.len() / 2;
    let mut store = Files(
        [Some(data[..half].to_vec()), Some(data[half..].to_vec())],
        None,
    );
    let mut out = vec![0u8; 4096];
    let mut scratch = vec![0u8; 4096];
    let r = save::read_slot_files(&mut store, &mut out, &mut scratch).expect("a RAM store reads");
    assert!(r.len <= 4096 || r.slot.is_none());
    let mut file = vec![0u8; 4096];
    save::write_slot_files(&mut store, b"next", &mut scratch, &mut file)
        .expect("a RAM store writes");
    let r2 = save::read_slot_files(&mut store, &mut out, &mut scratch).unwrap();
    assert_eq!(
        &out[..r2.len],
        b"next",
        "a slot-file save written after any damage reads back"
    );
    assert_eq!(r2.status, trino_core::SaveStatus::Ok);
    // The legacy single-frame layout.
    if data.len() >= save::HEADER {
        let h: [u8; save::HEADER] = data[..save::HEADER].try_into().unwrap();
        keep(save::verify(&h, &data[save::HEADER..]));
    }
}

/// Two slot files in RAM (the PC's files, the Web's keys, the 3DS's SD
/// files) and a legacy one.
pub struct Files(pub [Option<Vec<u8>>; 2], pub Option<Vec<u8>>);

impl trino_core::save::SlotStore for Files {
    fn read_slot(
        &mut self,
        slot: u8,
        buf: &mut [u8],
    ) -> Result<Option<usize>, trino_core::SaveError> {
        Ok(self.0[slot as usize].as_ref().map(|f| {
            let n = f.len().min(buf.len());
            buf[..n].copy_from_slice(&f[..n]);
            f.len()
        }))
    }
    fn write_slot(&mut self, slot: u8, bytes: &[u8]) -> Result<(), trino_core::SaveError> {
        self.0[slot as usize] = Some(bytes.to_vec());
        Ok(())
    }
    fn read_legacy(&mut self, buf: &mut [u8]) -> Result<Option<usize>, trino_core::SaveError> {
        Ok(self.1.as_ref().map(|f| {
            let n = f.len().min(buf.len());
            buf[..n].copy_from_slice(&f[..n]);
            f.len()
        }))
    }
}

/// The save compressor's decoder, and compress → decompress round trips.
pub fn save_lz(data: &[u8]) {
    use trino_core::save;
    let mut out = [0u8; 2048];
    keep(save::decompress(data, &mut out));
    let src = &data[..data.len().min(2048)];
    let mut packed = [0u8; 4096];
    let mut lz = save::Lz::default();
    if let Some(n) = save::compress(src, &mut packed, &mut lz) {
        let mut back = [0u8; 2048];
        assert_eq!(save::decompress(&packed[..n], &mut back), Some(src.len()));
        assert_eq!(&back[..src.len()], src, "LZ round trip");
    }
}

// -------------------------------------------------------------- text files

/// Text the core parses: the quality preset table and settings lines, the
/// accessibility settings, thread counts, tilemaps, language tags.
pub fn core_text(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    use trino_core::quality::{PresetTable, QualitySettings};
    let table = PresetTable::parse(text);
    keep(table);
    keep(QualitySettings::parse_text(
        QualitySettings::default(),
        text,
    ));
    keep(trino_core::AccessSettings::parse_text(
        trino_core::AccessSettings::DEFAULT,
        text,
    ));
    for line in text.lines().take(64) {
        keep(trino_core::ThreadSetting::parse(line));
        keep(trino_core::LangTag::parse(line));
        keep(trino_core::PadFamily::parse(line));
        keep(trino_core::CrashConsent::parse(line));
    }
    if let Ok(map) = trino_core::tilemap::Tilemap::parse(text) {
        keep(map.pixel_size());
        for c in map.cells().take(4096) {
            keep(c);
        }
        keep(map.tile(-1, 3));
        keep(map.is_solid(1000, 1000));
    }
}

/// The 3-byte accessibility save form.
pub fn access_pack(data: &[u8]) {
    let mut b = [0u8; 3];
    let n = data.len().min(3);
    b[..n].copy_from_slice(&data[..n]);
    if let Some(s) = trino_core::AccessSettings::unpack(b) {
        assert_eq!(
            trino_core::AccessSettings::unpack(s.pack()),
            Some(s),
            "access repack is stable"
        );
    }
}

/// A console build's asset index (`index.tsv`, issue #121): every line as
/// the N64 and 3DS loaders read it (`IndexLine`, the tier column, a sound's
/// codec and bus), and the same text as a group entry's locator. A parsed
/// line written back as the bake writes it parses to the same line.
pub fn asset_index(data: &[u8]) {
    use trino_core::HwTier;
    use trino_core::asset::{IndexKind, IndexLine, Locator, SoundColumns};
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    for raw in text.lines().take(4096) {
        let loc = Locator::parse(raw);
        keep(loc.file);
        keep(SoundColumns::read(&mut loc.extra()));
        keep(loc.extra().count());
        let Some(line) = IndexLine::parse(raw) else {
            continue;
        };
        keep((line.loads_on(HwTier::Base), line.loads_on(HwTier::Expanded)));
        keep(SoundColumns::read(&mut line.extra()));
        keep(line.extra().count());
        let label = match line.kind {
            IndexKind::Entry(k) => k.label(),
            IndexKind::Group => "group",
            IndexKind::Boot => "boot",
            IndexKind::Compression => "compression",
            IndexKind::Mixer => "mixer",
            IndexKind::Unknown(l) => l,
        };
        // The bake's form (`{id:08x} \t kind \t file`) reads back the same.
        let canon = format!("{:08x}\t{label}\t{}", line.id, line.file);
        let back = IndexLine::parse(&canon).expect("a written index line parses");
        assert_eq!(
            (back.id, back.kind, back.file, back.tier),
            (line.id, line.kind, line.file, None)
        );
    }
}
