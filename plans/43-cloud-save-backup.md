# 43 — Cloud Save & Backup

## 1. Genel Bakış

Strata'nın cloud save sistemi oyuncu verilerini otomatik yedekler ve bulut senkronizasyonu sağlar.

### Temel Prensipler

- **Auto-backup:** Belirli aralıklarla otomatik yedekleme
- **Sync:** Birden fazla cihaz arasında senkronizasyon
- **Conflict resolution:** Çakışan save'ler için çözüm stratejisi
- **Versioning:** Geçmiş save'lere geri dönüş

---

## 2. Cloud Save Manager

```rust
pub struct CloudSaveManager {
    pub provider: Box<dyn CloudProvider>,
    pub sync_interval: Duration,
    pub last_sync: Instant,
    pub pending_uploads: Vec<PendingUpload>,
}

pub trait CloudProvider {
    async fn upload(&self, key: &str, data: &[u8]) -> Result<()>;
    async fn download(&self, key: &str) -> Result<Vec<u8>>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
    async fn delete(&self, key: &str) -> Result<()>;
}

pub struct SaveVersion {
    pub timestamp: u64,
    pub size: u64,
    pub hash: String,
    pub is_cloud: bool,
}
```

---

## 3. Conflict Resolution

```rust
pub enum ConflictResolution {
    /// En yeni save'i kullan.
    UseNewest,

    /// Lokal save'i kullan.
    UseLocal,

    /// Cloud save'i kullan.
    UseCloud,

    /// Kullanıcıya sor.
    AskUser,

    /// Birleştir (mümkünse).
    Merge,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  cloud_save/
    ├── mod.rs
    ├── manager.rs
    ├── provider.rs
    ├── sync.rs
    ├── conflict.rs
    └── versioning.rs
```
