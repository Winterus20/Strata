# 50 — Crash Reporting & Telemetry

## 1. Genel Bakış

Strata'nın hata raporlama ve telemetri sistemi çökmeleri ve kullanım verilerini toplar.

### Temel Prensipler

- **Crash dump:** Otomatik crash raporu oluşturma
- **Stack trace:** Detaylı hata izleme
- **Opt-in:** Kullanıcı onayı ile veri toplama
- **Privacy-first:** Kişisel veri toplanmaz
- **Minimized:** Minimum performans etkisi

---

## 2. Crash Reporter

```rust
pub struct CrashReporter {
    pub enabled: bool,
    pub endpoint: String,
    pub last_crash: Option<CrashReport>,
}

pub struct CrashReport {
    pub timestamp: u64,
    pub version: String,
    pub platform: String,
    pub stack_trace: Vec<StackFrame>,
    pub system_info: SystemInfo,
    pub game_state: GameStateSnapshot,
    pub logs: Vec<String>,
}

pub struct StackFrame {
    pub function: String,
    pub file: String,
    pub line: u32,
    pub module: String,
}

pub struct SystemInfo {
    pub os: String,
    pub cpu: String,
    pub gpu: String,
    pub ram_mb: u64,
    pub disk_free_gb: u64,
}

impl CrashReporter {
    pub fn capture(&self, error: &dyn std::error::Error) -> CrashReport;
    pub async fn submit(&self, report: &CrashReport) -> Result<()>;
}
```

---

## 3. Telemetry

```rust
pub struct TelemetryCollector {
    pub enabled: bool,
    pub session_id: String,
    pub events: Vec<TelemetryEvent>,
    pub flush_interval: Duration,
}

pub struct TelemetryEvent {
    pub timestamp: u64,
    pub event_type: String,
    pub properties: HashMap<String, serde_json::Value>,
}

impl TelemetryCollector {
    pub fn record(&mut self, event_type: &str, properties: HashMap<String, serde_json::Value>);
    pub async fn flush(&mut self) -> Result<()>;
}

// Örnek event'ler
// - game_start, game_end
// - block_placed, block_broken
// - entity_killed, death
// - fps_sample, memory_sample
// - settings_changed
```

---

## 4. Crate Organizasyonu

```
crates/
  telemetry/
    ├── mod.rs
    ├── crash.rs
    ├── collector.rs
    ├── events.rs
    └── privacy.rs
```
