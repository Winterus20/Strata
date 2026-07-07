//! Minimal, extensible plugin API for Strata (prototype).
//!
//! Mirrors the constitution plugin architecture (`04-plugin-api.md`) but exposes
//! only `build` for M1. Hooks (sector load/unload, block change) arrive later.

use bevy::app::App;
use bevy::prelude::*;

/// A Strata engine plugin. Unlike Bevy's `Plugin`, this is Strata's own trait so
/// plugin names can be recorded for diagnostics and future hook dispatch.
pub trait StrataPlugin: Send + Sync {
    fn name(&self) -> &'static str {
        "unnamed"
    }

    fn build(&self, app: &mut App);
}

/// Boxed form so heterogeneous plugins can be aggregated in [`StrataCorePlugins`].
impl StrataPlugin for Box<dyn StrataPlugin> {
    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn build(&self, app: &mut App) {
        (**self).build(app);
    }
}

/// Records the ordered list of registered plugin names (diagnostics / debug HUD).
#[derive(Debug, Resource, Default)]
pub struct RegisteredPlugins(pub Vec<&'static str>);

/// App extension to register a [`StrataPlugin`] and record its name.
pub trait AddStrataPlugin {
    fn add_strata_plugin<P: StrataPlugin + 'static>(&mut self, plugin: P) -> &mut Self;
}

impl AddStrataPlugin for App {
    fn add_strata_plugin<P: StrataPlugin + 'static>(&mut self, plugin: P) -> &mut Self {
        let name = plugin.name();
        plugin.build(self);
        if !self.world().contains_resource::<RegisteredPlugins>() {
            self.insert_resource(RegisteredPlugins::default());
        }
        self.world_mut()
            .resource_mut::<RegisteredPlugins>()
            .0
            .push(name);
        self
    }
}

/// Aggregates core plugins and builds them in registration order.
pub struct StrataCorePlugins {
    plugins: Vec<Box<dyn StrataPlugin>>,
}

impl StrataCorePlugins {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Register a plugin. Returns `self` for chaining.
    pub fn add_plugin<P: StrataPlugin + 'static>(mut self, plugin: P) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Build every registered plugin in order.
    pub fn build(self, app: &mut App) {
        for plugin in self.plugins {
            app.add_strata_plugin(plugin);
        }
    }
}

impl Default for StrataCorePlugins {
    fn default() -> Self {
        Self::new()
    }
}
