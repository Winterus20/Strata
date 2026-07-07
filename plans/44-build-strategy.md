# Build Strategy & Workspace Architecture

Bu belge, Strata projesinin derleme (build) stratejisini, kod organizasyonunu ve Cargo
optimizasyon kararlarını açıklamaktadır. Plan 44, 2026-07 tarihli teknik denetim (audit)
sonuçlarına göre güncellenmiştir; tüm öneriler 2025–2026 güncel Rust/Cargo/Bevy SOTA'sı ile
uyumludur.

## 1. Alınan Karar: Hybrid Cargo Workspace

Proje, kod tekrarını önlemek, derleme sürelerini kısaltmak ve istemci (client) ile sunucu
(server) mantığını güvenli bir şekilde ayırmak için **Hybrid Cargo Workspace** modelini
kullanacaktır.

Devasa monorepo araçları (Bazel, Buck2) yerine, Rust ekosisteminin yerel standardı olan
**Cargo** kullanılmaya devam edilecektir. Bazel/Buck2 yalnızca ~100k LOC / 100+ crate
ölçeğinde öder; Strata tek ürünlü bir engine olduğu için bu araçlar overkill'tir. Cargo'nun
performansı linker, cache ve profil ayarlarıyla maksimize edilecektir.

### 1.1 Workspace Kök Yapılandırması (Zorunlu)
- Kök `Cargo.toml` **virtual manifest** olacak ve `resolver = "2"` içerecek (feature
  unification davranışı için gerekli).
- Tüm ortak bağımlılıklar (bevy, serde, vb.) `[workspace.dependencies]` altında bir kez
  tanımlanacak; alt crateler sürümü buradan miras alacak.

## 2. Klasör ve Crate (Paket) Yapısı

```text
strata/
├── Cargo.toml               # Ana Workspace manifestosu (virtual, resolver = "2")
├── crates/                  # ORTAK KÜTÜPHANELER (Client ve Server paylaşır)
│   ├── strata_core/         # ECS componentleri, math, temel veri yapıları
│   ├── strata_network/      # QUIC/Replicon ağ mantığı ve Postcard paketleri
│   ├── strata_world/        # Voxel chunk mantığı, terrain generation (XBrickMap)
│   ├── strata_physics/      # Rapier fizik wrapper'ı
│   └── strata_render/       # YALNIZCA CLIENT: bevy_render/bevy_pbr/bevy_winit/bevy_audio
└── bin/                     # ÇALIŞTIRILABİLİR DOSYALAR
    ├── client/              # OYUN İSTEMCİSİ (Bevy App, strata_render'e bağımlı)
    │   ├── Cargo.toml
    │   └── src/main.rs      # Pencereli, grafikli başlatıcı
    └── server/              # OYUN SUNUCUSU (Headless)
        ├── Cargo.toml       # Bevy render/audio/winit özellikleri KAPALI
        └── src/main.rs      # GPU'suz, penceresiz (ScheduleRunnerPlugin) başlatıcı
```

### 2.1 Bağımlılık Yönü ve Headless Garantisi
- **Kural:** `bins → crates` yönü tek yönlüdür; crateler asla bir `bin`'e bağımlı olamaz.
- **Render kodu izole edilecek:** `bevy_render`/`bevy_pbr`/`bevy_winit`/`bevy_audio` gibi
  GPU/UI özellikleri yalnızca `strata_render` (client-only) crate'inde veya doğrudan
  `bin/client` içinde bulunur. Paylaşılan crateler (`strata_core`, `strata_world`,
  `strata_physics`, `strata_network`) `bevy = { default-features = false, features = [/* minimal */] }`
  ile derlenir.
- **Server headless güvencesi (KRİTİK):** Cargo, bir arada derlenen paketler arasında
  feature'ları *birleştirir (union)*. Kökte `cargo build` / `cargo build --workspace`
  çalıştırılırsa `bevy`, client+server feature'larının birleşimiyle derlenir ve server
  sessizce `bevy_render`/`bevy_audio`/`bevy_winit` çeker. Bu nedenle "hafif server" garantisi
  **yalnızca server izole derlendiğinde** geçerlidir.
  - Sunucu yalnızca `cargo build -p strata_server` ile derlenmeli.
  - Release sunucu hattı asla whole-workspace derlemesinin parçası olmamalı.
  - CI adımı: `cargo tree -e features -p strata_server | grep -c bevy_render` == 0
    kontrolü (veya server girişinde
    `#[cfg(feature = "bevy_render")] compile_error!("server must not enable render");`).
- Server app, `MinimalPlugins` + yalnızca gerekli pluginler ile kurulur (test/headless
  desenine uygun). Ağ için `bevy_replicon` (server-authoritative, headless-safe) değerlendirilebilir.

## 3. Derleme Hızlandırma (Cargo Steroids)

### 3.1 Hızlı Linker Kullanımı
- **Windows:** Rust'ın varsayılan (yavaş) MSVC `link.exe` linker'ı yerine `rust-lld`
  kullanılacak. Anahtar: `linker = "rust-lld.exe"` (genel `lld` değil). Kurulum için
  `rustup component add llvm-tools` belgelenmeli. PDB desteği `link.exe` ile eşdeğerdir;
  tek kısıt `/DEBUG:FASTLINK` desteklenmez.
- **Linux:** Rust 1.90 (2025-09) itibarıyla `rust-lld` x86_64'te **zaten varsayılan
  linker**; ek bir ayar gerekmez. Üstüne **`mold`** (en güvenli maksimum hız, lld'den
  ~1.5–2× hızlı) kurulabilir. Çok çekirdekli Linux kutuları için `wild` (Rust, lld'den
  ~2.5–2.8× hızlı) opsiyonel opt-in'dir — ancak `--gdb-index` üretmez (debugger başlangıcı
  yavaşlar). `gold` **kullanılmayacak** (deprecated).
- Linker yapılandırması `.cargo/config.toml` üzerinden hedef başına ayrı tablolarla verilir.

### 3.2 Cache (sccache) — Yalnızca CI
- `sccache` **`bin`, `dylib`, `cdylib`, `proc-macro` ve inkremental** kodları cache'leyemez;
  bir `bin` oyun + bol `proc-macro` (Bevy) içeren Strata'da yerel iterasyonu hızlandırmaz.
- **Değerli olduğu yer:** CI / cloud runner'lar (`CARGO_INCREMENTAL=0` + GHA/S3/Redis/GCS).
  Takım genelinde %50–80 rebuild azalması sağlar.
- Yerel dev için beklenmemeli. Modern alternatif: `kache` (2026, Apache-2.0).

### 3.3 Asıl Büyük Kaldıraçlar (Linker'dan daha önemli)
Bevy'ye özgü bu kaldıraçlar Strata'nın asıl derleme süresini belirler:
- `bevy/dynamic_linking` (en büyük tek kazanç; aşağıda §4.2).
- Nightly: `-Zshare-generics=y` (Bevy generic patlamasını vurur) + `-Zthreads=0`
  (paralel frontend). Windows'ta 65k sembol limiti nedeniyle `share-generics` devre dışı
  bırakılabilir.
- Dev codegen backend olarak **Cranelift** (~%30 daha hızlı derleme; debug bilgisi yok,
  Windows'ta crash riski, `dynamic_linking` ile uyumsuz).
- Hot crate tespiti: `cargo build --timings` + `cargo-llvm-lines` (Bevy'de `bevy_pbr`
  derlemenin ~%75'i).

## 4. Optimizasyon Seviyeleri ve Dev Profili

### 4.1 Split Optimizasyon Profili (Zorunlu)
Kendi kodumuz hızlı derlenir, bağımlılıklar bir kez optimize edilir:

```toml
[profile.dev]
opt-level = 1                  # kendi kodumuz: hızlı recompile + generic paylaşımı
debug = 1                      # yalnızca line tables; link yükünü azaltır
split-debuginfo = "unpacked"   # debug section'ları binary dışında
incremental = true             # dev varsayılanı; CI'da CARGO_INCREMENTAL=0
codegen-units = 256            # dev varsayılanı; yüksek tut (paralel codegen)
lto = false                    # dev'de asla açma

[profile.dev.package."*"]
opt-level = 3                  # bağımlılıklar: nadiren derlenir, hızlı çalıştır
```

**Kritik nüans:** `opt-level >= 2` olduğunda crate monomorfize generikleri *yeniden* üretir
→ iteratif rebuild yavaşlar. Bu yüzden **kendi kodumuz 3 yapılmamalı** (1 optimal). Bağımlılıkta
1 yerine 3 her zaman daha iyidir.

### 4.2 Dynamic Linking (Yalnızca Dev)
- `bevy/dynamic_linking`, Bevy'yi `dylib`'e çevirir; inkremental rebuild'de yalnızca kendi
  kodumuz relink edilir. Bevy 0.17/0.18'de hâlâ en büyük dev-iterasyon kazancıdır.
- **Asla release'de olmamalı.** Cargo'nun profile-spesifik feature'ı yok → `dev` feature'ı
  arkasına gizlenir:
  ```toml
  [features]
  dev = ["bevy/dynamic_linking"]
  ```
  Kullanım: `cargo run --features dev` (dev) / `cargo build --release` (varsayılan kapalı).
- **Windows tuzağı:** `dynamic_linking` açıkken performans optimizasyonları da açık olmalı,
  yoksa `LNK1189: library limit of 65535 objects exceeded`. Üretilen `.exe` hashed
  `bevy_dylib-<hash>.dll` + `std-<hash>.dll`'e bağımlıdır → `cargo run` dışında taşınmaz.
- **Cranelift ile uyumsuz** (crash); Cranelift konfigürasyonunda `dynamic_linking` kapalı olmalı.
- `mold` ile her zaman additive değildir; gerektiğinde `dynamic_linking` kapatılarak mold
  hızlandırılabilir.

### 4.3 Nightly Kaldıraçlar (Opsiyonel, Dev-Only)
```toml
# .cargo/config.toml (nightly bloğu varsayılan kapalı)
# [unstable]
# codegen-backend = true
# [profile.dev]
# codegen-backend = "cranelift"          # dynamic_linking KAPALI olmalı
# [profile.dev.package."*"]
# codegen-backend = "llvm"
# rustflags = ["-Zshare-generics=y", "-Zthreads=0"]
```
Toolchain **pinlenmeli** (stable 1.90+ veya bilinen iyi bir nightly); 1.95–1.96 nightly
Bevy derleme süresini geçici olarak 2× artırmıştı (#153910).

## 5. Build Orkestrasyonu ve Araçlar
- **İzole derleme giriş noktaları:** `cargo xtask` veya `make` hedefleri ile
  `cargo build -p strata_client` / `cargo build -p strata_server` sağlanır.
- **`cargo-hakari` çakışması (ÖNEMLİ):** `AGENTS.md` §6 P0'daki `cargo-hakari` önerisi bu
  planın headless garantisiyle **çelişir** — hakari workspace genelinde feature'ları
  birleştirerek bevy'yi bir kez (render/audio dahil) derler. Çözüm: hakari **bevy ve
  render-only crateleri unify set'ten çıkarmalı**, ya da yalnızca dev/CI tam derlemelerde
  çalıştırılmalı; server release hattı `-p strata_server` ile izole gitmeli. `AGENTS.md`
  P0 notu bu yönde güncellenmelidir.
- **`cargo-nextest`:** test çalıştırma hızı için.
- **`cargo-hack`:** CI'da per-package feature-matrix kontrolü (unification regresyonlarını
  yakalar).

## 6. Gelecek Adımlar
1. Root `Cargo.toml` oluştur (`resolver = "2"`, `[workspace.dependencies]`, `default-members`).
2. `crates/` ve `bin/` iskeletini birebir kur; `strata_render`'i client-only yap.
3. `.cargo/config.toml` linker tablolarını ekle (Windows `rust-lld.exe`, Linux `mold`).
4. `[profile.dev]` split profilini ve `dev` feature'lı `dynamic_linking`'i uygula.
5. CI pipeline: `-p strata_server` izole derleme + feature saflığı guard + sccache (CI-only).
6. `cargo-hakari` kullanılacaksa bevy'yi hariç tut; `AGENTS.md` P0'u senkronize et.
7. `cargo build --timings` ile hot crate'i tespit et, gerekirse ince ayar yap.
