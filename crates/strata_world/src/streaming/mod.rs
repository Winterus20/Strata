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

/// Sectors spawned per frame during streaming (initial ramp and movement).
/// Dripping spawns spreads entity creation + world-gen/mesh downstream work
/// across frames so the main-thread sync budget (~2–4 ms) is not blown in one
/// hitch (industry streaming best practice: per-frame spawn cap).
pub const SPAWN_PER_FRAME: usize = 8;

/// Sectors unloaded per frame when the player crosses a streaming boundary.
/// Unloading 49 sectors in one frame cascades into mesh eviction, GPU cache
/// teardown, and lighting teardown in the same tick — spreading keeps 144 Hz.
pub const UNLOAD_PER_FRAME: usize = 12;

/// Alias kept for call sites that referred to the initial ramp constant.
pub const INITIAL_SPAWN_PER_FRAME: usize = SPAWN_PER_FRAME;

/// Per-frame streaming bookkeeping cost (spawn/unload counts + wall µs).
#[derive(Resource, Default)]
pub struct StreamingTimers {
    pub us: u64,
    pub spawned: usize,
    pub unloaded: usize,
}

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

/// Load/mesh priority score: **lower = sooner**. Nearest sectors win; a sector
/// one step ahead of the player in the current movement direction is boosted
/// (predictive prefetch, plan 08 / 12 §5).
#[inline]
pub fn load_priority(player: SectorCoord, move_dir: SectorCoord, c: SectorCoord) -> i32 {
    let d = chebyshev(c, player);
    if move_dir == SectorCoord(0, 0, 0) {
        return d;
    }
    let ahead = SectorCoord(
        player.0 + move_dir.0,
        player.1 + move_dir.1,
        player.2 + move_dir.2,
    );
    if chebyshev(c, ahead) < d {
        d.saturating_sub(1)
    } else {
        d
    }
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
    /// Remaining sectors to spawn during the initial ramp. The very first frame
    /// `desired_resident_set` returns the full ~343-sector ball; spawning them
    /// all at once would create 343 entities + 343 world-gen tasks in one frame.
    /// We instead drip the initial load at `INITIAL_SPAWN_PER_FRAME` sectors/frame
    /// so the generate/mesh/upload pipeline stays budgeted (no first-frame hitch).
    initial_spawn_remaining: Option<usize>,
    /// Whether the initial ramp is active. Off by default (tests expect the full
    /// set in one call); the client enables it once at setup so the first real
    /// frame drips spawns instead of bombing.
    ramp_enabled: bool,
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
            initial_spawn_remaining: None,
            ramp_enabled: false,
        }
    }

    /// Effective load radius: base radius widened by the hysteresis ring.
    pub fn effective_radius(&self) -> i32 {
        self.radius + self.hysteresis
    }

    /// All sectors that must be resident for `player`: every coordinate within
    /// Chebyshev `effective_radius`, plus a predictive-prefetch sector one step
    /// *beyond* the ball (`radius+1` along `move_dir`) so movement pre-buffers
    /// the next shell (plan 12 §5).
    pub fn desired_resident_set(&self, player: SectorCoord) -> HashSet<SectorCoord> {
        let r = self.effective_radius();
        let mut set = HashSet::with_capacity(((2 * r + 1) as usize).pow(3) + 1);
        for dx in -r..=r {
            for dy in -r..=r {
                for dz in -r..=r {
                    set.insert(SectorCoord(player.0 + dx, player.1 + dy, player.2 + dz));
                }
            }
        }
        if self.move_dir != SectorCoord(0, 0, 0) {
            let ahead = r + 1;
            set.insert(SectorCoord(
                player.0 + self.move_dir.0 * ahead,
                player.1 + self.move_dir.1 * ahead,
                player.2 + self.move_dir.2 * ahead,
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
    /// Enable the initial spawn ramp (client only). When set, the first frame's
    /// full desired set is dripped at `INITIAL_SPAWN_PER_FRAME` sectors/frame
    /// instead of spawned all at once. Tests leave this off.
    pub ramp_enabled: bool,
}

impl Default for StreamingPlugin {
    fn default() -> Self {
        Self {
            radius: DEFAULT_RADIUS,
            hysteresis: DEFAULT_HYSTERESIS,
            ramp_enabled: false,
        }
    }
}

impl StreamingPlugin {
    pub fn new(radius: i32, hysteresis: i32) -> Self {
        Self {
            radius,
            hysteresis,
            ramp_enabled: false,
        }
    }

    pub fn with_ramp(mut self) -> Self {
        self.ramp_enabled = true;
        self
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
        let mut mgr = StreamingManager::new(self.radius, self.hysteresis);
        mgr.ramp_enabled = self.ramp_enabled;
        app.insert_resource(mgr);
        app.init_resource::<StreamingTimers>();
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
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn streaming_system(
    mut commands: Commands,
    mut manager: ResMut<StreamingManager>,
    mut timers: ResMut<StreamingTimers>,
    player: Query<&Transform, With<StreamingAnchor>>,
    sectors: Query<(Entity, &SectorCoord, Option<&XBrickMap>)>,
    pool: Res<GlobalBrickPool>,
    dirty_queue: Option<Res<strata_save::plugin::DirtyQueue>>,
    mut messages: Option<ResMut<bevy_ecs::message::Messages<strata_save::plugin::SectorSave>>>,
    mut pending_gen: Option<ResMut<crate::plugin::PendingWorldGen>>,
    mut pending_load: Option<ResMut<crate::plugin::PendingSectorLoad>>,
) {
    timers.us = 0;
    timers.spawned = 0;
    timers.unloaded = 0;
    let t0 = std::time::Instant::now();

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

    // Fast reconciliation: O(n_resident) entity-alive check + O(n_sectors) adopt
    // scan, zero per-frame HashMap allocation.
    let dead: Vec<SectorCoord> = manager
        .resident
        .iter()
        .filter(|&(_, e)| sectors.get(*e).is_err())
        .map(|(c, _)| *c)
        .collect();
    for c in &dead {
        manager.mark_unloaded(c);
    }
    // Adopt externally-spawned sectors (rare; mainly test support).
    for (e, c, _) in sectors.iter() {
        if !manager.is_resident(c) {
            manager.mark_loaded(*c, e);
        }
    }

    let desired = manager.desired_resident_set(player_sector);

    // 2/3. LOAD — Filter-First: spawn only the sectors missing from the resident
    //     set. `WorldGen` (same frame, later set) will generate each one.
    //     When `ramp_enabled` (client only, not tests) and this is the first
    //     substantial load, cap spawns at `INITIAL_SPAWN_PER_FRAME` so the full
    //     ~343-sector ball is dripped across frames instead of all at once; this
    //     keeps entity creation + world-gen task enqueue budgeted and avoids the
    //     first-frame spawn bomb. Tests leave the ramp off and get the full set.
    let mut initial_cap = if manager.ramp_enabled {
        SPAWN_PER_FRAME
    } else {
        usize::MAX
    };
    if manager.ramp_enabled
        && manager.initial_spawn_remaining.is_none()
        && desired.len() > manager.resident.len() + SPAWN_PER_FRAME
    {
        manager.initial_spawn_remaining = Some(desired.len());
    }
    let mut to_spawn: Vec<SectorCoord> = desired
        .iter()
        .filter(|c| !manager.is_resident(c))
        .copied()
        .collect();
    to_spawn.sort_by_key(|c| load_priority(player_sector, manager.move_dir, *c));
    for c in to_spawn {
        if initial_cap == 0 {
            break;
        }
        let e = commands.spawn(SectorCoord(c.0, c.1, c.2)).id();
        manager.mark_loaded(c, e);
        timers.spawned += 1;
        initial_cap -= 1;
        if let Some(rem) = manager.initial_spawn_remaining.as_mut() {
            // Saturating: the ramp counter is seeded with `desired.len()`
            // at ramp start, but the player can move (e.g. fall through Y
            // sectors) mid-ramp, shifting `desired` so total spawns exceed
            // the seed. `-= 1` then underflowed a `usize` (debug panic).
            // Hitting 0 just ends the ramp (handled below).
            *rem = rem.saturating_sub(1);
        }
    }
    if manager.initial_spawn_remaining == Some(0) {
        manager.initial_spawn_remaining = None;
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
    // Farthest-from-player first: frees the shell the player just left before
    // nearer sectors, and keeps the visible set stable while the budget drains.
    to_unload.sort_by_key(|c| std::cmp::Reverse(chebyshev(*c, player_sector)));

    let mut unload_budget = UNLOAD_PER_FRAME;
    if unload_budget > 0 && !to_unload.is_empty() {
        let mut inner_pool = pool.write_inner();
        for c in &to_unload {
            if unload_budget == 0 {
                break;
            }
            if let Some(e) = manager.entity_for(c) {
                // If the sector is dirty, emit a SectorSave event before despawning
                let is_dirty = dirty_queue
                    .as_ref()
                    .map(|dq| dq.tracker.is_dirty(*c))
                    .unwrap_or(false);
                #[allow(clippy::collapsible_if)]
                if is_dirty {
                    if let Some(ref mut msgs) = messages {
                        msgs.write(strata_save::plugin::SectorSave(*c));
                    }
                }
                if let Ok((_, _, Some(map))) = sectors.get(e) {
                    map.free_locked(&mut inner_pool);
                }
                commands.entity(e).despawn();
            }
            // Drop in-flight gen/load tasks so workers do not write into a
            // despawned (or soon-respawned) sector entity.
            if let Some(ref mut pg) = pending_gen {
                pg.tasks.remove(c);
            }
            if let Some(ref mut pl) = pending_load {
                pl.tasks.remove(c);
            }
            manager.mark_unloaded(c);
            timers.unloaded += 1;
            unload_budget -= 1;
        }
    }
    timers.us = t0.elapsed().as_micros() as u64;
}

#[cfg(test)]
mod tests;
