//! The bone poses the model shader skins with, for every (animation,
//! skeleton) pair in use, in one RGBA32F texture: WebGL2 has no storage
//! buffers, and one texture lets a single draw hold units in any
//! animation. Each key is stored already composed with the bind pose's
//! inverse, so the shader needs two texels per bone and frame: the
//! rotation, then the translation.
//!
//! The engine's own key interpolation (`CSkeletonAnimDef::BuildBoneMatrices`)
//! is here too, for placing props on their parent's bones.

use std::collections::HashMap;

use specula_assets::files::AnimationData;
use specula_assets::files::Bone;
use specula_assets::files::Skeleton;
use specula_assets::math::Mat4;
use specula_assets::math::Quat;
use specula_assets::math::Vec3;

/// Texels per row; model.wgsl has the same.
pub const WIDTH: u32 = 2048;
const FIRST_ROWS: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

#[derive(Clone, Copy)]
pub struct Range {
    /// First texel.
    pub base: u32,
    pub bones: u32,
}

impl Range {
    /// The first texel of `frame`.
    pub fn frame(&self, frame: u32) -> u32 {
        self.base + frame * self.bones * 2
    }
}

pub struct Atlas {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    rows: u32,
    max_rows: u32,
    /// The texture's content, RGBA per texel, so it can be regrown.
    data: Vec<f32>,
    used: u32,
    /// Rows written since the last upload.
    dirty: Option<(u32, u32)>,
    /// Bumped whenever `view` changes, so bind groups are remade.
    pub generation: u32,
    /// `None` when the pair does not fit: the keys do not match the bones,
    /// or the atlas is full.
    ranges: HashMap<(u64, u64), Option<Range>>,
    full_reported: bool,
}

impl Atlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let max_rows = device.limits().max_texture_dimension_2d;
        let rows = FIRST_ROWS.min(max_rows);
        let (texture, view) = create(device, rows);
        Atlas {
            texture,
            view,
            rows,
            max_rows,
            data: Vec::new(),
            used: 0,
            dirty: None,
            generation: 0,
            ranges: HashMap::new(),
            full_reported: false,
        }
    }

    pub fn range(
        &mut self,
        animation_hash: u64,
        animation: &AnimationData,
        skeleton_hash: u64,
        skeleton: &Skeleton,
    ) -> Option<Range> {
        if let Some(range) = self.ranges.get(&(animation_hash, skeleton_hash)) {
            return *range;
        }
        let range = self.add(animation, skeleton);
        self.ranges.insert((animation_hash, skeleton_hash), range);
        range
    }

    fn add(&mut self, animation: &AnimationData, skeleton: &Skeleton) -> Option<Range> {
        let bones = skeleton.bones.len() as u32;
        // `CModel::SetAnimation` refuses the same mismatch.
        if animation.keys != bones || bones == 0 {
            return None;
        }
        let texels = animation.frames * bones * 2;
        let end = self.used + texels;
        if end.div_ceil(WIDTH) > self.max_rows {
            if !self.full_reported {
                self.full_reported = true;
                crate::log("pose atlas full; further animations show the bind pose");
            }
            return None;
        }
        let range = Range {
            base: self.used,
            bones,
        };
        self.data
            .resize(end.div_ceil(WIDTH) as usize * WIDTH as usize * 4, 0.0);
        let inverse: Vec<(Quat, Vec3)> = skeleton.bones.iter().map(rigid).collect();
        for (index, state) in animation.states.iter().enumerate() {
            let (rotation, translation) = rigid(state);
            let (bind_rotation, bind_translation) = inverse[index % bones as usize];
            // key * bind^-1: rotate by both, and move the bind pose's origin
            // to the key's.
            let rotation = rotation.product(bind_rotation.inverse());
            let translation = translation + -rotation.rotate(bind_translation);
            let at = (self.used as usize + index * 2) * 4;
            self.data[at..at + 4]
                .copy_from_slice(&[rotation.x, rotation.y, rotation.z, rotation.w]);
            self.data[at + 4..at + 8].copy_from_slice(&[
                translation.x,
                translation.y,
                translation.z,
                0.0,
            ]);
        }
        let first_row = self.used / WIDTH;
        let last_row = (end - 1) / WIDTH;
        self.used = end;
        self.dirty = Some(match self.dirty {
            Some((first, last)) => (first.min(first_row), last.max(last_row)),
            None => (first_row, last_row),
        });
        Some(range)
    }

    /// Sends what was added since the last call, growing the texture first
    /// if it ran out of rows.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some((mut first, last)) = self.dirty.take() else {
            return;
        };
        if last >= self.rows {
            let mut rows = self.rows;
            while rows <= last {
                rows *= 2;
            }
            self.rows = rows.min(self.max_rows);
            (self.texture, self.view) = create(device, self.rows);
            self.generation += 1;
            first = 0;
        }
        let start = first as usize * WIDTH as usize * 4;
        let end = (last as usize + 1) * WIDTH as usize * 4;
        let bytes: Vec<u8> = self.data[start..end]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: first,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 16),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: last - first + 1,
                depth_or_array_layers: 1,
            },
        );
    }
}

fn create(device: &wgpu::Device, rows: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("poses"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: rows,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn rigid(bone: &Bone) -> (Quat, Vec3) {
    let [x, y, z, w] = bone.rotation;
    let [tx, ty, tz] = bone.translation;
    (Quat { x, y, z, w }, Vec3::new(tx, ty, tz))
}

/// Where an animation is at `time` ms, as `BuildBoneMatrices` reads it:
/// frame a, frame b and how far from a to b. A non-looping animation does
/// not blend its last frame back into its first.
pub fn frames(animation: &AnimationData, time: f32, looping: bool) -> (u32, u32, f32) {
    let position = time / animation.frame_ms.max(f32::EPSILON);
    let start = position.max(0.0).floor();
    let blend = position - start;
    let start = start as u32 % animation.frames;
    let end = (start + 1) % animation.frames;
    if !looping && end == 0 {
        (start, start, 0.0)
    } else {
        (start, end, blend)
    }
}

/// One bone in model space before the inverse bind, where props attach:
/// the keys slerped and lerped as the engine does, or the bind pose when
/// there is no animation.
pub fn bone_matrix(
    animation: Option<(&AnimationData, (u32, u32, f32))>,
    skeleton: &Skeleton,
    bone: usize,
) -> Mat4 {
    match animation {
        Some((animation, (a, b, blend))) if (bone as u32) < animation.keys => {
            let keys = animation.keys as usize;
            let (rotation_a, translation_a) = rigid(&animation.states[a as usize * keys + bone]);
            let (rotation_b, translation_b) = rigid(&animation.states[b as usize * keys + bone]);
            Mat4::from_bone(
                translation_a.lerp(translation_b, blend),
                rotation_a.slerp(rotation_b, blend),
            )
        }
        _ => {
            let (rotation, translation) = skeleton
                .bones
                .get(bone)
                .map_or((Quat::IDENTITY, Vec3::default()), rigid);
            Mat4::from_bone(translation, rotation)
        }
    }
}
