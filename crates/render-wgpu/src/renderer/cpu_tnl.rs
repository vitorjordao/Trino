//! The native profile's CPU T&L fallback over worker threads (issue #51,
//! Phase 4). Draws that stay on the CPU path are recorded as [`CpuJob`]s
//! and tessellated when the renderer's batch flushes: in one go on the
//! calling thread, or — when the batch holds enough work — in contiguous
//! chunks, one per worker (`std::thread::scope`), each with its own
//! scratch memory. The chunks' triangles are concatenated in submission
//! order, so the triangle stream, its painter tie-breaks and the counters
//! are exactly the single-threaded ones (`parallel_runs_emit_the_serial_stream`).
//!
//! The console simulation profiles don't defer: their T&L runs inside the
//! draw call, as on the consoles, and their counters (the N64 split, strict
//! mode) are taken there.

use std::collections::HashMap;

use trino_core::render3d::{
    MeshLayout, ScreenTri, SurfaceScratch, TnlContext, TnlScratch, TnlStats, tessellate_surface,
    tessellate_with,
};
use trino_core::{Mat34, Mesh, ModelParams};

use super::batch::TexKey;
use super::{NO_SURFACE, SurfTri};

/// Below this many source triangles in a batch, one thread does it all
/// (spawning workers costs tens of microseconds).
pub const PARALLEL_MIN_TRIS: u32 = 2048;

/// What a job tessellates with.
#[derive(Clone, Copy, Debug)]
pub enum JobKind {
    /// `tessellate_with`: vertex colors (the screen-space triangle pass).
    Plain,
    /// `tessellate_surface`: shaded per pixel by the lit pipeline.
    Lit { set: u32, mask: u32, gloss: f32 },
    /// A water surface: per pixel (`tessellate_surface`) or per vertex.
    Water {
        set: u32,
        gloss: f32,
        per_pixel: bool,
    },
    /// A billboard's two triangles, already built (kept in order).
    Ready([ScreenTri; 2]),
}

/// One recorded CPU draw.
#[derive(Clone, Copy, Debug)]
pub struct CpuJob {
    pub kind: JobKind,
    pub model: u32,
    pub matrix: Mat34,
    pub params: ModelParams,
    /// Index into the frame's T&L contexts.
    pub ctx: u32,
    pub texture: Option<TexKey>,
    /// Source triangles (load balancing).
    pub tris: u32,
}

/// One worker's memory, kept across frames.
pub struct Scratch {
    tnl: Box<TnlScratch>,
    surf: Box<SurfaceScratch>,
}

impl Scratch {
    pub fn new() -> Scratch {
        Scratch {
            tnl: Box::new(TnlScratch::new()),
            surf: Box::new(SurfaceScratch::new()),
        }
    }
}

/// What a run of jobs produced: the triangles in order, their per-pixel
/// inputs (parallel to `tris`, `None` for plain ones), and per job the
/// triangles it added and its counters.
#[derive(Default)]
pub struct ChunkOut {
    pub tris: Vec<ScreenTri>,
    pub surf: Vec<Option<SurfTri>>,
    pub per_job: Vec<(u32, TnlStats)>,
}

/// Tessellate `jobs` with up to `threads` workers; the chunks come back in
/// job order. `pool` grows to one scratch per worker.
pub fn run(
    meshes: &HashMap<u32, (Vec<u8>, MeshLayout)>,
    ctxs: &[TnlContext],
    jobs: &[CpuJob],
    pool: &mut Vec<Scratch>,
    threads: usize,
) -> Vec<ChunkOut> {
    let total: u32 = jobs.iter().map(|j| j.tris).sum();
    let workers = if total >= PARALLEL_MIN_TRIS {
        threads.clamp(1, jobs.len().max(1))
    } else {
        1
    };
    while pool.len() < workers {
        pool.push(Scratch::new());
    }
    if workers == 1 {
        return vec![run_chunk(meshes, ctxs, jobs, &mut pool[0])];
    }
    let chunks = split(jobs, workers, total);
    std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .zip(pool.iter_mut())
            .map(|(range, scratch)| {
                let part = &jobs[range.clone()];
                s.spawn(move || run_chunk(meshes, ctxs, part, scratch))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("T&L worker panicked"))
            .collect()
    })
}

/// Contiguous job ranges of about `total / workers` source triangles each
/// (never empty, at most `workers`).
fn split(jobs: &[CpuJob], workers: usize, total: u32) -> Vec<std::ops::Range<usize>> {
    let target = total.div_ceil(workers as u32).max(1);
    let mut out = Vec::with_capacity(workers);
    let (mut start, mut acc) = (0, 0u32);
    for (i, j) in jobs.iter().enumerate() {
        acc += j.tris;
        if acc >= target && out.len() + 1 < workers {
            out.push(start..i + 1);
            start = i + 1;
            acc = 0;
        }
    }
    if start < jobs.len() {
        out.push(start..jobs.len());
    }
    out
}

fn run_chunk(
    meshes: &HashMap<u32, (Vec<u8>, MeshLayout)>,
    ctxs: &[TnlContext],
    jobs: &[CpuJob],
    scratch: &mut Scratch,
) -> ChunkOut {
    let mut out = ChunkOut::default();
    for job in jobs {
        let before = out.tris.len();
        let stats = run_job(meshes, ctxs, job, scratch, &mut out);
        out.per_job.push(((out.tris.len() - before) as u32, stats));
    }
    out
}

fn run_job(
    meshes: &HashMap<u32, (Vec<u8>, MeshLayout)>,
    ctxs: &[TnlContext],
    job: &CpuJob,
    scratch: &mut Scratch,
    out: &mut ChunkOut,
) -> TnlStats {
    if let JobKind::Ready(tris) = job.kind {
        out.tris.extend_from_slice(&tris);
        out.surf.extend([None, None]);
        return TnlStats::default();
    }
    let Some((blob, layout)) = meshes.get(&job.model) else {
        return TnlStats::default();
    };
    let mesh = Mesh::from_layout(blob, layout);
    let ctx = &ctxs[job.ctx as usize];
    let ChunkOut { tris, surf, .. } = out;
    let mut surface = |tri: ScreenTri, v: &[trino_core::render3d::SurfaceVert; 3], s: SurfTri| {
        tris.push(tri);
        surf.push(Some(SurfTri { v: *v, ..s }));
    };
    match job.kind {
        JobKind::Plain => tessellate_with(
            ctx,
            &mut scratch.tnl,
            &mesh,
            &job.matrix,
            &job.params,
            &mut |tri| {
                tris.push(tri);
                surf.push(None);
            },
        ),
        JobKind::Lit { set, mask, gloss } => {
            let s = SurfTri {
                v: [NO_SURFACE; 3],
                gloss,
                set,
                mask,
                water: false,
            };
            tessellate_surface(
                ctx,
                &mut scratch.surf,
                &mesh,
                &job.matrix,
                &job.params,
                &mut |tri, v| surface(tri, v, s),
            )
        }
        JobKind::Water {
            set,
            gloss,
            per_pixel,
        } => {
            let s = SurfTri {
                v: [NO_SURFACE; 3],
                gloss,
                set,
                mask: 0,
                water: true,
            };
            if per_pixel {
                tessellate_surface(
                    ctx,
                    &mut scratch.surf,
                    &mesh,
                    &job.matrix,
                    &job.params,
                    &mut |tri, v| surface(tri, v, s),
                )
            } else {
                tessellate_with(
                    ctx,
                    &mut scratch.tnl,
                    &mesh,
                    &job.matrix,
                    &job.params,
                    &mut |tri| surface(tri, &[NO_SURFACE; 3], s),
                )
            }
        }
        JobKind::Ready(_) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
    use trino_core::{Color, Transform3, Vec2, Vec3};

    /// A `n`×`n` grid of quads (two triangles each) with per-vertex colors.
    fn grid_tmdl(n: u16) -> Vec<u8> {
        let mut pos = Vec::new();
        let mut col = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                pos.push([x as f32 - n as f32 / 2.0, 0.0, y as f32 - n as f32 / 2.0]);
                col.push([(x * 13 % 256) as u8, (y * 29 % 256) as u8, 90, 255]);
            }
        }
        let mut idx = Vec::new();
        let w = n + 1;
        for y in 0..n {
            for x in 0..n {
                let a = y * w + x;
                idx.extend([a, a + w, a + 1, a + 1, a + w, a + w + 1]);
            }
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"TMDL");
        b.extend_from_slice(&(pos.len() as u32).to_le_bytes());
        b.extend_from_slice(&(idx.len() as u32).to_le_bytes());
        for p in &pos {
            for v in p {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for _ in &pos {
            for v in [0.0f32, 1.0, 0.0] {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for c in &col {
            b.extend_from_slice(c);
        }
        for i in &idx {
            b.extend_from_slice(&i.to_le_bytes());
        }
        b
    }

    #[test]
    fn parallel_runs_emit_the_serial_stream() {
        let blob = grid_tmdl(24);
        let mesh = Mesh::from_tmdl(&blob).expect("grid parses");
        let layout = mesh.layout(&blob);
        let tris = (mesh.index_count / 3) as u32;
        let mut meshes = HashMap::new();
        meshes.insert(7u32, (blob.clone(), layout));
        let camera = Camera3 {
            eye: Vec3::new(3.0, 9.0, -14.0),
            target: Vec3::ZERO,
            ..Camera3::default()
        };
        let mut light = DEFAULT_LIGHT;
        light.fog.near = 6.0;
        light.fog.far = 30.0;
        light.fog.color = Color::rgb(90, 100, 140);
        let ctxs = [TnlContext::new(&camera, &light, Vec2::new(640.0, 480.0))];
        // Forty draws at different places (some crossing the near plane
        // and the guard band, some culled), every shading, tints.
        let jobs: Vec<CpuJob> = (0..40)
            .map(|k| {
                let t = Transform3 {
                    position: Vec3::new((k % 8) as f32 * 6.0 - 21.0, 0.0, (k / 8) as f32 * 5.0),
                    rotation: Vec3::new(0.0, k as f32 * 0.3, 0.0),
                    scale: Vec3::ONE,
                };
                CpuJob {
                    kind: JobKind::Plain,
                    model: 7,
                    matrix: t.matrix(),
                    params: ModelParams {
                        shading: [
                            trino_core::Shading::Gouraud,
                            trino_core::Shading::Flat,
                            trino_core::Shading::Unlit,
                        ][k % 3],
                        tint: Color::rgba(255, 200 + k as u8, 255, 200 + k as u8),
                        double_sided: k % 5 == 0,
                        ..ModelParams::DEFAULT
                    },
                    ctx: 0,
                    texture: None,
                    tris,
                }
            })
            .collect();
        let flat = |chunks: Vec<ChunkOut>| {
            let mut tris = Vec::new();
            let mut per_job = Vec::new();
            for c in chunks {
                tris.extend(c.tris.iter().map(|t| format!("{t:?}")));
                per_job.extend(c.per_job);
            }
            (tris, per_job)
        };
        let mut pool = Vec::new();
        let serial = flat(run(&meshes, &ctxs, &jobs, &mut pool, 1));
        assert!(serial.0.len() > 10_000, "a real workload");
        for threads in [2, 3, 8, 64] {
            let chunks = run(&meshes, &ctxs, &jobs, &mut pool, threads);
            assert_eq!(chunks.len(), threads.min(jobs.len()), "{threads} workers");
            assert_eq!(flat(chunks), serial, "{threads} workers");
        }
        // A small batch stays on one thread.
        let small = run(&meshes, &ctxs, &jobs[..1], &mut pool, 8);
        assert_eq!(small.len(), 1);
    }

    #[test]
    fn chunks_cover_the_jobs_in_order() {
        let job = |tris| CpuJob {
            kind: JobKind::Plain,
            model: 0,
            matrix: Mat34::IDENTITY,
            params: ModelParams::DEFAULT,
            ctx: 0,
            texture: None,
            tris,
        };
        let jobs: Vec<CpuJob> = [900, 10, 10, 10, 500, 500, 2000, 1, 1]
            .into_iter()
            .map(job)
            .collect();
        for workers in 1..=12 {
            let total = jobs.iter().map(|j| j.tris).sum();
            let ranges = split(&jobs, workers, total);
            assert!(ranges.len() <= workers && !ranges.is_empty());
            let mut next = 0;
            for r in &ranges {
                assert_eq!(r.start, next);
                assert!(r.end > r.start);
                next = r.end;
            }
            assert_eq!(next, jobs.len());
        }
    }
}
