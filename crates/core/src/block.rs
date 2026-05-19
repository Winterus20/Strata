use hashbrown::HashMap;

/// Numeric identifier for a block face (1–6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BlockFace {
    /// -X face (left)
    NegativeX = 1,
    /// +X face (right)
    PositiveX = 2,
    /// -Y face (bottom)
    NegativeY = 3,
    /// +Y face (top)
    PositiveY = 4,
    /// -Z face (front)
    NegativeZ = 5,
    /// +Z face (back)
    PositiveZ = 6,
}

impl BlockFace {
    /// Returns the numeric ID (1–6) for this face.
    #[inline]
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Returns the axis index (0=X, 1=Y, 2=Z) for this face.
    #[inline]
    pub fn axis(self) -> usize {
        match self {
            BlockFace::NegativeX | BlockFace::PositiveX => 0,
            BlockFace::NegativeY | BlockFace::PositiveY => 1,
            BlockFace::NegativeZ | BlockFace::PositiveZ => 2,
        }
    }

    /// Returns the direction (-1 or +1) along the axis for this face.
    #[inline]
    pub fn dir(self) -> i32 {
        match self {
            BlockFace::NegativeX | BlockFace::NegativeY | BlockFace::NegativeZ => -1,
            BlockFace::PositiveX | BlockFace::PositiveY | BlockFace::PositiveZ => 1,
        }
    }

    /// Creates a `BlockFace` from an axis (0=X, 1=Y, 2=Z) and direction (-1/+1).
    #[inline]
    pub fn from_axis_dir(axis: usize, dir: i32) -> Self {
        match (axis, dir) {
            (0, -1) => BlockFace::NegativeX,
            (0, 1) => BlockFace::PositiveX,
            (1, -1) => BlockFace::NegativeY,
            (1, 1) => BlockFace::PositiveY,
            (2, -1) => BlockFace::NegativeZ,
            (2, 1) => BlockFace::PositiveZ,
            _ => unreachable!(),
        }
    }
}

/// Numeric identifier for a block type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct BlockId(pub u16);

impl BlockId {
    /// Air (empty) block.
    pub const AIR: Self = Self(0);
    /// Stone block.
    pub const STONE: Self = Self(1);
    /// Dirt block.
    pub const DIRT: Self = Self(2);
    /// Grass block.
    pub const GRASS: Self = Self(3);
    /// Bedrock (indestructible) block.
    pub const BEDROCK: Self = Self(4);

    /// Returns `true` if this block is air.
    #[inline]
    pub fn is_air(self) -> bool {
        self == Self::AIR
    }
}

/// Static properties describing a block type.
#[derive(Debug, Clone)]
pub struct BlockProperties {
    /// Unique identifier for this block type.
    pub id: BlockId,
    /// Human-readable name.
    pub name: &'static str,
    /// Whether this block is transparent to light / rendering.
    pub transparent: bool,
    /// Whether this block has collision.
    pub solid: bool,
    /// Mining hardness (0 = instant break).
    pub hardness: f32,
    /// Light level emitted (0–15).
    pub light_emission: u8,
    /// Texture index per face [1..6]: [-X, +X, -Y, +Y, -Z, +Z].
    pub face_textures: [u16; 6],
}

impl BlockProperties {
    /// Returns the texture index for the given face.
    #[inline]
    pub fn texture_for_face(&self, face: BlockFace) -> u16 {
        self.face_textures[face.id() as usize - 1]
    }
}

/// Registry mapping block ids and names to their properties.
pub struct BlockRegistry {
    blocks: Vec<BlockProperties>,
    name_map: HashMap<&'static str, BlockId>,
}

impl Default for BlockRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockRegistry {
    /// Creates a new registry pre-populated with the air block.
    pub fn new() -> Self {
        let mut registry = Self {
            blocks: Vec::with_capacity(256),
            name_map: HashMap::new(),
        };
        registry.register(BlockProperties {
            id: BlockId::AIR,
            name: "air",
            transparent: true,
            solid: false,
            hardness: 0.0,
            light_emission: 0,
            face_textures: [0; 6],
        });
        registry
    }

    /// Registers a new block type, returning its id.
    pub fn register(&mut self, props: BlockProperties) -> BlockId {
        let id = props.id;
        self.name_map.insert(props.name, id);
        if id.0 as usize >= self.blocks.len() {
            let placeholder = BlockProperties {
                id: BlockId::AIR,
                name: "air",
                transparent: true,
                solid: false,
                hardness: 0.0,
                light_emission: 0,
                face_textures: [0; 6],
            };
            self.blocks.resize(id.0 as usize + 1, placeholder);
        }
        self.blocks[id.0 as usize] = props;
        id
    }

    /// Returns the properties for the given block id.
    #[inline]
    pub fn get(&self, id: BlockId) -> &BlockProperties {
        &self.blocks[id.0 as usize]
    }

    /// Looks up a block id by its human-readable name.
    #[inline]
    pub fn by_name(&self, name: &str) -> Option<BlockId> {
        self.name_map.get(name).copied()
    }
}
