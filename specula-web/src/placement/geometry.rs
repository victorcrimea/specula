//! The engine's square maths a placement check needs, in f64 metres rather
//! than its fixed point, so an edge case may differ from the engine by a
//! rounding.

/// `ICmpObstructionManager::ObstructionSquare`: a centre, two unit axes and
/// the half sizes along them.
#[derive(Clone, Copy, Debug)]
pub struct Square {
    pub x: f64,
    pub z: f64,
    pub u: [f64; 2],
    pub v: [f64; 2],
    pub hw: f64,
    pub hh: f64,
}

impl Square {
    /// `CCmpObstructionManager::GetStaticShapeObstruction`.
    pub fn new(x: f64, z: f64, angle: f64, width: f64, depth: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Square {
            x,
            z,
            u: [c, -s],
            v: [s, c],
            hw: width / 2.0,
            hh: depth / 2.0,
        }
    }

    fn centre(&self) -> [f64; 2] {
        [self.x, self.z]
    }

    /// The corners in the order `StaticEdgesNear` walks them.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        let at = |a: f64, b: f64| {
            [
                self.x + self.u[0] * a + self.v[0] * b,
                self.z + self.u[1] * a + self.v[1] * b,
            ]
        };
        [
            at(-self.hw, -self.hh),
            at(-self.hw, self.hh),
            at(self.hw, self.hh),
            at(self.hw, -self.hh),
        ]
    }
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scaled(a: [f64; 2], factor: f64) -> [f64; 2] {
    [a[0] * factor, a[1] * factor]
}

/// `Geometry::PointIsInSquare`, the point relative to the square's centre.
pub fn point_is_in_square(point: [f64; 2], u: [f64; 2], v: [f64; 2], hw: f64, hh: f64) -> bool {
    dot(point, u).abs() <= hw && dot(point, v).abs() <= hh
}

/// `Geometry::GetHalfBoundingBox`.
pub fn half_bounding_box(u: [f64; 2], v: [f64; 2], hw: f64, hh: f64) -> [f64; 2] {
    [
        (u[0] * hw).abs() + (v[0] * hh).abs(),
        (u[1] * hw).abs() + (v[1] * hh).abs(),
    ]
}

/// `Geometry::DistanceToSquareSquared` with `countInsideAsZero` true, the
/// point relative to the square's centre.
pub fn distance_to_square_squared(point: [f64; 2], square: &Square) -> f64 {
    let du = dot(point, square.u).abs();
    let dv = dot(point, square.v).abs();
    if du < square.hw {
        if dv < square.hh {
            0.0
        } else {
            (dv - square.hh).powi(2)
        }
    } else if dv < square.hh {
        (du - square.hw).powi(2)
    } else {
        (du - square.hw).powi(2) + (dv - square.hh).powi(2)
    }
}

/// `SquareSAT`: whether some corner of the square (axes `u`, `v`, half
/// sizes) is on the inner side of `axis`'s edge through `a`.
fn square_sat(a: [f64; 2], axis: [f64; 2], u: [f64; 2], v: [f64; 2], hw: f64, hh: f64) -> bool {
    let perpendicular = [axis[1], -axis[0]];
    [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)]
        .into_iter()
        .any(|(su, sv)| {
            let corner = [
                u[0] * hw * su + v[0] * hh * sv - a[0],
                u[1] * hw * su + v[1] * hh * sv - a[1],
            ];
            dot(perpendicular, corner) <= 0.0
        })
}

/// `Geometry::TestSquareSquare`.
pub fn test_square_square(first: &Square, second: &Square) -> bool {
    let corner = |square: &Square, sign: f64| {
        [
            square.x + sign * (square.u[0] * square.hw + square.v[0] * square.hh),
            square.z + sign * (square.u[1] * square.hw + square.v[1] * square.hh),
        ]
    };
    let neg = |a: [f64; 2]| [-a[0], -a[1]];
    let (c0, c1) = (first.centre(), second.centre());
    let (a0, b0) = (corner(first, 1.0), corner(first, -1.0));
    let (a1, b1) = (corner(second, 1.0), corner(second, -1.0));
    let against = |a: [f64; 2], axis: [f64; 2], square: &Square| {
        square_sat(a, axis, square.u, square.v, square.hw, square.hh)
    };
    against(sub(a0, c1), neg(first.u), second)
        && against(sub(a0, c1), first.v, second)
        && against(sub(b0, c1), first.u, second)
        && against(sub(b0, c1), neg(first.v), second)
        && against(sub(a1, c0), neg(second.u), first)
        && against(sub(a1, c0), second.v, first)
        && against(sub(b1, c0), second.u, first)
        && against(sub(b1, c0), neg(second.v), first)
}

/// `Geometry::DistanceToSegment`.
pub fn distance_to_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let direction = sub(b, a);
    let point_dot = dot(direction, point);
    if point_dot <= dot(direction, a) {
        return length(sub(point, a));
    }
    if point_dot >= dot(direction, b) {
        return length(sub(point, b));
    }
    let perpendicular = [direction[1], -direction[0]];
    let magnitude = length(perpendicular);
    let normal = if magnitude == 0.0 {
        perpendicular
    } else {
        scaled(perpendicular, 1.0 / magnitude)
    };
    (dot(normal, a) - dot(normal, point)).abs()
}

pub fn length(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

/// A row of navcells `[i0, i1)` at row `j`.
pub type Span = (i32, i32, i32);

/// `SimRasterize::RasterizeRectWithClearance` for a navcell size of 1 m,
/// the `(j, i0, i1)` spans.
pub fn rasterize_rect_with_clearance(shape: &Square, clearance: f64) -> Vec<Span> {
    // `Pathfinding::CLEARANCE_EXTENSION_RADIUS`.
    let raster_clearance = clearance + 1.0;
    let half_bound = half_bounding_box(
        shape.u,
        shape.v,
        shape.hw + raster_clearance,
        shape.hh + raster_clearance,
    );
    let i0 = (shape.x - half_bound[0]).floor() as i32;
    let j0 = (shape.z - half_bound[1]).floor() as i32;
    let i1 = (shape.x + half_bound[0]).ceil() as i32;
    let j1 = (shape.z + half_bound[1]).ceil() as i32;
    let mut spans = Vec::new();
    if j1 <= j0 {
        return spans;
    }
    let limit = raster_clearance * raster_clearance;
    let outside = |x: i32, z: i32| {
        distance_to_square_squared([f64::from(x) - shape.x, f64::from(z) - shape.z], shape) > limit
    };
    for j in j0..j1 {
        let mut span_i0 = i32::MAX;
        let mut span_i1 = i32::MIN;
        let mut previous_inside = false;
        let mut skip_next = false;
        for i in i0..i1 {
            if skip_next {
                skip_next = false;
                continue;
            }
            if outside(i + 1, j) || outside(i + 1, j + 1) {
                if previous_inside {
                    break;
                }
                skip_next = true;
                continue;
            }
            if !previous_inside {
                if outside(i, j) || outside(i, j + 1) {
                    continue;
                }
                previous_inside = true;
                span_i0 = i;
            }
            span_i1 = i + 1;
        }
        if span_i0 < span_i1 {
            spans.push((j, span_i0, span_i1));
        }
    }
    spans
}
