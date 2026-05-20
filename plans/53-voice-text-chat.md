# 53 — Voice & Text Chat

## 1. Genel Bakış

Strata'nın sesli ve yazılı sohbet sistemi multiplayer iletişimi sağlar.

### Temel Prensiples

- **Voice chat:** Proximity-based ve takım sesli sohbeti
- **Text chat:** Global, takım, özel mesaj
- **Spatial audio:** 3D ses konumlandırma
- **Push-to-talk:** Bas-konuş desteği
- **Profanity filter:** Filtreleme (opsiyonel)

---

## 2. Voice Chat

```rust
pub struct VoiceChatManager {
    pub input_device: Option<AudioDevice>,
    pub output_device: Option<AudioDevice>,
    pub active_streams: HashMap<PlayerId, VoiceStream>,
    pub config: VoiceConfig,
}

pub struct VoiceConfig {
    pub enabled: bool,
    pub mode: VoiceMode,
    pub push_to_talk_key: Option<KeyCode>,
    pub input_volume: f32,
    pub output_volume: f32,
    pub noise_suppression: bool,
    pub echo_cancellation: bool,
}

pub enum VoiceMode {
    AlwaysOn,
    PushToTalk,
    Proximity,
}

pub struct VoiceStream {
    pub player_id: PlayerId,
    pub decoder: Box<dyn AudioDecoder>,
    pub spatial_position: Option<Vec3>,
    pub volume: f32,
}

impl VoiceChatManager {
    pub fn capture_audio(&mut self) -> Vec<u8>;
    pub fn transmit(&mut self, data: &[u8], target: VoiceTarget);
    pub fn receive(&mut self, player_id: PlayerId, data: &[u8]);
    pub fn play_spatial(&mut self, player_id: PlayerId, position: Vec3);
}

pub enum VoiceTarget {
    All,
    Team,
    Player(PlayerId),
    Proximity(f32),
}
```

---

## 3. Text Chat

```rust
pub struct ChatManager {
    pub messages: Vec<ChatMessage>,
    pub max_messages: usize,
    pub channels: HashMap<String, ChatChannel>,
}

pub struct ChatMessage {
    pub id: u64,
    pub channel: String,
    pub sender: Option<PlayerId>,
    pub sender_name: String,
    pub content: String,
    pub timestamp: u64,
    pub message_type: ChatMessageType,
}

pub enum ChatMessageType {
    Player,
    System,
    Server,
    Whisper,
}

pub struct ChatChannel {
    pub name: String,
    pub messages: Vec<ChatMessage>,
    pub color: Color,
    pub is_global: bool,
}

impl ChatManager {
    pub fn send(&mut self, channel: &str, content: &str);
    pub fn receive(&mut self, message: ChatMessage);
    pub fn get_messages(&self, channel: &str) -> &[ChatMessage];
}
```

---

## 4. Crate Organizasyonu

```
crates/
  chat/
    ├── mod.rs
    ├── voice.rs
    ├── text.rs
    ├── spatial.rs
    └── filter.rs
```
