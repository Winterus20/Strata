//! ACTIVE-tier streaming manager (M9a, plan 08 / 12-streaming-active-tier).
//!
//! Pure ECS, headless-testable, no rendering. The manager keeps a resident set
//! of sectors within `radius + hysteresis` Chebyshev distance of the player and
//! spawns/despawns sector entities accordingly. Because the `Streaming` set is
//! ordered before `WorldGen`, freshly spawned sectors are generated in the same
//! frame (the existing `WorldGenPlugin` reacts to `SectorCoord` + missing
//! `Generated`). Unloading frees the sector's pooled bricks first so the global
//! pool stays at a steady-state size (LRU eviction, plan 06 §1.3 / 39).
//!
//! Filter-First discipline (AGENTS.md §3.A): the load path only spawns sectors
//! that are missing from the resident map — no per-entity `if` re-spawn check.

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::prelude::*;

use strata_core::prelude::*;

/// Default streaming radius in sectors (~96 m at 32 m/sector).
pub const DEFAULT_RADIUS: i32 = 3;
/// Hysteresis ring (±1 sector) that pre-buffers the load boundary to suppress
/// pop-in and load/unload flapping during small player jitter.
pub const DEFAULT_HYSTERESIS: i32 = 1;

/// Convert a world-space position to the sector that contains it.
#[inline]
pub fn world_pos_to_sector(pos: Vec3) -> SectorCoord {
    SectorCoord(
        (pos.x / 32.0).floor() as i32,
        (pos.y / 32.0).floor() as i32,
        (pos.z / 32.0).floor() as i32,
    )
}

/// Chebyshev distance between two sector coordinates.
#[inline]
pub fn chebyshev(a: SectorCoord, b: SectorCoord) -> i32 {
    (a.0 - b.0)
        .abs()
        .max((a.1 - b.1).abs())
        .max((a.2 - b.2).abs())
}

/// Streaming orchestration state (plan 08 §StreamingManager).
///
/// `resident` maps every live sector entity to its [`Entity`]; `lru` preserves
/// load order (front = least recently used) so eviction reclaims the oldest
/// out-of-range sectors first, keeping pool memory bounded.
#[derive(Resource)]
pub struct StreamingManager {
    /// Base load radius in sectors.
    pub radius: i32,
    /// Hysteresis buffer added to the load radius.
    pub hysteresis: i32,
    /// Authoritative current player sector (updated every frame).
    pub player_sector: SectorCoord,
    last_player_sector: Option<SectorCoord>,
    /// Last non-zero player movement delta, used for predictive prefetch.
    pub move_dir: SectorCoord,
    resident: HashMap<SectorCoord, Entity>,
    lru: VecDeque<SectorCoord>,
}

impl StreamingManager {
    pub fn new(radius: i32, hysteresis: i32) -> Self {
        Self {
            radius,
            hysteresis,
            player_sector: SectorCoord(0, 0, 0),
            last_player_sector: None,
            move_dir: SectorCoord(0, 0, 0),
            resident: HashMap::new(),
            lru: VecDeque::new(),
        }
    }

    /// Effective load radius: base radius widened by the hysteresis ring.
    pub fn effective_radius(&self) -> i32 {
        self.radius + self.hysteresis
    }

    /// All sectors that must be resident for `player`: every coordinate within
    /// Chebyshev `effective_radius`, plus a single predictive-prefetch sector
    /// one step ahead in the last movement direction (plan 12 §5).
    pub fn desired_resident_set(&self, player: SectorCoord) -> HashSet<SectorCoord> {
        let r = self.effective_radius();
        let mut set = HashSet::with_capacity(((2 * r + 1) as usize).pow(3));
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    set.insert(SectorCoord(player.0 + dx, player.1 + dy, player.2 + dz));
                }
            }
        }
        if self.move_dir != SectorCoord(0, 0, 0) {
            set.insert(SectorCoord(
                player.0 + self.move_dir.0,
                player.1 + self.move_dir.1,
                player.2 + self.move_dir.2,
            ));
        }
        set
    }

    /// Number of currently-resident sectors.
    pub fn resident_count(&self) -> usize {
        self.resident.len()
    }

    /// True if `c` is currently resident.
    pub fn is_resident(&self, c: &SectorCoord) -> bool {
        self.resident.contains_key(c)
    }

    /// The entity backing resident sector `c`, if any.
    pub fn entity_for(&self, c: &SectorCoord) -> Option<Entity> {
        self.resident.get(c).copied()
    }

    fn mark_loaded(&mut self, c: SectorCoord, e: Entity) {
        self.resident.entry(c).or_insert(e);
        if let Some(pos) = self.lru.iter().position(|x| *x == c) {
            self.lru.remove(pos);
        }
        self.lru.push_back(c);
    }

    fn mark_unloaded(&mut self, c: &SectorCoord) {
        self.resident.remove(c);
        if let Some(pos) = self.lru.iter().position(|x| *x == *c) {
            self.lru.remove(pos);
        }
    }
}

/// `StreamingPlugin`: owns the [`StreamingManager`] resource and registers the
/// load/unload system in the `Streaming` set (ordered before `WorldGen`).
pub struct StreamingPlugin {
    pub radius: i32,
    pub hysteresis: i32,
}

impl Default for StreamingPlugin {
    fn default() -> Self {
        Self {
            radius: DEFAULT_RADIUS,
            hysteresis: DEFAULT_HYSTERESIS,
        }
    }
}

impl StreamingPlugin {
    pub fn new(radius: i32, hysteresis: i32) -> Self {
        Self { radius, hysteresis }
    }
}

impl StrataPlugin for StreamingPlugin {
    fn name(&self) -> &'static str {
        "streaming"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GlobalBrickPool>() {
            app.insert_resource(GlobalBrickPool::new());
        }
        app.insert_resource(StreamingManager::new(self.radius, self.hysteresis));
        app.add_systems(Update, streaming_system.in_set(StrataSet::Streaming));
    }
}

/// Load/unload streaming system (runs in `Streaming`, before `WorldGen`).
///
/// Each frame it (1) resolves the player sector from the player's `Transform`
/// (defaulting to the origin when no player entity exists yet), (2) extends the
/// desired set with predictive prefetch, (3) spawns any missing sectors, and
/// (4) frees + despawns any sector that has drifted outside the effective
/// radius. Freeing the pooled bricks happens before despawn so the shared
/// [`GlobalBrickPool`] reclaims the memory (LRU steady-state).
#[allow(clippy::type_complexity)]
pub fn streaming_system(
    mut commands: Commands,
    mut manager: ResMut<StreamingManager>,
    player: Query<&Transform>,
    sectors: Query<(Entity, &SectorCoord, Option<&XBrickMap>)>,
    mut pool: ResMut<GlobalBrickPool>,
) {
    // 1. Resolve the player sector (only the player entity carries a Transform;
    //    sector entities carry only `SectorCoord`).
    let player_sector = player
        .iter()
        .next()
        .map(|t| world_pos_to_sector(t.translation))
        .unwrap_or(SectorCoord(0, 0, 0));

    // Track movement direction for predictive prefetch (plan 12 §5). The per-frame
    // delta is clamped to a single step so a large teleport does not prefetch a
    // distant sector; normal play moves at most one sector per frame.
    if let Some(last) = manager.last_player_sector {
        let d = SectorCoord(
            (player_sector.0 - last.0).clamp(-1, 1),
            (player_sector.1 - last.1).clamp(-1, 1),
            (player_sector.2 - last.2).clamp(-1, 1),
        );
        if d != SectorCoord(0, 0, 0) {
            manager.move_dir = d;
        }
    }
    manager.last_player_sector = Some(player_sector);
    manager.player_sector = player_sector;

    // Reconcile the resident map with the live sector entities (in case any were
    // despawned externally) so we never re-spawn an existing sector.
    let current: HashMap<SectorCoord, Entity> = sectors.iter().map(|(e, c, _)| (*c, e)).collect();
    for (c, e) in &current {
        if !manager.is_resident(c) {
            manager.mark_loaded(*c, *e);
        }
    }
    let dead: Vec<SectorCoord> = manager
        .resident
        .keys()
        .filter(|c| !current.contains_key(*c))
        .copied()
        .collect();
    for c in &dead {
        manager.mark_unloaded(c);
    }

    let desired = manager.desired_resident_set(player_sector);

    // 2/3. LOAD — Filter-First: spawn only the sectors missing from the resident
    //     set. `WorldGen` (same frame, later set) will generate each one.
    for c in &desired {
        if !manager.is_resident(c) {
            let e = commands.spawn(SectorCoord(c.0, c.1, c.2)).id();
            manager.mark_loaded(*c, e);
        }
    }

    // 4. UNLOAD — despawn sectors outside the desired set, freeing their pooled
    //    bricks first. Evict in LRU order (oldest first) for deterministic,
    //    steady-state pool memory.
    let mut to_unload: Vec<SectorCoord> = manager
        .resident
        .keys()
        .filter(|c| !desired.contains(*c))
        .copied()
        .collect();
    to_unload.sort_by_key(|c| {
        manager
            .lru
            .iter()
            .position(|x| x == c)
            .unwrap_or(usize::MAX)
    });

    for c in &to_unload {
        if let Some(e) = manager.entity_for(c) {
            if let Ok((_, _, Some(map))) = sectors.get(e) {
                map.free(&mut pool);
            }
            commands.entity(e).despawn();
        }
        manager.mark_unloaded(c);
    }
}

#[cfg(test)]
mod tests;
