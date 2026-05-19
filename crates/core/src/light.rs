/// Per-chunk light data: sky light and block light each stored as 4-bit nibbles,
/// packed 2 voxels per byte (even index → lower nibble, odd index → upper nibble).
///
/// Each array is `[u8; 32768]` (32 KB), giving 64 KB total per chunk for both light types.
#[derive(Debug, Clone)]
pub struct LightData {
    pub sky_light: Box<[u8; 32768]>,
    pub block_light: Box<[u8; 32768]>,
}

impl LightData {
    /// Create new light data with default values:
    /// - sky light: 15 (max) everywhere
    /// - block light: 0 everywhere
    pub fn new() -> Self {
        Self {
            sky_light: Box::new([15u8; 32768]),
            block_light: Box::new([0u8; 32768]),
        }
    }

    /// Get sky light level at the given index (0-15).
    #[inline]
    pub fn get_sky(&self, index: usize) -> u8 {
        if index & 1 == 0 {
            self.sky_light[index >> 1] & 0x0F
        } else {
            self.sky_light[index >> 1] >> 4
        }
    }

    /// Set sky light level at the given index (clamped to 0-15).
    #[inline]
    pub fn set_sky(&mut self, index: usize, value: u8) {
        let value = value.min(15);
        let byte = &mut self.sky_light[index >> 1];
        if index & 1 == 0 {
            *byte = (*byte & 0xF0) | value;
        } else {
            *byte = (*byte & 0x0F) | (value << 4);
        }
    }

    /// Get block light level at the given index (0-15).
    #[inline]
    pub fn get_block(&self, index: usize) -> u8 {
        if index & 1 == 0 {
            self.block_light[index >> 1] & 0x0F
        } else {
            self.block_light[index >> 1] >> 4
        }
    }

    /// Set block light level at the given index (clamped to 0-15).
    #[inline]
    pub fn set_block(&mut self, index: usize, value: u8) {
        let value = value.min(15);
        let byte = &mut self.block_light[index >> 1];
        if index & 1 == 0 {
            *byte = (*byte & 0xF0) | value;
        } else {
            *byte = (*byte & 0x0F) | (value << 4);
        }
    }
}

impl LightData {
    /// Creates light data from raw byte arrays (used for network deserialization).
    pub fn from_raw(sky_light: [u8; 32768], block_light: [u8; 32768]) -> Self {
        Self {
            sky_light: Box::new(sky_light),
            block_light: Box::new(block_light),
        }
    }
}

impl Default for LightData {
    fn default() -> Self {
        Self::new()
    }
}
