//! The engine's fixed-point physics against the sim's own `Fx` (issue #66),
//! outside `sim/` (which stays free of `trino_core`).

/// Issue #66 phase 4: the engine's fixed-point mode (`trino_core::physics::
/// Fx`, the character controller's `Scalar`) is this type's twin — same
/// representation, same rounding, same saturation — so the sim could take
/// the engine's controller and queries in fixed point without its match
/// hash moving. `docs/SOLID_ELEVEN.md` › Physics records the evaluation.
#[cfg(test)]
mod engine_fixed_point {
    use crate::sim::fx::Fx;
    use trino_core::physics::{Fx as EngineFx, Scalar};

    #[test]
    fn the_engines_fixed_point_matches_bit_for_bit() {
        let mut r = 0x2545_f491u32;
        let mut next = || {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            // Pitch-sized values (±200 m) and small fractions, both signs.
            let v = (r as i32) >> (r % 13 + 6);
            if r & 1 == 0 { v } else { -v }
        };
        for _ in 0..20_000 {
            let (a, b) = (next(), next());
            let (x, y) = (Fx(a), Fx(b));
            let (ex, ey) = (EngineFx(a), EngineFx(b));
            assert_eq!(x.mul(y).0, (ex * ey).0, "{a} * {b}");
            assert_eq!(x.div(y).0, (ex / ey).0, "{a} / {b}");
            assert_eq!((x + y).0, (ex + ey).0);
            assert_eq!((x - y).0, (ex - ey).0);
            assert_eq!(x.sqrt().0, Scalar::sqrt(ex).0, "sqrt {a}");
            assert_eq!(x.min(y).0, Scalar::min(ex, ey).0);
            let f = a as f32 / 65536.0;
            assert_eq!(Fx::from_f32(f).0, EngineFx::from_f32(f).0, "from_f32 {f}");
        }
        // Division by zero saturates the same way on both sides.
        assert_eq!(Fx::ONE.div(Fx::ZERO).0, (EngineFx::ONE / EngineFx::ZERO).0);
        assert_eq!(Fx(-5).div(Fx::ZERO).0, (EngineFx(-5) / EngineFx::ZERO).0);
    }
}
