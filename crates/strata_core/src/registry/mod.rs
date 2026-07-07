//! Block registry: SoA property store, data-driven TOML definitions, and the
//! sector-local palette bridge to the XBrickMap (plan 04 / 05 / 06).
//!
//! Per AGENTS.md §3.A and plan 05 §3, hot block properties live in parallel
//! `Vec` arrays (Structure-of-Arrays) rather than one fat `BlockDefinition`
//! struct walked in hot queries. String names are `&'static str` (leaked at
//! init time) so the registry is `'static` and shareable without per-query
//! allocation.

use bevy::prelude::*;
use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Block type identifier. `0` is the reserved AIR sentinel (empty space).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct BlockId(pub u16);

impl BlockId {
    /// The AIR sentinel (empty space). Always palette index 0 in a sector.
    pub const AIR: BlockId = BlockId(0);
}

bitflags! {
    /// Compact per-block property bitmask (plan 04 §3). Two bytes per block in
    /// hot queries; cache-line friendly vs a stringly-typed struct.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct BlockFlags: u16 {
        const SOLID       = 1 << 0;
        const TRANSPARENT = 1 << 1;
        const AIR         = 1 << 2;
        const LIGHT_SRC   = 1 << 3;
        const LIQUID      = 1 << 4;
    }
}

/// SoA block property store. Index = `BlockId.0`. No single AoS
/// `BlockDefinition` is iterated on the hot path (plan 05 §3.2).
#[derive(Debug, Clone, Resource, Default)]
pub struct BlockRegistry {
    pub id: Vec<BlockId>,
    pub name: Vec<&'static str>,
    pub flags: Vec<BlockFlags>,
    pub solid: Vec<bool>,
    pub transparent: Vec<bool>,
    pub light_emission: Vec<u8>,
    pub base_color: Vec<[u8; 3]>,
}

impl BlockRegistry {
    /// Number of registered block types.
    #[inline]
    pub fn count(&self) -> usize {
        self.id.len()
    }

    /// Resolve a block by its registered name (init-time / rare lookup).
    #[inline]
    pub fn id_by_name(&self, name: &str) -> Option<BlockId> {
        self.name
            .iter()
            .position(|n| *n == name)
            .map(|i| self.id[i])
    }

    /// Property flags for a block. Hot path.
    #[inline]
    pub fn flags(&self, id: BlockId) -> BlockFlags {
        self.flags[id.0 as usize]
    }

    /// Is the block a full solid (non-passable) cube?
    #[inline]
    pub fn is_solid(&self, id: BlockId) -> bool {
        self.solid[id.0 as usize]
    }

    /// Is the block transparent (face culling / light pass-through)?
    #[inline]
    pub fn is_transparent(&self, id: BlockId) -> bool {
        self.transparent[id.0 as usize]
    }

    /// Emitted light level 0..=15.
    #[inline]
    pub fn light_emission(&self, id: BlockId) -> u8 {
        self.light_emission[id.0 as usize]
    }

    /// Base RGB color used by meshing / vertex coloring.
    #[inline]
    pub fn base_color(&self, id: BlockId) -> [u8; 3] {
        self.base_color[id.0 as usize]
    }
}

// ── TOML data-driven loading ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BlocksToml {
    block: Vec<BlockDefToml>,
}

#[derive(Debug, Deserialize)]
struct BlockDefToml {
    name: String,
    id: u16,
    flags: Vec<String>,
    solid: bool,
    transparent: bool,
    light_emission: u8,
    base_color: [u8; 3],
}

fn flags_from_strs(flags: &[String]) -> BlockFlags {
    let mut f = BlockFlags::empty();
    for s in flags {
        f |= match s.as_str() {
            "SOLID" => BlockFlags::SOLID,
            "TRANSPARENT" => BlockFlags::TRANSPARENT,
            "AIR" => BlockFlags::AIR,
            "LIGHT_SRC" => BlockFlags::LIGHT_SRC,
            "LIQUID" => BlockFlags::LIQUID,
            _ => BlockFlags::empty(),
        };
    }
    f
}

/// Parse `blocks.toml` (embedded via `include_str!` for hermetic, deterministic
/// loading — no runtime file IO) into the SoA `BlockRegistry`.
///
/// AIR (`id = 0`) must be the first entry and is asserted to be transparent and
/// non-solid.
pub fn load_block_registry() -> BlockRegistry {
    let toml_str = include_str!("blocks.toml");
    let defs: BlocksToml = toml::from_str(toml_str).expect("blocks.toml must parse");
    let mut reg = BlockRegistry::default();
    for d in defs.block {
        let name: &'static str = Box::leak(d.name.into_boxed_str());
        reg.id.push(BlockId(d.id));
        reg.name.push(name);
        reg.flags.push(flags_from_strs(&d.flags));
        reg.solid.push(d.solid);
        reg.transparent.push(d.transparent);
        reg.light_emission.push(d.light_emission);
        reg.base_color.push(d.base_color);
    }
    // Invariant: AIR is id 0 and is transparent + non-solid.
    assert_eq!(
        reg.id[0],
        BlockId::AIR,
        "AIR must be registered first (id 0)"
    );
    assert!(
        reg.flags(BlockId::AIR)
            .contains(BlockFlags::AIR | BlockFlags::TRANSPARENT),
        "AIR must be TRANSPARENT"
    );
    assert!(!reg.is_solid(BlockId::AIR), "AIR must not be solid");
    reg
}

/// Bevy plugin that loads the embedded block registry once and inserts it as a
/// read-only `Resource` (plan 04 §5).
pub struct BlockRegistryPlugin;

impl crate::plugin::StrataPlugin for BlockRegistryPlugin {
    fn name(&self) -> &'static str {
        "block_registry"
    }

    fn build(&self, app: &mut App) {
        app.insert_resource(load_block_registry());
    }
}

// ── Sector-local palette (XBrickMap bridge, plan 05 §14 / 06 §1.4) ──────────

/// Sector-local mapping from a compact `u8` palette index (stored per voxel in
/// the XBrickMap) to a `BlockId`. Index 0 is permanently AIR.
///
/// In the full constitution this resolves to `(block_type, variant)`, but the
/// prototype stores only the block type (variant = 0). `get_or_insert` is the
/// single chokepoint used by `XBrickMap::set_block`.
#[derive(Debug, Clone, Default, Component)]
pub struct SectorPalette {
    entries: Vec<BlockId>,
    reverse: HashMap<BlockId, u8>,
}

impl SectorPalette {
    pub fn new() -> Self {
        let mut p = SectorPalette {
            entries: Vec::new(),
            reverse: HashMap::new(),
        };
        // Index 0 is reserved for AIR.
        p.entries.push(BlockId::AIR);
        p.reverse.insert(BlockId::AIR, 0);
        p
    }

    /// Number of distinct block types in this sector's palette.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Find (or create) the local index for `block`. Returns the `u8` index that
    /// gets written into the XBrickMap. AIR always maps to 0.
    #[inline]
    pub fn get_or_insert(&mut self, block: BlockId) -> u8 {
        if let Some(&idx) = self.reverse.get(&block) {
            return idx;
        }
        let idx = self.entries.len() as u8;
        self.entries.push(block);
        self.reverse.insert(block, idx);
        idx
    }

    /// Resolve a local index back to a `BlockId`.
    #[inline]
    pub fn resolve(&self, idx: u8) -> BlockId {
        self.entries[idx as usize]
    }

    /// Build a palette from a previously captured entry list (used by
    /// `CompressedChunkData::unpack`). Index 0 must be AIR.
    pub fn from_entries(entries: Vec<BlockId>) -> Self {
        let reverse = entries
            .iter()
            .enumerate()
            .map(|(i, &b)| (b, i as u8))
            .collect();
        SectorPalette { entries, reverse }
    }

    /// Read-only view of the palette entries (local index -> BlockId).
    pub fn entries(&self) -> &[BlockId] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_is_transparent_and_not_solid() {
        let reg = load_block_registry();
        let air = reg.id_by_name("air").unwrap();
        assert_eq!(air, BlockId::AIR);
        assert!(
            reg.flags(air)
                .contains(BlockFlags::AIR | BlockFlags::TRANSPARENT)
        );
        assert!(!reg.is_solid(air));
        assert!(reg.is_transparent(air));
    }

    #[test]
    fn stone_has_solid_flag_and_solid_true() {
        let reg = load_block_registry();
        let stone = reg.id_by_name("stone").unwrap();
        assert!(reg.flags(stone).contains(BlockFlags::SOLID));
        assert!(reg.is_solid(stone));
        assert!(!reg.is_transparent(stone));
    }

    #[test]
    fn glowstone_is_light_source() {
        let reg = load_block_registry();
        let g = reg.id_by_name("glowstone").unwrap();
        assert!(reg.flags(g).contains(BlockFlags::LIGHT_SRC));
        assert_eq!(reg.light_emission(g), 15);
    }

    #[test]
    fn water_is_liquid_and_transparent() {
        let reg = load_block_registry();
        let w = reg.id_by_name("water").unwrap();
        assert!(
            reg.flags(w)
                .contains(BlockFlags::LIQUID | BlockFlags::TRANSPARENT)
        );
        assert!(!reg.is_solid(w));
    }

    #[test]
    fn toml_round_trip_parse_equal() {
        // Re-parse the embedded TOML and compare against the loaded registry.
        let reg = load_block_registry();
        let toml_str = include_str!("blocks.toml");
        let defs: BlocksToml = toml::from_str(toml_str).unwrap();
        assert_eq!(reg.count(), defs.block.len());
        for (i, d) in defs.block.iter().enumerate() {
            assert_eq!(reg.id[i], BlockId(d.id));
            assert_eq!(reg.name[i], d.name.as_str());
            assert_eq!(reg.flags[i], flags_from_strs(&d.flags));
            assert_eq!(reg.solid[i], d.solid);
            assert_eq!(reg.transparent[i], d.transparent);
            assert_eq!(reg.light_emission[i], d.light_emission);
            assert_eq!(reg.base_color[i], d.base_color);
        }
    }

    #[test]
    fn sector_palette_get_or_insert_unique() {
        let mut p = SectorPalette::new();
        assert_eq!(p.get_or_insert(BlockId::AIR), 0);
        let s = p.get_or_insert(BlockId(1));
        let s2 = p.get_or_insert(BlockId(1));
        assert_eq!(s, s2, "same block must reuse index");
        let d = p.get_or_insert(BlockId(2));
        assert_ne!(s, d);
        assert_eq!(p.resolve(s), BlockId(1));
        assert_eq!(p.resolve(d), BlockId(2));
        assert_eq!(p.resolve(0), BlockId::AIR);
    }
}
