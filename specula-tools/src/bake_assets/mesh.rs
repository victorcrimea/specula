//! A PMD as the client draws it: vertices ready for a vertex buffer, skin
//! weights quantised for the GPU, coarser levels of detail, and the bind
//! pose split out as a skeleton so meshes sharing it share one file. The
//! coarsest level and the vertices it uses come first, so they can be
//! sent as a file of their own (`Mesh::split`).

use specula_assets::files;
use specula_assets::files::Bone;
use specula_assets::files::Mesh;
use specula_assets::files::PropPoint;
use specula_assets::files::Skeleton;
use specula_assets::math::Vec3;
use specula_tools::pmd;
use specula_tools::pmd::Model;

/// Share of the full triangle count each coarser level aims for, as
/// bake-unit's levels.
const RATIOS: [f32; 2] = [0.4, 0.15];
/// The largest deviation allowed, relative to the mesh's own extents (with
/// `Sparse`); it stops a level short of its ratio rather than wreck the
/// silhouette of a small mesh.
const MAX_ERROR: f32 = 0.08;
/// UV and normal error against position error for the coarsest level.
const ATTRIBUTE_WEIGHTS: [f32; 5] = [1.0, 1.0, 0.5, 0.5, 0.5];

/// The mesh, without its skeleton reference, and the skeleton if it has
/// bones; the caller writes the skeleton first to learn its name.
pub fn convert(name: &str, model: &Model) -> Result<(Mesh, Option<Skeleton>), String> {
    let bone_count = model.bones.len();
    let mut vertices = Vec::with_capacity(model.vertices.len() * files::VERTEX_SIZE);
    let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
    for vertex in &model.vertices {
        min = min.min(vertex.position);
        max = max.max(vertex.position);
        for value in [
            vertex.position.x,
            vertex.position.y,
            vertex.position.z,
            vertex.normal.x,
            vertex.normal.y,
            vertex.normal.z,
            vertex.uv[0],
            // ModelRenderer::BuildUV hands the GPU 1 - v, against textures
            // stored top row first.
            1.0 - vertex.uv[1],
        ] {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
        let (bones, weights) = if bone_count == 0 {
            ([0; 4], [0; 4])
        } else {
            influences(name, vertex, bone_count)?
        };
        vertices.extend_from_slice(&bones);
        vertices.extend_from_slice(&weights);
    }
    if model.vertices.is_empty() {
        min = Vec3::default();
        max = Vec3::default();
    }

    let mut indices = Vec::with_capacity(model.faces.len() * 3);
    for face in &model.faces {
        for &index in face {
            if usize::from(index) >= model.vertices.len() {
                return Err(format!("{name}: face index out of range"));
            }
            indices.push(index);
        }
    }
    let levels = add_levels(model, &mut indices);
    let (vertices, vertex_count) = coarse_first(&vertices, &mut indices, &levels);

    let prop_points = model
        .prop_points
        .iter()
        .map(|point| PropPoint {
            name: point.name.clone(),
            position: [point.position.x, point.position.y, point.position.z],
            rotation: [
                point.rotation.x,
                point.rotation.y,
                point.rotation.z,
                point.rotation.w,
            ],
            bone: point.bone,
        })
        .collect();
    let skeleton = (bone_count > 0).then(|| Skeleton {
        bones: model
            .bones
            .iter()
            .map(|bone| Bone {
                translation: [bone.translation.x, bone.translation.y, bone.translation.z],
                rotation: [
                    bone.rotation.x,
                    bone.rotation.y,
                    bone.rotation.z,
                    bone.rotation.w,
                ],
            })
            .collect(),
    });
    let mesh = Mesh {
        vertex_count,
        vertices,
        indices,
        levels,
        skeleton: None,
        bone_count: bone_count as u32,
        prop_points,
        min: [min.x, min.y, min.z],
        max: [max.x, max.y, max.z],
        detail: None,
    };
    Ok((mesh, skeleton))
}

/// The engine's up to four influences, `NO_BONE` ending them early
/// (`CModelDef::SkinPoint`), as u8 bones and unorm8 weights. The rounding
/// error goes to the heaviest influence, so the weights still sum to one.
fn influences(
    name: &str,
    vertex: &pmd::Vertex,
    bone_count: usize,
) -> Result<([u8; 4], [u8; 4]), String> {
    let mut bones = [0u8; 4];
    let mut weights = [0u8; 4];
    let mut total = 0i32;
    let mut heaviest = None;
    for (slot, (&bone, &weight)) in vertex.bones.iter().zip(&vertex.weights).enumerate() {
        if bone == pmd::NO_BONE {
            break;
        }
        // Index `bone_count` is the bind shape, the model's own transform.
        if usize::from(bone) > bone_count {
            return Err(format!("{name}: a vertex names a missing bone"));
        }
        bones[slot] = bone;
        weights[slot] = (weight.clamp(0.0, 1.0) * 255.0).round() as u8;
        total += i32::from(weights[slot]);
        if heaviest.is_none_or(|best: usize| weights[slot] > weights[best]) {
            heaviest = Some(slot);
        }
    }
    if let Some(slot) = heaviest {
        let fixed = i32::from(weights[slot]) + 255 - total;
        weights[slot] = fixed.clamp(0, 255) as u8;
    }
    Ok((bones, weights))
}

/// meshoptimizer collapses edges in the bind pose; each level indexes the
/// same vertices, so it animates exactly like the full mesh. The indices
/// are laid out coarsest first, the levels listed full first.
///
/// The meshes are split at every hard edge and UV seam, and collapses that
/// keep the seams stop most of them near their full size, which would
/// leave the coarse file holding most of the vertices. The coarsest level
/// may cross seams, weighed by how far UVs and normals move, as it is only
/// seen a few dozen pixels tall.
fn add_levels(model: &Model, indices: &mut Vec<u16>) -> Vec<(u32, u32)> {
    let full: Vec<u32> = indices.iter().map(|index| u32::from(*index)).collect();
    if full.is_empty() {
        return vec![(0, 0)];
    }
    let positions: Vec<[f32; 3]> = model
        .vertices
        .iter()
        .map(|vertex| [vertex.position.x, vertex.position.y, vertex.position.z])
        .collect();
    let attributes: Vec<f32> = model
        .vertices
        .iter()
        .flat_map(|vertex| {
            [
                vertex.uv[0],
                vertex.uv[1],
                vertex.normal.x,
                vertex.normal.y,
                vertex.normal.z,
            ]
        })
        .collect();
    let locks = vec![false; model.vertices.len()];
    let mut lists = vec![full.clone()];
    for (level, ratio) in RATIOS.into_iter().enumerate() {
        let target = ((full.len() / 3) as f32 * ratio) as usize * 3;
        let coarsest = level == RATIOS.len() - 1;
        lists.push(if coarsest {
            meshopt::simplify_with_attributes_and_locks_decoder(
                &full,
                &positions,
                &attributes,
                &ATTRIBUTE_WEIGHTS,
                5 * 4,
                &locks,
                target,
                MAX_ERROR,
                meshopt::SimplifyOptions::Sparse | meshopt::SimplifyOptions::Permissive,
                None,
            )
        } else {
            meshopt::simplify_decoder(
                &full,
                &positions,
                target,
                MAX_ERROR,
                meshopt::SimplifyOptions::Sparse,
                None,
            )
        });
    }
    indices.clear();
    let mut levels = vec![(0, 0); lists.len()];
    for (level, list) in lists.iter().enumerate().rev() {
        levels[level] = (indices.len() as u32, list.len() as u32);
        indices.extend(list.iter().map(|index| *index as u16));
    }
    levels
}

/// Renumbers the vertices by first use, coarsest level first, so the
/// coarse file holds a prefix of them; vertices no face uses are dropped.
/// Returns the vertex bytes and count.
fn coarse_first(vertices: &[u8], indices: &mut [u16], levels: &[(u32, u32)]) -> (Vec<u8>, u32) {
    let size = files::VERTEX_SIZE;
    let mut renumbered: Vec<Option<u16>> = vec![None; vertices.len() / size];
    let mut out = Vec::with_capacity(vertices.len());
    // The coarsest level starts the indices, so index order is use order.
    debug_assert_eq!(levels.last().map(|level| level.0), Some(0));
    for index in indices.iter_mut() {
        let old = usize::from(*index);
        *index = *renumbered[old].get_or_insert_with(|| {
            let new = (out.len() / size) as u16;
            out.extend_from_slice(&vertices[old * size..][..size]);
            new
        });
    }
    let count = (out.len() / size) as u32;
    (out, count)
}
