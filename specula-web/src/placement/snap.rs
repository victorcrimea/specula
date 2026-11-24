//! Where a foundation snaps to: a nearby tower, the edges of nearby
//! structures, or the water at a dock.

use std::f64::consts::PI;
use std::f64::consts::TAU;

use super::data::Footprint;
use super::data::PlaceData;
use super::geometry::Square;
use super::geometry::distance_to_segment;
use super::restrictions::World;
use super::restrictions::pose_of;
use crate::stream::Ground;
use crate::stream::ObstructionKind;
use crate::stream::Scene;

/// `gui.session.snaptoedgesdistancethreshold`'s default.
const SNAP_EDGE_THRESHOLD: f64 = 10.0;
/// `ObstructionSnap.prototype.MinimalDistanceToSnap` and `EPS`.
const MINIMAL_DISTANCE_TO_SNAP: f64 = 5.0;
const EPS: f64 = 1e-3;
/// `ObstructionSnap`'s padding against a loss of accuracy.
const SNAP_PADDING: f64 = 0.05;

type Vector = [f64; 2];

fn dot(a: Vector, b: Vector) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// An edge of a static obstruction, as `StaticEdgesNear` lists them, with
/// the distances `getValidEdges` works out for a position.
#[derive(Clone, Copy)]
pub struct Edge {
    begin: Vector,
    end: Vector,
    angle: f64,
    normal: Vector,
    /// An edge the snap reaches from inside its own entity.
    counter_clockwise: bool,
    signed_distance: f64,
    offset_distance: f64,
}

/// `StaticEdgesNear`: the edges of every static obstruction of the scene
/// within the threshold of a point, large enough to snap to.
pub fn edges_near(scene: &Scene, x: f64, z: f64) -> Vec<Edge> {
    let mut edges = Vec::new();
    for dot_ in scene.dots.values() {
        let Some(obstruction) = scene
            .template_details
            .get(dot_.template as usize)
            .and_then(|details| details.as_ref())
            .and_then(|details| details.obstruction.as_ref())
            .filter(|obstruction| obstruction.kind == ObstructionKind::Static)
        else {
            continue;
        };
        let (half_width, half_depth) = (obstruction.width / 2.0, obstruction.depth / 2.0);
        if half_width == 0.0 || half_depth == 0.0 || half_width.max(half_depth) <= 2.0 {
            continue;
        }
        let (px, pz, angle) = pose_of(dot_);
        let corners = Square::new(px, pz, angle, obstruction.width, obstruction.depth).corners();
        for (index, corner) in corners.iter().enumerate() {
            let next = corners[(index + 1) % corners.len()];
            if distance_to_segment([x, z], *corner, next) > SNAP_EDGE_THRESHOLD {
                continue;
            }
            // `-(next - corner).Perpendicular()`, normalised.
            let (dx, dz) = (next[0] - corner[0], next[1] - corner[1]);
            let length = dx.hypot(dz);
            let normal = if length == 0.0 {
                [-dz, dx]
            } else {
                [-dz / length, dx / length]
            };
            edges.push(Edge {
                begin: *corner,
                end: next,
                angle,
                normal,
                counter_clockwise: false,
                signed_distance: 0.0,
                offset_distance: 0.0,
            });
        }
    }
    edges
}

/// `ObstructionSnap.getValidEdges`.
fn valid_edges(all: &[Edge], position: Vector, max_side: f64) -> Vec<Edge> {
    let mut edges = Vec::new();
    for edge in all {
        let signed_distance = dot(edge.normal, position) - dot(edge.normal, edge.begin);
        // Negative: the position lies behind the edge.
        if signed_distance < -MINIMAL_DISTANCE_TO_SNAP - max_side
            || signed_distance > MINIMAL_DISTANCE_TO_SNAP + max_side
        {
            continue;
        }
        let along = [edge.begin[0] - edge.end[0], edge.begin[1] - edge.end[1]];
        let length = along[0].hypot(along[1]);
        let first = if length == 0.0 {
            along
        } else {
            [along[0] / length, along[1] / length]
        };
        let second = [-first[0], -first[1]];
        let mut offset_distance = (dot(first, position) - dot(first, edge.begin))
            .max(dot(second, position) - dot(second, edge.end));
        if offset_distance > MINIMAL_DISTANCE_TO_SNAP + max_side {
            continue;
        }
        // A projection that lies inside the edge needs no offset.
        if offset_distance < 0.0 {
            offset_distance = 0.0;
        }
        edges.push(Edge {
            signed_distance,
            offset_distance,
            ..*edge
        });
    }
    edges
}

fn padding(edge: &Edge) -> f64 {
    // Not for an edge whose normal points inside its entity, which the
    // snap approaches from its inner side.
    if edge.counter_clockwise {
        0.0
    } else {
        SNAP_PADDING
    }
}

/// `ObstructionSnap.compareEdges`: negative when `a` is the better base.
fn compare_edges(a: &Edge, b: &Edge) -> f64 {
    let behind_a = a.signed_distance < -EPS;
    let behind_b = b.signed_distance < -EPS;
    let score_a = a.signed_distance.abs() + a.offset_distance;
    let score_b = b.signed_distance.abs() + b.offset_distance;
    if (score_a - score_b).abs() < EPS {
        if behind_a != behind_b {
            return f64::from(u8::from(behind_a)) - f64::from(u8::from(behind_b));
        }
        if !behind_a {
            return a.offset_distance - b.offset_distance;
        }
        return -a.signed_distance - -b.signed_distance;
    }
    score_a - score_b
}

fn best_edge(edges: &[Edge]) -> Option<Edge> {
    let mut best = *edges.first()?;
    for edge in edges {
        if compare_edges(edge, &best) < 0.0 {
            best = *edge;
        }
    }
    Some(best)
}

/// `ObstructionSnap.getPosition` for a static obstruction of the given
/// size.
pub fn snap_to_edges(
    edges: &[Edge],
    obstruction: (f64, f64),
    x: f64,
    z: f64,
    angle: f64,
) -> Option<(f64, f64, f64)> {
    let (width, depth) = (obstruction.0 / 2.0, obstruction.1 / 2.0);
    let max_side = width.max(depth);
    let mut position = [x, z];
    let mut template_angle = angle;
    let near = valid_edges(edges, position, max_side);
    let base = best_edge(&near)?;
    // The first axis fixes the angle: the nearest of the four a right
    // angle apart.
    for step in 0..4 {
        let candidate = base.angle + f64::from(step) * PI / 2.0;
        let mut difference = (candidate - template_angle).abs();
        difference = difference.min(TAU - difference);
        if difference < PI / 4.0 + EPS {
            template_angle = candidate;
            break;
        }
    }
    // The side of the front face that meets the base edge.
    let direction = [template_angle.sin(), template_angle.cos()];
    let (to_base, to_paired) = if dot(direction, base.normal).abs() > (PI / 4.0).cos() {
        (depth, width)
    } else {
        (width, depth)
    };
    let distance = dot(base.normal, position) - dot(base.normal, base.begin);
    let shift = distance - to_base - padding(&base);
    position = [
        position[0] - base.normal[0] * shift,
        position[1] - base.normal[1] * shift,
    ];
    let edges = valid_edges(edges, position, max_side);
    if edges.len() > 1 {
        let mut paired = Vec::new();
        for edge in &edges {
            // A rectangle meets its edges at right angles.
            if dot(base.normal, edge.normal).abs() > EPS {
                continue;
            }
            paired.push(*edge);
            paired.push(Edge {
                begin: edge.end,
                end: edge.begin,
                normal: [-edge.normal[0], -edge.normal[1]],
                signed_distance: -edge.signed_distance,
                offset_distance: edge.offset_distance,
                counter_clockwise: true,
                ..*edge
            });
        }
        if let Some(second) = best_edge(&paired) {
            let distance = dot(second.normal, position) - dot(second.normal, second.begin);
            let shift = distance - to_paired - padding(&second);
            position = [
                position[0] - second.normal[0] * shift,
                position[1] - second.normal[1] * shift,
            ];
        }
    }
    Some((position[0], position[1], template_angle))
}

/// `GetDockAngle`: the direction of the most open water round a point.
pub fn dock_angle(ground: &Ground, footprint: Footprint, x: f64, z: f64) -> Option<f64> {
    let half_size = match footprint {
        Footprint::Square { width, depth } => width.max(depth) / 2.0,
        Footprint::Circle { radius } => radius,
        Footprint::None => 0.0,
    };
    const POINTS: usize = 16;
    let raw = |metres: f64| (metres * 65536.0).round() as i64;
    let in_water = |x: f64, z: f64| ground.at_fixed(raw(x), raw(z)) < i64::from(ground.water_level);
    for dist in 0..4 {
        let water: Vec<usize> = (0..POINTS)
            .filter(|&i| {
                let angle = i as f64 / POINTS as f64 * TAU;
                let d = half_size * f64::from(dist + 1);
                in_water(x - d * angle.sin(), z + d * angle.cos())
            })
            .collect();
        let length = water.len();
        if length == 0 {
            continue;
        }
        let consecutive: Vec<usize> = (0..length)
            .map(|i| {
                let mut count = 0;
                for j in 0..length - 1 {
                    if (water[(i + j) % length] + 1) % POINTS == water[(i + j + 1) % length] {
                        count += 1;
                    } else {
                        break;
                    }
                }
                count
            })
            .collect();
        let mut start = 0;
        let mut count = 0;
        for (index, run) in consecutive.iter().enumerate() {
            if *run > count {
                start = index;
                count = *run;
            }
        }
        // All round is no shoreline: look further out.
        if count != POINTS - 1 {
            let middle = (water[start] as f64 + consecutive[start] as f64 / 2.0) % POINTS as f64;
            return Some(-middle / POINTS as f64 * TAU);
        }
    }
    None
}

/// A tower or foundation a wall may start or end at.
#[derive(Clone, Copy)]
pub struct SnapTower {
    pub handle: u32,
    pub x: f64,
    pub z: f64,
    pub angle: f64,
    pub groups: [u32; 2],
    pub foundation: bool,
}

/// The first stage of `GetFoundationSnapData`: the tower whose centre is
/// the nearest within `radius`.
pub fn snap_to_entity(candidates: &[SnapTower], x: f64, z: f64, radius: f64) -> Option<SnapTower> {
    let radius2 = radius * radius;
    let mut best: Option<(f64, SnapTower)> = None;
    for tower in candidates {
        let distance2 = (x - tower.x).powi(2) + (z - tower.z).powi(2);
        if distance2 > radius2 {
            continue;
        }
        if best.is_none_or(|(least, _)| distance2 < least) {
            best = Some((distance2, *tower));
        }
    }
    best.map(|(_, tower)| tower)
}

/// The other stages for a building: with the snap to edges wanted, the
/// edges of nearby structures; else, for a dock, the water's direction.
pub fn foundation_snap(
    world: &World,
    data: &PlaceData,
    x: f64,
    z: f64,
    angle: f64,
    edges: bool,
) -> Option<(f64, f64, f64)> {
    if edges && let Some(obstruction) = data.obstruction {
        let near = edges_near(world.scene, x, z);
        if let Some(spot) = snap_to_edges(&near, obstruction, x, z, angle) {
            return Some(spot);
        }
    }
    if data.placement_type == "shore" {
        return dock_angle(world.ground, data.footprint, x, z).map(|angle| (x, z, angle));
    }
    None
}
