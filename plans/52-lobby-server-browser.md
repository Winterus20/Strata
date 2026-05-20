# 52 — Multiplayer Lobby & Server Browser

## 1. Genel Bakış

Strata'nın multiplayer lobby ve sunucu tarayıcı sistemi oyuncuların sunucuları bulmasını ve bağlanmasını sağlar.

### Temel Prensipler

- **Server list:** Filtrelenebilir sunucu listesi
- **Favorites:** Favori sunucular
- **Server info:** Ping, oyuncu sayısı, mod, harita
- **Direct connect:** IP ile doğrudan bağlanma
- **LAN discovery:** Yerel ağ sunucu keşfi

---

## 2. Server Browser

```rust
pub struct ServerBrowser {
    pub servers: Vec<ServerInfo>,
    pub filters: ServerFilters,
    pub sort_by: ServerSort,
    pub favorites: HashSet<ServerId>,
}

pub struct ServerInfo {
    pub id: ServerId,
    pub name: String,
    pub address: SocketAddr,
    pub players: (u32, u32), // current / max
    pub map: String,
    pub game_mode: String,
    pub ping_ms: u32,
    pub version: String,
    pub mods: Vec<String>,
    pub password_protected: bool,
    pub is_favorite: bool,
}

pub struct ServerFilters {
    pub min_players: Option<u32>,
    pub max_players: Option<u32>,
    pub max_ping: Option<u32>,
    pub game_mode: Option<String>,
    pub mod_filter: Option<Vec<String>>,
    pub hide_password_protected: bool,
    pub hide_full: bool,
    pub favorites_only: bool,
}

pub enum ServerSort {
    Name,
    Players,
    Ping,
    Map,
}

impl ServerBrowser {
    pub async fn refresh(&mut self) -> Result<()>;
    pub async fn ping_server(&self, address: SocketAddr) -> Result<u32>;
    pub fn apply_filters(&self) -> Vec<&ServerInfo>;
}
```

---

## 3. Lobby System

```rust
pub struct LobbyManager {
    pub current_lobby: Option<Lobby>,
    pub pending_invites: Vec<LobbyInvite>,
}

pub struct Lobby {
    pub id: LobbyId,
    pub host: PlayerId,
    pub members: Vec<LobbyMember>,
    pub max_players: u32,
    pub is_private: bool,
    pub server_address: Option<SocketAddr>,
}

pub struct LobbyMember {
    pub id: PlayerId,
    pub name: String,
    pub is_ready: bool,
    pub is_host: bool,
}

pub struct LobbyInvite {
    pub lobby_id: LobbyId,
    pub inviter: PlayerId,
    pub expires_at: Instant,
}

impl LobbyManager {
    pub fn create_lobby(&mut self, max_players: u32, is_private: bool) -> Result<LobbyId>;
    pub fn join_lobby(&mut self, lobby_id: LobbyId) -> Result<()>;
    pub fn leave_lobby(&mut self);
    pub fn send_invite(&self, player_id: PlayerId, lobby_id: LobbyId);
    pub fn accept_invite(&mut self, invite: &LobbyInvite);
}
```

---

## 4. LAN Discovery

```rust
pub struct LanDiscovery {
    pub broadcast_port: u16,
    pub discovered: HashMap<SocketAddr, LanServerInfo>,
}

pub struct LanServerInfo {
    pub address: SocketAddr,
    pub name: String,
    pub players: (u32, u32),
    pub last_seen: Instant,
}

impl LanDiscovery {
    pub fn start(&mut self);
    pub fn broadcast_presence(&self);
    pub fn refresh(&mut self);
}
```

---

## 5. Crate Organizasyonu

```
crates/
  multiplayer/
    ├── mod.rs
    ├── browser.rs
    ├── lobby.rs
    ├── lan_discovery.rs
    └── connect.rs
```
