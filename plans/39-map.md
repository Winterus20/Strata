# 37 — Minimap & World Map

## 1. Genel Bakış

Strata'nın harita sistemi **minimap (HUD)** ve **tam dünya haritası** destekler. Keşfedilen alanlar kaydedilir.

### Temel Prensipler

- **Minimap:** Sol üst köşe, gerçek zamanlı
- **World Map:** Tam ekran harita (M tuşu)
- **Fog of war:** Keşfedilmemiş alanlar gizli
- **Waypoints:** İşaret noktası ekleme
- **Biome colors:** Her biome farklı renk

---

## 2. Minimap Component

```rust
#[derive(Component)]
pub struct Minimap {
    /// Pozisyon (ekran koordinatları).
    pub position: Vec2,

    /// Boyut.
    pub size: f32,

    /// Zoom seviyesi.
    pub zoom: f32,

    /// Rotasyon (oyuncu yönüne göre).
    pub rotate_with_player: bool,

    /// Görünürlük.
    pub visible: bool,
}
```

---

## 3. Map Data

```rust
pub struct MapData {
    /// Keşfedilen bloklar (2D top-down).
    pub explored: HashMap<ChunkCoord, ChunkMapData>,

    /// Waypoint'ler.
    pub waypoints: Vec<Waypoint>,

    /// Oyuncu pozisyonu.
    pub player_position: Vec2,
}

pub struct ChunkMapData {
    /// Renk buffer'ı (her blok 1 pixel).
    pub colors: Vec<[u8; 4]>,

    /// Keşfedildi mi?
    pub explored: bool,
}

pub struct Waypoint {
    pub name: String,
    pub position: Vec3,
    pub icon: WaypointIcon,
    pub color: [u8; 4],
}
```

---

## 4. Minimap Rendering

```rust
// Minimap texture olarak render edilir
// Her chunk'ın en üst bloğu renklendirilir
// Oyuncu ortada, etrafı zoom'a göre gösterilir
// Entity'ler (mob, oyuncu) ikon olarak gösterilir
```

---

## 5. Crate Organizasyonu

```
crates/
  map/
    ├── mod.rs
    ├── minimap.rs
    ├── world_map.rs
    ├── data.rs
    ├── waypoints.rs
    └── rendering.rs
```
