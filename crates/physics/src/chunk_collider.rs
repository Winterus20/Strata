use bevy_math::Vec3;
use bevy_rapier3d::geometry::Collider;
use strata_meshing::MeshData;

/// Creates a `Collider::trimesh` from `MeshData`.
/// Returns `None` if the mesh is empty or has no triangles.
pub fn mesh_data_to_collider(mesh_data: &MeshData) -> Option<Collider> {
    if mesh_data.is_empty() || mesh_data.indices.is_empty() {
        return None;
    }

    let vertices: Vec<Vec3> = mesh_data
        .vertices
        .iter()
        .map(|v| Vec3::from_array(v.position))
        .collect();

    let indices: Vec<[u32; 3]> = mesh_data
        .indices
        .chunks_exact(3)
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect();

    if indices.is_empty() {
        return None;
    }

    Collider::trimesh(vertices, indices).ok()
}
