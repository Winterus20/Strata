# 49 — Update & Patch System

## 1. Genel Bakış

Strata'nın güncelleme sistemi oyun dosyalarını güvenli şekilde günceller. Delta patching ile bant genişliği tasarrufu sağlar.

### Temel Prensipler

- **Delta patches:** Sadece değişen dosyalar indirilir
- **Background download:** Arka planda indirme
- **Integrity check:** İndirilen dosyalar doğrulanır
- **Rollback:** Başarısız güncellemelerde geri alma
- **Versioning:** Semantic versioning desteği

---

## 2. Update Manager

```rust
pub struct UpdateManager {
    pub current_version: Version,
    pub latest_version: Option<Version>,
    pub download_progress: f32,
    pub state: UpdateState,
}

pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub pre_release: Option<String>,
    pub build: Option<String>,
}

pub enum UpdateState {
    Checking,
    Available,
    Downloading,
    Applying,
    Complete,
    Failed(String),
}

impl UpdateManager {
    pub async fn check_for_updates(&self, channel: UpdateChannel) -> Result<Option<Version>>;
    pub async fn download_update(&mut self) -> Result<()>;
    pub async fn apply_update(&mut self) -> Result<()>;
    pub fn rollback(&mut self) -> Result<()>;
}

pub enum UpdateChannel {
    Stable,
    Beta,
    Nightly,
}
```

---

## 3. Delta Patching

```rust
pub struct DeltaPatch {
    pub from_version: Version,
    pub to_version: Version,
    pub patches: Vec<FilePatch>,
    pub total_size: u64,
    pub checksum: String,
}

pub struct FilePatch {
    pub path: String,
    pub patch_type: PatchType,
    pub size: u64,
    pub checksum: String,
}

pub enum PatchType {
    New,
    Modified { delta: Vec<u8> },
    Deleted,
}
```

---

## 4. Integrity Verification

```rust
pub struct IntegrityChecker {
    pub manifest: FileManifest,
}

pub struct FileManifest {
    pub version: Version,
    pub files: Vec<FileEntry>,
}

pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub hash: String,
    pub permissions: u8,
}

impl IntegrityChecker {
    pub fn verify(&self) -> Result<Vec<IntegrityViolation>>;
    pub fn repair(&self, violations: &[IntegrityViolation]) -> Result<()>;
}
```

---

## 5. Crate Organizasyonu

```
crates/
  updater/
    ├── mod.rs
    ├── manager.rs
    ├── delta.rs
    ├── integrity.rs
    ├── rollback.rs
    └── channel.rs
```
