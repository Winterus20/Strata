use std::collections::{HashMap, HashSet};
use bevy_app::App;
use crate::r#trait::GamePlugin;

/// Yüklü eklentileri tutan ve bağımlılık sırasına göre lifecycle
/// yönetimini gerçekleştiren kayıt defteri.
pub struct PluginRegistry {
    plugins: HashMap<&'static str, Box<dyn GamePlugin>>,
    load_order: Vec<&'static str>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    /// Yeni bir PluginRegistry oluşturur.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            load_order: Vec::new(),
        }
    }

    /// Bir eklentiyi kayıt defterine ekler.
    pub fn register(&mut self, plugin: Box<dyn GamePlugin>) {
        let name = plugin.name();
        tracing::info!("Eklenti kayıt ediliyor: {} (v{})", name, plugin.version());
        self.plugins.insert(name, plugin);
    }

    /// Kayıtlı eklentilerin bağımlılıklarını çözer (Topological Sort)
    /// ve doğru sırayla yükleme sırasını (`load_order`) oluşturur.
    pub fn resolve_dependencies(&mut self) -> Result<(), String> {
        let mut visited = HashSet::new();
        let mut temp = HashSet::new();
        let mut order = Vec::new();

        // Her eklenti için DFS yardımıyla bağımlılıkları ziyaret et
        for &name in self.plugins.keys() {
            if !visited.contains(&name) {
                self.dfs(name, &mut visited, &mut temp, &mut order)?;
            }
        }

        self.load_order = order;
        tracing::info!("Eklenti bağımlılıkları çözüldü. Yükleme sırası: {:?}", self.load_order);
        Ok(())
    }

    fn dfs(
        &self,
        name: &'static str,
        visited: &mut HashSet<&'static str>,
        temp: &mut HashSet<&'static str>,
        order: &mut Vec<&'static str>,
    ) -> Result<(), String> {
        if temp.contains(&name) {
            return Err(format!("Döngüsel bağımlılık tespit edildi: {}", name));
        }

        if !visited.contains(&name) {
            temp.insert(name);

            // Eklentinin bağımlılıklarını al
            if let Some(plugin) = self.plugins.get(name) {
                for &dep in &plugin.dependencies() {
                    if !self.plugins.contains_key(dep) {
                        return Err(format!(
                            "Eklenti '{}' için gereken bağımlılık '{}' bulunamadı!",
                            name, dep
                        ));
                    }
                    self.dfs(dep, visited, temp, order)?;
                }
            }

            temp.remove(&name);
            visited.insert(name);
            order.push(name);
        }

        Ok(())
    }

    /// Tüm eklentileri Bevy App'e kayıt eder.
    pub fn build_all(&self, app: &mut App) {
        for &name in &self.load_order {
            if let Some(plugin) = self.plugins.get(name) {
                tracing::debug!("Eklenti on_register tetikleniyor: {}", name);
                plugin.on_register(app);
            }
        }
    }

    /// Tüm eklentilerin startup sistemlerini çalıştırır.
    pub fn startup_all(&self, app: &mut App) {
        for &name in &self.load_order {
            if let Some(plugin) = self.plugins.get(name) {
                tracing::info!("Eklenti başlatılıyor (on_startup): {}", name);
                plugin.on_startup(app);
            }
        }
    }

    /// Tüm eklentileri düzgün bir şekilde kapatır (ters yükleme sırasıyla).
    pub fn shutdown_all(&self, app: &mut App) {
        // Kapatma işlemi bağımlılık sırasının tersine yapılmalıdır
        for &name in self.load_order.iter().rev() {
            if let Some(plugin) = self.plugins.get(name) {
                tracing::info!("Eklenti kapatılıyor (on_shutdown): {}", name);
                plugin.on_shutdown(app);
            }
        }
    }

    /// Kayıtlı eklentilerin listesini döner.
    pub fn loaded_plugins(&self) -> Vec<&'static str> {
        self.load_order.clone()
    }
}
