use bevy_app::App;
use semver::Version;

/// Ana motorun modüler genişletilebilirlik arayüzü.
/// Her alt sistem ve mod bu trait'i uygulayarak motora entegre olur.
pub trait GamePlugin: Send + Sync {
    /// Eklentinin benzersiz adı (örneğin: "strata-render")
    fn name(&self) -> &'static str;

    /// Eklentinin SemVer uyumlu sürümü
    fn version(&self) -> Version;

    /// Bu eklentiden önce yüklenmesi gereken diğer eklentilerin adları
    fn dependencies(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Eklenti sisteme kayıt edildiğinde tetiklenir.
    /// Genellikle kaynakların (Resource) ve başlangıç ayarlarının eklenmesi için kullanılır.
    fn on_register(&self, app: &mut App);

    /// Motor ve ECS sistemi çalışmaya başlarken tetiklenir.
    /// Sistemlerin (System) ve olayların (Event) kaydedilmesi için uygundur.
    fn on_startup(&self, app: &mut App);

    /// Motor kapatılırken tetiklenir.
    /// Kaynakların temizlenmesi ve durumun (state) diske yazılması için kullanılır.
    fn on_shutdown(&self, app: &mut App);
}
