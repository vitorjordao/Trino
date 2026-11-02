//! The `graph crowd` self-test step (issue #138): what an animation graph
//! costs a crowd. Solid Eleven's player graph (a 1D blend space for the
//! gait and two kick layers, `examples/solid-eleven/assets/anim/
//! player.anim.ron`) drives [`CROWD`] instances through a scripted match:
//! speeds that change every few frames, a kick per player every 40 frames,
//! the layers fading in and out. Each frame updates every instance, then
//! asks for every pose; the step times both on the platform's clock
//! (`TRINO_GRAPH_COST:`, `scene=micro-graph` on the N64) and hashes the
//! playback, which must be [`CROWD_HASH`] on every target.

use trino_core::anim::{AnimGraph, GraphInstance, GraphRef, GraphTables};
use trino_core::mem::Arena;
use trino_core::{Platform, TextBuf};

/// Solid Eleven's player graph, as the game embeds it.
pub static PLAYER_TANG: &[u8] = include_bytes!("../../solid-eleven/src/render/player.tang");

/// Players on a pitch.
pub const CROWD: usize = 22;
/// Updates the script runs (two seconds at 60 Hz).
pub const FRAMES: u32 = 120;
/// Instances measured at once: half the crowd, so the step's stack frame
/// stays small on the consoles (the N64's 64 KB).
const BATCH: usize = 11;

/// The script's hash (every instance's [`AnimGraphInstance::digest`] and
/// pose, every frame), the same on every target. `xtask/tests/
/// engine_checks.rs` prints a moved value.
pub const CROWD_HASH: u32 = 0x50a5_e8bb;

/// What one variant of the crowd measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VariantCost {
    /// Mean µs of one instance's `update`, ×100 (two decimals).
    pub update_cus: u32,
    /// Mean µs of one instance's `pose`, ×100.
    pub pose_cus: u32,
    /// `size_of` the instance the variant uses.
    pub instance_bytes: u32,
    /// Transitions the crowd took (the kicks fired).
    pub transitions: u32,
    pub hash: u32,
}

/// The variants, in [`CrowdCost::variants`]' order: an
/// `AnimGraphInstance` over the blob (every graph before issue #138), the
/// same over the decoded tables, and a crowd instance (`GraphInstance<3,
/// 0, 0>`: the base and two layers, no inertia, no events) over the
/// tables.
pub const VARIANTS: [&str; 3] = ["blob", "tables", "crowd"];

/// What the step measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CrowdCost {
    pub variants: [VariantCost; 3],
    /// Arena bytes the decoded tables took.
    pub table_bytes: u32,
}

/// The crowd's instance: the base and the two kick layers.
pub type CrowdInstance = trino_core::anim::GraphInstance<3, 0, 0>;

/// Arena for the decoded tables (what `GraphTables::bytes_for` asks for
/// the player graph, with room).
const TABLE_ARENA: usize = 6 * 1024;

/// The graph's parameter indices.
#[derive(Clone, Copy)]
struct Params {
    speed: usize,
    kind: usize,
    strike: usize,
    arms: usize,
}

/// Player `k`'s parameters at frame `f`: integers turned into floats, so
/// every target computes the same values.
fn drive(k: usize, f: u32) -> (f32, f32, f32) {
    let f = f as usize;
    // A new speed every 12 frames, 0..7.9 m/s.
    let speed = ((k * 37 + (f / 12) * 23) % 80) as f32 * 0.1;
    // A kick every 40 frames, at the player's own offset, for 24 frames.
    let since = (f + 40 - (k * 7) % 40) % 40;
    let (kind, strike) = if since < 24 {
        let ramp = if since < 6 {
            since as f32 / 6.0
        } else if since >= 18 {
            (24 - since) as f32 / 6.0
        } else {
            1.0
        };
        ((1 + k % 5) as f32, ramp)
    } else {
        (0.0, 0.0)
    };
    (speed, kind, strike)
}

/// FNV-1a over words.
fn mix(h: &mut u32, v: u32) {
    for b in v.to_le_bytes() {
        *h ^= b as u32;
        *h = h.wrapping_mul(0x0100_0193);
    }
}

/// Instances `first..first + BATCH` of the crowd through the script:
/// (update µs, pose µs, transitions, hash).
#[inline(never)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn batch<const L: usize, const I: usize, const E: usize>(
    platform: &mut dyn Platform,
    g: GraphRef,
    p: Params,
    first: usize,
) -> (u64, u64, u32, u32) {
    let mut crowd = [GraphInstance::<L, I, E>::new(g); BATCH];
    let (mut up, mut po, mut h) = (0u64, 0u64, 0x811c_9dc5u32);
    for f in 0..FRAMES {
        for (i, inst) in crowd.iter_mut().enumerate() {
            let (speed, kind, strike) = drive(first + i, f);
            inst.set_float(p.speed, speed);
            inst.set_float(p.kind, kind);
            inst.set_float(p.strike, strike);
            inst.set_float(p.arms, if speed < 3.0 { strike } else { 0.0 });
        }
        let t0 = platform.now_micros();
        for inst in crowd.iter_mut() {
            inst.update(g, 1.0 / 60.0);
        }
        let t1 = platform.now_micros();
        for inst in crowd.iter() {
            core::hint::black_box(inst.pose(g));
        }
        let t2 = platform.now_micros();
        up += t1.saturating_sub(t0);
        po += t2.saturating_sub(t1);
        // The hash, outside the timed spans.
        for inst in crowd.iter() {
            mix(&mut h, inst.pose(g).digest());
            mix(&mut h, inst.digest());
        }
    }
    let tr = crowd.iter().map(|i| i.transitions).sum();
    (up, po, tr, h)
}

/// The whole crowd of one variant.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
fn variant<const L: usize, const I: usize, const E: usize>(
    platform: &mut dyn Platform,
    g: GraphRef,
    p: Params,
) -> Result<VariantCost, &'static str> {
    let mut v = VariantCost {
        instance_bytes: core::mem::size_of::<GraphInstance<L, I, E>>() as u32,
        hash: 0x811c_9dc5,
        ..VariantCost::default()
    };
    let (mut up, mut po) = (0u64, 0u64);
    for first in (0..CROWD).step_by(BATCH) {
        let (u, q, tr, h) = batch::<L, I, E>(platform, g, p, first);
        up += u;
        po += q;
        v.transitions += tr;
        mix(&mut v.hash, h);
    }
    let n = (CROWD as u64) * FRAMES as u64;
    v.update_cus = (up * 100 / n) as u32;
    v.pose_cus = (po * 100 / n) as u32;
    // Five kicks per player in two seconds, each into its layer state and
    // back to rest, on both layers (the slow ones).
    if v.transitions < CROWD as u32 * 2 {
        return Err("graph crowd: the kicks did not fire");
    }
    Ok(v)
}

/// The step without the pinned hash (the PC test prints it): the three
/// [`VARIANTS`] must agree with each other.
#[inline(never)]
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn measure(platform: &mut dyn Platform) -> Result<CrowdCost, &'static str> {
    let g = AnimGraph::parse(PLAYER_TANG).map_err(|_| "graph crowd: player.tang does not parse")?;
    let param = |n: &str| {
        g.param_named(n)
            .ok_or("graph crowd: a parameter is missing")
    };
    let p = Params {
        speed: param("speed")?,
        kind: param("kind")?,
        strike: param("strike")?,
        arms: param("arms")?,
    };
    let mut cost = CrowdCost {
        variants: [variant::<4, 4, 1>(platform, (&g).into(), p)?; 3],
        table_bytes: 0,
    };
    let mut memory = [core::mem::MaybeUninit::<u8>::uninit(); TABLE_ARENA];
    if GraphTables::bytes_for(&g) > TABLE_ARENA {
        return Err("graph crowd: the decoded tables outgrew the step's arena");
    }
    let arena = Arena::new(&mut memory);
    let t = GraphTables::decode_in(&g, &arena).ok_or("graph crowd: the tables did not decode")?;
    cost.table_bytes = arena.used() as u32;
    cost.variants[1] = variant::<4, 4, 1>(platform, (&t).into(), p)?;
    cost.variants[2] = variant::<3, 0, 0>(platform, (&t).into(), p)?;
    let [a, b, c] = cost.variants;
    if a.hash != b.hash || a.hash != c.hash || a.transitions != c.transitions {
        return Err("graph crowd: the tables or the crowd instance played another script");
    }
    Ok(cost)
}

/// The `graph crowd` step: [`measure`], its hash against the PC's.
pub fn run(platform: &mut dyn Platform) -> Result<CrowdCost, &'static str> {
    let c = measure(platform)?;
    if c.variants[0].hash != CROWD_HASH {
        return Err("graph crowd: the playback hash differs from the PC's (CROWD_HASH)");
    }
    Ok(c)
}

/// `TRINO_STEP:graph crowd …` and a `TRINO_GRAPH_COST: scene=micro-graph
/// …` line per variant.
#[cfg_attr(any(target_os = "none", target_os = "horizon"), optimize(size))]
pub fn log(platform: &mut dyn Platform, c: &CrowdCost) {
    let mut line: TextBuf<256> = TextBuf::new();
    let v = c.variants[0];
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:graph crowd hash={:08x} transitions={} table_bytes={}\n",
            v.hash, v.transitions, c.table_bytes
        ),
    );
    platform.log(line.as_str());
    for (name, v) in VARIANTS.iter().zip(c.variants) {
        let mut line: TextBuf<256> = TextBuf::new();
        let us = |cus: u32| (cus / 100, cus % 100);
        let (uu, uf) = us(v.update_cus);
        let (pu, pf) = us(v.pose_cus);
        let _ = core::fmt::Write::write_fmt(
            &mut line,
            format_args!(
                "TRINO_GRAPH_COST: scene=micro-graph variant={name} instances={CROWD} \
                 frames={FRAMES} update_us={uu}.{uf:02} pose_us={pu}.{pf:02} instance_bytes={}\n",
                v.instance_bytes
            ),
        );
        platform.log(line.as_str());
    }
}
