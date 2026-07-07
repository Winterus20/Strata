//! Deterministic value-noise + fBm helpers (plan 11 §1 / 08 §2).
//!
//! Hand-rolled 2D/3D value noise (no external dependency) so world generation
//! stays dependency-light and 100% reproducible. All lattice hashes are derived
//! from integer coordinates + a `salt`, so the same world coordinate always maps
//! to the same noise value — a prerequisite for chunk-independent generation.

const SALT_A: u64 = 0x9E37_79B9_7F4A_7C15;
const SALT_B: u64 = 0xC2B2_AE3D_27D4_EB4F;

#[inline]
fn smoother(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Hash a 2D integer lattice point + `salt` into a u32.
#[inline]
fn hash2(x: i32, z: i32, salt: u64) -> u32 {
    let mut h = (x as u64)
        .wrapping_mul(SALT_A)
        .wrapping_add((z as u64).wrapping_mul(SALT_B))
        .wrapping_add(salt);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 32) as u32
}

/// Hash a 3D integer lattice point + `salt` into a u32.
#[inline]
fn hash3(x: i32, y: i32, z: i32, salt: u64) -> u32 {
    let mut h = (x as u64)
        .wrapping_mul(SALT_A)
        .wrapping_add((y as u64).wrapping_mul(SALT_B))
        .wrapping_add((z as u64).wrapping_mul(0x85EB_CA77_C2B2_AE63))
        .wrapping_add(salt);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 32) as u32
}

/// 2D value noise in `[0, 1]` (bilinear, smoothstep-interpolated lattice).
pub fn vnoise2(x: f32, z: f32, salt: u64) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let fx = x - x0;
    let fz = z - z0;
    let ix0 = x0 as i32;
    let iz0 = z0 as i32;
    let u = smoother(fx);
    let v = smoother(fz);

    let n00 = hash2(ix0, iz0, salt) as f32 / u32::MAX as f32;
    let n10 = hash2(ix0 + 1, iz0, salt) as f32 / u32::MAX as f32;
    let n01 = hash2(ix0, iz0 + 1, salt) as f32 / u32::MAX as f32;
    let n11 = hash2(ix0 + 1, iz0 + 1, salt) as f32 / u32::MAX as f32;

    let nx0 = n00 + (n10 - n00) * u;
    let nx1 = n01 + (n11 - n01) * u;
    nx0 + (nx1 - nx0) * v
}

/// 3D value noise in `[0, 1]` (trilinear, smoothstep-interpolated lattice).
pub fn vnoise3(x: f32, y: f32, z: f32, salt: u64) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let z0 = z.floor();
    let fx = x - x0;
    let fy = y - y0;
    let fz = z - z0;
    let ix0 = x0 as i32;
    let iy0 = y0 as i32;
    let iz0 = z0 as i32;
    let u = smoother(fx);
    let w = smoother(fy);
    let v = smoother(fz);

    let c000 = hash3(ix0, iy0, iz0, salt) as f32 / u32::MAX as f32;
    let c100 = hash3(ix0 + 1, iy0, iz0, salt) as f32 / u32::MAX as f32;
    let c010 = hash3(ix0, iy0 + 1, iz0, salt) as f32 / u32::MAX as f32;
    let c110 = hash3(ix0 + 1, iy0 + 1, iz0, salt) as f32 / u32::MAX as f32;
    let c001 = hash3(ix0, iy0, iz0 + 1, salt) as f32 / u32::MAX as f32;
    let c101 = hash3(ix0 + 1, iy0, iz0 + 1, salt) as f32 / u32::MAX as f32;
    let c011 = hash3(ix0, iy0 + 1, iz0 + 1, salt) as f32 / u32::MAX as f32;
    let c111 = hash3(ix0 + 1, iy0 + 1, iz0 + 1, salt) as f32 / u32::MAX as f32;

    let x00 = c000 + (c100 - c000) * u;
    let x10 = c010 + (c110 - c010) * u;
    let x01 = c001 + (c101 - c001) * u;
    let x11 = c011 + (c111 - c011) * u;
    let y0l = x00 + (x10 - x00) * w;
    let y1l = x01 + (x11 - x01) * w;
    y0l + (y1l - y0l) * v
}

/// 2D fractal Brownian motion in `[0, 1]` (4 octaves).
pub fn fbm2(x: f32, z: f32, salt: u64) -> f32 {
    let mut amp = 1.0f32;
    let mut freq = 1.0f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    for o in 0..4u64 {
        sum += amp
            * vnoise2(
                x * freq,
                z * freq,
                salt.wrapping_add(o.wrapping_mul(SALT_A)),
            );
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

/// 3D fractal Brownian motion in `[0, 1]` (4 octaves).
pub fn fbm3(x: f32, y: f32, z: f32, salt: u64) -> f32 {
    let mut amp = 1.0f32;
    let mut freq = 1.0f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    for o in 0..4u64 {
        sum += amp
            * vnoise3(
                x * freq,
                y * freq,
                z * freq,
                salt.wrapping_add(o.wrapping_mul(SALT_B)),
            );
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}
