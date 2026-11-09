//! Picking entities under the pointer and in a box, a port of
//! `simulation2/helpers/Selection.cpp` and `getPreferredEntities` of
//! `gui/session/input.js`. The view records a `Pickable` for each entity
//! it draws, so only what is on screen can be picked.

use specula_assets::math::Mat4;
use specula_assets::math::Vec3;

use crate::stream::TemplateDetails;
use crate::unit::dot;
use crate::unit::normalize;
use crate::unit::sub;

/// `TemplateDetails::flags`, as `SpeculaFeed.h` kind 11 numbers them.
pub const FLAG_SELECTABLE: u32 = 1;
pub const FLAG_EDITOR_ONLY: u32 = 2;
pub const FLAG_OUTLINE: u32 = 4;
pub const FLAG_ALWAYS_VISIBLE: u32 = 8;
pub const FLAG_FOOTPRINT_SELECTION: u32 = 16;
pub const FLAG_SQUARE_FOOTPRINT: u32 = 32;
pub const FLAG_OVERLAY_SQUARE: u32 = 64;
pub const FLAG_OVERLAY_CIRCLE: u32 = 128;
pub const FLAG_FOOTPRINT: u32 = 256;
pub const FLAG_STATUS_BARS: u32 = 512;
pub const FLAG_INFINITE_SUPPLY: u32 = 1024;

/// An entity drawn this frame, in world space.
pub struct Pickable {
    pub id: u32,
    /// `ICmpVisual::GetPosition`: the model's translation.
    pub position: [f32; 3],
    /// The engine's `unitX` for the interpolated heading, unit length: the
    /// drawn model's -x on the ground, as models are turned by an extra PI.
    pub facing: [f32; 2],
    pub bounds: OrientedBox,
}

/// `CBoundingBoxOriented`.
pub struct OrientedBox {
    pub centre: [f32; 3],
    axes: [[f32; 3]; 3],
    half: [f32; 3],
}

impl OrientedBox {
    /// `CBoundingBoxAligned::Transform` into an oriented box.
    pub fn new(low: Vec3, high: Vec3, transform: &Mat4) -> Self {
        let centre = transform.transform((low + high) * 0.5);
        let extent = [high.x - low.x, high.y - low.y, high.z - low.z];
        let units = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ];
        let mut axes = [[0.0; 3]; 3];
        let mut half = [0.0; 3];
        for axis in 0..3 {
            let moved = transform.rotate(units[axis]);
            let moved = [moved.x, moved.y, moved.z];
            let length = dot(moved, moved).sqrt();
            axes[axis] = if length > 0.0 {
                normalize(moved)
            } else {
                moved
            };
            half[axis] = extent[axis] * length / 2.0;
        }
        OrientedBox {
            centre: [centre.x, centre.y, centre.z],
            axes,
            half,
        }
    }

    /// `CBoundingBoxOriented::RayIntersect`, the slab test.
    fn ray_intersect(&self, ray: &Ray) -> bool {
        let mut t_min = f32::MIN;
        let mut t_max = f32::MAX;
        let p = sub(self.centre, ray.origin);
        for axis in 0..3 {
            let e = dot(self.axes[axis], p);
            let f = dot(self.axes[axis], ray.dir);
            if f.abs() > 1e-10 {
                let mut t1 = (e + self.half[axis]) / f;
                let mut t2 = (e - self.half[axis]) / f;
                if t1 > t2 {
                    std::mem::swap(&mut t1, &mut t2);
                }
                t_min = t_min.max(t1);
                t_max = t_max.min(t2);
                if t_min > t_max || t_max < 0.0 {
                    return false;
                }
            } else if e.abs() > self.half[axis] {
                return false;
            }
        }
        true
    }
}

/// The box a template picks by, in the model's space: its footprint when
/// `VisualActor/SelectionShape` says so, else `bounds`, the model's,
/// clipped at the ground (`CModelAbstract::CalcSelectionBox`). `None`
/// when the template is not selectable, or picks by the model's bounds
/// and `bounds` is `None`, as a decal's, which the engine leaves empty.
pub fn local_box(
    details: Option<&TemplateDetails>,
    bounds: Option<(Vec3, Vec3)>,
) -> Option<(Vec3, Vec3)> {
    let details = details?;
    if details.flags & FLAG_SELECTABLE == 0 || details.flags & FLAG_EDITOR_ONLY != 0 {
        return None;
    }
    let footprint = FLAG_FOOTPRINT_SELECTION | FLAG_FOOTPRINT;
    if details.flags & footprint == footprint {
        // A circle's sizes are its radius; the engine boxes it all the same.
        let scale = if details.flags & FLAG_SQUARE_FOOTPRINT != 0 {
            0.5
        } else {
            1.0
        };
        let (x, z) = (
            details.footprint_width * scale,
            details.footprint_depth * scale,
        );
        return Some((
            Vec3::new(-x, 0.0, -z),
            Vec3::new(x, details.footprint_height, z),
        ));
    }
    let (mut low, high) = bounds?;
    // So the box does not reach through the terrain.
    if high.y > 0.0 {
        low.y = low.y.max(0.0);
    }
    Some((low, high))
}

pub struct Ray {
    origin: [f32; 3],
    /// Unit length.
    dir: [f32; 3],
}

/// `CCamera::BuildCameraRay` from the camera's basis: x and y run 0..1
/// across the canvas from its top left, `tan_half` is the tangent of half
/// the vertical field of view.
pub fn camera_ray(
    eye: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
    tan_half: f32,
    aspect: f32,
    [x, y]: [f32; 2],
) -> Ray {
    let across = (2.0 * x - 1.0) * tan_half * aspect;
    let rise = (1.0 - 2.0 * y) * tan_half;
    let dir = std::array::from_fn(|i| forward[i] + right[i] * across + up[i] * rise);
    Ray {
        origin: eye,
        dir: normalize(dir),
    }
}

/// Where `ray` first meets the ground (`ground` gives its height, water
/// counted as ground) within a map `map_size` metres across, x and z in
/// metres; the order's target point. Marched in steps, then halved down,
/// which is near enough for an order.
pub fn ground_hit(ray: &Ray, ground: impl Fn(f32, f32) -> f32, map_size: f32) -> Option<[f32; 2]> {
    const STEP: f32 = 2.0;
    const REACH: f32 = 4000.0;
    let at = |t: f32| std::array::from_fn::<f32, 3, _>(|i| ray.origin[i] + ray.dir[i] * t);
    let below = |t: f32| {
        let point = at(t);
        point[1] <= ground(point[0], point[2])
    };
    let mut near = 0.0;
    let mut far = STEP;
    while !below(far) {
        near = far;
        far += STEP;
        if far > REACH {
            return None;
        }
    }
    for _ in 0..12 {
        let middle = (near + far) / 2.0;
        if below(middle) {
            far = middle;
        } else {
            near = middle;
        }
    }
    let point = at(far);
    let inside = |value: f32| (0.0..=map_size).contains(&value);
    (inside(point[0]) && inside(point[2])).then_some([point[0], point[2]])
}

/// `PickEntityAtPoint`: of the boxes the ray goes through, the one whose
/// centre is nearest the ray, not the nearest to the eye.
pub fn pick_at_point(pickables: &[Pickable], ray: &Ray) -> Option<u32> {
    pickables
        .iter()
        .filter(|pickable| pickable.bounds.ray_intersect(ray))
        .map(|pickable| {
            let centre = pickable.bounds.centre;
            let along = dot(sub(centre, ray.origin), ray.dir);
            let closest: [f32; 3] = std::array::from_fn(|i| ray.origin[i] + ray.dir[i] * along);
            let off = sub(closest, centre);
            (dot(off, off), pickable.id)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// Where a point shows, 0..1 across the canvas from its top left, or
/// `None` off screen or behind the camera (`CFrustum::IsPointVisible`).
fn screen_point(view_proj: &[[f32; 4]; 4], [x, y, z]: [f32; 3]) -> Option<[f32; 2]> {
    let clip: [f32; 4] = std::array::from_fn(|row| {
        let m = view_proj[row];
        m[0] * x + m[1] * y + m[2] * z + m[3]
    });
    let w = clip[3];
    let inside =
        w > 0.0 && clip[0].abs() <= w && clip[1].abs() <= w && (0.0..=w).contains(&clip[2]);
    inside.then(|| [(clip[0] / w + 1.0) / 2.0, (1.0 - clip[1] / w) / 2.0])
}

/// `PickEntitiesInRect`: by the position alone, not the box, which makes
/// busy places easier to pick from. Corners 0..1 across the canvas.
pub fn pick_in_rect(
    pickables: &[Pickable],
    view_proj: &[[f32; 4]; 4],
    from: [f32; 2],
    to: [f32; 2],
) -> Vec<u32> {
    let (x0, x1) = (from[0].min(to[0]), from[0].max(to[0]));
    let (y0, y1) = (from[1].min(to[1]), from[1].max(to[1]));
    pickables
        .iter()
        .filter(|pickable| {
            screen_point(view_proj, pickable.position)
                .is_some_and(|[x, y]| (x0..=x1).contains(&x) && (y0..=y1).contains(&y))
        })
        .map(|pickable| pickable.id)
        .collect()
}

/// `PickSimilarEntities` on screen: the entities `similar` accepts.
pub fn pick_similar(
    pickables: &[Pickable],
    view_proj: &[[f32; 4]; 4],
    similar: impl Fn(u32) -> bool,
) -> Vec<u32> {
    pickables
        .iter()
        .filter(|pickable| screen_point(view_proj, pickable.position).is_some())
        .filter(|pickable| similar(pickable.id))
        .map(|pickable| pickable.id)
        .collect()
}

/// `getPreferredEntities` without its hotkeys: the units, else the
/// defensive structures, else all of them.
pub fn preferred(ids: Vec<u32>, has_class: impl Fn(u32, &str) -> bool) -> Vec<u32> {
    for class in ["Unit", "Defensive"] {
        let kept: Vec<u32> = ids
            .iter()
            .copied()
            .filter(|id| has_class(*id, class))
            .collect();
        if !kept.is_empty() {
            return kept;
        }
    }
    ids
}

/// `GetIdentityClasses`: the classes, the visible classes and the rank.
pub fn has_class(details: &TemplateDetails, strings: &[String], class: &str) -> bool {
    [details.classes, details.visible_classes, details.rank]
        .into_iter()
        .filter_map(|index| {
            (index as usize)
                .checked_sub(1)
                .and_then(|index| strings.get(index))
        })
        .any(|tokens| tokens.split_whitespace().any(|token| token == class))
}
