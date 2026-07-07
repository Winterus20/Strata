# Asset Format Strategy

Bu belge, Strata projesinin okunabilir aset formatı (Block Registry, Config dosyaları, Modlama verileri vb.) stratejisini ve alınan mimari kararları detaylandırmaktadır.

## 1. Problemin Tanımı

Strata gibi geniş çaplı bir Voxel oyununda binlerce farklı blok, eşya (item) ve entegite (entity) tanımı yer alır. Bu verilerin depolanma formatı üç ana ihtiyacı karşılamalıdır:
1. **İnsan-okunabilir ve Modlanabilir:** Mod yapımcıları ve tasarımcılar oyuna yeni bloklar ekleyebilmeli, bunu yaparken çok fazla sözdizimi (syntax) kuralıyla uğraşmamalıdır. Yorum satırları kesinlikle desteklenmelidir.
2. **Karmaşık ve İç İçe (Nested) Veri Desteği:** Bir blok sadece id ve candan (hp) oluşmaz. Bir bloğun malzemesi, kendine özel collision şekli, yaydığı ışık miktarı ve state (durum) makinesi gibi özellikleri bulunur.
3. **Rust / Bevy Uyumluluğu:** Oyun motoru (Bevy) verileri çalışma zamanında (runtime) memory'ye yüklerken, verilerin formattan Rust struct ve enum'larına kolayca dönüştürülmesi (serialization/deserialization) gerekir.

## 2. Format Alternatifleri ve Analizi

### A. TOML (Tom's Obvious, Minimal Language)
- **Avantajlar:** Rust ekosisteminde (Cargo) standarttır. Çok yaygındır ve basit veriler için okuması harikadır.
- **Dezavantajlar:** Veriler derinleşip iç içe geçtiğinde ve listeler (array of objects) kullanılmaya başlandığında yazması ve okuması zor bir hal alır.
- **Karar:** Düz ayar (settings) dosyaları için uygun olsa da, kompleks `Block Registry` yapıları için kısıtlayıcıdır.

### B. RON (Rusty Object Notation)
- **Avantajlar:** Serde ve Bevy ile mükemmel uyumludur. Çünkü RON, adından da anlaşılacağı üzere doğrudan Rust veri yapılarına (Struct, Enum, Tuple, Option) karşılık gelecek şekilde tasarlanmıştır. Yorum satırlarını (`//`) destekler ve sözdizimi Rust yazmaya çok benzer.
- **Dezavantajlar:** Sadece Rust ekosisteminde popülerdir.
- **Karar:** En güçlü aday.

### C. JSON / JSON5
- **Avantajlar:** Neredeyse her dilde mükemmel desteği vardır.
- **Dezavantajlar:** Klasik JSON yorum satırlarını desteklemez. Çok fazla süslü parantez ve tırnak işareti gerektirir, bu da mod yapımcıları için yazım hatalarını artırır.
- **Karar:** Elendi.

### D. KDL (KDL Document Language)
- **Avantajlar:** XML alternatifi modern bir document-oriented dildir. Formatı (boşlukları, yorum satırlarını) asla bozmadan programatik olarak editlenmeye uygundur.
- **Dezavantajlar:** Serde entegrasyonu RON kadar kusursuz değildir. Daha az bilinen bir formattır.
- **Karar:** Elendi.

## 3. Alınan Karar: RON (Rusty Object Notation)

Yapılan analizler sonucunda projenin metin tabanlı aset formatı olarak **RON** kullanılmasına karar verilmiştir.

### Neden RON?
1. **Enum Desteği:** Voxel oyunlarında bir özelliğin birden fazla çeşidi (variant) olması çok sıktır. Örneğin `Shape::Cube`, `Shape::Slab(0.5)`, `Shape::Custom("model.obj")` gibi Enum yapılarının RON'daki gösterimi, Rust'taki ile birebir aynıdır. TOML veya JSON'da Enum'ları serileştirmek karmaşıktır.
2. **Bevy Ekosistemi:** RON, Bevy topluluğu ve Bevy'nin kendi `Reflect` / `bevy_asset` sistemleri ile endüstri standardı olarak kabul edilir.
3. **Modlama Ergonomisi:** Bir mod yapımcısı, `.ron` uzantılı bir dosyayı açtığında tıpkı bir Rust struct'ını doldurur gibi, kolay ve temiz bir sözdizimi ile karşılaşır.

### Örnek RON Blok Tanımı

```rust
// assets/blocks/dirt.ron
BlockDefinition(
    id: "strata:dirt",
    name: "Dirt Block",
    max_stack: 64,
    properties: {
        "hardness": Float(1.5),
        "tool_required": Enum(Shovel),
    },
    collider: Voxel(
        size: (8, 8, 8),
    ),
    visuals: (
        textures: {
            Top: "textures/dirt.png",
            Bottom: "textures/dirt.png",
            Sides: "textures/dirt.png",
        },
    ),
)
```

- **Save Dosyaları, Load Mekaniği ve Ağ Trafiği (Network Binary):** -> `Postcard + Bytemuck`
- **Block Registry, Oyun Ayarları ve Asetler (Metin/İnsan Okunabilir):** -> `RON`
