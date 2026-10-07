//! A PMD as the client draws it: vertices ready for a vertex buffer, with
//! the engine's MikkTSpace tangents, skin weights quantised for the GPU,
//! coarser levels of detail, and the bind
//! pose split out as a skeleton so meshes sharing it share one file. The
//! coarsest level and the vertices it uses come first, so they can be
//! sent as a file of their own (`Mesh::split`).

use std::collections::HashMap;

use mikktspace::Geometry;
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
/// Share of the full mesh's surface area a level must keep, else the finer
/// level is used in its place. Foliage is cards, each its own island of
/// one or two quads, and meshoptimizer meets its triangle target by
/// collapsing whole cards, which thins a wheat field or a tree crown to a
/// few stalks; a level that only drops small detail keeps most of its area.
const MIN_AREA: f32 = 0.75;
/// UV and normal error against position error for the coarsest level.
const ATTRIBUTE_WEIGHTS: [f32; 5] = [1.0, 1.0, 0.5, 0.5, 0.5];

/// The mesh, without its skeleton reference, and the skeleton if it has
/// bones; the caller writes the skeleton first to learn its name.
pub fn convert(name: &str, model: &Model) -> Result<(Mesh, Option<Skeleton>), String> {
    let bone_count = model.bones.len();
    for face in &model.faces {
        if face
            .iter()
            .any(|&index| usize::from(index) >= model.vertices.len())
        {
            return Err(format!("{name}: face index out of range"));
        }
    }
    let (welded, faces) = tangents(name, model)?;
    let sources: Vec<&pmd::Vertex> = welded
        .iter()
        .map(|(source, _)| &model.vertices[*source])
        .collect();
    let mut vertices = Vec::with_capacity(welded.len() * files::VERTEX_SIZE);
    let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = Vec3::new(f32::MIN, f32::MIN, f32::MIN);
    for vertex in &model.vertices {
        min = min.min(vertex.position);
        max = max.max(vertex.position);
    }
    for (vertex, (_, tangent)) in sources.iter().zip(&welded) {
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
            vertex.uv2[0],
            1.0 - vertex.uv2[1],
        ] {
            vertices.extend_from_slice(&value.to_le_bytes());
        }
        vertices.extend(tangent.map(|value| (value.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8));
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

    let mut indices: Vec<u16> = faces.into_iter().flatten().collect();
    let levels = add_levels(&sources, &mut indices);
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

/// `ModelRenderer::GenTangents` and the `WeldMesh` after it
/// (InstancingModelRenderer.cpp): a MikkTSpace tangent per face corner, then
/// corners welded back where every attribute matches. Corners of one PMD
/// vertex differ only in their tangent, so a vertex splits where its
/// faces disagree on it. Returns the welded vertices as (PMD vertex,
/// tangent with the bitangent's sign in w) and the faces indexing them.
#[allow(clippy::type_complexity)]
fn tangents(name: &str, model: &Model) -> Result<(Vec<(usize, [f32; 4])>, Vec<[u16; 3]>), String> {
    let mut corners = Corners {
        model,
        tangents: vec![[1.0, 0.0, 0.0, 1.0]; model.faces.len() * 3],
    };
    // A mesh MikkTSpace cannot handle keeps the default above; the engine
    // draws it with whatever it was left with too.
    if !model.faces.is_empty() {
        mikktspace::generate_tangents(&mut corners);
    }
    let mut welded = Vec::new();
    let mut index_of: HashMap<(u16, [u32; 4]), u16> = HashMap::new();
    let mut faces = Vec::with_capacity(model.faces.len());
    for (face, corner_tangents) in model.faces.iter().zip(corners.tangents.chunks_exact(3)) {
        let mut out = [0u16; 3];
        for (slot, (&source, tangent)) in face.iter().zip(corner_tangents).enumerate() {
            let key = (source, tangent.map(f32::to_bits));
            out[slot] = match index_of.get(&key) {
                Some(&index) => index,
                None => {
                    let index = u16::try_from(welded.len())
                        .map_err(|_| format!("{name}: more than 65535 vertices with tangents"))?;
                    welded.push((usize::from(source), *tangent));
                    index_of.insert(key, index);
                    index
                }
            };
        }
        faces.push(out);
    }
    Ok((welded, faces))
}

/// `MikkTSpace`'s callbacks (MikktspaceWrap.cpp): the first UV set as the
/// GPU gets it, `1 - v`.
struct Corners<'a> {
    model: &'a Model,
    tangents: Vec<[f32; 4]>,
}

impl Corners<'_> {
    fn vertex(&self, face: usize, vert: usize) -> &pmd::Vertex {
        &self.model.vertices[usize::from(self.model.faces[face][vert])]
    }
}

impl Geometry for Corners<'_> {
    fn num_faces(&self) -> usize {
        self.model.faces.len()
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        let p = self.vertex(face, vert).position;
        [p.x, p.y, p.z]
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        let n = self.vertex(face, vert).normal;
        [n.x, n.y, n.z]
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        let uv = self.vertex(face, vert).uv;
        [uv[0], 1.0 - uv[1]]
    }

    fn set_tangent_encoded(&mut self, tangent: [f32; 4], face: usize, vert: usize) {
        self.tangents[face * 3 + vert] = tangent;
    }
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
fn add_levels(vertices: &[&pmd::Vertex], indices: &mut Vec<u16>) -> Vec<(u32, u32)> {
    let full: Vec<u32> = indices.iter().map(|index| u32::from(*index)).collect();
    if full.is_empty() {
        return vec![(0, 0)];
    }
    let positions: Vec<[f32; 3]> = vertices
        .iter()
        .map(|vertex| [vertex.position.x, vertex.position.y, vertex.position.z])
        .collect();
    let attributes: Vec<f32> = vertices
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
    let locks = vec![false; vertices.len()];
    let mut lists = vec![full.clone()];
    let full_area = area(&positions, &full);
    for (level, ratio) in RATIOS.into_iter().enumerate() {
        let target = ((full.len() / 3) as f32 * ratio) as usize * 3;
        let coarsest = level == RATIOS.len() - 1;
        let list = if coarsest {
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
        };
        if area(&positions, &list) >= full_area * MIN_AREA {
            lists.push(list);
        } else {
            lists.push(lists[lists.len() - 1].clone());
        }
    }
    indices.clear();
    let mut levels = vec![(0, 0); lists.len()];
    for (level, list) in lists.iter().enumerate().rev() {
        levels[level] = (indices.len() as u32, list.len() as u32);
        indices.extend(list.iter().map(|index| *index as u16));
    }
    levels
}

fn area(positions: &[[f32; 3]], list: &[u32]) -> f32 {
    list.chunks_exact(3)
        .map(|triangle| {
            let [a, b, c] = [0, 1, 2].map(|corner| positions[triangle[corner] as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt() / 2.0
        })
        .sum()
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
