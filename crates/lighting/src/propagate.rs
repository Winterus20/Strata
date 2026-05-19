use crate::block_light::BlockLightPropagator;
use crate::sunlight::SunlightPropagator;
use strata_core::Chunk;
use strata_core::light::LightData;

/// Complete light propagation for a chunk (sky + block light).
pub fn propagate_all(chunk: &mut Chunk, light_emission: &[u8]) {
    let mut light = LightData::new();
    SunlightPropagator::init(&chunk.blocks, &chunk.heightmap_top, &mut light);
    SunlightPropagator::propagate_bfs(&chunk.blocks, &mut light);
    BlockLightPropagator::init(&chunk.blocks, &mut light, light_emission);
    chunk.light = light;
}

/// Full re-propagation after a block change (creates fresh light data).
pub fn on_block_change(chunk: &mut Chunk, light_emission: &[u8]) {
    propagate_all(chunk, light_emission);
}
