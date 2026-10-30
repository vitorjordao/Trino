//! The host side of a hot-reloaded game's zones (issue #79, #105 item 4):
//! under `cargo xtask watch pc --trace` the game dylib links its own copy of
//! `trino-core`, whose zones would go to a ring nobody installed.
//! [`attach`] is what [`crate::system::PcPlatform`] hands the game through
//! `Platform::trace_hooks`: hooks that record into this binary's ring.
//!
//! A dylib's zone names live in the dylib, which a reload unmaps, while the
//! ring keeps names until it is flushed: each name is copied once into
//! leaked memory (a game has a few dozen).

use std::collections::HashSet;
use std::sync::Mutex;

use trino_core::trace::{self, Hooks, TraceAttach};

static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// `name`, as a string that outlives any dylib.
pub fn keep(name: &str) -> &'static str {
    let mut set = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let set = set.get_or_insert_with(HashSet::new);
    if let Some(k) = set.get(name) {
        return k;
    }
    let k: &'static str = Box::leak(name.to_owned().into_boxed_str());
    set.insert(k);
    k
}

fn begin(track: u8, _name: &'static str) {
    trace::forward_begin(track);
}

fn end(track: u8, name: &'static str, start: u32, end: u32) {
    trace::forward_end(keep(name), track, start, end);
}

fn counter(name: &'static str, value: u32) {
    trace::counter(keep(name), value);
}

fn frame(n: u32) {
    trace::frame_mark(n);
}

/// The hooks, when this binary records (`TRINO_TRACE` on a `trace` build).
pub fn attach() -> Option<TraceAttach> {
    (trace::ENABLED && trace::is_on()).then_some(TraceAttach {
        hooks: Hooks {
            begin,
            end,
            counter,
            frame,
        },
        clock: trace::now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_copied_once_and_outlive_their_source() {
        let a = keep(&String::from("ai"));
        let b = keep("ai");
        assert!(std::ptr::eq(a, b), "one copy per name");
        assert_eq!(a, "ai");
        // Without a recording ring there is nothing to attach to.
        if !trace::is_on() {
            assert!(attach().is_none());
        }
    }
}
