# 18 — UI/HUD Sistemi (glyphon)

## 1. Genel Bakış

Strata'nın UI sistemi **glyphon 0.12+** ile GPU-accelerated text rendering kullanır. Tüm UI elementleri ECS component'ları olarak tanımlanır.

### Temel Prensipler

- **ECS-based:** Her UI element bir component
- **GPU-accelerated:** glyphon ile text rendering
- **Responsive:** Farklı çözünürlüklere uyum
- **Themeable:** Tema desteği

---

## 2. UI Layout Sistemi

```rust
/// UI node — tüm UI elementlerinin temel yapısı.
#[derive(Component)]
pub struct UiNode {
    /// Parent node (None = root).
    pub parent: Option<Entity>,

    /// Child node'lar.
    pub children: Vec<Entity>,

    /// Layout stili.
    pub style: UiStyle,

    /// Görünürlük.
    pub visible: bool,

    /// Z-index (katman sırası).
    pub z_index: i32,
}

/// UI layout stili (Flexbox benzeri).
#[derive(Clone)]
pub struct UiStyle {
    /// Display tipi.
    pub display: DisplayType,

    /// Flex direction.
    pub flex_direction: FlexDirection,

    /// Justify content.
    pub justify_content: JustifyContent,

    /// Align items.
    pub align_items: AlignItems,

    /// Boyutlar.
    pub size: Size,

    /// Padding.
    pub padding: Rect<f32>,

    /// Margin.
    pub margin: Rect<f32>,

    /// Border.
    pub border: Rect<f32>,

    /// Arkaplan rengi.
    pub background_color: Option<Color>,

    /// Border rengi.
    pub border_color: Option<Color>,

    /// Border radius.
    pub border_radius: f32,
}

#[derive(Clone, Copy)]
pub enum DisplayType {
    Flex,
    Grid,
    None, // Gizli
}

#[derive(Clone, Copy)]
pub enum FlexDirection {
    Row,
    Column,
}

#[derive(Clone, Copy)]
pub enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
}

#[derive(Clone, Copy)]
pub enum AlignItems {
    Start,
    Center,
    End,
    Stretch,
}

/// Boyut tanımı.
#[derive(Clone, Copy)]
pub struct Size {
    pub width: Dimension,
    pub height: Dimension,
}

#[derive(Clone, Copy)]
pub enum Dimension {
    Auto,
    Pixels(f32),
    Percent(f32),
    MinContent,
    MaxContent,
}
```

---

## 3. UI Elementleri

### 3.1 Text

```rust
/// Text element.
#[derive(Component)]
pub struct UiText {
    /// Metin içeriği.
    pub content: String,

    /// Font boyutu.
    pub font_size: f32,

    /// Font ailesi.
    pub font_family: String,

    /// Renk.
    pub color: Color,

    /// Hizalama.
    pub alignment: TextAlignment,

    /// Word wrap.
    pub word_wrap: bool,

    /// Text shadow.
    pub shadow: Option<TextShadow>,
}

pub enum TextAlignment {
    Left,
    Center,
    Right,
}

pub struct TextShadow {
    pub offset: Vec2,
    pub color: Color,
    pub blur: f32,
}
```

### 3.2 Image

```rust
/// Image element.
#[derive(Component)]
pub struct UiImage {
    /// Texture ID.
    pub texture_id: String,

    /// Scale mode.
    pub scale_mode: ScaleMode,

    /// Opacity.
    pub opacity: f32,

    /// Tint rengi.
    pub tint: Color,
}

pub enum ScaleMode {
    Stretch,
    Fit,
    Fill,
    Tile,
    NinePatch { border: Rect<u32> },
}
```

### 3.3 Button

```rust
/// Button element.
#[derive(Component)]
pub struct UiButton {
    /// Label.
    pub label: String,

    /// Button state.
    pub state: ButtonState,

    /// Click event.
    pub on_click: Option<UiEvent>,

    /// Hover event.
    pub on_hover: Option<UiEvent>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    Hovered,
    Pressed,
    Disabled,
}
```

### 3.4 Inventory Slot

```rust
/// Envanter slot'u.
#[derive(Component)]
pub struct InventorySlot {
    /// Slot index.
    pub slot_index: u8,

    /// İçerik (item stack).
    pub content: Option<ItemStack>,

    /// Seçili mi (hotbar).
    pub selected: bool,

    /// Dragging state.
    pub dragging: bool,
}

/// ItemStack tanımı.
pub struct ItemStack {
    pub item_id: u16,
    pub count: u8,
    pub max_stack: u8,
    pub durability: Option<u16>,
    pub nbt: Option<ItemNbt>,
}
```

---

## 4. HUD Layout

```rust
/// HUD root layout.
///
/// ┌──────────────────────────────────────────────────────┐
/// │  Hotbar                                              │
/// │  ┌───┬───┬───┬───┬───┬───┬───┬───┬───┐              │
/// │  │ 1 │ 2 │ 3 │ 4 │ 5 │ 6 │ 7 │ 8 │ 9 │              │
/// │  └───┴───┴───┴───┴───┴───┴───┴───┴───┘              │
/// │                                                      │
│  │                                                      │
│  │                    Crosshair                         │
│  │                      +                               │
│  │                                                      │
│  │                                                      │
│  │  Hearts                    Hunger                    │
│  │  ❤❤❤❤❤❤❤❤❤❤              🍖🍖🍖🍖🍖🍖🍖🍖🍖🍖       │
│  │                                                      │
│  │  XP Bar: ████████████████░░░░░░░░ (Level 15)         │
│  │                                                      │
│  │  [Debug HUD - F3]                                    │
│  └──────────────────────────────────────────────────────┘
pub struct HudLayout {
    /// Crosshair.
    pub crosshair: CrosshairConfig,

    /// Hotbar.
    pub hotbar: HotbarConfig,

    /// Health bar.
    pub health_bar: StatusBarConfig,

    /// Hunger bar.
    pub hunger_bar: StatusBarConfig,

    /// XP bar.
    pub xp_bar: XpBarConfig,

    /// Debug overlay.
    pub debug_overlay: DebugOverlayConfig,

    /// Chat.
    pub chat: ChatConfig,

    /// Boss bar.
    pub boss_bar: Option<BossBarConfig>,
}

pub struct CrosshairConfig {
    pub size: f32,
    pub color: Color,
    pub gap: f32,
    pub thickness: f32,
}

pub struct HotbarConfig {
    pub slot_size: f32,
    pub slot_gap: f32,
    pub selected_border: Color,
    pub background: Color,
}

pub struct StatusBarConfig {
    pub icon_count: u8,
    pub icon_size: f32,
    pub full_color: Color,
    pub empty_color: Color,
    pub position: BarPosition,
}

pub enum BarPosition {
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}
```

---

## 5. glyphon Entegrasyonu

```rust
/// glyphon text renderer.
pub struct GlyphonRenderer {
    /// glyphon text renderer.
    text_renderer: glyphon::TextRenderer,

    /// Font cache.
    font_cache: glyphon::FontCache,

    /// Swash cache (font rasterization).
    swash_cache: glyphon::SwashCache,

    /// Viewport.
    viewport: glyphon::Viewport,

    /// Text buffer'ları.
    text_buffers: HashMap<Entity, glyphon::Buffer>,
}

impl GlyphonRenderer {
    /// Yeni renderer oluştur.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let font_cache = glyphon::FontCache::new();
        let swash_cache = glyphon::SwashCache::new();

        let viewport = glyphon::Viewport::new(device, &font_cache);

        let text_renderer = glyphon::TextRenderer::new(
            device,
            queue,
            &font_cache,
            glyphon::Multisampling::None,
        );

        Self {
            text_renderer,
            font_cache,
            swash_cache,
            viewport,
            text_buffers: HashMap::new(),
        }
    }

    /// Text buffer oluştur.
    pub fn create_text_buffer(
        &mut self,
        entity: Entity,
        text: &str,
        font_size: f32,
        font_family: &str,
    ) {
        let mut buffer = glyphon::Buffer::new(&mut self.font_cache, glyphon::Metrics {
            font_size,
            line_height: font_size * 1.2,
        });

        buffer.set_text(&mut self.font_cache, text, glyphon::Attrs::new().family(glyphon::Family::Name(font_family)));
        buffer.shape_until_scroll(&mut self.font_cache);

        self.text_buffers.insert(entity, buffer);
    }

    /// Tüm text'leri render et.
    pub fn render(
        &mut self,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        depth_stencil: Option<&wgpu::TextureView>,
    ) {
        self.text_renderer
            .render(
                &self.font_cache,
                &self.viewport,
                glyphon::Resolution {
                    width: view.width() as u32,
                    height: view.height() as u32,
                },
                queue,
                &self.swash_cache,
            )
            .unwrap();
    }
}
```

---

## 6. Input Handling

```rust
/// UI input handler.
pub struct UiInputHandler {
    /// Mouse pozisyonu.
    pub mouse_position: Vec2,

    /// Hovered element.
    pub hovered: Option<Entity>,

    /// Focused element.
    pub focused: Option<Entity>,

    /// Dragging state.
    pub dragging: Option<DragState>,
}

pub struct DragState {
    pub source: Entity,
    pub start_position: Vec2,
    pub current_position: Vec2,
    pub data: Option<UiDragData>,
}

pub enum UiDragData {
    ItemStack { item: ItemStack, slot: u8 },
}

impl UiInputHandler {
    /// Mouse event işle.
    pub fn handle_mouse(
        &mut self,
        event: &MouseEvent,
        ui_nodes: &Query<(Entity, &UiNode, &GlobalTransform)>,
    ) -> UiEvent {
        match event {
            MouseEvent::Moved(pos) => {
                self.mouse_position = *pos;

                // Hit test — hangi element'in üstünde?
                self.hovered = self.hit_test(pos, ui_nodes);

                UiEvent::MouseMove { position: *pos }
            }
            MouseEvent::Pressed(button) => {
                if let Some(hovered) = self.hovered {
                    // Drag başlat
                    self.dragging = Some(DragState {
                        source: hovered,
                        start_position: self.mouse_position,
                        current_position: self.mouse_position,
                        data: None,
                    });

                    UiEvent::MouseDown {
                        element: hovered,
                        button: *button,
                    }
                } else {
                    UiEvent::MouseDown {
                        element: Entity::PLACEHOLDER,
                        button: *button,
                    }
                }
            }
            MouseEvent::Released(button) => {
                let result = if let Some(drag) = &self.dragging {
                    self.dragging = None;

                    // Drop target bul
                    let target = self.hit_test(&self.mouse_position, ui_nodes);

                    UiEvent::Drop {
                        source: drag.source,
                        target,
                        data: drag.data.clone(),
                    }
                } else {
                    UiEvent::MouseUp { button: *button }
                };

                result
            }
            MouseEvent::Wheel(delta) => {
                UiEvent::MouseWheel { delta: *delta }
            }
        }
    }

    /// Hit test — pozisyondaki UI elementini bul.
    fn hit_test(
        &self,
        pos: &Vec2,
        ui_nodes: &Query<(Entity, &UiNode, &GlobalTransform)>,
    ) -> Option<Entity> {
        // Z-index'e göre sırala (yüksek önce)
        let mut sorted: Vec<_> = ui_nodes.iter().collect();
        sorted.sort_by(|a, b| b.1.z_index.cmp(&a.1.z_index));

        for (entity, node, transform) in sorted {
            if !node.visible {
                continue;
            }

            let bounds = transform.compute_bounds();
            if bounds.contains(*pos) {
                return Some(entity);
            }
        }

        None
    }
}
```

---

## 7. Crate Organizasyonu

```
crates/
  ui/
    ├── mod.rs              ← UI plugin entry point
    ├── layout/
    │   ├── mod.rs          ← UI layout sistemi
    │   ├── node.rs         ← UiNode
    │   ├── style.rs        ← UiStyle
    │   └── flexbox.rs      ← Flexbox layout engine
    ├── elements/
    │   ├── mod.rs          ← UI element tanımları
    │   ├── text.rs         ← UiText
    │   ├── image.rs        ← UiImage
    │   ├── button.rs       ← UiButton
    │   ├── slot.rs         ← InventorySlot
    │   ├── panel.rs        ← UiPanel
    │   └── progress.rs     ← ProgressBar
    ├── hud/
    │   ├── mod.rs          ← HUD layout
    │   ├── crosshair.rs    ← Crosshair
    │   ├── hotbar.rs       ← Hotbar
    │   ├── health.rs       ← Health/Hunger bars
    │   ├── xp.rs           ← XP bar
    │   └── chat.rs         ← Chat overlay
    ├── renderer/
    │   ├── mod.rs          ← GlyphonRenderer
    │   ├── text.rs         ← Text rendering
    │   └── shapes.rs       ← Shape rendering (rect, border)
    ├── input/
    │   ├── mod.rs          ← UiInputHandler
    │   ├── events.rs       ← UI event'leri
    │   └── drag.rs         ← Drag & drop
    └── theme/
        ├── mod.rs          ← Tema sistemi
        └── presets.rs      ← Varsayılan temalar
```
