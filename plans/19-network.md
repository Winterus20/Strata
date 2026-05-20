# 08 — Network Senkronizasyonu

## 1. Network Senkronizasyonu (Replicon/Renet2)

### 1.1 Tier-Bazlı Delta Sync

| Tier | Sync Yöntemi | Paket Boyutu | Frekans |
|---|---|---|---|
| **ACTIVE** | Brick delta (sparse) | 10-50 byte/değişiklik | Anlık |
| **WARM** | Brick delta + SVDAG root | 10-50B + 4B | Anlık + periyodik |
| **DISTANT** | SVDAG root index | 4 byte | Snapshot |
| **ARCHIVE** | Compressed SVDAG | 1-5KB | Lazy load |

### 1.2 Brick Delta Formatı

```rust
#[repr(C)]
pub struct BrickDelta {
    pub sector: I16Vec3,
    pub brick_index: u8,
    pub changed_sub_bricks: u8,
    pub new_materials: Vec<u16>,
}

/// Ortalama değişiklik: ~10-20 byte
/// 100 değişiklik/saniye = ~1-2 KB/s bant genişliği
```

### 1.3 SVDAG Snapshot Sync

```rust
pub fn send_sector_snapshot(sector: &Sector, peer: &mut Peer) {
    if let Some(root_index) = sector.svdag_root {
        peer.send(SectorSnapshot {
            sector: sector.coord,
            root_index: root_index,
            subtree_data: node_pool.export_subtree(root_index),
        });
    }
}
```

---

### 1.4 Delta Compression + Quantization

**Quantization + delta encoding** ile bant genişliği **%85-90** azaltılır.

#### Position Quantization

```rust
#[repr(C)]
pub struct QuantizedPosition {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

impl QuantizedPosition {
    pub fn from_vec3(pos: Vec3) -> Self {
        Self {
            x: (pos.x * 100.0) as i16,
            y: (pos.y * 100.0) as i16,
            z: (pos.z * 100.0) as i16,
        }
    }

    pub fn to_vec3(&self) -> Vec3 {
        Vec3::new(
            self.x as f32 / 100.0,
            self.y as f32 / 100.0,
            self.z as f32 / 100.0,
        )
    }
}
```

#### Quaternion Compression

```rust
#[repr(C)]
pub struct CompressedQuaternion {
    pub largest_index: u8,
    pub a: i16,
    pub b: i16,
    pub c: i16,
    _padding: u8,
}

impl CompressedQuaternion {
    pub fn from_quat(q: Quat) -> Self {
        let abs = [q.x.abs(), q.y.abs(), q.z.abs(), q.w.abs()];
        let largest = abs.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;

        let mut components = [q.x, q.y, q.z, q.w];
        let sign = if components[largest] >= 0.0 { 1.0 } else { -1.0 };

        Self {
            largest_index: largest as u8,
            a: (components[(largest + 1) % 4] * sign * 32767.0) as i16,
            b: (components[(largest + 2) % 4] * sign * 32767.0) as i16,
            c: (components[(largest + 3) % 4] * sign * 32767.0) as i16,
            _padding: 0,
        }
    }
}
```

#### Delta Encoding

```rust
pub struct DeltaEncoder {
    last_positions: HashMap<Entity, QuantizedPosition>,
    last_rotations: HashMap<Entity, CompressedQuaternion>,
}

impl DeltaEncoder {
    pub fn encode_entity(&mut self, entity: Entity, pos: Vec3, rot: Quat) -> Vec<u8> {
        let quantized_pos = QuantizedPosition::from_vec3(pos);
        let compressed_rot = CompressedQuaternion::from_quat(rot);

        let mut buffer = Vec::new();

        if let Some(last_pos) = self.last_positions.get(&entity) {
            let dx = quantized_pos.x - last_pos.x;
            let dy = quantized_pos.y - last_pos.y;
            let dz = quantized_pos.z - last_pos.z;

            if dx.abs() < 128 && dy.abs() < 128 && dz.abs() < 128 {
                buffer.push(0x01);
                buffer.push(dx as u8);
                buffer.push(dy as u8);
                buffer.push(dz as u8);
            } else {
                buffer.push(0x02);
                buffer.extend_from_slice(&quantized_pos.x.to_le_bytes());
                buffer.extend_from_slice(&quantized_pos.y.to_le_bytes());
                buffer.extend_from_slice(&quantized_pos.z.to_le_bytes());
            }
        } else {
            buffer.push(0x00);
            buffer.extend_from_slice(&quantized_pos.x.to_le_bytes());
            buffer.extend_from_slice(&quantized_pos.y.to_le_bytes());
            buffer.extend_from_slice(&quantized_pos.z.to_le_bytes());
        }

        if let Some(last_rot) = self.last_rotations.get(&entity) {
            if compressed_rot != *last_rot {
                buffer.push(0x01);
                buffer.extend_from_slice(&std::slice::from_ref(&compressed_rot.largest_index));
                buffer.extend_from_slice(&compressed_rot.a.to_le_bytes());
                buffer.extend_from_slice(&compressed_rot.b.to_le_bytes());
                buffer.extend_from_slice(&compressed_rot.c.to_le_bytes());
            }
        }

        self.last_positions.insert(entity, quantized_pos);
        self.last_rotations.insert(entity, compressed_rot);

        buffer
    }
}
```

#### Bant Genişliği Karşılaştırması

| Veri | Ham (byte) | Quantized (byte) | Delta (byte) |
|---|---|---|---|
| **Position** | 12 (Vec3) | 6 (3×i16) | 1-3 (varint delta) |
| **Rotation** | 16 (Quat) | 8 (smallest-three) | 0-8 (sadece değişim) |
| **Velocity** | 12 (Vec3) | 6 (3×i16) | 1-3 (varint delta) |
| **Toplam/entity/frame** | **40** | **20** | **2-14** |

**Sonuç:** 100KB/s → **10-15KB/s** (600+ oyuncu desteklenir).

---

### 1.5 Interest Management / AOI (Area of Interest)

#### Spatial Partitioning

```rust
pub struct InterestManager {
    grid: SpatialGrid,
    aois: HashMap<Entity, f32>,
    subscriptions: HashMap<Entity, HashSet<SectorCoord>>,
}

pub struct SpatialGrid {
    cell_size: f32,
    cells: HashMap<IVec2, Vec<Entity>>,
    entity_cells: HashMap<Entity, IVec2>,
}
```

#### AOI Update

```rust
impl InterestManager {
    pub fn update(&mut self, dt: f32) {
        for (entity, aois) in &self.aois {
            let current_pos = self.get_entity_position(*entity);
            let current_cell = self.pos_to_cell(current_pos);

            let old_subscriptions = self.subscriptions.entry(*entity).or_default().clone();
            let mut new_subscriptions = HashSet::new();
            let radius_cells = (aois / self.grid.cell_size).ceil() as i32;

            for dx in -radius_cells..=radius_cells {
                for dz in -radius_cells..=radius_cells {
                    let cell = current_cell + IVec2::new(dx, dz);
                    let dist = (cell - current_cell).as_vec2().length() * self.grid.cell_size;

                    if dist <= *aois {
                        if let Some(entities) = self.grid.cells.get(&cell) {
                            for e in entities {
                                if let Some(sector) = self.get_entity_sector(*e) {
                                    new_subscriptions.insert(sector);
                                }
                            }
                        }
                    }
                }
            }

            let added: Vec<_> = new_subscriptions.difference(&old_subscriptions).collect();
            let removed: Vec<_> = old_subscriptions.difference(&new_subscriptions).collect();

            if !added.is_empty() || !removed.is_empty() {
                self.send_subscription_updates(*entity, &added, &removed);
            }

            self.subscriptions.insert(*entity, new_subscriptions);
        }
    }
}
```

#### Performans

| Metrik | AOI Yok | AOI (50-100m) | Azalma |
|---|---|---|---|
| **Bant genişliği** | 100KB/s/oyuncu | **10-20KB/s/oyuncu** | **-80-90%** |
| **Network packet** | Tüm sector'lar | Sadece yakın sector'lar | **-85%** |
| **Maks oyuncu** | ~100 | **600+** | **6×** |
| **Server CPU** | Yüksek | Düşük | **-70%** |
