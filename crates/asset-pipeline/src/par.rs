//! Work spread over the build's threads (issue #101), results in input
//! order: a parallel bake writes the same files and reports the same
//! entries and errors, in the same order, as a serial one.

use std::sync::atomic::{AtomicUsize, Ordering};

/// `f` over `items` on up to `threads` workers; `out[i] = f(&items[i])`.
pub fn map_ordered<T: Sync, R: Send>(
    threads: u32,
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let workers = (threads.max(1) as usize).min(items.len());
    if workers <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut done: Vec<(usize, R)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        mine.push((i, f(item)));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    done.sort_unstable_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// The default build budget: `$TRINO_THREADS`, else all CPUs but one.
pub fn default_threads() -> u32 {
    std::env::var("TRINO_THREADS")
        .ok()
        .and_then(|v| trino_core::ThreadSetting::parse(&v))
        .unwrap_or_default()
        .resolve(std::thread::available_parallelism().map_or(1, |n| n.get() as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_keep_the_input_order_on_any_thread_count() {
        let items: Vec<u32> = (0..257).collect();
        let serial = map_ordered(1, &items, |v| v * 3);
        for threads in [2, 7, 64] {
            assert_eq!(map_ordered(threads, &items, |v| v * 3), serial);
        }
        assert!(map_ordered(8, &[] as &[u32], |v| *v).is_empty());
    }
}
