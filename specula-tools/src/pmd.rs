//! Loader for the engine's mesh format, ported from `CModelDef::Load`
//! (graphics/ModelDef.cpp).

use crate::unpacker::Unpacker;
use specula_assets::math::Quat;
use specula_assets::math::Vec3;

/// Versions before 3 keep vertices relative to the bind pose and need a
/// fix-up the converter has not needed for years; it writes version 4.
const MIN_VERSION: u32 = 3;
pub const NO_BONE: u8 = 0xFF;

pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: [f32; 2],
    /// `NO_BONE` ends the list early.
    pub bones: [u8; 4],
    pub weights: [f32; 4],
}

pub struct Bone {
    pub translation: Vec3,
    pub rotation: Quat,
}

pub struct PropPoint {
    pub name: String,
    pub position: Vec3,
    pub rotation: Quat,
    /// `NO_BONE` when relative to the model's origin.
    pub bone: u8,
}

pub struct Model {
    pub vertices: Vec<Vertex>,
    pub faces: Vec<[u16; 3]>,
    /// The bind pose.
    pub bones: Vec<Bone>,
    pub prop_points: Vec<PropPoint>,
}

pub fn load(data: &[u8]) -> Result<Model, String> {
    let mut file = Unpacker::new(data, b"PSMD")?;
    if file.version < MIN_VERSION {
        return Err(format!("PMD version {} is too old", file.version));
    }
    let vertex_count = file.size()?;
    let uv_sets = if file.version <= 3 { 1 } else { file.size()? };
    let mut vertices = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        let position = file.vec3()?;
        let normal = file.vec3()?;
        let mut uv = [0.0; 2];
        for set in 0..uv_sets {
            let pair = [file.f32()?, file.f32()?];
            if set == 0 {
                uv = pair;
            }
        }
        let bones = file.bytes(4)?.try_into().unwrap();
        let weights = [file.f32()?, file.f32()?, file.f32()?, file.f32()?];
        vertices.push(Vertex {
            position,
            normal,
            uv,
            bones,
            weights,
        });
    }

    let face_count = file.size()?;
    let mut faces = Vec::with_capacity(face_count);
    for _ in 0..face_count {
        faces.push([file.u16()?, file.u16()?, file.u16()?]);
    }

    let bone_count = file.size()?;
    let mut bones = Vec::with_capacity(bone_count);
    for _ in 0..bone_count {
        bones.push(Bone {
            translation: file.vec3()?,
            rotation: file.quat()?,
        });
    }

    let prop_count = file.size()?;
    let mut prop_points = Vec::with_capacity(prop_count);
    for _ in 0..prop_count {
        prop_points.push(PropPoint {
            name: file.string()?,
            position: file.vec3()?,
            rotation: file.quat()?,
            bone: file.u8()?,
        });
    }
    if !file.is_empty() {
        return Err("PMD longer than its contents".into());
    }
    Ok(Model {
        vertices,
        faces,
        bones,
        prop_points,
    })
}
