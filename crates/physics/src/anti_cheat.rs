use bevy_ecs::prelude::*;
use glam::{Vec3, IVec3};
use strata_ecs::components::{Position, Velocity};
use strata_ecs::systems::ChunkStorage;
use strata_core::BlockPos;
use crate::aabb::Aabb;

/// Configuration for the server-side movement and interaction verification.
#[derive(Resource, Debug, Clone)]
pub struct AntiCheatConfig {
    pub max_walk_speed: f32,
    pub max_sprint_speed: f32,
    pub max_vertical_speed: f32,
    pub speed_threshold_epsilon: f32,
    pub flight_allowance_time_ms: u64,
    pub grace_period_duration_ms: u64,
    pub max_reach_distance: f32,
}

impl Default for AntiCheatConfig {
    fn default() -> Self {
        Self {
            max_walk_speed: 6.0,          // m/s
            max_sprint_speed: 10.0,        // m/s
            max_vertical_speed: 15.0,      // m/s (maximum falling/jumping speed)
            speed_threshold_epsilon: 1.5,
            flight_allowance_time_ms: 500, // tolerance for latency
            grace_period_duration_ms: 1000, // 1 second grace after knockback/portals
            max_reach_distance: 6.0,       // maximum 6 blocks interaction reach
        }
    }
}

/// Tracking component for player cheat validation.
#[derive(Component, Debug)]
pub struct PlayerCheatState {
    pub last_verified_position: Vec3,
    pub last_verified_time: std::time::Instant,
    pub grace_period_end: Option<std::time::Instant>,
    pub violation_ticks: u32,
}

impl PlayerCheatState {
    /// Creates a new state tracking from the given initial position.
    pub fn new(initial_pos: Vec3) -> Self {
        Self {
            last_verified_position: initial_pos,
            last_verified_time: std::time::Instant::now(),
            grace_period_end: None,
            violation_ticks: 0,
        }
    }

    /// Triggers a grace period (e.g. during portal transition, knockback, explosion).
    pub fn trigger_grace_period(&mut self, duration_ms: u64) {
        self.grace_period_end = Some(
            std::time::Instant::now() + std::time::Duration::from_millis(duration_ms)
        );
        self.violation_ticks = 0;
    }
}

/// Helper to check if a player's bounding box intersects with any solid block.
pub fn check_wallhack(
    player_pos: Vec3,
    chunk_storage: &ChunkStorage,
) -> bool {
    // Player AABB approximation (half extents: width=0.3, height=0.9, depth=0.3)
    let player_aabb = Aabb::new(player_pos, Vec3::new(0.3, 0.9, 0.3));
    
    // We check block positions overlapping the player's bounding box
    let min_bx = player_aabb.min.x.floor() as i32;
    let max_bx = player_aabb.max.x.floor() as i32;
    let min_by = player_aabb.min.y.floor() as i32;
    let max_by = player_aabb.max.y.floor() as i32;
    let min_bz = player_aabb.min.z.floor() as i32;
    let max_bz = player_aabb.max.z.floor() as i32;

    for y in min_by..=max_by {
        if y < 0 || y >= 256 {
            continue;
        }
        for x in min_bx..=max_bx {
            for z in min_bz..=max_bz {
                let block_pos = BlockPos(IVec3::new(x, y, z));
                if let Some((chunk_pos, lx, ly, lz)) = block_pos.to_chunk_local() {
                    if let Some(chunk) = chunk_storage.get_chunk(chunk_pos) {
                        if !chunk.get_block(lx, ly, lz).is_air() {
                            // Represent block as an AABB
                            let block_center = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5);
                            let block_aabb = Aabb::new(block_center, Vec3::new(0.5, 0.5, 0.5));
                            
                            if player_aabb.intersects(&block_aabb) {
                                return true; // Collision with a solid block detected!
                            }
                        }
                    }
                }
            }
        }
    }
    
    false
}

/// Validates player movement distance and resets their position (rubberbands) if they cheat.
pub fn validate_player_movement_system(
    mut query: Query<(Entity, &mut Position, &mut PlayerCheatState, Option<&Velocity>)>,
    config: Res<AntiCheatConfig>,
    chunk_storage: Res<ChunkStorage>,
) {
    let now = std::time::Instant::now();
    
    for (entity, mut pos, mut cheat_state, vel) in query.iter_mut() {
        let dt = now.duration_since(cheat_state.last_verified_time).as_secs_f32();
        if dt <= 0.001 {
            continue;
        }
        
        // 1. Grace Period active check
        if let Some(grace_end) = cheat_state.grace_period_end {
            if now < grace_end {
                // Skip movement validation, just update tracking
                cheat_state.last_verified_position = pos.0;
                cheat_state.last_verified_time = now;
                cheat_state.violation_ticks = 0;
                continue;
            } else {
                cheat_state.grace_period_end = None; // grace period expired
            }
        }
        
        // 2. Wallhack check (clip into solid blocks)
        if check_wallhack(pos.0, &chunk_storage) {
            cheat_state.violation_ticks += 1;
            if cheat_state.violation_ticks > 3 {
                tracing::warn!("Anti-Cheat: Player {:?} clipped inside solid blocks! Rubberbanding.", entity);
                pos.0 = cheat_state.last_verified_position; // rubberband
                cheat_state.violation_ticks = 0;
            }
            cheat_state.last_verified_time = now;
            continue;
        }
        
        // 3. Speed & Flight verification
        let delta = pos.0 - cheat_state.last_verified_position;
        let horizontal_dist = Vec3::new(delta.x, 0.0, delta.z).length();
        let vertical_dist = delta.y.abs();
        
        // Determine expected limit
        let base_speed = vel.map(|v| v.0.length()).unwrap_or(0.0).max(config.max_sprint_speed);
        let max_horizontal_limit = base_speed * dt + config.speed_threshold_epsilon;
        let max_vertical_limit = config.max_vertical_speed * dt + config.speed_threshold_epsilon;
        
        if horizontal_dist > max_horizontal_limit || vertical_dist > max_vertical_limit {
            cheat_state.violation_ticks += 1;
            
            if cheat_state.violation_ticks > 3 {
                tracing::warn!(
                    "Anti-Cheat: Player {:?} moved too fast! Speed: horizontal {} m/s, vertical {} m/s. Rubberbanding.",
                    entity,
                    horizontal_dist / dt,
                    vertical_dist / dt
                );
                
                pos.0 = cheat_state.last_verified_position; // rubberband
                cheat_state.violation_ticks = 0;
            }
        } else {
            cheat_state.violation_ticks = 0;
            cheat_state.last_verified_position = pos.0;
        }
        
        cheat_state.last_verified_time = now;
    }
}

/// Verifies whether the player's interaction with a block is within maximum reach distance.
pub fn validate_interaction_reach(
    player_pos: Vec3,
    block_pos: IVec3,
    config: &AntiCheatConfig,
) -> bool {
    let block_center = Vec3::new(block_pos.x as f32 + 0.5, block_pos.y as f32 + 0.5, block_pos.z as f32 + 0.5);
    let dist = player_pos.distance(block_center);
    dist <= config.max_reach_distance
}
