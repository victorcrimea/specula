//! Skins every frame of the chosen animations on the CPU, the way the
//! engine's CPU skinning path does (`CModel::ValidatePosition`,
//! `CModelDef::SkinPoint` and `SkinNormal`), and flattens the actor's model
//! tree into one vertex list so the client only reads positions back.

use std::fs;
use std::path::Path;

use specula_assets::math::Mat4;
use specula_assets::math::Vec3;
use specula_tools::convert::cached_path;
use specula_tools::pmd;
use specula_tools::pmd::Model;
use specula_tools::psa;
use specula_tools::psa::Animation;

use crate::actor::Part;
use crate::texture;
use specula_tools::image::Image;

/// `REQUIRE_ALPHA_GEQUAL` of the opaque pass in
/// shaders/effects/model_transparent.xml. The engine also draws a blended
/// pass for the soft edges; the client does not yet.
const TRANSPARENT_ALPHA_REF: f32 = 0.6375;
/// The engine shows a skeletal model with no animation in its bind pose;
/// baked as a one-frame animation it shows whether skinning is right.
pub const BIND_POSE: &str = "bind pose";

pub struct Baked {
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub parts: Vec<BakedPart>,
    pub animations: Vec<BakedAnimation>,
    pub min: Vec3,
    pub max: Vec3,
}

pub struct BakedPart {
    pub actor: String,
    pub vertices: usize,
    /// First index and index count of each level of detail, the full mesh
    /// first.
    pub levels: Vec<(u32, u32)>,
    pub player_color: bool,
    /// Fragments with less alpha are discarded; 0 for none.
    pub alpha_ref: f32,
    pub texture: String,
    pub image: Image,
}

pub struct BakedAnimation {
    pub name: String,
    pub file: String,
    pub frame_ms: f32,
    pub frames: usize,
    /// `frames * vertex count`, frame-major.
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
}

/// One model of the flattened tree.
struct Node<'a> {
    part: &'a Part,
    model: Model,
    inverse_bind: Vec<Mat4>,
    /// The node it is propped on and the index of the prop point there.
    parent: Option<(usize, usize)>,
    first_vertex: usize,
}

/// A node's placement in one frame.
struct Pose {
    transform: Mat4,
    /// Bone to model space, before the inverse bind; props attach here.
    bones: Vec<Mat4>,
}

pub fn bake(
    root: &Part,
    mod_dir: &Path,
    converted: &Path,
    animation_names: &[&str],
) -> Result<Baked, String> {
    let mut nodes = Vec::new();
    flatten(root, None, converted, &mut nodes)?;

    let mut baked = Baked {
        uvs: Vec::new(),
        indices: Vec::new(),
        parts: Vec::new(),
        animations: Vec::new(),
        min: Vec3::new(f32::MAX, f32::MAX, f32::MAX),
        max: Vec3::new(f32::MIN, f32::MIN, f32::MIN),
    };
    for node in &mut nodes {
        node.first_vertex = baked.uvs.len();
        baked
            .uvs
            // ModelRenderer::BuildUV hands the GPU 1 - v, against textures
            // stored top row first.
            .extend(
                node.model
                    .vertices
                    .iter()
                    .map(|vertex| [vertex.uv[0], 1.0 - vertex.uv[1]]),
            );
        let first_index = baked.indices.len() as u32;
        for face in &node.model.faces {
            for &index in face {
                if usize::from(index) >= node.model.vertices.len() {
                    return Err(format!("{}: face index out of range", node.part.mesh));
                }
                baked
                    .indices
                    .push((node.first_vertex + usize::from(index)) as u32);
            }
        }
        let (texture, image) = match &node.part.texture {
            Some(name) => {
                let path = format!("art/textures/skins/{name}");
                let bytes =
                    fs::read(mod_dir.join(&path)).map_err(|error| format!("{path}: {error}"))?;
                let image =
                    texture::decode(name, &bytes).map_err(|error| format!("{path}: {error}"))?;
                (name.clone(), image)
            }
            None => ("(none)".to_string(), Image::white()),
        };
        baked.parts.push(BakedPart {
            actor: node.part.actor.clone(),
            vertices: node.model.vertices.len(),
            levels: vec![(first_index, baked.indices.len() as u32 - first_index)],
            player_color: node.part.material.player_color,
            alpha_ref: if node.part.material.transparent {
                TRANSPARENT_ALPHA_REF
            } else {
                0.0
            },
            texture,
            image,
        });
    }

    let vertex_count = baked.uvs.len();
    for name in std::iter::once(BIND_POSE).chain(animation_names.iter().copied()) {
        let (file, frame_ms, animation) = if name == BIND_POSE {
            ("-".to_string(), 0.0, None)
        } else {
            let source = root
                .animations
                .get(name)
                .and_then(|list| list.first())
                .ok_or_else(|| {
                    format!(
                        "{} has no {name} animation; it has {:?}",
                        root.actor,
                        root.animations.keys().collect::<Vec<_>>()
                    )
                })?;
            let path = cached_path(converted, &format!("art/animation/{}", source.file));
            let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let animation =
                psa::load(&bytes).map_err(|error| format!("{}: {error}", source.file))?;
            // CModel::SetAnimation refuses the same mismatch.
            if animation.keys != nodes[0].model.bones.len() {
                return Err(format!(
                    "{} has {} keys, {} has {} bones",
                    source.file,
                    animation.keys,
                    root.mesh,
                    nodes[0].model.bones.len()
                ));
            }
            (
                source.file.clone(),
                animation.frame_time / source.speed,
                Some(animation),
            )
        };
        let frames = animation.as_ref().map_or(1, |animation| animation.frames);
        let mut positions = Vec::with_capacity(frames * vertex_count);
        let mut normals = Vec::with_capacity(frames * vertex_count);
        for frame in 0..frames {
            let poses = pose_frame(&nodes, animation.as_ref(), frame)?;
            for (node, pose) in nodes.iter().zip(&poses) {
                skin(node, pose, &mut positions, &mut normals)?;
            }
        }
        for position in &positions {
            baked.min = baked.min.min(*position);
            baked.max = baked.max.max(*position);
        }
        baked.animations.push(BakedAnimation {
            name: name.to_string(),
            file,
            frame_ms,
            frames,
            positions,
            normals,
        });
    }
    Ok(baked)
}

fn flatten<'a>(
    part: &'a Part,
    parent: Option<(usize, usize)>,
    converted: &Path,
    nodes: &mut Vec<Node<'a>>,
) -> Result<(), String> {
    let path = cached_path(converted, &format!("art/meshes/{}", part.mesh));
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let model = pmd::load(&bytes).map_err(|error| format!("{}: {error}", part.mesh))?;
    let inverse_bind = model
        .bones
        .iter()
        .map(|bone| Mat4::inverse_bone(bone.translation, bone.rotation))
        .collect();
    let index = nodes.len();
    let prop_points: Vec<String> = model
        .prop_points
        .iter()
        .map(|point| point.name.clone())
        .collect();
    nodes.push(Node {
        part,
        model,
        inverse_bind,
        parent,
        first_vertex: 0,
    });
    for prop in &part.props {
        let point = prop_points
            .iter()
            .position(|name| *name == prop.point)
            .ok_or_else(|| {
                format!(
                    "{} has no prop point {} for {}; it has {prop_points:?}",
                    part.mesh, prop.point, prop.part.actor
                )
            })?;
        flatten(&prop.part, Some((index, point)), converted, nodes)?;
    }
    Ok(())
}

/// Parents come before their props in `nodes`, so one pass places them all.
fn pose_frame(
    nodes: &[Node],
    animation: Option<&Animation>,
    frame: usize,
) -> Result<Vec<Pose>, String> {
    let mut poses: Vec<Pose> = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let transform = match node.parent {
            None => Mat4::identity(),
            Some((parent, point)) => {
                let point = &nodes[parent].model.prop_points[point];
                let parent = &poses[parent];
                let mut transform = Mat4::from_bone(point.position, point.rotation);
                if point.bone == pmd::NO_BONE {
                    transform.concatenate(&parent.transform);
                } else {
                    let bone = parent
                        .bones
                        .get(usize::from(point.bone))
                        .ok_or_else(|| format!("prop point {} names a missing bone", point.name))?;
                    transform.concatenate(bone);
                }
                transform
            }
        };
        // Only the root plays the animation; props stay in their bind pose,
        // as the engine shows a skeletal model without one.
        let bones = node
            .model
            .bones
            .iter()
            .enumerate()
            .map(|(bone, bind)| {
                let mut matrix = match animation.filter(|_| index == 0) {
                    Some(animation) => {
                        let key = animation.key(frame, bone);
                        Mat4::from_bone(key.translation, key.rotation)
                    }
                    None => Mat4::from_bone(bind.translation, bind.rotation),
                };
                matrix.concatenate(&transform);
                matrix
            })
            .collect();
        poses.push(Pose { transform, bones });
    }
    Ok(poses)
}

fn skin(
    node: &Node,
    pose: &Pose,
    positions: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
) -> Result<(), String> {
    if node.model.bones.is_empty() {
        for vertex in &node.model.vertices {
            positions.push(pose.transform.transform(vertex.position));
            normals.push(pose.transform.rotate(vertex.normal).normalized());
        }
        return Ok(());
    }
    // Index `bones.len()` is the bind shape, which the engine fills with the
    // model's own transform.
    let mut matrices: Vec<Mat4> = pose
        .bones
        .iter()
        .zip(&node.inverse_bind)
        .map(|(bone, inverse)| bone.mul(inverse))
        .collect();
    matrices.push(pose.transform);
    for vertex in &node.model.vertices {
        let mut position = Vec3::default();
        let mut normal = Vec3::default();
        let mut influences = 0;
        for (bone, weight) in vertex.bones.iter().zip(vertex.weights) {
            if *bone == pmd::NO_BONE {
                break;
            }
            let matrix = matrices
                .get(usize::from(*bone))
                .ok_or_else(|| format!("{}: vertex names a missing bone", node.part.mesh))?;
            position = position + matrix.transform(vertex.position) * weight;
            normal = normal + matrix.rotate(vertex.normal) * weight;
            influences += 1;
        }
        positions.push(position);
        normals.push(if influences > 1 {
            normal.normalized()
        } else {
            normal
        });
    }
    Ok(())
}
