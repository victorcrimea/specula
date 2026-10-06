//! Loader for the engine's skeletal animation format, ported from
//! `CSkeletonAnimDef::Load` (graphics/SkeletonAnimDef.cpp).

use crate::pmd::Bone;
use crate::unpacker::Unpacker;

pub struct Animation {
    /// In milliseconds.
    pub frame_time: f32,
    pub frames: usize,
    /// One per bone of the model it animates.
    pub keys: usize,
    /// Frame-major, as `GetKey(frame, bone)` indexes it.
    pub states: Vec<Bone>,
}

impl Animation {
    pub fn key(&self, frame: usize, bone: usize) -> &Bone {
        &self.states[frame * self.keys + bone]
    }
}

pub fn load(data: &[u8]) -> Result<Animation, String> {
    let mut file = Unpacker::new(data, b"PSSA")?;
    // The name is unused, as in the engine.
    file.string()?;
    let frame_time = file.f32()?;
    let keys = file.size()?;
    let frames = file.size()?;
    if frames == 0 {
        return Err("animation has no frames".into());
    }
    let mut states = Vec::with_capacity(keys * frames);
    for _ in 0..keys * frames {
        states.push(Bone {
            translation: file.vec3()?,
            rotation: file.quat()?,
        });
    }
    if !file.is_empty() {
        return Err("PSA longer than its contents".into());
    }
    Ok(Animation {
        frame_time,
        frames,
        keys,
        states,
    })
}
