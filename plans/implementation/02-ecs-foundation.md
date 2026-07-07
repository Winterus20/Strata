# 02 — ECS Foundation (M1)

**Kaynak:** `03-ecs-architecture.md`
**Hedef:** Plugin-first, data-oriented iskelet; system set sıralaması; change-detection disiplini.

## 1. Core Types (`strata_core`)
```rust
pub struct SectorCoord(pub i32, pub i32, pub i32); // 32³ sector
pub struct SectorTransform { pub coord: SectorCoord, pub tier: Tier }
pub struct SectorEntity { pub coord: SectorCoord } // spawn-only immutable
// ZST markers
pub struct ChunkDirty;
pub struct NeedsRemesh;
pub struct NeedsBake;
```
- `Tier` enum: prototipte sadece `Active` (08 minimal).

## 2. Plugin Trait (minimal)
`StrataPlugin` trait → `fn build(&self, app: &mut App)`. `StrataCorePlugins` alt-plugin'leri toplar (04 ile genişler).

## 3. System Sets (sıralama garantisi)
```rust
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum StrataSet {
    Input,          // pre
    WorldGen,       // generate sector data
    Meshing,        // build mesh (async apply)
    Physics,        // rapier step
    Lighting,       // L0/L1
    RenderUpdate,   // upload to GPU
}
```
`.configure_sets(...)链式`: `Input -> WorldGen -> Meshing -> Physics -> Lighting -> RenderUpdate`.

## 4. Filter-First Kuralları (AGENTS.md §3.A)
- **Yasak:** `query.iter().filter(|(e, c)| c.is_some())`.
- **Zorunlu:** `With<ChunkDirty>` / `Without<NeedsRemesh>` / ZST component filtresi.
- Query sonuçlarını `Query<&mut V, With<Dirty>>` ile daralt.

## 5. Change Detection
- `if *old != new { *old = new; }` veya `comp.set_if_neq(new)`.
- `mut` alan her sistemde guard; gereksiz `Changed<T>` tetikleme yok.
- Batch mutation: `bypass_change_detection()` yalnızca GPU upload gibi izlenmeyen write'larda.

## 6. Adımlar
1. `strata_core` types + marker'lar.
2. `StrataSet` + boş system stub'ları (sıralı, no-op).
3. `StrataCorePlugins` kayıt (world, meshing, physics, lighting, render placeholder).
4. Change-detection helper (`set_if_neq` wrapper) ekle.
5. Bir "hello" system: `ChunkDirty` olan sector sayısını logla (filter-first kanıtı).

## 7. Doğrulama
- `cargo test`: filter-first sistem sadece dirty sector'leri işler (unit test ile sahte entity).
- `cargo clippy` temiz; `mut` guard testi (değişmeyen atama `Changed` tetiklemez).

## 8. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| System ordering döngüsel | `configure_sets` ile açık DAG; erken test |
| ZST marker explode etmesi | Sadece gerçek state-transition'larda; over-marking yasak |
