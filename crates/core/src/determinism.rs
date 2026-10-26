//! Determinism guard rails (issue #67, ADR 0036).
//!
//! Trino's sim runs the same on the PC (x86-64 SSE, AArch64), the N64
//! (VR4300) and the 3DS (ARM11 VFP) because every basic `f32` operation is
//! IEEE 754 on all of them, Rust never fuses or reorders float math, and
//! the engine's own `sin`/`cos`/`sqrt`/`atan2` ([`crate::math3d`]) replace
//! the platforms' libm. What still differs between those CPUs:
//!
//! - **Subnormals.** libdragon runs the VR4300 with flush-to-zero
//!   (`FCR31.FS`) and the PC keeps gradual underflow, so a value below
//!   `f32::MIN_POSITIVE` (about 1.2e-38) is zero on the N64 and not on the
//!   PC. (The `replay` self-test step logs each target's float control
//!   word.)
//! - **NaN bits.** A NaN's payload and sign depend on the CPU (x86 makes
//!   `0xffc00000`, ARM's default NaN is `0x7fc00000`, MIPS has its own),
//!   so hashing a NaN's bits differs even when the code is the same; and an
//!   N64 build with the invalid-operation exception enabled stops on one.
//! - **Anything outside the engine's math:** a platform libm (`f32::sin`
//!   is std-only, so `no_std` game code can't reach it by accident), `f64`
//!   code paths on one target only, or input the sim reads from the
//!   platform rather than from [`InputState`](crate::InputState).
//!
//! The `strict-determinism` feature of `trino-core` turns these into
//! errors where they can be caught: [`check_f32`] (which
//! [`StateHasher::f32`](crate::StateHasher::f32) calls on every float it
//! hashes) and [`checked`] panic on a NaN or a subnormal, naming the value,
//! so a desync is found where it starts on the PC instead of thousands of
//! frames later in ares. Without the feature they compile to nothing. A
//! fixed-point sim (`trino_core::physics`'s `Fx`, Solid Eleven) never meets
//! either.

/// Is the `strict-determinism` feature on?
pub const STRICT: bool = cfg!(feature = "strict-determinism");

/// Would `v` behave the same on every target? (Not a NaN, not subnormal.)
#[inline]
pub fn is_portable(v: f32) -> bool {
    !v.is_nan() && !v.is_subnormal()
}

/// Under `strict-determinism`, panic on a value whose bits or arithmetic
/// differ between targets (a NaN, a subnormal). A no-op otherwise.
#[inline]
pub fn check_f32(v: f32) {
    if STRICT && !is_portable(v) {
        fail(None, v);
    }
}

/// [`check_f32`] with a name for the panic message, passing `v` through:
/// `let speed = checked("player speed", speed);`.
#[inline]
pub fn checked(name: &'static str, v: f32) -> f32 {
    if STRICT && !is_portable(v) {
        fail(Some(name), v);
    }
    v
}

#[cold]
#[inline(never)]
fn fail(name: Option<&'static str>, v: f32) -> ! {
    let what = if v.is_nan() { "a NaN" } else { "a subnormal" };
    match name {
        Some(n) => panic!(
            "strict-determinism: {n} is {what} ({:#010x}); its bits or arithmetic differ between the PC, the N64 and the 3DS",
            v.to_bits()
        ),
        None => panic!(
            "strict-determinism: a hashed float is {what} ({:#010x}); its bits or arithmetic differ between the PC, the N64 and the 3DS",
            v.to_bits()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_values_are_normal_numbers_and_zero() {
        for v in [0.0, -0.0, 1.0, f32::MIN_POSITIVE, f32::MAX, f32::INFINITY] {
            assert!(is_portable(v), "{v}");
        }
        assert!(!is_portable(f32::NAN));
        assert!(!is_portable(f32::MIN_POSITIVE / 2.0));
        assert_eq!(checked("x", 2.5), 2.5);
    }

    #[cfg(feature = "strict-determinism")]
    #[test]
    #[should_panic(expected = "strict-determinism: speed is a subnormal")]
    fn strict_mode_names_a_subnormal() {
        checked("speed", f32::MIN_POSITIVE / 4.0);
    }

    #[cfg(feature = "strict-determinism")]
    #[test]
    #[should_panic(expected = "a hashed float is a NaN")]
    fn strict_mode_stops_a_nan_in_a_hash() {
        crate::StateHasher::new().f32(f32::NAN);
    }

    #[cfg(not(feature = "strict-determinism"))]
    #[test]
    fn without_the_feature_nothing_panics() {
        check_f32(f32::NAN);
        crate::StateHasher::new().f32(f32::MIN_POSITIVE / 4.0);
    }
}
