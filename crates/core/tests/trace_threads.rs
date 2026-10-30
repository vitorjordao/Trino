//! Zones on other threads (issue #130): each bound thread records into a
//! lane of its own, merged into the global ring by the main thread, so a
//! busy frame loses nothing and every thread's zones nest on their own track.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU32, Ordering};
use trino_core::trace::{
    self, GlobalRing, Lane, Packet, ThreadHooks, TraceLine, TracePlatform, track,
};

thread_local! {
    static LANE: Cell<Option<&'static Lane>> = const { Cell::new(None) };
}

fn hooks() -> ThreadHooks {
    ThreadHooks {
        get: || LANE.with(Cell::get),
        set: |l| LANE.with(|c| c.set(l)),
        grow: || Some(Box::leak(Box::new(Lane::new()))),
    }
}

fn clock() -> u32 {
    static T: AtomicU32 = AtomicU32::new(0);
    T.fetch_add(1, Ordering::Relaxed)
}

/// Every event of a run's packets: (name, kind, track, depth, ts, value),
/// and the dropped total.
/// (name, kind, track, depth, ts, value).
type Event = (String, u8, u8, u8, u32, u32);

fn decode(text: &str) -> (Vec<Event>, u64) {
    let mut by_session: Vec<Vec<u8>> = Vec::new();
    // One flush is one run of lines numbered from 0.
    let mut current: Vec<TraceLine> = Vec::new();
    let lines: Vec<TraceLine> = text.lines().filter_map(TraceLine::parse).collect();
    for l in lines {
        if l.seq == 0 && !current.is_empty() {
            by_session.push(packet(&current));
            current.clear();
        }
        current.push(l);
    }
    if !current.is_empty() {
        by_session.push(packet(&current));
    }
    let (mut names, mut out, mut dropped) = (Vec::<String>::new(), Vec::new(), 0u64);
    for bytes in by_session {
        let (p, _) = Packet::parse(&bytes).unwrap();
        assert_eq!(p.name_base as usize, names.len());
        names.extend(p.names().map(str::to_string));
        dropped += p.dropped as u64;
        for e in p.events() {
            out.push((
                names[e.name as usize].clone(),
                e.kind,
                e.track,
                e.depth,
                e.ts,
                e.value,
            ));
        }
    }
    (out, dropped)
}

fn packet(lines: &[TraceLine]) -> Vec<u8> {
    let mut out = vec![0u8; 1 << 20];
    let n = trace::decode_lines(lines.iter().copied(), &mut out).unwrap();
    out.truncate(n);
    out
}

#[test]
fn threads_record_on_their_own_tracks_nested_and_lose_nothing() {
    const THREADS: u8 = 6;
    const ROUNDS: u32 = 60;
    static mut RING: GlobalRing = GlobalRing::new();
    // SAFETY: the test's only reference.
    assert!(trace::install(
        unsafe { &mut *core::ptr::addr_of_mut!(RING) },
        clock,
        TracePlatform::Pc
    ));
    // Without hooks a thread can't bind: its zones go to the global ring.
    assert!(!trace::bind_thread(track::WORKER).is_bound());
    trace::set_thread_hooks(Some(hooks()));

    let mut text = String::new();
    let flush = |text: &mut String| {
        trace::with_ring(|r, p| r.write_lines(p, "threads", &mut 0, text).unwrap()).unwrap();
    };
    // Twice: the second round's threads claim the lanes the first released.
    for round in 0..2 {
        let start = Barrier::new(THREADS as usize + 1);
        std::thread::scope(|s| {
            for i in 0..THREADS {
                let start = &start;
                s.spawn(move || {
                    let bound = trace::bind_thread(track::on(track::WORKER, i + 1));
                    assert!(bound.is_bound());
                    // A second binding keeps the first.
                    assert!(!trace::bind_thread(track::AUDIO).is_bound());
                    start.wait();
                    for n in 0..ROUNDS {
                        trino_core::zone!("job");
                        {
                            trino_core::zone!("step");
                            trace::counter("items", n);
                        }
                        std::hint::black_box(n);
                    }
                });
            }
            start.wait();
            // The main thread keeps recording and merging meanwhile: a
            // busy frame on every thread at once.
            for f in 0..40 {
                trace::frame_mark(round * 100 + f);
                trino_core::zone!("main");
                {
                    trino_core::zone!("main inner");
                }
                if f % 10 == 9 {
                    flush(&mut text);
                }
            }
        });
        // The workers' last events reach the ring at the next merge.
        flush(&mut text);
        assert!(trace::lane_count() <= THREADS as usize, "lanes are reused");
    }
    trace::set_thread_hooks(None);
    trace::uninstall().unwrap();

    let (events, dropped) = decode(&text);
    assert_eq!(dropped, 0, "nothing lost under a busy frame");
    // Every worker's zones on its own track, nested under their job.
    for i in 1..=THREADS {
        let t = track::on(track::WORKER, i);
        let mine: Vec<_> = events.iter().filter(|e| e.2 == t).collect();
        let jobs: Vec<_> = mine.iter().filter(|e| e.0 == "job").collect();
        let steps: Vec<_> = mine.iter().filter(|e| e.0 == "step").collect();
        let items = mine.iter().filter(|e| e.0 == "items").count();
        assert_eq!(jobs.len(), 2 * ROUNDS as usize, "worker {i}");
        assert_eq!(
            (steps.len(), items),
            (2 * ROUNDS as usize, 2 * ROUNDS as usize)
        );
        assert!(jobs.iter().all(|e| e.3 == 0) && steps.iter().all(|e| e.3 == 1));
        assert!(mine.iter().filter(|e| e.0 == "items").all(|e| e.1 == 1));
        // Each step lies inside its job (the lane keeps the thread's order).
        for (job, step) in jobs.iter().zip(&steps) {
            assert!(
                step.4 >= job.4 && step.4 + step.5 <= job.4 + job.5,
                "{job:?} {step:?}"
            );
        }
    }
    // The main thread's zones stayed on the CPU track, nested alike.
    let main: HashMap<&str, Vec<u8>> = events
        .iter()
        .filter(|e| e.2 == track::CPU && e.1 == 0)
        .fold(HashMap::new(), |mut m, e| {
            m.entry(e.0.as_str()).or_default().push(e.3);
            m
        });
    assert_eq!(main["main"], vec![0; 80]);
    assert_eq!(main["main inner"], vec![1; 80]);
    assert_eq!(
        events.iter().filter(|e| e.1 == 2).count(),
        80,
        "frame marks"
    );
    assert_eq!(
        track::label(track::on(track::WORKER, 3)).to_string(),
        "worker 3"
    );
    assert_eq!(track::label(track::AUDIO).to_string(), "audio");
}
