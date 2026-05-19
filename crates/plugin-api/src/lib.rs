pub mod native_loader;
pub mod registry;
pub mod r#trait;

pub use native_loader::NativePluginLoader;
pub use registry::PluginRegistry;
pub use r#trait::GamePlugin;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::App;
    use semver::Version;

    struct MockPlugin {
        name: &'static str,
        deps: Vec<&'static str>,
    }

    impl GamePlugin for MockPlugin {
        fn name(&self) -> &'static str {
            self.name
        }
        fn version(&self) -> Version {
            Version::new(1, 0, 0)
        }
        fn dependencies(&self) -> Vec<&'static str> {
            self.deps.clone()
        }
        fn on_register(&self, _app: &mut App) {}
        fn on_startup(&self, _app: &mut App) {}
        fn on_shutdown(&self, _app: &mut App) {}
    }

    #[test]
    fn test_dependency_resolution() {
        let mut registry = PluginRegistry::new();

        registry.register(Box::new(MockPlugin {
            name: "PluginC",
            deps: vec!["PluginB", "PluginA"],
        }));
        registry.register(Box::new(MockPlugin {
            name: "PluginA",
            deps: vec![],
        }));
        registry.register(Box::new(MockPlugin {
            name: "PluginB",
            deps: vec!["PluginA"],
        }));

        assert!(registry.resolve_dependencies().is_ok());
        let order = registry.loaded_plugins();
        
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], "PluginA");
        assert_eq!(order[1], "PluginB");
        assert_eq!(order[2], "PluginC");
    }

    #[test]
    fn test_cyclic_dependency_detection() {
        let mut registry = PluginRegistry::new();

        registry.register(Box::new(MockPlugin {
            name: "PluginX",
            deps: vec!["PluginY"],
        }));
        registry.register(Box::new(MockPlugin {
            name: "PluginY",
            deps: vec!["PluginX"],
        }));

        assert!(registry.resolve_dependencies().is_err());
    }
}
