/// Debug overlay data for rendering stats.
/// On-screen rendering deferred until glyphon/wgpu 28+ upgrade.
/// Currently displays all info in window title via `debug_string()`.
#[derive(Debug, Clone)]
pub struct DebugOverlay {
    pub fps: f32,
    pub chunk_count: usize,
    pub visible_chunks: usize,
    pub player_position: (f32, f32, f32),
}

impl DebugOverlay {
    pub fn new() -> Self {
        Self {
            fps: 60.0,
            chunk_count: 0,
            visible_chunks: 0,
            player_position: (0.0, 0.0, 0.0),
        }
    }

    /// Returns a formatted debug string for window title.
    pub fn debug_string(&self) -> String {
        format!(
            "Strata | FPS: {:.1} | Chunks: {}/{} | Pos: ({:.1}, {:.1}, {:.1})",
            self.fps,
            self.visible_chunks,
            self.chunk_count,
            self.player_position.0,
            self.player_position.1,
            self.player_position.2,
        )
    }

    /// On-screen render placeholder.
    /// Proper text rendering requires glyphon (wgpu 28+) or a custom font texture atlas.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        _format: wgpu::TextureFormat,
        _view: &wgpu::TextureView,
        _width: u32,
        _height: u32,
    ) {
    }

    /// Init placeholder for future glyphon integration.
    #[allow(dead_code)]
    pub fn init(&mut self, _device: &wgpu::Device, _format: wgpu::TextureFormat) {}
}

impl Default for DebugOverlay {
    fn default() -> Self {
        Self::new()
    }
}
