# Strata Depolama ve Kayıt Kılavuzu (M11d + M11e)

Bu belge `strata_storage` (durable katman) ve `strata_save` (oyun durumu kaydı)
crate'lerinin operasyonel kullanımını açıklar. Tasarım gerekçeleri `plans/15-storage-and-persistence.md`
(anayasa) içindedir.

## 1. Kayıt Dosyası Yapısı

Dünya başına bir kayıt dizini:

```
%APPDATA%/Strata/saves/<world>/
├── world.dat          # WorldMetadata (SaveEnvelope, plan 15 §38 §4)
├── world.dat.bak      # Son iyi kopya (atomic write adım 3)
├── player.dat         # PlayerSaveData (SaveEnvelope, plan 15 §38 §3)
├── player.dat.bak
└── regions/           # Region dosyaları (strata_storage, plan 15 §1.2)
    ├── r.0.0.0.strata
    ├── r.1.-1.2.strata
    └── <world>.metadata/  # fjall keyspace (sector_metadata partition, §1.5)
```

Save konumu Windows'ta `std::env::var("APPDATA")` ile, yoksa
`home_dir()/Strata/saves` fallback'i ile bulunur.

### SaveEnvelope formatı (plan 15 §38 §2)

Her `.dat` dosyası bir `SaveEnvelope` taşır:

```
magic:        [u8; 4]   "STSV"
save_version: u32       (SAVE_FORMAT_VERSION=2, byte layout / hash domain)
generator_version: u32  (terrain algoritması versiyonu)
payload_hash: [u8; 32]  BLAKE3 — v2: header‖payload; v1 legacy: payload-only
payload_size: u32
signature:    [u8; 32]  placeholder (sıfırlı; hash'e dahil değil)
payload:      Vec<u8>   postcard::to_vec(&WorldMetadata | &PlayerSaveData)
```

`save_version` yalnızca on-disk byte layout / hash domain değişince bump edilir;
`generator_version` terrain algoritmasını belirtir (ikisi ayrı, plan 15 §38).

### F6 shutdown (client)

1. Yeni enqueue'i kes.
2. `AsyncStorageBackend::sync().await` — in-flight I/O bariyeri.
3. `AsyncStorageBackend::flush().await` — region dosyalarını fsync.
4. Dirty bit yalnız durable commit sonrası `DirtyTracker::clear`.

## 2. Atomic Yazım Sırası (plan 15 §38 §5)

Her kayıt 5 adımlı atomik yazımla diske iner:

1. `save.dat.tmp` yazılır.
2. `fsync(save.dat.tmp)` (NTFS/ext4 güvenli rename için).
3. Varsa `save.dat` → `save.dat.bak` kopyalanır (son iyi kopya korunur).
4. `rename(save.dat.tmp → save.dat)` (atomik).
5. Sonraki okumada BLAKE3 doğrulaması — bozuksa `.bak` geri yüklenir.

## 3. Yedekleme Stratejisi

- `.bak` dosyaları **her başarılı yazımda** güncellenir (adım 3). Bu, son iyi
  durumu korur; ek bir cron gerektirmez.
- Cloud/versioned backup (plan 15 §43) isteğe bağlı katman: idempotent anahtar
  = `client_uuid + hash`. Bu M11 kapsamı dışındadır.

## 4. Bozulma Kurtarma Prosedürü

Okuma sırasında `SaveEnvelope::open`:

1. BLAKE3 `payload_hash` doğrular.
2. Uyuşmazlıkta `.bak` dosyasını okur ve birincil konuma kopyalar.
3. `.bak` da yoksa `StorageError::Envelope` döner (sessiz çöp yükleme YOK).

Region dosyaları için bozulma `SectorHeader::verify` (BLAKE3 + xxHash64) ile
tespit edilir ve `StorageError::CorruptPayload` üretir; kurtarma metadata
store ile reconcile (plan 15 §1.5.1) yoluyla yapılır.

## 5. Migration Politikası (plan 15 §38 §2)

`save_version` bump edildiğinde bir `MigrationChain` adımı eklenir:

```rust
pub const CURRENT_SAVE_VERSION: u32 = 2;

pub struct MigrationChain {
    pub from: u32,
    pub to: u32,
    pub transform: fn(WorldMetadata) -> WorldMetadata,
}
```

Load akışı: `open → migrate (v1→v2→…→vN saf fonksiyon zinciri) → decode`.
Her migratör saf (no I/O, no global). v1→v2: identity metadata + header‖payload
hash domain'una re-pack (`SaveEnvelope`).

## 6. Performans Ayarı

- **WARM cache:** varsayılan **512 MB** byte bütçesi (`DEFAULT_BYTE_BUDGET`,
  moka S3-FIFO, plan 15 §1.1.1 / D5). Büyük dünyalarda yükseltilebilir.
- **Pristine atlama:** seed'den yeniden üretilebilir (dirty=false) sektörler
  diske **yazılmaz** (plan 15 §1.1.4) — en büyük I/O tasarrufu.
- **Async backend:** tek tokio runtime + priority channel; ACTIVE > WARM >
  DISTANT > ARCHIVE önceliği (plan 15 §1.4 / D8). Yazımlar `spawn_blocking`
  içinde buffered I/O ile yapılır (mmap kullanılmaz).
- **Tier sıkıştırma:** WARM=zstd-1, DISTANT=zstd-3, ARCHIVE=zstd-19 (§1.7).
- **Metadata store:** fjall birincil (LSM, yazma-ağır), bölge başına shard
  (redb 4 TiB tavanı nedeniyle). `dirty` kolonu kurtarma otoritesidir.
- **Dirty tracker:** sharded `AtomicU64` bitset, sticky flag; queue yalnız
  `SectorCoord` tutar (plan 15 §1.1.3).
