# 20 — AI & Pathfinding Sistemi

## 1. Genel Bakış

Strata'nın AI sistemi **behavior tree** tabanlıdır. Pathfinding **A*** algoritması kullanır ve voxel dünyası için optimize edilmiştir.

### Temel Prensipler

- **Behavior Tree:** Modüler, genişletilebilir AI
- **Voxel-aware:** 3D voxel grid'de pathfinding
- **Tier-bazlı:** Uzak entity'ler basitleştirilmiş AI
- **Parallel:** Birden fazla entity paralel AI update

---

## 2. Behavior Tree

```rust
/// Behavior tree node.
pub trait BtNode: Send + Sync {
    /// Node çalıştır.
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus;
}

/// Behavior tree execution durumu.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtStatus {
    /// Başarılı.
    Success,

    /// Başarısız.
    Failure,

    /// Devam ediyor.
    Running,
}

/// Behavior tree context.
pub struct BtContext<'a> {
    /// Entity pozisyonu.
    pub position: Vec3,

    /// Entity velocity.
    pub velocity: Vec3,

    /// Dünya referansı.
    pub world: &'a World,

    /// Hedef (varsa).
    pub target: Option<Entity>,

    /// Path (varsa).
    pub path: Option<Vec<IVec3>>,

    /// Blackboard (paylaşılan veri).
    pub blackboard: &'a mut Blackboard,
}

/// Blackboard — behavior tree'ler arası veri paylaşımı.
pub struct Blackboard {
    data: HashMap<String, BlackboardValue>,
}

#[derive(Clone)]
pub enum BlackboardValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    Vec3(Vec3),
    Entity(Entity),
    String(String),
}
```

### 2.1 Composite Node'lar

```rust
/// Sequence — tüm child'lar başarılı olmalı.
pub struct Sequence {
    children: Vec<Box<dyn BtNode>>,
    current: usize,
}

impl BtNode for Sequence {
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus {
        for i in self.current..self.children.len() {
            match self.children[i].execute(ctx, dt) {
                BtStatus::Success => continue,
                BtStatus::Failure => return BtStatus::Failure,
                BtStatus::Running => return BtStatus::Running,
            }
        }
        BtStatus::Success
    }
}

/// Selector — ilk başarılı child döner.
pub struct Selector {
    children: Vec<Box<dyn BtNode>>,
    current: usize,
}

impl BtNode for Selector {
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus {
        for i in self.current..self.children.len() {
            match self.children[i].execute(ctx, dt) {
                BtStatus::Success => return BtStatus::Success,
                BtStatus::Failure => continue,
                BtStatus::Running => return BtStatus::Running,
            }
        }
        BtStatus::Failure
    }
}

/// Parallel — tüm child'lar paralel çalışır.
pub struct Parallel {
    children: Vec<Box<dyn BtNode>>,
    success_threshold: usize,
}

impl BtNode for Parallel {
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus {
        let mut success_count = 0;

        for child in &self.children {
            match child.execute(ctx, dt) {
                BtStatus::Success => success_count += 1,
                BtStatus::Failure => return BtStatus::Failure,
                BtStatus::Running => {}
            }
        }

        if success_count >= self.success_threshold {
            BtStatus::Success
        } else {
            BtStatus::Running
        }
    }
}
```

### 2.2 Decorator Node'lar

```rust
/// Inverter — sonucu ters çevirir.
pub struct Inverter {
    child: Box<dyn BtNode>,
}

impl BtNode for Inverter {
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus {
        match self.child.execute(ctx, dt) {
            BtStatus::Success => BtStatus::Failure,
            BtStatus::Failure => BtStatus::Success,
            BtStatus::Running => BtStatus::Running,
        }
    }
}

/// Repeater — child'ı N kez tekrarlar.
pub struct Repeater {
    child: Box<dyn BtNode>,
    count: usize,
    current: usize,
}

/// Cooldown — child'ı belirli süre çalıştırmaz.
pub struct Cooldown {
    child: Box<dyn BtNode>,
    cooldown: Duration,
    last_execution: Option<Instant>,
}
```

### 2.3 Leaf Node'lar

```rust
/// Koşul node'u.
pub struct Condition {
    predicate: Box<dyn Fn(&BtContext) -> bool + Send + Sync>,
}

impl BtNode for Condition {
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus {
        if (self.predicate)(ctx) {
            BtStatus::Success
        } else {
            BtStatus::Failure
        }
    }
}

/// Aksiyon node'u.
pub struct Action {
    action: Box<dyn Fn(&mut BtContext, f32) -> BtStatus + Send + Sync>,
}

impl BtNode for Action {
    fn execute(&self, ctx: &mut BtContext, dt: f32) -> BtStatus {
        (self.action)(ctx, dt)
    }
}
```

---

## 3. Pathfinding (A*)

```rust
/// A* pathfinder — voxel grid için optimize.
pub struct VoxelPathfinder {
    /// Open set (priority queue).
    open_set: BinaryHeap<PathNode>,

    /// Closed set.
    closed_set: HashSet<IVec3>,

    /// G scores (başlangıçtan maliyet).
    g_scores: HashMap<IVec3, f32>,

    /// F scores (tahmini toplam maliyet).
    f_scores: HashMap<IVec3, f32>,

    /// Parent mapping (path reconstruction).
    parents: HashMap<IVec3, IVec3>,
}

struct PathNode {
    position: IVec3,
    f_score: f32,
}

impl PartialEq for PathNode {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position
    }
}

impl Eq for PathNode {}

impl PartialOrd for PathNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        other.f_score.partial_cmp(&self.f_score)
    }
}

impl Ord for PathNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl VoxelPathfinder {
    /// Yol bul.
    pub fn find_path(
        &mut self,
        start: IVec3,
        goal: IVec3,
        world: &World,
        max_steps: usize,
    ) -> Option<Vec<IVec3>> {
        self.open_set.clear();
        self.closed_set.clear();
        self.g_scores.clear();
        self.f_scores.clear();
        self.parents.clear();

        self.open_set.push(PathNode {
            position: start,
            f_score: self.heuristic(start, goal),
        });
        self.g_scores.insert(start, 0.0);

        let mut steps = 0;

        while let Some(current) = self.open_set.pop() {
            steps += 1;
            if steps > max_steps {
                return None; // Max steps aşıldı
            }

            if current.position == goal {
                return Some(self.reconstruct_path(start, goal));
            }

            self.closed_set.insert(current.position);

            // Komşuları kontrol et
            for neighbor in self.get_neighbors(current.position, world) {
                if self.closed_set.contains(&neighbor) {
                    continue;
                }

                let tentative_g = self.g_scores[&current.position]
                    + self.move_cost(current.position, neighbor);

                if tentative_g < *self.g_scores.get(&neighbor).unwrap_or(&f32::INFINITY) {
                    self.parents.insert(neighbor, current.position);
                    self.g_scores.insert(neighbor, tentative_g);
                    self.f_scores.insert(
                        neighbor,
                        tentative_g + self.heuristic(neighbor, goal),
                    );

                    self.open_set.push(PathNode {
                        position: neighbor,
                        f_score: self.f_scores[&neighbor],
                    });
                }
            }
        }

        None // Yol bulunamadı
    }

    /// Heuristic (tahmini maliyet).
    fn heuristic(&self, a: IVec3, b: IVec3) -> f32 {
        // Octile distance (diagonal movement destekli)
        let dx = (a.x - b.x).abs() as f32;
        let dy = (a.y - b.y).abs() as f32;
        let dz = (a.z - b.z).abs() as f32;

        let d1 = 1.0; // Orthogonal cost
        let d2 = 1.414; // Diagonal cost

        d1 * (dx + dy + dz) + (d2 - 2.0 * d1) * dx.min(dy).min(dz)
    }

    /// Komşu pozisyonları al.
    fn get_neighbors(&self, pos: IVec3, world: &World) -> Vec<IVec3> {
        let mut neighbors = Vec::new();

        // 6 yön (3D grid)
        let directions = [
            IVec3::new(1, 0, 0),
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 1, 0),
            IVec3::new(0, -1, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(0, 0, -1),
        ];

        for dir in directions {
            let neighbor = pos + dir;

            // Geçilebilir mi?
            if world.is_passable(neighbor) {
                neighbors.push(neighbor);
            }
        }

        neighbors
    }

    /// Hareket maliyeti.
    fn move_cost(&self, from: IVec3, to: IVec3) -> f32 {
        let dx = (from.x - to.x).abs();
        let dy = (from.y - to.y).abs();
        let dz = (from.z - to.z).abs();

        if dx + dy + dz == 1 {
            1.0 // Orthogonal
        } else {
            1.414 // Diagonal
        }
    }

    /// Path reconstruction.
    fn reconstruct_path(&self, start: IVec3, goal: IVec3) -> Vec<IVec3> {
        let mut path = Vec::new();
        let mut current = goal;

        while current != start {
            path.push(current);
            current = self.parents[&current];
        }

        path.push(start);
        path.reverse();
        path
    }
}
```

---

## 4. Mob AI Behavior Trees

### 4.1 Zombie AI

```rust
/// Zombie behavior tree.
pub fn zombie_ai() -> Box<dyn BtNode> {
    Selector {
        children: vec![
            // 1. Hedef varsa saldır
            Box::new(Sequence {
                children: vec![
                    Box::new(Condition {
                        predicate: Box::new(|ctx| ctx.target.is_some()),
                    }),
                    Box::new(Condition {
                        predicate: Box::new(|ctx| {
                            if let Some(target) = ctx.target {
                                let dist = ctx.position.distance(target.position);
                                dist < 2.0
                            } else {
                                false
                            }
                        }),
                    }),
                    Box::new(Action {
                        action: Box::new(|ctx, dt| {
                            // Saldır
                            BtStatus::Success
                        }),
                    }),
                ],
            }),

            // 2. Hedef varsa yol bul ve git
            Box::new(Sequence {
                children: vec![
                    Box::new(Condition {
                        predicate: Box::new(|ctx| ctx.target.is_some()),
                    }),
                    Box::new(Action {
                        action: Box::new(|ctx, dt| {
                            if let Some(target) = ctx.target {
                                if ctx.path.is_none() {
                                    // Yeni path bul
                                    let pathfinder = VoxelPathfinder::new();
                                    if let Some(path) = pathfinder.find_path(
                                        ctx.position.as_ivec3(),
                                        target.position.as_ivec3(),
                                        ctx.world,
                                        1000,
                                    ) {
                                        ctx.path = Some(path);
                                    }
                                }

                                // Path'i takip et
                                if let Some(path) = &ctx.path {
                                    if path.len() > 1 {
                                        let next = path[1];
                                        let dir = (next.as_vec3() - ctx.position).normalize();
                                        ctx.velocity = dir * 2.0; // Zombie speed
                                    }
                                }

                                BtStatus::Running
                            } else {
                                BtStatus::Failure
                            }
                        }),
                    }),
                ],
            }),

            // 3. Rastgele dolaş
            Box::new(Action {
                action: Box::new(|ctx, dt| {
                    // Rastgele yön seç
                    let angle = rand::random::<f32>() * std::f32::consts::TAU;
                    let dir = Vec3::new(angle.cos(), 0.0, angle.sin());
                    ctx.velocity = dir * 1.0;
                    BtStatus::Running
                }),
            }),
        ],
    }
}
```

### 4.2 Hayvan AI (Cow, Pig, Sheep)

```rust
/// Pasif hayvan behavior tree.
pub fn passive_mob_ai() -> Box<dyn BtNode> {
    Selector {
        children: vec![
            // 1. Tehlike varsa kaç
            Box::new(Sequence {
                children: vec![
                    Box::new(Condition {
                        predicate: Box::new(|ctx| {
                            // Oyuncu yakınsa tehlike
                            if let Some(target) = ctx.target {
                                let dist = ctx.position.distance(target.position);
                                dist < 10.0
                            } else {
                                false
                            }
                        }),
                    }),
                    Box::new(Action {
                        action: Box::new(|ctx, dt| {
                            // Oyuncunun ters yönüne kaç
                            if let Some(target) = ctx.target {
                                let dir = (ctx.position - target.position).normalize();
                                ctx.velocity = dir * 3.0; // Kaçış hızı
                            }
                            BtStatus::Running
                        }),
                    }),
                ],
            }),

            // 2. Otla (yemek ye)
            Box::new(Sequence {
                children: vec![
                    Box::new(Condition {
                        predicate: Box::new(|ctx| {
                            // Altında çimen var mı?
                            ctx.world.is_grass(ctx.position.as_ivec3() - IVec3::new(0, 1, 0))
                        }),
                    }),
                    Box::new(Action {
                        action: Box::new(|ctx, dt| {
                            // Otla — dur ve ye
                            ctx.velocity = Vec3::ZERO;
                            BtStatus::Running
                        }),
                    }),
                ],
            }),

            // 3. Rastgele dolaş
            Box::new(Action {
                action: Box::new(|ctx, dt| {
                    let angle = rand::random::<f32>() * std::f32::consts::TAU;
                    let dir = Vec3::new(angle.cos(), 0.0, angle.sin());
                    ctx.velocity = dir * 0.8;
                    BtStatus::Running
                }),
            }),
        ],
    }
}
```

---

## 5. AI Update Sistemi

```rust
/// AI update sistemi.
pub fn ai_update_system(
    time: Res<Time>,
    mut entities: Query<(
        Entity,
        &mut AiState,
        &mut Transform,
        &mut Velocity,
    )>,
    players: Query<&Transform, With<Player>>,
    world: Res<World>,
) {
    let dt = time.delta_secs();

    for (entity, mut ai, mut transform, mut velocity) in entities.iter_mut() {
        // AI update throttling
        if ai.last_update.elapsed() < ai.update_interval {
            continue;
        }

        ai.last_update = Instant::now();

        // En yakın oyuncuyu bul (target)
        let nearest_player = players.iter()
            .min_by(|a, b| {
                let dist_a = a.position.distance_squared(transform.position);
                let dist_b = b.position.distance_squared(transform.position);
                dist_a.partial_cmp(&dist_b).unwrap()
            });

        // Context oluştur
        let mut ctx = BtContext {
            position: transform.position,
            velocity: velocity.0,
            world: &world,
            target: nearest_player.map(|(e, _)| e),
            path: ai.path.clone(),
            blackboard: &mut ai.blackboard,
        };

        // Behavior tree çalıştır
        let status = ai.behavior_tree.execute(&mut ctx, dt);

        // Sonuçları uygula
        velocity.0 = ctx.velocity;
        ai.path = ctx.path;

        // Path smoothing
        if let Some(path) = &ai.path {
            if path.len() > 1 {
                let next = path[1];
                let dir = (next.as_vec3() - transform.position).normalize();
                velocity.0 = dir * ai.move_speed;
            }
        }
    }
}
```

---

## 6. Crate Organizasyonu

```
crates/
  ai/
    ├── mod.rs              ← AI plugin entry point
    ├── behavior_tree/
    │   ├── mod.rs          ← Behavior tree sistemi
    │   ├── node.rs         ← BtNode trait
    │   ├── composite.rs    ← Sequence, Selector, Parallel
    │   ├── decorator.rs    ← Inverter, Repeater, Cooldown
    │   ├── leaf.rs         ← Condition, Action
    │   └── context.rs      ← BtContext, Blackboard
    ├── pathfinding/
    │   ├── mod.rs          ← VoxelPathfinder
    │   ├── astar.rs        ← A* algoritması
    │   └── heuristic.rs    ← Heuristic fonksiyonları
    ├── mobs/
    │   ├── mod.rs          ← Mob AI tanımları
    │   ├── zombie.rs       ← Zombie AI
    │   ├── skeleton.rs     ← Skeleton AI
    │   ├── creeper.rs      ← Creeper AI
    │   ├── spider.rs       ← Spider AI
    │   └── passive.rs      ← Cow, Pig, Sheep AI
    └── update.rs           ← AI update sistemi
```


# 33 — Entity/Mob Lifecycle

## 1. Genel Bakış

Strata'nın entity sistemi **mob spawn, loot, drop, ve lifecycle** yönetimini sağlar.

### Temel Prensipler

- **Spawn rules:** Biome, zaman, yükseklik bazlı spawn
- **Loot tables:** Deterministik drop tabloları
- **Lifecycle:** Spawn → Active → Idle → Death → Despawn
- **Entity tracking:** Server-client entity sync

---

## 2. Entity Component

```rust
#[derive(Component)]
pub struct Mob {
    /// Mob tipi.
    pub mob_type: MobTypeId,

    /// Sağlık.
    pub health: f32,

    /// Maksimum sağlık.
    pub max_health: f32,

    /// Hasar.
    pub damage: f32,

    /// Hız.
    pub speed: f32,

    /// Lifecycle state.
    pub state: MobState,
}

#[derive(Clone, Copy)]
pub enum MobState {
    Idle,
    Wandering,
    Chasing,
    Attacking,
    Fleeing,
    Dead,
}
```

---

## 3. Loot Tables

```rust
pub struct LootTable {
    pub entries: Vec<LootEntry>,
}

pub struct LootEntry {
    pub item_id: u16,
    pub min_count: u8,
    pub max_count: u8,
    pub chance: f32,
    pub enchantment_chance: f32,
}
```

---

## 4. Spawn System

```rust
pub struct SpawnRule {
    pub mob_type: MobTypeId,
    pub valid_biomes: Vec<BiomeId>,
    pub time_range: (f32, f32),
    pub height_range: (i32, i32),
    pub max_per_chunk: u8,
    pub spawn_weight: u32,
}
```

---

## 5. Crate Organizasyonu

```
crates/
  entities/
    ├── mod.rs
    ├── mob.rs
    ├── lifecycle.rs
    ├── loot.rs
    ├── spawn.rs
    └── tracking.rs
```
