# Strata Plan 02/03/04 — Alternatif Araştırma Raporu

**Tarih:** 6 Temmuz 2026  
**Kapsam:** `02-implementation.md`, `03-ecs-architecture.md`, `04-plugin-api.md`  
**Amaç:** Kesinleşmiş planlar (01–10) için internette daha iyi, daha verimli ve daha optimize alternatifler bulmak  

---

## 1. PLAN 02 — Uygulama Planı ve Crate Organizasyonu

### 1.1 Mevcut Yaklaşım Özeti

Plan 02, Strata'nın crate organizasyonunu flat `crates/` yapısı içinde domain bazlı gruplamayla tanımlıyor:
- `core/` (xbrickmap, registry, sector)
- `meshing/`, `render/`, `physics/`, `lighting/`, `network/`, `storage/`, `streaming/`, `audio/`, `ui/`, `commands/`

Her crate kendi plugin'ini ve sistem set'ini tanımlıyor. Uygulama sırası 6 faz, 30 hafta olarak belirlenmiş.

### 1.2 Araştırma Bulguları

#### A. Rust Workspace Best Practices (2025–2026)

**Kaynaklar:**  
- matklad, "Large Rust Workspaces" (https://matklad.github.io/2021/08/22/large-rust-workspaces.html)  
- paiOS ADR-008, "Workspace Layout for Hexagonal Engine" (https://docs.aurintex.com/architecture/adr/008-workspace-architecture/)  
- corrode.dev, "Tips For Faster Rust Compile Times" (https://corrode.dev/blog/tips-for-faster-rust-compile-times/)  
- cargo-crate-split tool (https://github.com/zenide/cargo-crate-split)

**Temel Bulgular:**

1. **Flat Layout En İyi:** matklad (rust-analyzer yazarı, 200K LOC) flat `crates/` yapısını öneriyor — Strata'nın mevcut yaklaşımı buyla uyumlu. ✅

2. **Coarse-Grained Domain Crates:** paiOS ADR-008, micro-crate patlamasını önlmek için "coarse-grained domain crates" (common, core, vision, audio, inference) öneriyor. Hexagonal Architecture (ports/adapters) domain crate içinde modül seviyesinde uyguluyor — crate sayısı düşük, iç yapı `domain/`, `ports/`, `adapters/` modülleriyle organize.

   **Strata Değerlendirmesi:** Strata'nın mevcut yaklaşımı zaten domain bazlı (core, meshing, render, physics, lighting). Bu paiOS pattern'ine yakın. Ancak Hexagonal `ports/` trait izolasyonu Strata'da eksik — plugin API trait'leri (`04`) bu rolü kısmen dolduruyor.

3. **Composition Root Pattern:** paiOS tek executable crate'te (`pai-engine`) dependency injection yapıyor. Strata'da `client` / `server` binary crate'leri bu rolde. ✅

4. **`cargo-hakari` Kritik:** Workspace-wide feature harmonization için `cargo-hakari` tool'u öneriliyor. Aynı dependency farklı feature setlerle birden fazla crate'te kullanıldığında %50 build time azaltma sağlıyor. Strata Bevy 0.18'in birçok sub-crate'ini farklı feature'larla kullanacak — bu tool'u erken aşamada entegre etmek büyük kazanç.

   **Öneri:** `cargo-hakari` workspace-hack crate'i Phase 1'de kur.

5. **Crate Split Timing:** "Premature abstraction is worse than a little duplication." 3–4 crate ile başla, coupling pain oluştuğunda böl. Strata'nın 12 crate'i başlangıç için fazla olabilir — Phase 1'de `core` ve `client`/`server` ile başla, Faz 2'de meshing/render'ı böl.

6. **Linker Optimizasyonu:** `mold` (Linux) veya `lld` (Windows/macOS) linker'ı, 5–30 saniye linking kazancı sağlar. Windows üzerinde MSVC+lld kullanılabilir.

#### B. Compile Time Optimization

**Kaynaklar:**  
- atharvapandey.com, "Lesson 10: Compile Time Optimization"  
- jendrikillner.com, "Rust Game Series - Part 9"  

**Temel Bulgular:**

1. **Incremental Build:** Workspace splitting, 50K LOC projede incremental build 45s → 8s azaltma. Strata 200K+ LOC hedefliyor — bu kritik.

2. **`opt-level = 2` for Dependencies:** Dev profile'da dependency'ler için opt-level=2, runtime perf'u artırırken iterative build'i yavaşlatmaz.

3. **`cargo-crate-split`:** Bu tool modül dependency graph'ı analiz eder ve circular dependency-free crate split önerisi verir. Strata mevcut modül yapısını bu tool'a vererek optimum split noktası bulunabilir.

4. **Cranelift Backend (dev):** Dev build'lerde 30-50% daha hızlı compilation sağlar. Production'da LLVM kullanılır.

#### C. Crate Boundary Design Patterns

**Kaynaklar:**  
- morgenthum.dev, "How to introduce layers into Bevy games"  
- cubething.dev, "bevy architecture 1 - plugin hierarchies"  
- JohnBasrai, "Explicit Module Boundary Pattern (EMBP)"  

**Temel Bulgular:**

1. **4-Layer Architecture (morgenthum):**  
   - Engine Modules (generic, game-agnostic)  
   - Game Core (shared contracts, events, markers — no logic)  
   - Game Features (each its own crate + plugin)  
   - Game Assembly (composition root, no logic)  

   **Strata Mapping:**  
   - Engine Modules → `StrataCorePlugins` (XBrickMap, Meshing, Render) ✅  
   - Game Core → `core` crate + shared event/component types — **Strata'da `strata-types` veya `strata-core` ayrı crate eksik**  
   - Game Features → `PlayerPlugin`, `NetworkPlugin`, etc. ✅  
   - Game Assembly → `client`/`server` binary ✅  

   **Öneri:** Game Core ayrı crate (`strata-core`) oluşturulmalı — tüm plugin'lerin paylaştığı event, component, trait tanımları burada. Mevcut plan'da bu `core` crate içinde hybrid (`xbrickmap` + `registry` + `sector`) — veri yapısı ve ortak tip ayrılmalı.

2. **Injectable Schedules:** morgenthum, her plugin'in kendi schedule set'lerini parametre olarak almasını öneriyor. Bevy'de şu an `SystemSet` enum'ları ile yapılıyor — Strata bu pattern'i zaten kullanıyor. ✅

3. **EMBP (Explicit Module Boundary Pattern):** Her crate `lib.rs` gateway dosyasıyla public API kontrol eder. Strata crate'leri için bu önerilir — internal modüller `pub(crate)` ile kilitlenir.

### 1.3 Spesifik Alternatifler ve Karşılaştırma

| Konu | Mevcut Yaklaşım | Alternatif | Artı | Eksi | Öneri |
|------|-----------------|-----------|------|------|-------|
| Crate sayısı | 12 crate başlangıç | 3–4 crate başlangıç, pain oluştuğunda böl | Daha hızlı ilk dev, daha az cognitive load | Geç bölme refactor maliyeti | **Phase 1: 3 crate** (strata-core, strata-engine, client/server); Faz 2'de böl |
| Dependency harmonization | Manuel feature yönetimi | `cargo-hakari` | %50 build time azaltma | Ek tool dependency, workspace-hack crate | **Kur** — Bevy'nin feature set'i karmaşık |
| Linker | MSVC default | `lld` | 5–30s linking kazancı | Windows'ta ek config | **Kur** — CI ve dev |
| Core types isolation | `core` crate içinde hybrid | Ayrı `strata-types` crate | Net API boundary, derin refactor güvenliği | Ek crate overhead | **Orta优先** — Phase 1'de yap |
| Hexagonal ports/adapters | Plugin trait'leri kısmen dolduruyor | Explicit `ports/` modül pattern | Adapter değiştirme kolaylığı | Boilerplate | **Phase 5+ değerlendir** — T2 native modding için zaten dispatcher pattern var |
| Build profiling | Yok | `cargo build --timings` + `sccache` | Bottleneck tespiti, cache | Ek tooling | **Phase 1'de kur** |

### 1.4 Önemli Bulgular ve Öneriler

1. **`cargo-hakari` erken kurulmalı.** Bevy 0.18'in 100+ sub-crate'i farklı feature'larla kullanılacak — bu olmadan duplicate compilation katlanır.
2. **Phase 1 crate sayısı 3'e düşürülmeli:** `strata-types` (ortak tipler, events), `strata-engine` (voxel çekirdek + meshing + render), `client`/`server`. Faz 2'de engine'i domain crate'lere böl.
3. **`lld` linker Windows CI'da kurulmalı.**
4. **`strata-types` crate'i oluşturulmalı** — mevcut `core` crate hybrid (veri yapısı + ortak tip); bunları ayırmak API boundary'yi netler.

---

## 2. PLAN 03 — ECS Mimarisi (Bevy ECS 0.18+)

### 2.1 Mevcut Yaklaşım Özeti

Plan 03, Bevy ECS 0.18+ üzerine DOD-first ECS mimarisini tanımlıyor:
- Archetype-based storage (default Table) + SparseSet geçici component'lar
- Filter-First query design (With<T>, Without<T> archetype-level)
- ZST marker component'lar (ChunkDirty, NeedsRemesh, etc.)
- Hot/Cold data split
- Change Detection guard pattern
- ComponentHooks lifecycle
- Immutable components
- Disabled component for distant chunks
- Event/Message system (EventWriter/EventReader)
- Plugin-first architecture (StrataCorePlugins + StrataPlugin)
- BlockEntity pattern (palette vs sparse entity)

### 2.2 Araştırma Bulguları

#### A. Bevy 0.16 → 0.17 → 0.18 API Değişiklikleri (Kritik!)

**Kaynaklar:**  
- Bevy 0.16 release notes (https://bevy.org/news/bevy-0-16/)  
- Bevy 0.17 release notes (https://bevy.org/news/bevy-0-17/)  
- 0.16→0.17 Migration Guide (https://bevy.org/learn/migration-guides/0-16-to-0-17/)  
- Bevy PR #23156 (reparenting perf)  

**EN KRİTİK BULGU — API ISIM DEĞİŞİKLİKLERI:**

Bevy 0.17, Event/Observer sistemini tamamen yeniden tasarladı:

| Bevy ≤0.16 | Bevy 0.17+ | Açıklama |
|------------|-----------|----------|
| `Event` (buffered) | **`Message`** | Buffered event artık "Message" — `MessageWriter`, `MessageReader`, `Messages<M>` |
| `Event` (observable) | **`Event`** | Sadece observer event'ler için |
| `Trigger<E>` | **`On<E>`** | Observer parametre değişimi |
| `OnAdd`, `OnRemove` | **`Add`**, **`Remove`**, **`Insert`**, **`Replace`**, **`Despawn`** | Lifecycle event isimleri |
| `commands.trigger_targets()` | **`world.trigger()`** + `EntityEvent` trait | Entity-targeted events |
| `Entity::PLACEHOLDER` | **`Option<Entity>`** | Global trigger target None olur |

**Strata Etkisi:** Plan 03 ve 04, Bevy 0.17+'nın **Message** terminology'sini kullanmıyor — `EventWriter`/`EventReader` kullanıyor. Strata 0.18+ hedefliyor, bu isimler 0.17'den itibaren `MessageWriter`/`MessageReader` olmalı.

**ÖNERİ:** Plan 03'teki tüm `EventWriter<T>` → `MessageWriter<T>`, `EventReader<T>` → `MessageReader<T>`, `add_event::<T>()` → `add_message::<T>()` olarak güncellenmeli. Observer event'ler (`PlayerDied` etc.) `Event` trait ile kalır.

#### B. SparseSet Storage Tartışması (Bevy Discussion #19164)

**Kaynak:** https://github.com/bevyengine/bevy/discussions/19164

**Bulgu:** Bevy topluluğu SparseSet storage'ı kaldırmayı tartışıyor. Sebep:
- SparseSet iteration Table'dan yavaş
- Entity-level sparse-set row map overhead O(num_entities × num_sparse_set_components)
- Alternatif: "Entity Kind" pattern — her entity türü için bit-set flag

**Strata Etkisi:** Plan 03, `ChunkDirty`, `NeedsRemesh`, `NeedsColliderUpdate`, `TierChange` gibi geçici component'lar için SparseSet öneriyor. Bevy 0.18+ SparseSet'i kaldırırsa:
- ZST marker'lar için `Option<T>` Table storage veya bit-set flag pattern kullanılmalı
- Bevy Discussion #19164'nin "entity kind" yaklaşımı: her entity türü bit-set ile filtrelenir

**ÖNERİ:** Phase 1'de SparseSet'i mevcut şekilde kullan (0.18 henüz kaldırmadı), ama Phase 3'te Bevy'nin kararını takip et. ZST marker'lar zaten 0 byte kaplar — SparseSet vs Table farkı ZST için minimal. **Gerçek risk:** `TierChange` gibi data-carrying SparseSet component'lar — bunlar için Table + `Option<T>` pattern'i değerlendir.

#### C. Observer Sıralama Sorunu

**Kaynak:** https://docs.rs/bevy/latest/bevy/ecs/observer/struct.Observer.html

**Bulgu:** Bevy 0.17+, aynı event'e birden fazla observer'ın sıralamasını garanti etmiyor — "arbitrary" sıralama. Ayrıca observer'ların command'ları hemen apply edilmiyor — tüm observerlar çalışır, sonra command'lar batch apply olur.

**Strata Etkisi:** Plan 04, Observer'ları "düşük frekanslı anlık olaylar" için kullanıyor (`PlayerDied`, özel kapı açılma). Birden fazla observer aynı event'e bağlanırsa sıralama garanti değil.

**ÖNERİ:** Observer'ları sadece gerçekten sıralama-bağımsız reaksiyonlar için kullan. Sıralama kritik ise one-shot system + scheduler pattern'i tercih et. Bu zaten plan 04'te öneriliyor (§5 "KRİTİK KARAR ÇİZGİSİ" tablosu).

#### D. Change Detection Optimizasyonu (Bevy Issue #23152, #21861)

**Kaynaklar:**  
- https://github.com/bevyengine/bevy/issues/23152  
- https://github.com/bevyengine/bevy/issues/21861  
- https://docs.rs/bevy/latest/bevy/ecs/change_detection/trait.DetectChangesMut.html  

**Bulgu:** Bevy change detection şu sorunlara sahip:
- `Changed<T>` filter "table scan" yapıyor — her entity'nin tick'ini tek tek kontrol eder
- Change ticks her component'te zorunlu — opt-in değil
- Render ve physics gibi kritik sistemler için yavaş

**Çözüm Yolları (Araştırma):**
1. **`bypass_change_detection()`**: Hot loop'ta Mut<T> üzerinden change tick bypass
2. **`set_if_neq()` / `replace_if_neq()`**: Değer gerçekten değişirse tick set
3. **"Entity Pages" approach (PR #23519)**: Change detection'ı page seviyesinde yapıyor, per-entity scan yerine
4. **`map_unchanged()`**: Struct'ın sadece bir field'ını değiştirirken gereksiz tick trigger önler

**Strata Etkisi:** Plan 03 change detection guard pattern'ini (`if new_val != *comp { *comp = new_val; }`) öneriyor. Bu `set_if_neq()` ile daha temiz yapılabilir.

**ÖNERİ:** Change detection guard'ı `Mut::set_if_neq()` veya `replace_if_neq()` ile uygulama. Render SubApp extract'te `bypass_change_detection()` kullan.

#### E. Reparenting Performance (Bevy PR #23156)

**Kaynak:** https://github.com/bevyengine/bevy/pull/23156

**Bulgu:** Bevy 0.17+, `update_reparented` sistemini O(n) per-frame scan'den O(1) per-event Observer reaction'a geçirdi. `Changed<ChildOf>` query → `On<Insert, ChildOf>` / `On<Remove, ChildOf>` observer.

**Strata Etkisi:** Strata'da entity hiyerarşisi kullanılırsa (region → sector), bu pattern performans kritik.

**ÖNERİ:** Sector entity'leri region parent'ı altında organize edilirse, `On<Insert, ChildOf>` observer ile spatial index bakım yap — O(changes) değil O(n).

#### F. ECS Benchmark Karşılaştırma (EnTT vs Flecs vs Bevy)

**Kaynaklar:**  
- https://github.com/abeimler/ecs_benchmark  
- https://ajmmertens.medium.com/building-an-ecs-data-oriented-hierarchies-62fb2847d100  
- https://medium.com/@jordangrilly/what-is-an-ecs-and-why-rust-a-deep-dive-into-data-oriented-game-engine-design-887680a5583a  

**Bulgu:** Archetype-based ECS (Flecs, Bevy, Unity DOTS) iteration'da SparseSet-based ECS'ten (EnTT) daha hızlı. Ancak structural change (add/remove component) SparseSet'te daha hızlı.

**Strata için:** Archetype-based Bevy ECS doğru seçim — chunk entity'leri bir kez oluşturulur, sık structural change yok. ZST marker'lar ek/kaldırma sıklığı SparseSet'e gerekçe veriyor ama Discussion #19164 bunu da kaldırma yönünde.

#### G. Archetype SoA vs OOP vs AoS Academic Benchmark (SAC 2026)

**Kaynak:** https://boyang.cs.uwm.edu/publication/sac2026_ECS.pdf  

**Bulgu:** Tower Defense simulation benchmark'ta Archetype SoA, OOP'e ~10× daha hızlı. SoA-PAR (parallel) 100+ entity/ms sürdürdü. Bevy ECS archetype-table SoA pattern'ine uyumlu.

**Strata:** Mevcut DOD yaklaşımı academically validated. ✅

#### H. Voxel Engine ECS Design Pattern (Unity Discussion)

**Kaynak:** https://discussions.unity.com/t/designing-a-voxel-structure-around-ecs/789222  

**Bulgu:** Per-block entity FELAKET — milyon archetype patlaması. Hibrit model (chunk entity + DynamicBuffer<Voxel> + sparse BlockEntity) Strata'nın BlockEntity pattern'iyle aynı.

**Strata:** Plan 03 §10.6.1 "State Ownership" tablosu bu konuyu doğru çözüyor — palette vs BlockEntity split. ✅

### 2.3 Spesifik Alternatifler ve Karşılaştırma

| Konu | Mevcut Yaklaşım | Alternatif | Artı | Eksi | Öneri |
|------|-----------------|-----------|------|------|-------|
| Event terminology | `EventWriter`/`EventReader` | `MessageWriter`/`MessageReader` (0.17+) | Bevy 0.17+ API uyumu | Migration | **Güncelle** — 0.17+ terminology'e geç |
| Observer lifecycle names | `OnAdd`, `OnRemove` | `Add`, `Remove`, `Insert`, `Replace`, `Despawn` (0.17+) | Kısa, okunaklı | Migration | **Güncelle** |
| Entity-targeted events | `commands.trigger_targets()` | `world.trigger()` + `EntityEvent` trait | Daha temiz API | Migration | **Güncelle** |
| SparseSet storage | Geçici component'lar SparseSet | Table + `Option<T>` veya entity-kind bit-set | Bevy geleceği uyumu, Discussion #19164 | ZST için fark minimal | **Phase 1: mevcut**; Phase 3: Bevy kararını takip |
| Change detection guard | `if new != old { comp = new }` | `Mut::set_if_neq()` / `replace_if_neq()` | Daha temiz, built-in | Yeni API öğrenme | **Güncelle** — set_if_neq kullan |
| Change detection bypass | Yok | `bypass_change_detection()` hot loop'ta | Render/physics extract'te critical | Riskli (yanlış kullanım) | **Render SubApp extract'te kullan** |
| Reparenting | O(n) scan | O(1) observer reaction (PR #23156) | Ölçeklenebilir | Bevy 0.17+ gerek | **Sector→Region hiyerarşisinde uygulama** |
| `map_unchanged()` | Yok | Struct field-level change detection | Gereksiz trigger önleme | Extra API complexity | **Phase 2+ değerlendir** |

### 2.4 Önemli Bulgular ve Öneriler

1. **BEVY 0.17+ API TERMINOLOGY GÜNCELLEMESI ZORUNLU:** `EventWriter` → `MessageWriter`, `EventReader` → `MessageReader`, lifecycle `OnAdd` → `Add`. Plan 03 ve 04 bu değişiklikleri henüz içermez.
2. **Change detection optimizasyonu:** `set_if_neq()` ve `bypass_change_detection()` render/physics SubApp extract'te kullanılmalı.
3. **SparseSet geleceği:** Bevy Discussion #19164 SparseSet kaldırma eğiliminde. Phase 1'de mevcut pattern'i kullan, Phase 3'te Bevy'nin kararını takip et. ZST marker'lar için fark minimal.
4. **Observer sıralama:** Birden fazla observer aynı event'e bağlanırsa sıralama garanti değil. Sıralama-bağımsız reaksiyonlar için observer, sıralama-kritik için scheduler + one-shot system.
5. **Reparenting observer pattern:** Sector entity'leri region parent altında organize edilirse `On<Insert, ChildOf>` observer ile spatial index bakım O(1) per-event yapılabilir.

---

## 3. PLAN 04 — Plugin API & Modüler Mimari Sistemi

### 3.1 Mevcut Yaklaşım Özeti

Plan 04, Strata'nın plugin-first mimarisini tanımlıyor:
- SubApp izolasyonu (Render, Physics) + extract/write-back
- StrataCorePlugins (bootstrap) + StrataPlugin (oyun katmanı) + ModdingPlugin
- SystemSet scheduling (StrataSets, StrataPhysicsSets)
- Event/Observer ayrımı (sıcak: Message, soğuk: Observer)
- L0–L4 katman modeli (çekirdek native → dispatcher → strateji registry → WASM policy hook → data pack)
- T0/T1/T2 güven kademesi
- Dispatcher + Strateji Registry pattern (FluidSolverRegistry, etc.)
- WASM modding: wasmtime 45.0, dar host yüzeyi (8 fonksiyon), SegQueue drain
- ComponentHooks lifecycle
- ZST dirty-flag lifecycle (3 aşama: mark, consume, clear)
- RON config management

### 3.2 Araştırma Bulguları

#### A. Bevy Plugin Architecture Patterns (2025–2026)

**Kaynaklar:**  
- morgenthum.dev, "How to introduce layers into Bevy games" (https://morgenthum.dev/blog/introduce-layers-into-bevy/)  
- cubething.dev, "bevy architecture 1 - plugin hierarchies" (https://www.cubething.dev/posts/2025-05-16----architecture-1---plugin-hierarchies)  
- tbillington/bevy_best_practices (https://github.com/tbillington/bevy_best_practices)  
- bevy-cheatbook, "Plugin System and Modularity" (https://deepwiki.com/bevy-cheatbook/bevy-cheatbook/2.3-plugin-system-and-modularity)  

**Bulgu:** Modern Bevy plugin pattern'leri:

1. **Plugin Hierarchy (cubething):**  
   - Her crate bir major feature plugin  
   - Crate içinde her submodule kendi sub-plugin  
   - Exposed plugin = sub-plugin'lerin aggregate'i  
   - Feature flags ile modüler compile  
   
   **Strata Mapping:** Strata'nın `StrataCorePlugins` PluginGroup + `StrataPlugin` ayrımı bu pattern'e uyumlu. ✅

2. **Layered Architecture (morgenthum):**  
   - Engine Modules → Game Core → Game Features → Game Assembly  
   - Injectable schedules: her plugin schedule set'lerini parametre olarak alabilir  
   
   **Strata Mapping:** Strata'nın iki katmanlı yükleme (Core + Game) morgenthum'un 4-layer'ının basitleştirilmiş versiyonu. Game Core crate eksik (bkz. Plan 02 önerisi).

3. **SystemSet Contract API:** Plugin yazarları public SystemSet enum'larını export eder — consumer'lar `.before()` / `.after()` ile bu set'lere hook eder. Implementation detail sistem fonksiyonları gizli kalır.

   **Strata Mapping:** Plan 03 §5.1 SystemSet enum'ları bu pattern'i uyguluyor. ✅ Ancak cross-plugin `.before()` / `.after()` bağlantıları `StrataPlugin::build` içinde yapılıyor — bu doğru.

4. **Bevy Best Practices (tbillington):**  
   - "All Update systems should be bound by run conditions on State and SystemSet"  
   - Plugin struct = config + lifecycle; simple function plugin only for internal use  
   - Library authors MUST expose Plugin struct (not function) — breaking change without struct  

   **Strata:** Tüm Strata plugin'leri struct-based (`StrataMeshingPlugin`, etc.). ✅

#### B. SubApp İzolasyonu Alternatifleri

**Kaynaklar:**  
- Bevy docs.rs SubApp (https://docs.rs/bevy/0.18.1/bevy/prelude/struct.SubApp.html)  
- Bevy issue #15841 (SubApp update_schedule zorunlu)  
- Stratkit DOTS Guide (https://www.stratkit.dev/Developer%20Guide/DOTS%20Guide/Target%20Architecture/)  

**Bulgu:**

1. **Stratkit 3-Layer Architecture:**  
   - DOTS Layer (pure ECS, simulation)  
   - Managed Layer (OOP, UI, async, sound)  
   - Bridge Layer (data pipe between two)  
   
   **Strata Mapping:** Strata'nın SubApp pattern'i Stratkit'in DOTS/Managed/Bridge ayrımına benzer:
   - Physics SubApp = DOTS Layer  
   - Main App = Managed Layer  
   - Extract + Write-back = Bridge Layer  

   **ÖNERI:** Stratkit'in Bridge Layer interface pattern'i Strata'nın write-back channel'ına eklenmesi değerlendir — Bridge layer'da system-based interface exposure (query + public method) consumer'lar için daha temiz.

2. **Bevy SubApp `update_schedule` ZORUNLU:** Issue #15841'de, `update_schedule` atanmazsa SubApp sistemleri hiç koşmaz. Plan 04 §2'de bu doğru şekilde handle edilmiş (`render_app.update_schedule = Some(Main.intern())`). ✅

#### C. WASM Modding Alternatifleri (2025–2026)

**Kaynaklar:**  
- WebAssembly in 2026 guide (https://masturbyte.com/wasm-2026.html)  
- Systemshardening.com, "WASM sandboxing articles"  
- tartanllama.xyz, "Building Native Plugin Systems with WebAssembly Components"  
- benw.is, "Plugins with Rust and WASI Preview 2"  
- wasm-game / wasm-components-test-zelda (GitHub)  
- Wasvy (https://github.com/ProffDea/wasvy) — Bevy WASM modding engine  

**EN KRİTİK BULGU — WASM COMPONENT MODEL:**

WASI Preview 2 (stable, Jan 2026) + Component Model 1.0 (2026) completely改变了WASM plugin architecture:

1. **WIT Interface Definition:** `.wit` dosyaları ile language-agnostic API contract tanımlanır. `record`, `list`, `variant`, `option`, `result`, `flags`, `resource` tipleri — C ABI sınırları aşılır.

2. **Component Model vs Raw Module:**  
   - Raw Module: `func_wrap` + raw pointer ABI → unsafe, boilerplate-heavy  
   - Component Model: WIT typed interface → safe, `wit-bindgen` generates both host & guest bindings  

   **Strata Etkisi:** Plan 04 §7.1 "8 host fonksiyon" MVP raw pointer ABI kullanıyor. Plan 04 §10.5 "WASM Component Model + WIT geçişi" ileride değerlendirilecek olarak listelenmiş. Araştırma bunun **şimdi** planlanması gerektiğini gösteriyor — raw pointer ABI uzun vadede unsustainable.

3. **WASI Preview 2 / 3:**  
   - Preview 2 stable (Jan 2026) — capability-based, no ambient authority  
   - Preview 3 (draft) — async I/O, streams, futures  
   - Wasmtime 20+ (May 2026) full WASI P2 support  
   - Strata hedef wasmtime 45.0 — bu P2+ tamamen destekler  

4. **Wasvy — Bevy WASM Modding Engine:**  
   Wasvy, Bevy + wasmtime ile WASM modding sağlayan experimental proje:
   - Hot reloading  
   - WASI support  
   - ECS access from WASM  
   - `ModloaderPlugin` ile easy integration  
   
   **Strata Etkisi:** Wasvy Strata'nın WASM modding plan'ına (32-modding.md) çok benzer. Ancak Wasvy'nin ECS access WASM'a açılması Strata'nın "GPU/network/tam-ECS WASM'a kapalı" kuralıyla çelişiyor.

5. **Sandboxing Hardening (Production):**  
   - Fuel metering: CPU instruction budget per call  
   - Memory caps: ResourceLimiter trait  
   - Epoch interrupts: cooperative wall-clock timeout  
   - Capability allowlists: Linker'de sadece granted WASI interfaces  
   - Import-level access control: Parse WASM imports before instantiation, reject unapproved imports  

   **Strata Mapping:** Plan 04 §7.1 "bütçe" (frame budget, host-call limit) mevcut. Production hardening pattern'leri (fuel, epoch, memory cap) Strata'nın `WasmModBudgetConfig`'ına eklenmeli.

6. **Zero-Copy Linear Memory (Strata zaten var):** Plan 04 §10.1 "zero-copy linear memory" SegQueue pattern. Component Model'in limitation: "If your host has to share a lot of data with plugins without copying, the Component Model in its current state may not be the best fit" — Sy Brand'in uyarısı.

   **Strata Etkisi:** Strata'nın `get_blocks_region` zero-copy bulk I/O pattern'i Component Model'in serialization overhead'iyle çelişebilir. Çözüm: bulk data transfer'de Component Model'in `list<u8>` linear buffer pass-through'u + host-side memory slice sharing.

#### D. Unreal Engine Module / Plugin Architecture

**Kaynaklar:**  
- Unreal Engine 5.8 Modules docs (https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-modules)  
- UE FModuleManager architecture analysis (https://imzlp.com/posts/24007/)  
- cyubeVR UE modding case study  

**Bulgu:**

1. **UE Module Loading Phases:** `PostConfigInit` → `PreDefault` → `Default` → `PostDefault` — runtime dynamic loading via `FModuleManager::LoadModule`. Interface classes for loose coupling.

   **Strata Mapping:** Strata'nın iki katmanlı yükleme (Core → Game → Modding) UE'nin loading phase'lerinin basitleştirilmiş versiyonu. UE'nin `LoadingPhase` konfigürasyonu Strata'da yok — plugin yükleme sırası hardcoded.

   **ÖNERI:** Plugin loading phase'leri RON config'ten configurable yap — özellikle mod loading sırası runtime'da değişebilir.

2. **UE Dynamic vs Static Module Loading:** Static = `PrivateDependencyModuleNames` (auto-loaded); Dynamic = `DynamicallyLoadedModuleNames` + interface class.

   **Strata Mapping:** Strata T2 native plugin'leri static (compile-time birlikte derleme). Dynamic loading Rust'ta güvenli değil (dlclose issue) — plan 04 §10.3 bu reddi doğru.

3. **cyubeVR Voxel Modding:**  
   - 3 mod türü: UE pak mods, C++ API template (DLL), custom blocks (data-only)  
   - Voxel system C++ bespoke, blueprint modlar voxel'e erişemez  
   - C++ API: template project ile exposed functions, mod DLL çağrılır  
   
   **Strata Mapping:** cyubeVR'nin 3-tier mod sistemi Strata'nın T0/T1/T2 modeline çok benzer:
   - Pak mods → T0 data pack  
   - C++ API DLL → T2 native (cyubeVR Rust'ta olmadığı için DLL; Strata compile-time native)  
   - Custom blocks → T0 data pack  
   - Blueprint (voxel erişmez) → T1 WASM (limited access) ✅

#### E. Godot GDExtension Pattern

**Kaynaklar:**  
- Godot 4.4 GDExtension docs  
- godotengine.org, "Introducing GDExtension"  

**Bulgu:** GDExtension C-based interface, runtime dynamic library loading, `.gdextension` manifest file. Initialization levels: Core → Servers → Scene → Editor. ClassDB registration.

**Strata Mapping:** GDExtension runtime dylib loading + manifest pattern. Strata T2 native plugin'ler compile-time birlikte derleniyor — GDExtension'in runtime loading modeli Rust'ta güvenli değil (plan 04 §10.3 reddi doğru). Ancak GDExtension'in initialization level (Core → Scene → Editor) Strata'nın SubApp scheduling'ine benzer.

#### F. Luanti (Minetest) Voxel Engine Architecture

**Kaynak:** https://devuly.com/luanti-open-source-voxel-engine-architecture-lua-mod-development/  

**Bulgu:** Luanti C++ çekirdek + Lua mod katmanı ayrımı:
- Engine: rendering, networking, mapgen, physics, filesystem  
- Lua mods: gameplay rules, node registration, recipes  
- `mod.conf` manifest + `init.lua` entry point  

**Strata Mapping:** Luanti'nin C++/Lua ayrımı Strata'nın native/WASM ayrımına benzer. Ancak Lua interpreter WASM'den çok daha hızlı (no sandbox overhead) ama güvenlik yok. Strata'nın WASM sandboxing daha güvenli.

#### G. Flecs + Unreal Voxel Plugin (dorizzdt)

**Kaynak:** https://medium.com/mossyblog/building-a-binary-voxel-grid-that-doesnt-suck-and-plays-nice-with-unreal-and-flecs-b875a4862200  

**Bulgu:** Flecs ECS ile voxel sim, Unreal ile render, interop layer ile bridge. Chunk visibility query <50ns, full chunk recompute 6ms worst case. Memory per chunk ~37KB, 30% reduction over traditional 2-byte-per-voxel.

**Strata Mapping:** Flecs+Unreal split Strata'nın ECS+SubApp split'ine benzer. Strata'nın XBrickMap palette compression daha iyi (~8 byte vs 2 byte per brick for uniform).

### 3.3 Spesifik Alternatifler ve Karşılaştırma

| Konu | Mevcut Yaklaşım | Alternatif | Artı | Eksi | Öneri |
|------|-----------------|-----------|------|------|-------|
| WASM API interface | 8 raw host fonksiyon MVP | WIT Component Model | Typed, safe, language-agnostic, sustainable | Serialization overhead, zero-copy limitation | **WIT planlamayı şimdi başlat** — MVP raw fonksiyon ile devam, ama WIT interface spec Phase 5'te hazır olmalı |
| WASM sandbox hardening | Frame budget only | Fuel metering + epoch interrupt + memory cap + import access control | Production-grade güvenlik | Ek config complexity | **Phase 5'te implement** — WasmModBudgetConfig'e fuel/epoch/memory ek |
| WASM hot reload | Yok | Wasvy pattern + cargo-component + file_watcher | Dev iteration speed | Stability risk | **Phase 5+ değerlendir** — Bevy `file_watcher` ile WASM hot-reload |
| Plugin loading phase | Hardcoded sıra | Configurable loading phase (RON) | Mod loading sırası runtime configurable | Ek config | **Phase 5+ değerlendir** |
| Event terminology | `EventWriter`/`EventReader` | `MessageWriter`/`MessageReader` | 0.17+ API uyumu | Migration | **Güncelle** (bkz. Plan 03) |
| SubApp bridge | PhysicsWriteBackChannel (SegQueue) | Stratkit Bridge Layer system-based interface | Consumer query + public method | Extra boilerplate | **Phase 2+ değerlendir** — mevcut SegQueue pattern basit ve çalışıyor |
| Dispatcher + Registry | FluidSolverRegistry pattern | Hexagonal ports/adapters within crate | Adapter swap kolaylığı | Module-level boilerplate | **Phase 5+ değerlendir** — T2 native modding için dispatcher zaten doğru |

### 3.4 Önemli Bulgular ve Öneriler

1. **WIT COMPONENT MODEL GEÇIŞI PLANLANMALI:** Raw pointer `func_wrap` ABI uzun vadede unsustainable. WIT interface spec Phase 5'te hazır olmalı, Phase 6'da geçiş. MVP (8 fonksiyon) ile devam, ama WIT world tanımı `32-modding.md`'de şimdi tasarlanmalı.
2. **WASM PRODUCTION HARDENING:** Fuel metering, epoch interrupt, memory cap, import-level access control Phase 5'te implement. Mevcut `WasmModBudgetConfig` sadece frame budget — bu yetersiz.
3. **BEVY 0.17+ MESSAGE TERMINOLOGY:** `EventWriter` → `MessageWriter`, `EventReader` → `MessageReader`. Observer lifecycle `Add`, `Remove`, `Insert`, `Replace`, `Despawn`.
4. **WASM ZERO-COPY BULK DATA:** Component Model serialization overhead ile `get_blocks_region` zero-copy çelişebilir. Çözüm: `list<u8>` linear buffer pass-through + host-side memory slice sharing.
5. **Plugin loading phase configurable:** RON config'ten mod loading sırası configurable — özellikle multiplayer server'da mod allowlist sırası.

---

## 4. GENEL DEĞERLENDIRME VE PRIORITIZED ÖNERILER

### 4.1 High Priority (Phase 1–2)

1. **Bevy 0.17+ API terminology güncellemesi** — Plan 03/04'deki `EventWriter`/`EventReader` → `MessageWriter`/`MessageReader`, lifecycle event isimleri
2. **`cargo-hakari` kurulumu** — Workspace feature harmonization
3. **`lld` linker kurulumu** — Windows CI + dev
4. **`strata-types` crate ayrımı** — Ortak tipler, events, trait tanımları ayrı crate
5. **Change detection optimization** — `set_if_neq()`, `bypass_change_detection()` render/physics SubApp'te

### 4.2 Medium Priority (Phase 3–4)

1. **SparseSet geleceği takip** — Bevy Discussion #19164 kararını izle, ZST marker'lar için alternative pattern hazırla
2. **Observer sıralama dikkat** — Sıralama-bağımsız reaksiyonlar için observer, sıralama-kritik için scheduler
3. **Reparenting observer pattern** — Sector→Region hiyerarşisinde O(1) per-event spatial index bakım

### 4.3 Long-term (Phase 5+)

1. **WIT Component Model geçişi** — Raw pointer ABI → typed WIT interface. MVP ile devam ama WIT spec şimdi tasarla
2. **WASM production hardening** — Fuel, epoch, memory cap, import-level access control
3. **WASM hot-reload** — Wasvy pattern + cargo-component + Bevy file_watcher
4. **Hexagonal ports/adapters within crate** — T2 native modding için adapter swap kolaylığı
5. **Plugin loading phase config** — RON config'ten mod loading sırası

### 4.4 Mevcut Yaklaşımla Uyumlu (Değişiklik Gerekmez)

- Flat `crates/` layout ✅ (matklad validated)
- Domain-based crate grouping ✅ (paiOS validated)
- Archetype-based ECS ✅ (academic benchmark validated)
- BlockEntity hybrid pattern ✅ (Unity community validated)
- SubApp izolasyonu ✅ (Stratkit Bridge Layer pattern validated)
- Dispatcher + Strateji Registry ✅ (Flecs/unreal pattern validated)
- T0/T1/T2 güven kademesi ✅ (cyubeVR 3-tier validated)
- Compile-time T2 native ✅ (Rust ABI stability issue validated)

---

## 5. REFERANSLAR

| Kaynak | URL | Kategori |
|--------|-----|----------|
| matklad, Large Rust Workspaces | https://matklad.github.io/2021/08/22/large-rust-workspaces.html | Crate organizasyonu |
| paiOS ADR-008, Hexagonal Engine Workspace | https://docs.aurintex.com/architecture/adr/008-workspace-architecture/ | Crate organizasyonu |
| corrode.dev, Rust Compile Times | https://corrode.dev/blog/tips-for-faster-rust-compile-times/ | Build optimization |
| cargo-crate-split | https://github.com/zenide/cargo-crate-split | Crate splitting |
| cargo-hakari | https://docs.rs/cargo-hakari/latest/cargo_hakari/ | Feature harmonization |
| Bevy 0.17 release notes | https://bevy.org/news/bevy-0-17/ | ECS API changes |
| 0.16→0.17 Migration Guide | https://bevy.org/learn/migration-guides/0-16-to-0-17/ | ECS API changes |
| Bevy Discussion #19164, Remove SparseSet | https://github.com/bevyengine/bevy/discussions/19164 | ECS storage |
| Bevy PR #23156, reparenting perf | https://github.com/bevyengine/bevy/pull/23156 | Observer optimization |
| Bevy Issue #23152, Change Detection | https://github.com/bevyengine/bevy/issues/23152 | Change detection |
| Bevy Issue #21861, Raw table iteration | https://github.com/bevyengine/bevy/issues/21861 | Query optimization |
| DetectChangesMut docs | https://docs.rs/bevy/latest/bevy/ecs/change_detection/trait.DetectChangesMut.html | Change detection API |
| ECS benchmark (abeimler) | https://github.com/abeimler/ecs_benchmark | ECS performance |
| Flecs hierarchy article (Sander Mertens) | https://ajmmertens.medium.com/building-an-ecs-data-oriented-hierarchies-62fb2847d100 | ECS hierarchies |
| SAC 2026 ECS benchmark paper | https://boyang.cs.uwm.edu/publication/sac2026_ECS.pdf | Academic validation |
| morgenthum, Bevy layers | https://morgenthum.dev/blog/introduce-layers-into-bevy/ | Plugin architecture |
| cubething, Bevy plugin hierarchies | https://www.cubething.dev/posts/2025-05-16----architecture-1---plugin-hierarchies | Plugin architecture |
| tbillington, Bevy best practices | https://github.com/tbillington/bevy_best_practices | Plugin patterns |
| EMBP pattern | https://github.com/JohnBasrai/architecture-patterns/blob/main/rust/embp.md | Module boundaries |
| WASM 2026 guide | https://masturbyte.com/wasm-2026.html | WASM ecosystem |
| WASM sandboxing | https://www.systemshardening.com/articles/wasm/user-provided-wasm-execution/ | WASM security |
| WASM Component Model plugins (Sy Brand) | https://tartanllama.xyz/posts/wasm-plugins/ | WASM plugins |
| WASI Preview 2 plugins | https://benw.is/posts/plugins-with-rust-and-wasi | WASM plugins |
| WIT specification | https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md | WASM interface |
| wit-bindgen | https://github.com/bytecodealliance/wit-bindgen/ | WASM tooling |
| Wasvy (Bevy WASM modding) | https://github.com/ProffDea/wasvy | WASM Bevy integration |
| wasm-game (Component Model) | https://github.com/mytechnotalent/wasm-game | WASM game example |
| UE5 Modules docs | https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-modules | UE plugin system |
| UE FModuleManager analysis | https://imzlp.com/posts/24007/ | UE module loading |
| cyubeVR UE modding | https://buckminsterfullerene02.github.io/dev-guide/CaseStudies/cyubeVR.html | UE voxel modding |
| Godot GDExtension docs | https://docs.godotengine.org/en/4.4/tutorials/scripting/gdextension/what_is_gdextension.html | Godot plugins |
| Luanti voxel architecture | https://devuly.com/luanti-open-source-voxel-engine-architecture-lua-mod-development/ | Voxel modding |
| Flecs+Unreal voxel (dorizzdt) | https://medium.com/mossyblog/building-a-binary-voxel-grid-that-doesnt-suck-and-plays-nice-with-unreal-and-flecs-b875a4862200 | Flecs voxel |
| Unity DOTS ECS guide (2026) | https://dev.to/linou518/unity-ecs-and-dots-a-practical-performance-architecture-guide-for-indie-developers-in-2026-4jm8 | Unity ECS |
| Stratkit DOTS Guide | https://www.stratkit.dev/Developer%20Guide/DOTS%20Guide/Target%20Architecture/ | DOTS bridge layer |
| Hot reloading Rust gamedev | https://rygoldstein.com/posts/hot-reloading-rust | Hot reload |
| Wasmtime production hardening | https://www.systemshardening.com/articles/wasm/wasmtime-production-hardening/ | WASM security |
| WASM platform extension security | https://www.systemshardening.com/articles/wasm/wasm-platform-extension-security/ | WASM security |
| bitarena (arena alternative) | https://github.com/mehdiakiki/bitarena | Arena data structure |
| Aokana GPU voxel rendering | https://arxiv.org/html/2505.02017v1 | Voxel rendering research |
| Transform-Aware SVDAG | https://doi.org/10.1145/3728301 | SVDAG research |
| SVDAG Compression (VMV 2024) | https://diglib.eg.org/items/47beb00d-d8e4-4d26-97d0-d2217818d7e8 | SVDAG research |
