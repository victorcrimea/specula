//! Coarser levels of detail for far units. meshoptimizer collapses edges
//! of each part's mesh in its bind pose; the result indexes the same
//! vertices, so the vertex animation textures need nothing new and the
//! coarse meshes animate exactly like the full one.

use crate::bake::Baked;

/// Share of the full triangle count each coarser level aims for.
const RATIOS: [f32; 2] = [0.4, 0.15];
/// The largest deviation allowed, relative to the part's own extents (with
/// `Sparse`); it stops a level short of its ratio rather than wreck the
/// silhouette of a small part.
const MAX_ERROR: f32 = 0.08;

pub fn add_levels(baked: &mut Baked) {
    // The bind pose is the first animation, one frame.
    let positions: Vec<[f32; 3]> = baked.animations[0]
        .positions
        .iter()
        .map(|position| [position.x, position.y, position.z])
        .collect();
    for part in &mut baked.parts {
        let (first, count) = part.levels[0];
        let full = baked.indices[first as usize..(first + count) as usize].to_vec();
        for ratio in RATIOS {
            let target = ((full.len() / 3) as f32 * ratio) as usize * 3;
            let coarse = meshopt::simplify_decoder(
                &full,
                &positions,
                target,
                MAX_ERROR,
                meshopt::SimplifyOptions::Sparse,
                None,
            );
            let first = baked.indices.len() as u32;
            baked.indices.extend_from_slice(&coarse);
            part.levels.push((first, coarse.len() as u32));
        }
    }
}
