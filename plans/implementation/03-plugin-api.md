# 03 — Plugin API (M1)

**Kaynak:** `04-plugin-api.md`
**Hedef:** Prototip için minimal ama genişleyebilir plugin lifecycle.

## 1. Minimal Trait
```rust
pub trait StrataPlugin: Send + Sync {
    fn name(&self) -> &'static str { "unnamed" }
    fn build(&self, app: &mut App);
    // hooks (ileride): fn on_sector_load / on_block_change
}
```
- Prototipte sadece `build`. `on_add`/`on_remove` lifecycle'ları (`Add`/`Remove`) sonraki faz.

## 2. Registry (basit)
`App` extension: `app.add_strata_plugin(P)` → `P.build(app)` + ismi kaydet (debug HUD için).
- Plugin sırası: `Core -> World -> Meshing -> Physics -> Lighting -> Player -> Render`.

## 3. Granülerlik × Güven (04 §L0–L4) — prototip eşlemesi
| Seviye | Prototip kullanımı |
|--------|--------------------|
| L0 data | Block TOML (04) |
| L1 WASM | — (sonraki faz) |
| L2 native | — |
| L3 engine | Core plugin'ler |
| L4 unsafe | Yok |

## 4. Adımlar
1. `StrataPlugin` trait + `AddStrataPlugin` extension.
2. `StrataCorePlugins::new().add(P)` zinciri.
3. Her M*-plugin'ini bu trait üzerinden kaydet (02'deki stub'lar buraya taşınır).
4. Plugin adlarını `Resources` içinde `Vec<&str>` tut (diagnostics sonrası).

## 5. Doğrulama
- 3 plugin sırayla `build` olur; boot log sırası doğru.
- `cargo test`: plugin ekleme idempotent (çift ekleme guard).

## 6. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Plugin bağımlılık sırası | Açık `depends_on` sonraki faz; prototipte manuel sıra |
