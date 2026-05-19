use strata_core::{CHUNK_VOLUME, Chunk, ChunkPos};
use thiserror::Error;

// TECH-DEBT: rkyv 0.8 has no "validation" feature. Plan intended runtime validation
// of serialized chunk data via rkyv's validation API. Currently we only check magic
// bytes + version header. For Faz 2, add a CRC32 checksum to the header to detect
// corrupted or tampered save files.
//
// Header layout (18 bytes total):
//   [0..4]   magic "VXCL"
//   [4..6]   version u16
//   [6..10]  chunk_x i32
//   [10..14] chunk_z i32
//   [14..18] data_len u32

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Invalid magic bytes")]
    InvalidMagic,
    #[error("Version mismatch: expected {0}, got {1}")]
    VersionMismatch(u16, u16),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

const MAGIC: [u8; 4] = *b"VXCL";
const VERSION: u16 = 1;

/// Serializes a chunk into a binary format with zstd compression.
pub fn serialize_chunk(chunk: &Chunk) -> Result<Vec<u8>, StorageError> {
    let mut header = Vec::with_capacity(18);
    header.extend_from_slice(&MAGIC);
    header.extend_from_slice(&VERSION.to_le_bytes());
    header.extend_from_slice(&chunk.position.0.x.to_le_bytes());
    header.extend_from_slice(&chunk.position.0.y.to_le_bytes());

    let raw_data: Vec<u8> = chunk
        .as_slice()
        .iter()
        .flat_map(|b| b.to_le_bytes())
        .collect();

    let compressed = zstd::encode_all(raw_data.as_slice(), 3)?;
    let data_len = compressed.len() as u32;

    header.extend_from_slice(&data_len.to_le_bytes());

    let mut output = header;
    output.extend(compressed);

    Ok(output)
}

/// Deserializes a chunk from the binary format produced by [`serialize_chunk`].
pub fn deserialize_chunk(data: &[u8]) -> Result<Chunk, StorageError> {
    if data.len() < 18 {
        return Err(StorageError::InvalidMagic);
    }

    if data[0..4] != MAGIC {
        return Err(StorageError::InvalidMagic);
    }

    let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
    if version != VERSION {
        return Err(StorageError::VersionMismatch(VERSION, version));
    }

    let chunk_x = i32::from_le_bytes(data[6..10].try_into().unwrap());
    let chunk_z = i32::from_le_bytes(data[10..14].try_into().unwrap());
    let data_len = u32::from_le_bytes(data[14..18].try_into().unwrap()) as usize;

    let compressed = &data[18..18 + data_len];
    let raw_data = zstd::decode_all(compressed)?;

    let mut blocks = Vec::with_capacity(CHUNK_VOLUME);
    for slice in raw_data.chunks_exact(2) {
        blocks.push(u16::from_le_bytes(slice.try_into().unwrap()));
    }

    let mut chunk = Chunk::new(ChunkPos(glam::IVec2::new(chunk_x, chunk_z)));
    chunk.blocks = blocks;
    chunk.rebuild_all_heightmaps();
    chunk.dirty = false;

    Ok(chunk)
}
