# 22 — Testing & Benchmark Stratejisi

## 1. Genel Bakış

Strata'nın testing stratejisi **4 katmanlı** test piramidi kullanır: unit tests, integration tests, benchmark tests, ve visual regression tests.

### Temel Prensipler

- **Test piramidi:** Çok unit, az integration, az benchmark
- **Deterministik:** Aynı girdi = aynı çıktı (world gen, physics)
- **CI/CD:** Her commit'te testler otomatik çalışır
- **Performance regression:** Benchmark sonuçları takip edilir

---

## 2. Unit Tests

### 2.1 Block Registry Tests

```rust
#[cfg(test)]
mod block_registry_tests {
    use super::*;

    #[test]
    fn test_register_block() {
        let mut registry = BlockRegistry::new();
        let id = registry.register(BlockDefinition {
            name: "stone".into(),
            // ...
        }).unwrap();

        assert_eq!(id, 1);
        assert_eq!(registry.get_id("stone"), Some(1));
    }

    #[test]
    fn test_duplicate_name_rejected() {
        let mut registry = BlockRegistry::new();
        registry.register(BlockDefinition { name: "stone".into(), .. }).unwrap();

        let result = registry.register(BlockDefinition { name: "stone".into(), .. });
        assert!(result.is_err());
    }

    #[test]
    fn test_block_flags() {
        let flags = BlockFlags(BlockFlags::OPAQUE | BlockFlags::EMITS_LIGHT);
        assert!(flags.is_opaque());
        assert!(flags.emits_light());
        assert!(!flags.is_passable());
    }

    #[test]
    fn test_block_state_encoding() {
        let state = BlockState { type_id: 42, variant: 7 };
        let id = state.to_id();

        assert_eq!(id & 0x0FFF, 42);
        assert_eq!((id >> 12) & 0xF, 7);

        let decoded = BlockState::from_id(id);
        assert_eq!(decoded.type_id, 42);
        assert_eq!(decoded.variant, 7);
    }
}
```

### 2.2 XBrickMap Tests

```rust
#[cfg(test)]
mod xbrickmap_tests {
    use super::*;

    #[test]
    fn test_empty_sector() {
        let sector = Sector::empty();
        assert_eq!(sector.get_block(IVec3::new(0, 0, 0)), None);
    }

    #[test]
    fn test_set_and_get_block() {
        let mut sector = Sector::empty();
        sector.set_block(IVec3::new(5, 10, 15), Some(42));

        assert_eq!(sector.get_block(IVec3::new(5, 10, 15)), Some(42));
        assert_eq!(sector.get_block(IVec3::new(5, 10, 16)), None);
    }

    #[test]
    fn test_remove_block() {
        let mut sector = Sector::empty();
        sector.set_block(IVec3::new(5, 10, 15), Some(42));
        sector.set_block(IVec3::new(5, 10, 15), None);

        assert_eq!(sector.get_block(IVec3::new(5, 10, 15)), None);
    }

    #[test]
    fn test_boundary_coordinates() {
        let mut sector = Sector::empty();

        // Min boundary
        sector.set_block(IVec3::new(0, 0, 0), Some(1));
        assert_eq!(sector.get_block(IVec3::new(0, 0, 0)), Some(1));

        // Max boundary
        sector.set_block(IVec3::new(31, 127, 31), Some(2));
        assert_eq!(sector.get_block(IVec3::new(31, 127, 31)), Some(2));
    }

    #[test]
    fn test_popcnt_correctness() {
        let mask: u64 = 0b10101010_10101010_10101010_10101010_10101010_10101010_10101010_10101010;
        assert_eq!(mask.count_ones(), 32);
    }

    #[test]
    fn test_soa_layout_consistency() {
        // AOS ve SOA layout'lar aynı sonucu vermeli
        let mut aos_slab = SlabAOS::new();
        let mut soa_slab = SlabSOA::new();

        // Aynı veriyi her ikisine de yaz
        for i in 0..10 {
            aos_slab.set_brick(i, BrickData { mask: 0xFF, .. });
            soa_slab.set_brick(i, BrickData { mask: 0xFF, .. });
        }

        // Sonuçları karşılaştır
        for i in 0..10 {
            assert_eq!(aos_slab.get_brick(i), soa_slab.get_brick(i));
        }
    }
}
```

### 2.3 SVDAG Tests

```rust
#[cfg(test)]
mod svdag_tests {
    use super::*;

    #[test]
    fn test_node_pool_alloc() {
        let mut pool = SharedNodePool::new(1024);
        let idx = pool.alloc().unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_node_pool_free() {
        let mut pool = SharedNodePool::new(1024);
        let idx = pool.alloc().unwrap();
        pool.free(idx);

        // Serbest slot yeniden kullanılabilir
        let idx2 = pool.alloc().unwrap();
        assert_eq!(idx2, idx);
    }

    #[test]
    fn test_node_pool_exhaustion() {
        let mut pool = SharedNodePool::new(2);
        pool.alloc().unwrap();
        pool.alloc().unwrap();

        assert!(pool.alloc().is_err());
    }

    #[test]
    fn test_deduplication() {
        let mut pool = SharedNodePool::new(1024);

        // Aynı geometri iki kez oluştur
        let node1 = pool.create_leaf(42);
        let node2 = pool.create_leaf(42);

        // Deduplication aynı node'u döndürmeli
        assert_eq!(node1, node2);
    }

    #[test]
    fn test_transform_aware_dedup() {
        let mut pool = SharedNodePool::new(1024);

        // Aynı geometri, farklı transform
        let node1 = pool.create_leaf(42);
        let node2 = pool.create_leaf_with_transform(42, SvdagTransform::MirrorX);

        // Transform-aware dedup farklı node döndürür
        // ama aynı geometriyi referans eder
        assert_ne!(node1, node2);
    }
}
```

### 2.4 Lighting Tests

```rust
#[cfg(test)]
mod lighting_tests {
    use super::*;

    #[test]
    fn test_light_data_packing() {
        let light = LightData::new(15, 10, 8, 5);

        assert_eq!(light.sky(), 15);
        assert_eq!(light.block_r(), 10);
        assert_eq!(light.block_g(), 8);
        assert_eq!(light.block_b(), 5);
    }

    #[test]
    fn test_light_data_clamping() {
        // 4-bit max = 15
        let light = LightData::new(20, 20, 20, 20);

        assert_eq!(light.sky(), 15);
        assert_eq!(light.block_r(), 15);
        assert_eq!(light.block_g(), 15);
        assert_eq!(light.block_b(), 15);
    }

    #[test]
    fn test_wlp_max() {
        let a = 0x00030005; // R=5, G=3
        let b = 0x00070002; // R=2, G=7

        let result = LightData::wlp_max(a, b);

        // Her kanal için max alınmalı
        assert_eq!(result & 0xF, 7);  // G: max(3, 7) = 7
        assert_eq!((result >> 4) & 0xF, 5); // R: max(5, 2) = 5
    }

    #[test]
    fn test_bfs_propagation() {
        let mut engine = BlockLightEngine::new();
        let mut sector = Sector::empty();

        // Işık kaynağı yerleştir
        let updates = engine.place_light(&sector, IVec3::new(5, 10, 5), 15, LightColor::Red);

        assert!(!updates.is_empty());

        // En yakın komşu level 14 olmalı
        let neighbor_light = sector.get_light(IVec3::new(6, 10, 5));
        assert_eq!(neighbor_light, 14);
    }

    #[test]
    fn test_bfs_removal() {
        let mut engine = BlockLightEngine::new();
        let mut sector = Sector::empty();

        // Işık kaynağı yerleştir
        engine.place_light(&sector, IVec3::new(5, 10, 5), 15, LightColor::Red);

        // Işık kaynağını kaldır
        let updates = engine.remove_light(&sector, IVec3::new(5, 10, 5), LightColor::Red);

        // Tüm light level'lar sıfırlanmalı
        for update in &updates {
            assert_eq!(update.light, 0);
        }
    }
}
```

---

## 3. Integration Tests

### 3.1 World Generation Integration

```rust
#[cfg(test)]
mod world_gen_integration {
    use super::*;

    #[test]
    fn test_deterministic_generation() {
        let seed = WorldSeed { seed: 12345, .. };
        let gen1 = TerrainGenerator::from_seed(seed);
        let gen2 = TerrainGenerator::from_seed(seed);

        let sector1 = gen1.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));
        let sector2 = gen2.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));

        // Aynı seed = aynı sonuç
        assert_eq!(sector1, sector2);
    }

    #[test]
    fn test_different_sectors_different() {
        let gen = TerrainGenerator::from_seed(WorldSeed { seed: 12345, .. });

        let sector1 = gen.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));
        let sector2 = gen.generate_sector(SectorCoord(IVec3::new(1, 0, 0)));

        // Farklı koordinatlar = farklı sonuç (büyük ihtimalle)
        assert_ne!(sector1, sector2);
    }

    #[test]
    fn test_sector_boundary_continuity() {
        let gen = TerrainGenerator::from_seed(WorldSeed { seed: 12345, .. });

        let sector_a = gen.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));
        let sector_b = gen.generate_sector(SectorCoord(IVec3::new(1, 0, 0)));

        // Boundary'deki bloklar uyumlu olmalı
        for y in 0..128 {
            for z in 0..32 {
                let block_a = sector_a.get_block(IVec3::new(31, y, z));
                let block_b = sector_b.get_block(IVec3::new(0, y, z));

                assert_eq!(block_a, block_b);
            }
        }
    }
}
```

### 3.2 Physics Integration

```rust
#[cfg(test)]
mod physics_integration {
    use super::*;

    #[test]
    fn test_character_ground_check() {
        let mut sector = Sector::empty();

        // Zemin oluştur
        for x in 0..32 {
            for z in 0..32 {
                sector.set_block(IVec3::new(x, 0, z), Some(STONE));
            }
        }

        let controller = CharacterController::new();
        let ground = controller.ground_check_xbrickmap(
            &sector,
            Vec3::new(16.0, 2.0, 16.0),
            0.3,
        );

        assert!(matches!(ground, GroundState::Grounded { .. }));
    }

    #[test]
    fn test_character_air_check() {
        let sector = Sector::empty();

        let controller = CharacterController::new();
        let ground = controller.ground_check_xbrickmap(
            &sector,
            Vec3::new(16.0, 50.0, 16.0),
            0.3,
        );

        assert!(matches!(ground, GroundState::Air));
    }

    #[test]
    fn test_collider_incremental_update() {
        let mut sector = Sector::empty();
        let mut collider = sector.build_collider();

        // Tek blok ekle
        sector.set_block(IVec3::new(5, 10, 5), Some(STONE));
        let changes = sector.get_changes();

        sector.update_collider(&mut collider, &changes);

        // Collider güncellenmiş olmalı
        assert!(collider.is_valid());
    }
}
```

### 3.3 Network Integration

```rust
#[cfg(test)]
mod network_integration {
    use super::*;

    #[test]
    fn test_delta_encoding_roundtrip() {
        let mut encoder = DeltaEncoder::new();
        let entity = Entity::from_raw(1);

        let pos = Vec3::new(10.5, 20.3, 30.7);
        let rot = Quat::from_rotation_y(0.5);

        let encoded = encoder.encode_entity(entity, pos, rot);
        assert!(!encoded.is_empty());

        // Decode ve doğrula
        let decoded = decode_entity(&encoded);
        assert!((decoded.position.x - pos.x).abs() < 0.01);
    }

    #[test]
    fn test_quantization_precision() {
        let pos = Vec3::new(100.123, 50.456, -200.789);
        let quantized = QuantizedPosition::from_vec3(pos);
        let restored = quantized.to_vec3();

        // 1cm hassasiyet
        assert!((restored.x - pos.x).abs() < 0.01);
        assert!((restored.y - pos.y).abs() < 0.01);
        assert!((restored.z - pos.z).abs() < 0.01);
    }

    #[test]
    fn test_aoi_subscription() {
        let mut manager = InterestManager::new();
        let player = Entity::from_raw(1);

        // Oyuncuyu ekle
        manager.add_player(player, 100.0);

        // Pozisyon güncelle
        manager.update_player_position(player, Vec3::new(0.0, 0.0, 0.0));

        // Abonelikleri hesapla
        manager.update(0.016);

        // Yakın sector'lar abonelikte olmalı
        let subscriptions = manager.get_subscriptions(player);
        assert!(!subscriptions.is_empty());
    }
}
```

---

## 4. Benchmark Tests

```rust
#[cfg(test)]
mod benchmarks {
    use super::*;
    use criterion::{criterion_group, criterion_main, Criterion};

    fn benchmark_xbrickmap_get_block(c: &mut Criterion) {
        let mut sector = Sector::empty();

        // Dolu sector oluştur
        for x in 0..32 {
            for y in 0..128 {
                for z in 0..32 {
                    if rand::random::<f32>() > 0.5 {
                        sector.set_block(IVec3::new(x, y, z), Some(STONE));
                    }
                }
            }
        }

        c.bench_function("xbrickmap_get_block", |b| {
            b.iter(|| {
                let x = rand::random::<i32>() % 32;
                let y = rand::random::<i32>() % 128;
                let z = rand::random::<i32>() % 32;
                black_box(sector.get_block(IVec3::new(x, y, z)));
            });
        });
    }

    fn benchmark_xbrickmap_set_block(c: &mut Criterion) {
        let mut sector = Sector::empty();

        c.bench_function("xbrickmap_set_block", |b| {
            b.iter(|| {
                let x = rand::random::<i32>() % 32;
                let y = rand::random::<i32>() % 128;
                let z = rand::random::<i32>() % 32;
                black_box(sector.set_block(IVec3::new(x, y, z), Some(STONE)));
            });
        });
    }

    fn benchmark_lighting_propagation(c: &mut Criterion) {
        let mut engine = BlockLightEngine::new();
        let sector = Sector::empty();

        c.bench_function("lighting_propagation_level14", |b| {
            b.iter(|| {
                black_box(engine.place_light(
                    &sector,
                    IVec3::new(16, 64, 16),
                    15,
                    LightColor::Red,
                ));
            });
        });
    }

    fn benchmark_svdag_bake(c: &mut Criterion) {
        let sector = generate_test_sector();

        c.bench_function("svdag_bake", |b| {
            b.iter(|| {
                black_box(bake_sector_to_svgdag(&sector));
            });
        });
    }

    fn benchmark_pathfinding(c: &mut Criterion) {
        let world = generate_test_world();
        let mut pathfinder = VoxelPathfinder::new();

        c.bench_function("pathfinding_100_blocks", |b| {
            b.iter(|| {
                black_box(pathfinder.find_path(
                    IVec3::new(0, 64, 0),
                    IVec3::new(100, 64, 100),
                    &world,
                    1000,
                ));
            });
        });
    }

    criterion_group!(
        benches,
        benchmark_xbrickmap_get_block,
        benchmark_xbrickmap_set_block,
        benchmark_lighting_propagation,
        benchmark_svdag_bake,
        benchmark_pathfinding,
    );
    criterion_main!(benches);
}
```

---

## 5. CI/CD Pipeline

```yaml
# .github/workflows/ci.yml
name: CI

on:
  push:
    branches: [main, dev]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - name: Cache dependencies
        uses: actions/cache@v3
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Format check
        run: cargo fmt -- --check

      - name: Clippy
        run: cargo clippy --workspace -- -D warnings

      - name: Unit tests
        run: cargo test --workspace

      - name: Integration tests
        run: cargo test --workspace --test '*'

      - name: Benchmarks (quick)
        run: cargo bench -- --noplot

  benchmark:
    runs-on: windows-latest
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4

      - name: Run benchmarks
        run: cargo bench -- --output-format bencher | tee benchmark_results.txt

      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: benchmark_results.txt
```

---

## 6. Crate Organizasyonu

```
crates/
  core/
    src/
      # ... unit tests inline (#[cfg(test)] mod tests)

  tests/
    src/
      # Integration tests
      ├── world_gen.rs
      ├── physics.rs
      ├── network.rs
      ├── lighting.rs
      └── storage.rs

  benches/
    src/
      # Benchmark tests
      ├── xbrickmap.rs
      ├── svdag.rs
      ├── lighting.rs
      ├── pathfinding.rs
      └── storage.rs
```
