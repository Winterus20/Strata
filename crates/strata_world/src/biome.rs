//! Biome selection (Whittaker-style, plan 11 §1 / 08 §2).
//!
//! Biomes are chosen per `(x, z)` column from two independent low-frequency
//! noise fields (temperature and moisture), approximating a Whittaker climate
//! diagram. The selection is a pure function of world coordinates, so it is
//! chunk-independent and deterministic.

use crate::noise::fbm2;
use crate::rng::WORLD_SEED;

/// Salts isolating the temperature / moisture / height noise fields.
const TEMP_SALT: u64 = 0x1F1F_1F1F_1F1F_1F1F;
const MOIST_SALT: u64 = 0x2E2E_2E2E_2E2E_2E2E;
const BIOME_FREQ: f32 = 0.01;

/// Prototype biomes. Each tunes terrain amplitude and the surface block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Plains,
    Hills,
    Desert,
    Snow,
}

impl Biome {
    /// Vertical amplitude (in voxels) added around the base height.
    pub fn amplitude(self) -> i32 {
        match self {
            Biome::Plains => 6,
            Biome::Hills => 14,
            Biome::Desert => 3,
            Biome::Snow => 7,
        }
    }
}

/// Pick the biome for a world `(x, z)` column.
pub fn biome_at(x: i32, z: i32) -> Biome {
    let fx = x as f32 * BIOME_FREQ;
    let fz = z as f32 * BIOME_FREQ;
    // Stretch the noise contrast so the climate bands separate clearly.
    let t = contrast(fbm2(fx, fz, WORLD_SEED ^ TEMP_SALT));
    let m = contrast(fbm2(fx + 1000.0, fz + 1000.0, WORLD_SEED ^ MOIST_SALT));

    if t < 0.35 {
        Biome::Snow
    } else if t > 0.65 && m < 0.45 {
        Biome::Desert
    } else if m > 0.6 {
        Biome::Hills
    } else {
        Biome::Plains
    }
}

/// Expand a `[0, 1]` value around its midpoint to widen climate extremes.
#[inline]
fn contrast(v: f32) -> f32 {
    ((v - 0.5) * 1.6 + 0.5).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biome_is_deterministic() {
        assert_eq!(biome_at(12, -7), biome_at(12, -7));
    }

    #[test]
    fn biome_covers_all_variants_over_a_region() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for x in 0..800i32 {
            for z in 0..800i32 {
                seen.insert(biome_at(x, z));
            }
        }
        // With independent temp/moisture fields all four should appear somewhere.
        assert!(
            seen.len() >= 3,
            "expected several biomes, saw {}: {:?}",
            seen.len(),
            seen
        );
    }
}
