use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use strata_core::BlockId;

use crate::noise::TerrainNoise;

/// 5D noise parameter space for biome selection.
#[derive(Debug, Clone, Copy)]
pub struct NoiseParams {
    pub continentalness: f32,
    pub erosion: f32,
    pub weirdness: f32,
    pub temperature: f32,
    pub humidity: f32,
}

impl NoiseParams {
    #[inline]
    pub fn from_noise(noise: &TerrainNoise, x: i32, z: i32) -> Self {
        Self {
            continentalness: noise.continental(x, z),
            erosion: noise.erosion(x, z),
            weirdness: noise.weirdness(x, z),
            temperature: noise.temperature(x, z),
            humidity: noise.humidity(x, z),
        }
    }

    #[inline]
    pub fn from_grid(
        continental_vals: &[f32],
        erosion_vals: &[f32],
        weirdness_vals: &[f32],
        temperature_vals: &[f32],
        humidity_vals: &[f32],
        col: usize,
    ) -> Self {
        Self {
            continentalness: continental_vals[col],
            erosion: erosion_vals[col],
            weirdness: weirdness_vals[col],
            temperature: temperature_vals[col],
            humidity: humidity_vals[col],
        }
    }
}

/// Tree type for feature placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TreeType {
    None,
    Oak,
    Birch,
    Pine,
    Jungle,
    Acacia,
    DarkOak,
    Cactus,
}

impl TreeType {
    #[inline]
    pub fn is_tree(&self) -> bool {
        !matches!(self, TreeType::None)
    }
}

/// A biome definition with surface block palette and terrain modifiers.
///
/// Faz 4: Data-driven — can be serialized/deserialized from JSON/TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Biome {
    pub id: u16,
    pub name: String,

    // Surface block palette
    pub top_block: String,
    pub filler_block: String,
    pub filler_depth: u8,
    pub ocean_block: Option<String>,

    // Terrain
    pub base_height_offset: f32,
    pub height_variation: f32,
    pub erosion_resistance: f32,
    pub tree_density: f32,
    pub tree_type: TreeType,
    pub cave_density: f32,
    pub cave_modifier: f32,

    // 5D selection hyper-rectangle
    pub temperature_range: (f32, f32),
    pub humidity_range: (f32, f32),
    pub continentalness_range: (f32, f32),
    pub erosion_range: (f32, f32),
    pub weirdness_range: (f32, f32),
}

impl Biome {
    /// Resolve block name strings to BlockIds using a lookup function.
    pub fn resolve_blocks<F: Fn(&str) -> BlockId>(&self, resolver: F) -> ResolvedBiome<'_> {
        ResolvedBiome {
            id: self.id,
            name: &self.name,
            top_block: resolver(&self.top_block),
            filler_block: resolver(&self.filler_block),
            filler_depth: self.filler_depth,
            ocean_block: self.ocean_block.as_ref().map(|s| resolver(s)),
            base_height_offset: self.base_height_offset,
            height_variation: self.height_variation,
            erosion_resistance: self.erosion_resistance,
            tree_density: self.tree_density,
            tree_type: self.tree_type,
            cave_density: self.cave_density,
            cave_modifier: self.cave_modifier,
            temperature_range: self.temperature_range,
            humidity_range: self.humidity_range,
            continentalness_range: self.continentalness_range,
            erosion_range: self.erosion_range,
            weirdness_range: self.weirdness_range,
        }
    }
}

/// Resolved biome with BlockId values (runtime-optimized).
#[derive(Debug, Clone)]
pub struct ResolvedBiome<'a> {
    pub id: u16,
    pub name: &'a str,
    pub top_block: BlockId,
    pub filler_block: BlockId,
    pub filler_depth: u8,
    pub ocean_block: Option<BlockId>,
    pub base_height_offset: f32,
    pub height_variation: f32,
    pub erosion_resistance: f32,
    pub tree_density: f32,
    pub tree_type: TreeType,
    pub cave_density: f32,
    pub cave_modifier: f32,
    pub temperature_range: (f32, f32),
    pub humidity_range: (f32, f32),
    pub continentalness_range: (f32, f32),
    pub erosion_range: (f32, f32),
    pub weirdness_range: (f32, f32),
}

impl<'a> ResolvedBiome<'a> {
    #[inline]
    fn is_surface(&self) -> bool {
        self.id < 100
    }

    #[inline]
    fn contains(&self, params: &NoiseParams) -> bool {
        params.temperature >= self.temperature_range.0
            && params.temperature <= self.temperature_range.1
            && params.humidity >= self.humidity_range.0
            && params.humidity <= self.humidity_range.1
            && params.continentalness >= self.continentalness_range.0
            && params.continentalness <= self.continentalness_range.1
            && params.erosion >= self.erosion_range.0
            && params.erosion <= self.erosion_range.1
            && params.weirdness >= self.weirdness_range.0
            && params.weirdness <= self.weirdness_range.1
    }
}

/// Data-driven biome registry with serialization support.
///
/// Faz 4: Biomes can be loaded from JSON/TOML files at startup,
/// enabling modding and data-driven configuration.
#[derive(Debug, Clone)]
pub struct BiomeRegistry {
    biomes: Vec<SerializableBiome>,
}

/// Serializable biome variant for data-driven loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableBiome {
    pub id: u16,
    pub name: String,
    pub top_block: String,
    pub filler_block: String,
    pub filler_depth: u8,
    pub ocean_block: Option<String>,
    pub base_height_offset: f32,
    pub height_variation: f32,
    pub erosion_resistance: f32,
    pub tree_density: f32,
    pub tree_type: TreeType,
    pub cave_density: f32,
    pub cave_modifier: f32,
    pub temperature_range: [f32; 2],
    pub humidity_range: [f32; 2],
    pub continentalness_range: [f32; 2],
    pub erosion_range: [f32; 2],
    pub weirdness_range: [f32; 2],
}

/// Container for JSON-serializable biome list.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BiomeList {
    pub biomes: Vec<SerializableBiome>,
}

impl Default for BiomeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn block_id_from_name(name: &str) -> BlockId {
    match name {
        "air" => BlockId::AIR,
        "stone" => BlockId::STONE,
        "dirt" => BlockId::DIRT,
        "grass" => BlockId::GRASS,
        "bedrock" => BlockId::BEDROCK,
        "wood" => BlockId::WOOD,
        "leaves" => BlockId::LEAVES,
        "sand" => BlockId::SAND,
        "gravel" => BlockId::GRAVEL,
        "water" => BlockId::WATER,
        "snow" => BlockId::SNOW,
        _ => BlockId::STONE,
    }
}

fn sblock(name: &str) -> String {
    name.to_string()
}

fn sob(name: Option<&str>) -> Option<String> {
    name.map(|s| s.to_string())
}

impl BiomeRegistry {
    /// Returns 20 default biomes for Faz 4.
    pub fn new() -> Self {
        Self::from_serialized(Self::default_biomes())
    }

    /// Load biomes from a JSON file.
    pub fn load_json(path: impl AsRef<Path>) -> Result<Self, String> {
        let data = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read biome file: {e}"))?;
        let list: BiomeList =
            serde_json::from_str(&data).map_err(|e| format!("Failed to parse biome JSON: {e}"))?;
        Ok(Self::from_serialized(list.biomes))
    }

    /// Load biomes from a TOML file.
    pub fn load_toml(path: impl AsRef<Path>) -> Result<Self, String> {
        let data = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read biome file: {e}"))?;
        let list: BiomeList =
            toml::from_str(&data).map_err(|e| format!("Failed to parse biome TOML: {e}"))?;
        Ok(Self::from_serialized(list.biomes))
    }

    fn from_serialized(biomes: Vec<SerializableBiome>) -> Self {
        Self { biomes }
    }

    /// Convert runtime biomes back to serializable form for export.
    pub fn resolve_all(&self) -> Vec<ResolvedBiome<'_>> {
        self.biomes
            .iter()
            .map(|b| ResolvedBiome {
                id: b.id,
                name: &b.name,
                top_block: block_id_from_name(&b.top_block),
                filler_block: block_id_from_name(&b.filler_block),
                filler_depth: b.filler_depth,
                ocean_block: b.ocean_block.as_deref().map(block_id_from_name),
                base_height_offset: b.base_height_offset,
                height_variation: b.height_variation,
                erosion_resistance: b.erosion_resistance,
                tree_density: b.tree_density,
                tree_type: b.tree_type,
                cave_density: b.cave_density,
                cave_modifier: b.cave_modifier,
                temperature_range: (b.temperature_range[0], b.temperature_range[1]),
                humidity_range: (b.humidity_range[0], b.humidity_range[1]),
                continentalness_range: (b.continentalness_range[0], b.continentalness_range[1]),
                erosion_range: (b.erosion_range[0], b.erosion_range[1]),
                weirdness_range: (b.weirdness_range[0], b.weirdness_range[1]),
            })
            .collect()
    }

    /// Selects a biome using 5D hypercube containment first, then
    /// nearest-neighbor fallback.
    pub fn select(&self, params: &NoiseParams) -> ResolvedBiome<'_> {
        let resolved = self.resolve_all();

        // First pass: exact hyper-rectangle containment
        for biome in &resolved {
            if biome.is_surface() && biome.contains(params) {
                return biome.clone();
            }
        }

        // Fallback: nearest neighbor in 5D space
        let mut best_dist = f32::MAX;
        let mut best_idx = 0;
        for (i, biome) in resolved.iter().enumerate() {
            if !biome.is_surface() {
                continue;
            }
            let dist = hypercube_distance(params, biome);
            if dist < best_dist {
                best_dist = dist;
                best_idx = i;
            }
        }
        resolved[best_idx].clone()
    }

    /// Returns the biome for cave/underground contexts.
    pub fn select_cave(&self, params: &NoiseParams, _depth: f32) -> ResolvedBiome<'_> {
        self.select(params)
    }

    pub fn all(&self) -> &[SerializableBiome] {
        &self.biomes
    }

    /// Register a biome dynamically (Faz 4 data-driven).
    pub fn register(&mut self, biome: SerializableBiome) {
        self.biomes.push(biome);
    }

    /// Export current biomes to JSON string.
    pub fn export_json(&self) -> Result<String, String> {
        let list = BiomeList {
            biomes: self.biomes.clone(),
        };
        serde_json::to_string_pretty(&list).map_err(|e| format!("JSON export failed: {e}"))
    }

    fn default_biomes() -> Vec<SerializableBiome> {
        vec![
            // ── Oceanic ──
            SerializableBiome {
                id: 0,
                name: sblock("deep_ocean"),
                top_block: sblock("gravel"),
                filler_block: sblock("gravel"),
                filler_depth: 3,
                ocean_block: sob(Some("water")),
                base_height_offset: -35.0,
                height_variation: 5.0,
                erosion_resistance: 0.0,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.0,
                cave_modifier: 0.0,
                temperature_range: [-1.0, 1.0],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [-1.0, -0.4],
                erosion_range: [-1.0, 1.0],
                weirdness_range: [-1.0, 1.0],
            },
            SerializableBiome {
                id: 1,
                name: sblock("ocean"),
                top_block: sblock("sand"),
                filler_block: sblock("sand"),
                filler_depth: 4,
                ocean_block: sob(Some("water")),
                base_height_offset: -20.0,
                height_variation: 5.0,
                erosion_resistance: 0.0,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.0,
                cave_modifier: 0.0,
                temperature_range: [-1.0, 1.0],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [-0.4, -0.2],
                erosion_range: [-1.0, 1.0],
                weirdness_range: [-1.0, 1.0],
            },
            SerializableBiome {
                id: 2,
                name: sblock("warm_ocean"),
                top_block: sblock("sand"),
                filler_block: sblock("sand"),
                filler_depth: 3,
                ocean_block: sob(Some("water")),
                base_height_offset: -18.0,
                height_variation: 4.0,
                erosion_resistance: 0.0,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.0,
                cave_modifier: 0.0,
                temperature_range: [0.2, 1.0],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [-0.4, -0.2],
                erosion_range: [-1.0, 1.0],
                weirdness_range: [-1.0, 1.0],
            },
            // ── Coastal ──
            SerializableBiome {
                id: 3,
                name: sblock("beach"),
                top_block: sblock("sand"),
                filler_block: sblock("sand"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: -2.0,
                height_variation: 2.0,
                erosion_resistance: 0.0,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.3,
                cave_modifier: 0.5,
                temperature_range: [-1.0, 1.0],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [-0.2, -0.05],
                erosion_range: [-1.0, 1.0],
                weirdness_range: [-1.0, 1.0],
            },
            SerializableBiome {
                id: 4,
                name: sblock("snowy_beach"),
                top_block: sblock("snow"),
                filler_block: sblock("sand"),
                filler_depth: 2,
                ocean_block: None,
                base_height_offset: -2.0,
                height_variation: 2.0,
                erosion_resistance: 0.0,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.3,
                cave_modifier: 0.5,
                temperature_range: [-1.0, -0.3],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [-0.2, -0.05],
                erosion_range: [-1.0, 1.0],
                weirdness_range: [-1.0, 1.0],
            },
            // ── Lowlands ──
            SerializableBiome {
                id: 5,
                name: sblock("plains"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 0.0,
                height_variation: 8.0,
                erosion_resistance: 0.3,
                tree_density: 0.02,
                tree_type: TreeType::Oak,
                cave_density: 0.8,
                cave_modifier: 1.0,
                temperature_range: [-0.3, 0.5],
                humidity_range: [-0.5, 0.5],
                continentalness_range: [-0.05, 0.3],
                erosion_range: [0.2, 1.0],
                weirdness_range: [-0.3, 0.3],
            },
            SerializableBiome {
                id: 6,
                name: sblock("sunflower_plains"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 0.0,
                height_variation: 7.0,
                erosion_resistance: 0.3,
                tree_density: 0.01,
                tree_type: TreeType::Oak,
                cave_density: 0.8,
                cave_modifier: 1.0,
                temperature_range: [0.2, 0.6],
                humidity_range: [-0.2, 0.3],
                continentalness_range: [-0.05, 0.2],
                erosion_range: [0.3, 1.0],
                weirdness_range: [-0.2, 0.2],
            },
            // ── Forests ──
            SerializableBiome {
                id: 7,
                name: sblock("forest"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 2.0,
                height_variation: 12.0,
                erosion_resistance: 0.5,
                tree_density: 0.08,
                tree_type: TreeType::Oak,
                cave_density: 0.8,
                cave_modifier: 1.0,
                temperature_range: [-0.2, 0.5],
                humidity_range: [0.1, 0.8],
                continentalness_range: [-0.05, 0.3],
                erosion_range: [-0.3, 0.4],
                weirdness_range: [-0.3, 0.3],
            },
            SerializableBiome {
                id: 8,
                name: sblock("birch_forest"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 2.0,
                height_variation: 10.0,
                erosion_resistance: 0.5,
                tree_density: 0.09,
                tree_type: TreeType::Birch,
                cave_density: 0.8,
                cave_modifier: 1.0,
                temperature_range: [-0.2, 0.4],
                humidity_range: [0.2, 0.7],
                continentalness_range: [-0.05, 0.3],
                erosion_range: [-0.2, 0.3],
                weirdness_range: [-0.2, 0.2],
            },
            SerializableBiome {
                id: 9,
                name: sblock("dark_forest"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 2.0,
                height_variation: 11.0,
                erosion_resistance: 0.5,
                tree_density: 0.15,
                tree_type: TreeType::DarkOak,
                cave_density: 0.7,
                cave_modifier: 0.8,
                temperature_range: [-0.2, 0.4],
                humidity_range: [0.5, 1.0],
                continentalness_range: [-0.05, 0.3],
                erosion_range: [-0.4, 0.2],
                weirdness_range: [-0.3, 0.3],
            },
            // ── Dry ──
            SerializableBiome {
                id: 10,
                name: sblock("desert"),
                top_block: sblock("sand"),
                filler_block: sblock("sand"),
                filler_depth: 5,
                ocean_block: None,
                base_height_offset: 0.0,
                height_variation: 6.0,
                erosion_resistance: 0.6,
                tree_density: 0.0,
                tree_type: TreeType::Cactus,
                cave_density: 0.6,
                cave_modifier: 0.7,
                temperature_range: [0.5, 1.0],
                humidity_range: [-1.0, -0.3],
                continentalness_range: [0.0, 0.5],
                erosion_range: [0.3, 1.0],
                weirdness_range: [-0.2, 0.3],
            },
            SerializableBiome {
                id: 11,
                name: sblock("savanna"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 2,
                ocean_block: None,
                base_height_offset: 2.0,
                height_variation: 8.0,
                erosion_resistance: 0.4,
                tree_density: 0.03,
                tree_type: TreeType::Acacia,
                cave_density: 0.7,
                cave_modifier: 0.9,
                temperature_range: [0.5, 1.0],
                humidity_range: [-0.3, 0.2],
                continentalness_range: [0.0, 0.4],
                erosion_range: [0.1, 0.7],
                weirdness_range: [-0.2, 0.2],
            },
            // ── Cold ──
            SerializableBiome {
                id: 12,
                name: sblock("taiga"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 0.0,
                height_variation: 15.0,
                erosion_resistance: 0.4,
                tree_density: 0.06,
                tree_type: TreeType::Pine,
                cave_density: 0.9,
                cave_modifier: 1.1,
                temperature_range: [-0.5, -0.1],
                humidity_range: [0.2, 0.8],
                continentalness_range: [0.0, 0.3],
                erosion_range: [-0.3, 0.5],
                weirdness_range: [-0.3, 0.3],
            },
            SerializableBiome {
                id: 13,
                name: sblock("snowy_taiga"),
                top_block: sblock("snow"),
                filler_block: sblock("dirt"),
                filler_depth: 2,
                ocean_block: None,
                base_height_offset: 0.0,
                height_variation: 14.0,
                erosion_resistance: 0.4,
                tree_density: 0.04,
                tree_type: TreeType::Pine,
                cave_density: 0.9,
                cave_modifier: 1.1,
                temperature_range: [-1.0, -0.4],
                humidity_range: [0.3, 0.9],
                continentalness_range: [0.0, 0.3],
                erosion_range: [-0.3, 0.5],
                weirdness_range: [-0.3, 0.3],
            },
            // ── Highlands ──
            SerializableBiome {
                id: 14,
                name: sblock("hills"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 2,
                ocean_block: None,
                base_height_offset: 20.0,
                height_variation: 25.0,
                erosion_resistance: 0.7,
                tree_density: 0.03,
                tree_type: TreeType::Oak,
                cave_density: 1.0,
                cave_modifier: 1.2,
                temperature_range: [-0.3, 1.0],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [0.3, 0.6],
                erosion_range: [-0.3, 0.3],
                weirdness_range: [-0.2, 0.2],
            },
            SerializableBiome {
                id: 15,
                name: sblock("mountains"),
                top_block: sblock("stone"),
                filler_block: sblock("stone"),
                filler_depth: 1,
                ocean_block: None,
                base_height_offset: 50.0,
                height_variation: 30.0,
                erosion_resistance: 0.9,
                tree_density: 0.01,
                tree_type: TreeType::Pine,
                cave_density: 1.2,
                cave_modifier: 1.5,
                temperature_range: [-1.0, 1.0],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [0.5, 0.85],
                erosion_range: [-0.3, 0.3],
                weirdness_range: [-0.4, 0.5],
            },
            SerializableBiome {
                id: 16,
                name: sblock("snowy_peaks"),
                top_block: sblock("snow"),
                filler_block: sblock("stone"),
                filler_depth: 1,
                ocean_block: None,
                base_height_offset: 75.0,
                height_variation: 25.0,
                erosion_resistance: 1.0,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 1.0,
                cave_modifier: 1.3,
                temperature_range: [-1.0, -0.2],
                humidity_range: [-1.0, 1.0],
                continentalness_range: [0.7, 1.0],
                erosion_range: [-1.0, 0.1],
                weirdness_range: [0.3, 1.0],
            },
            SerializableBiome {
                id: 17,
                name: sblock("jungle"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: None,
                base_height_offset: 5.0,
                height_variation: 15.0,
                erosion_resistance: 0.3,
                tree_density: 0.12,
                tree_type: TreeType::Jungle,
                cave_density: 0.7,
                cave_modifier: 0.8,
                temperature_range: [0.6, 1.0],
                humidity_range: [0.6, 1.0],
                continentalness_range: [-0.05, 0.3],
                erosion_range: [-0.3, 0.3],
                weirdness_range: [-0.3, 0.3],
            },
            SerializableBiome {
                id: 18,
                name: sblock("swamp"),
                top_block: sblock("grass"),
                filler_block: sblock("dirt"),
                filler_depth: 3,
                ocean_block: sob(Some("water")),
                base_height_offset: -5.0,
                height_variation: 5.0,
                erosion_resistance: 0.1,
                tree_density: 0.05,
                tree_type: TreeType::Oak,
                cave_density: 0.5,
                cave_modifier: 0.5,
                temperature_range: [0.2, 0.8],
                humidity_range: [0.7, 1.0],
                continentalness_range: [-0.05, 0.15],
                erosion_range: [0.5, 1.0],
                weirdness_range: [-0.5, -0.1],
            },
            // ── Faz 4: New Biomes ──
            SerializableBiome {
                id: 19,
                name: sblock("ice_spikes"),
                top_block: sblock("snow"),
                filler_block: sblock("stone"),
                filler_depth: 1,
                ocean_block: None,
                base_height_offset: 5.0,
                height_variation: 20.0,
                erosion_resistance: 0.8,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.9,
                cave_modifier: 1.0,
                temperature_range: [-1.0, -0.5],
                humidity_range: [-0.5, 0.0],
                continentalness_range: [0.0, 0.3],
                erosion_range: [-0.5, 0.2],
                weirdness_range: [-0.5, 0.5],
            },
            SerializableBiome {
                id: 20,
                name: sblock("badlands"),
                top_block: sblock("sand"),
                filler_block: sblock("dirt"),
                filler_depth: 4,
                ocean_block: None,
                base_height_offset: 5.0,
                height_variation: 18.0,
                erosion_resistance: 0.7,
                tree_density: 0.0,
                tree_type: TreeType::None,
                cave_density: 0.7,
                cave_modifier: 0.9,
                temperature_range: [0.7, 1.0],
                humidity_range: [-1.0, -0.5],
                continentalness_range: [0.1, 0.4],
                erosion_range: [-0.4, 0.1],
                weirdness_range: [-0.3, 0.5],
            },
        ]
    }
}

/// 5D Euclidean distance from params to biome hyper-rectangle center.
#[inline(always)]
fn hypercube_distance(params: &NoiseParams, biome: &ResolvedBiome) -> f32 {
    let dc = center_dist(params.continentalness, biome.continentalness_range);
    let de = center_dist(params.erosion, biome.erosion_range);
    let dw = center_dist(params.weirdness, biome.weirdness_range);
    let dt = center_dist(params.temperature, biome.temperature_range);
    let dh = center_dist(params.humidity, biome.humidity_range);
    (dc * dc + de * de + dw * dw + dt * dt + dh * dh).sqrt()
}

#[inline(always)]
fn center_dist(value: f32, range: (f32, f32)) -> f32 {
    let center = (range.0 + range.1) * 0.5;
    (value - center).abs()
}
