//! Deterministic RNG for world generation (plan 11 §1 / 08 §1).
//!
//! `Pcg32` is the standard PCG32 permutation (cheap, well-distributed, 64-bit
//! state) and `wyhash` is used as a finalizer/hash to derive a stable per-column
//! or per-sector seed from coordinates. Generation is therefore fully
//! reproducible and chunk-independent: the same seed + `SectorCoord` always
//! yields byte-identical output.

/// Global world seed. Constant so every build generates the same world.
pub const WORLD_SEED: u64 = 0x243F_6A88_85A3_08D3;

/// Standard PCG32 (permuted linear congruential generator).
#[derive(Clone, Copy, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    /// Build a PCG32 from `seed` and a `stream` selector. The stream lets
    /// different subsystems (e.g. trees vs. caves) draw independent sequences
    /// from the same base seed.
    pub fn new(seed: u64, stream: u64) -> Self {
        let inc = (stream << 1) | 1;
        let mut p = Pcg32 { state: 0, inc };
        // Standard PCG init: step once, mix the seed in, step again.
        let _ = p.next_u32();
        p.state = p.state.wrapping_add(seed);
        let _ = p.next_u32();
        p
    }

    #[inline]
    fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
    }

    /// Next 32-bit output (the core PCG output function).
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = ((old >> 59) as u32) & 31;
        let shift = ((-(rot as i32)) as u32) & 31;
        (xorshifted >> rot) | (xorshifted << shift)
    }

    /// Next float in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// wyhash finalizer (wangxor-style mix). Used as a deterministic hash to fold
/// coordinate values into a seed; `x` is mixed against `seed`.
#[inline]
pub fn wyhash(x: u64, seed: u64) -> u64 {
    const P0: u64 = 0xa0_76_1d_64_78_bd_64_2f;
    let mut r = x.wrapping_add(seed);
    r = (r ^ (r >> 30)).wrapping_mul(P0);
    r = (r ^ (r >> 27)).wrapping_mul(P0);
    r ^ (r >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcg32_is_deterministic_and_in_range() {
        let mut a = Pcg32::new(12345, 7);
        let mut b = Pcg32::new(12345, 7);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
            let fa = a.next_f32();
            let fb = b.next_f32();
            assert!((0.0..1.0).contains(&fa));
            assert_eq!(fa, fb);
        }
    }

    #[test]
    fn pcg32_streams_are_distinct() {
        let mut a = Pcg32::new(12345, 1);
        let mut b = Pcg32::new(12345, 2);
        assert_ne!(a.next_u32(), b.next_u32());
    }

    #[test]
    fn wyhash_is_stable() {
        assert_eq!(wyhash(42, WORLD_SEED), wyhash(42, WORLD_SEED));
        assert_ne!(wyhash(1, WORLD_SEED), wyhash(2, WORLD_SEED));
    }
}
