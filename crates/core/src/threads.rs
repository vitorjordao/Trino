//! How many CPU threads the engine's tools and hosts may use (issue #101).
//!
//! One rule for every place that fans work out: the build's asset bakes and
//! console shader assembly, a PC/Web host's shader precompilation and its
//! asset-group decoding. The default leaves one CPU to the rest of the
//! machine (the game's own thread, the OS, the editor): **all logical CPUs
//! minus one, never fewer than one**.
//!
//! The setting is text so `trino.toml`, `TRINO_THREADS` and `--threads`
//! share one parser ([`ThreadSetting::parse`]):
//!
//! | Text | Meaning |
//! |---|---|
//! | `auto` (or empty) | all − 1, at least 1 |
//! | `4` | four threads (at most the machine's CPUs) |
//! | `-2` | all − 2, at least 1 |
//! | `0` | the same as `auto` |
//!
//! Consoles never read it: nothing compiles there at run time, and their
//! hosts own their cores (the 3DS worker core, #54).

/// A thread budget as written by a person, resolved against the machine
/// with [`ThreadSetting::resolve`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThreadSetting {
    /// Every logical CPU but one.
    #[default]
    Auto,
    /// Exactly this many (clamped to `1..=available`).
    Count(u32),
    /// Every logical CPU but this many.
    AllBut(u32),
}

impl ThreadSetting {
    /// `auto`, `N` or `-N` (surrounding spaces ignored). `None` for
    /// anything else, so a typo is reported instead of guessed.
    pub fn parse(text: &str) -> Option<ThreadSetting> {
        let t = text.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("auto") {
            return Some(ThreadSetting::Auto);
        }
        if let Some(rest) = t.strip_prefix('-') {
            return parse_u32(rest).map(ThreadSetting::AllBut);
        }
        match parse_u32(t)? {
            0 => Some(ThreadSetting::Auto),
            n => Some(ThreadSetting::Count(n)),
        }
    }

    /// Threads to use on a machine with `available` logical CPUs (0 is
    /// read as 1: `available_parallelism` failing). Always in
    /// `1..=max(available, 1)`.
    pub fn resolve(self, available: u32) -> u32 {
        let available = available.max(1);
        let n = match self {
            ThreadSetting::Auto => available.saturating_sub(1),
            ThreadSetting::Count(n) => n,
            ThreadSetting::AllBut(n) => available.saturating_sub(n),
        };
        n.clamp(1, available.max(1))
    }

    /// The text [`parse`](Self::parse) reads back.
    pub fn write(self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
        match self {
            ThreadSetting::Auto => out.write_str("auto"),
            ThreadSetting::Count(n) => write!(out, "{n}"),
            ThreadSetting::AllBut(n) => write!(out, "-{n}"),
        }
    }
}

fn parse_u32(t: &str) -> Option<u32> {
    if t.is_empty() || t.len() > 9 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.bytes().try_fold(0u32, |n, b| {
        n.checked_mul(10)?.checked_add((b - b'0') as u32)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_leaves_one_cpu_and_never_goes_below_one() {
        assert_eq!(ThreadSetting::Auto.resolve(16), 15);
        assert_eq!(ThreadSetting::Auto.resolve(2), 1);
        assert_eq!(ThreadSetting::Auto.resolve(1), 1);
        assert_eq!(ThreadSetting::Auto.resolve(0), 1);
    }

    #[test]
    fn counts_clamp_to_the_machine() {
        assert_eq!(ThreadSetting::Count(4).resolve(16), 4);
        assert_eq!(ThreadSetting::Count(64).resolve(16), 16);
        assert_eq!(ThreadSetting::Count(3).resolve(1), 1);
        assert_eq!(ThreadSetting::AllBut(2).resolve(16), 14);
        assert_eq!(ThreadSetting::AllBut(20).resolve(16), 1);
        assert_eq!(ThreadSetting::AllBut(0).resolve(8), 8);
    }

    #[test]
    fn text_round_trips_and_rejects_typos() {
        assert_eq!(ThreadSetting::parse("auto"), Some(ThreadSetting::Auto));
        assert_eq!(ThreadSetting::parse(" AUTO "), Some(ThreadSetting::Auto));
        assert_eq!(ThreadSetting::parse(""), Some(ThreadSetting::Auto));
        assert_eq!(ThreadSetting::parse("0"), Some(ThreadSetting::Auto));
        assert_eq!(ThreadSetting::parse("6"), Some(ThreadSetting::Count(6)));
        assert_eq!(ThreadSetting::parse("-2"), Some(ThreadSetting::AllBut(2)));
        for bad in ["all", "4.5", "--1", "-", "1e3", "99999999999"] {
            assert_eq!(ThreadSetting::parse(bad), None, "{bad}");
        }
        for s in [
            ThreadSetting::Auto,
            ThreadSetting::Count(12),
            ThreadSetting::AllBut(3),
        ] {
            let mut t = String::new();
            s.write(&mut t).unwrap();
            assert_eq!(ThreadSetting::parse(&t), Some(s));
        }
    }
}
