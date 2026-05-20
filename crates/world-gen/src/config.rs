/// Sea level in block Y coordinates.
pub const SEA_LEVEL: i32 = 63;

/// Maximum world height.
pub const MAX_HEIGHT: i32 = 255;

/// Minimum world height (bedrock layer).
pub const MIN_HEIGHT: i32 = 1;

/// Continentalness spline control points: (continentalness, base_height)
/// Models the transition from deep ocean → ocean → coast → plains → hills → mountains.
/// Sharp rise at -0.3..-0.1 creates natural coastlines.
pub const CONTINENTAL_SPLINE: [(f32, f32); 10] = [
    (-1.0, 8.0),  // Deep ocean floor
    (-0.4, 8.0),  // Ocean floor (flat)
    (-0.3, 55.0), // Coastline — sharp rise creates beaches
    (-0.1, 66.0), // Shore / low plains
    (0.0, 70.0),  // Plains baseline
    (0.2, 82.0),  // Rolling hills
    (0.4, 105.0), // Mountain foothills
    (0.6, 135.0), // Mountains
    (0.8, 165.0), // High peaks
    (1.0, 195.0), // Extreme peaks
];

/// Erosion offset spline control points: (erosion, height_offset)
/// Low erosion → flat plateaus and plains.
/// High erosion → carved valleys, canyons, and jagged terrain.
pub const EROSION_SPLINE: [(f32, f32); 10] = [
    (-1.0, 15.0), // Flat plateaus
    (-0.7, 15.0),
    (-0.7, 5.0),
    (-0.3, 5.0),
    (-0.3, 0.0),
    (0.2, 0.0),
    (0.2, -5.0),
    (0.6, -5.0),
    (0.6, -15.0),
    (1.0, -15.0), // Deep valleys / canyons
];

/// Weirdness (peaks/valleys) offset spline: (weirdness, height_offset)
/// Controls dramatic terrain features — ridges, cliffs, and saddle valleys.
pub const WEIRDNESS_SPLINE: [(f32, f32); 7] = [
    (-1.0, -8.0), // Valleys
    (-0.6, -3.0),
    (-0.2, 0.0), // Neutral
    (0.0, 0.0),
    (0.3, 5.0),  // Rising ridges
    (0.7, 12.0), // Sharp peaks
    (1.0, 18.0), // Extreme peaks
];

/// Cheese cave threshold (base).
pub const CHEESE_CAVE_THRESHOLD: f32 = 0.4;

/// Spaghetti cave threshold band.
pub const SPAGHETTI_CAVE_LOWER: f32 = 0.35;
pub const SPAGHETTI_CAVE_UPPER: f32 = 0.45;

/// Height bias multiplier for 3D density.
/// Controls how strongly Y position affects density — lower = taller terrain variation.
/// Minecraft uses ~0.004; we use 0.006 for slightly more dramatic features.
pub const HEIGHT_BIAS_MULTIPLIER: f32 = 0.006;

/// 3D detail noise amplitude.
/// Lower = smoother terrain surface. Minecraft uses ~0.5.
pub const DETAIL_AMPLITUDE: f32 = 0.6;

/// Maximum Y extent for cave carving.
pub const CAVE_Y_MAX: i32 = SEA_LEVEL - 1;

/// Heightmap bounding padding (extra Y range above/below heightmap).
pub const HEIGHTMAP_PADDING: usize = 5;

/// Default view distance in chunks.
pub const DEFAULT_VIEW_DISTANCE: u32 = 8;

/// Default load distance (view + buffer).
pub const DEFAULT_LOAD_DISTANCE: u32 = 10;

// ── Noodle Caves ──────────────────────────────────────────────────────

/// Noodle cave threshold band (lower bound).
pub const NOODLE_CAVE_LOWER: f32 = 0.40;

/// Noodle cave threshold band (upper bound).
pub const NOODLE_CAVE_UPPER: f32 = 0.45;

/// Minimum Y depth for noodle caves (below sea level).
pub const NOODLE_CAVE_Y_OFFSET: i32 = 20;

// ── Domain Warp ────────────────────────────────────────────────────────

/// Domain warp amplitude in blocks.
/// Controls how much terrain coordinates are displaced by warp noise.
/// Minecraft uses ~15-20. Too high (80+) causes chaotic, disconnected terrain.
pub const DOMAIN_WARP_AMPLITUDE: f32 = 18.0;

/// Whether domain warping is enabled (Faz 3 feature).
pub const DOMAIN_WARP_ENABLED: bool = true;

// ── Aquifer (Faz 4 — Full Minecraft Style) ────────────────────────────

/// Aquifer noise threshold below which caves are empty.
pub const AQUIFER_EMPTY_THRESHOLD: f32 = 0.35;

/// Aquifer noise threshold above which caves are flooded.
pub const AQUIFER_FLOODED_THRESHOLD: f32 = 0.75;

/// Aquifer chunk cell width (chunks).
pub const AQUIFER_CELL_CHUNKS: i32 = 2;

/// Lava level (below this Y, aquifers produce lava instead of water).
pub const AQUIFER_LAVA_LEVEL: i32 = -55;

/// Lava pocket density threshold (higher = fewer lava pockets).
pub const AQUIFER_LAVA_DENSITY: f32 = 0.6;

/// Lava pocket noise scale multiplier.
pub const AQUIFER_POCKET_SCALE: f32 = 1.5;

/// Local water level variation range in blocks.
pub const AQUIFER_LOCAL_VARIATION: f32 = 24.0;

/// Aquifer barrier noise threshold (above this = barrier exists).
pub const AQUIFER_BARRIER_THRESHOLD: f32 = 0.7;

/// Aquifer barrier height scale.
pub const AQUIFER_BARRIER_SCALE: f32 = 20.0;

/// Aquifer noise frequency (separate from cave noise).
pub const AQUIFER_NOISE_FREQ: f32 = 0.003;

// ── Carver ──────────────────────────────────────────────────────────────

/// Probability of a ravine per chunk (0.0 - 1.0).
pub const CARVER_RAVINE_PROBABILITY: f32 = 0.02;

/// Minimum ravine length in blocks.
pub const CARVER_RAVINE_MIN_LENGTH: f32 = 30.0;

/// Maximum ravine length in blocks.
pub const CARVER_RAVINE_MAX_LENGTH: f32 = 80.0;

/// Minimum ravine width in blocks.
pub const CARVER_RAVINE_MIN_WIDTH: f32 = 2.0;

/// Maximum ravine width in blocks.
pub const CARVER_RAVINE_MAX_WIDTH: f32 = 5.0;

// ── Noise Cache ────────────────────────────────────────────────────────

/// Maximum number of cached noise regions (LRU eviction).
pub const NOISE_CACHE_MAX_REGIONS: usize = 32;

/// Size of a cached noise region in chunks (3 = 3x3).
pub const NOISE_CACHE_REGION_SIZE: i32 = 3;

// ── Multi-Resolution Noise (Faz 4 LOD) ─────────────────────────────────

/// Number of LOD levels for noise detail.
pub const LOD_LEVELS: usize = 4;

/// Base frequency for LOD 0 (highest detail).
pub const LOD_BASE_FREQ: f32 = 0.05;

/// Frequency multiplier per LOD level.
pub const LOD_FREQ_MULTIPLIER: f32 = 0.5;

/// Block size at each LOD level (blend between levels).
pub const LOD_BLOCK_SIZES: [i32; 4] = [1, 2, 4, 8];

/// Maximum distance (in chunks) for each LOD level.
pub const LOD_DISTANCES: [i32; 4] = [8, 24, 48, 96];

// ── Structure Placement (Faz 4) ────────────────────────────────────────

/// Village placement: minimum chunks between villages.
pub const VILLAGE_SPACING: i32 = 32;

/// Dungeon placement: minimum chunks between dungeons.
pub const DUNGEON_SPACING: i32 = 16;

/// Ruin placement: minimum chunks between ruins.
pub const RUIN_SPACING: i32 = 20;

/// Swamp hut placement: minimum chunks between swamp huts.
pub const SWAMP_HUT_SPACING: i32 = 24;

/// Maximum number of structures per chunk.
pub const MAX_STRUCTURES_PER_CHUNK: usize = 3;

// ── GPU Compute Terrain (Faz 4 Opsiyonel) ──────────────────────────────

/// Whether GPU compute terrain generation is enabled.
pub const GPU_TERRAIN_ENABLED: bool = false;

/// Workgroup size for GPU compute terrain shader.
pub const GPU_TERRAIN_WORKGROUP_SIZE: u32 = 256;

/// Maximum chunks per GPU dispatch.
pub const GPU_TERRAIN_MAX_CHUNKS_PER_DISPATCH: u32 = 64;
