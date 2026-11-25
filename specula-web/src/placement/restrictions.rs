//! `BuildRestrictions.CheckPlacement` and what it calls, run on the seat's
//! own view of the world: the entities, terrain, territory and vision its
//! stream holds.

use std::f64::consts::TAU;

use super::data::PlaceData;
use super::geometry::Square;
use super::geometry::half_bounding_box;
use super::geometry::point_is_in_square;
use super::geometry::rasterize_rect_with_clearance;
use super::geometry::test_square_square;
use super::passability::BUILDING_LAND;
use super::passability::BUILDING_SHORE;
use super::passability::DEFAULT_TERRAIN_ONLY;
use super::passability::PassClass;
use super::passability::ShoreGrid;
use super::passability::navcell_passable;
use crate::lang;
use crate::panel::mutual_allies;
use crate::pick;
use crate::stream::Dot;
use crate::stream::Ground;
use crate::stream::LOS_EXPLORED;
use crate::stream::LOS_VISIBLE;
use crate::stream::OBSTRUCTION_ACTIVE;
use crate::stream::OBSTRUCTION_BLOCK_FOUNDATION;
use crate::stream::Obstruction;
use crate::stream::ObstructionKind;
use crate::stream::PLACEMENT_BUILD_RESTRICTIONS;
use crate::stream::Scene;
use crate::stream::TERRITORY_UNKNOWN;
use crate::territory;

/// A preview's control groups: 0 is its own, which nothing else is in; any
/// other is a token a wall piece of the same seat's stream carries.
pub type Groups = [u32; 2];

/// What a placement check looks at.
pub struct World<'a> {
    pub scene: &'a Scene,
    pub seat: i32,
    pub ground: &'a Ground,
    pub shore: &'a ShoreGrid,
}

pub struct Candidate<'a> {
    pub data: &'a PlaceData,
    pub x: f64,
    pub z: f64,
    pub angle: f64,
    pub groups: Groups,
}

/// A value of a refusal's message, which `to_markup` fills in.
enum Parameter {
    /// A string the engine translates.
    Translated(String),
    Number(f64),
    /// `{context, _string}`.
    Context(&'static str, String),
    /// `{context, list}`, joined as the engine's enumeration.
    List(&'static str, Vec<String>),
}

/// Why `check_placement` refused, as the engine's result says it.
pub struct Refusal {
    message: &'static str,
    /// The plural message and the number it is chosen by.
    plural: Option<(&'static str, f64)>,
    parameters: Vec<(&'static str, Parameter)>,
}

impl Refusal {
    fn new(message: &'static str, data: &PlaceData) -> Self {
        Refusal {
            message,
            plural: None,
            parameters: vec![("name", Parameter::Translated(data.generic_name.clone()))],
        }
    }

    /// What `CheckPlacement` answers without a terrain to check against.
    pub fn unknown(data: &PlaceData) -> Self {
        Refusal::new(UNKNOWN, data)
    }

    /// `updateBuildingPlacementPreview`'s tooltip text.
    pub fn to_markup(&self) -> String {
        let message = match self.plural {
            Some((plural, count)) => lang::translate_plural(self.message, plural, count),
            None => lang::translate(self.message),
        };
        let values: Vec<(&str, String)> = self
            .parameters
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    Parameter::Translated(text) => lang::translate(text),
                    Parameter::Number(number) => number.to_string(),
                    Parameter::Context(context, text) => lang::translate_context(context, text),
                    Parameter::List(context, items) => items
                        .iter()
                        .map(|item| lang::translate_context(context, item))
                        .collect::<Vec<_>>()
                        .join(&lang::translate_context("enumeration", ", ")),
                };
                (*key, text)
            })
            .collect();
        let pairs: Vec<(&str, &str)> = values
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        lang::format(&message, &pairs)
    }
}

/// `markForTranslation` strings of BuildRestrictions.js.
const UNKNOWN: &str = "%(name)s cannot be built due to unknown error";
const UNEXPLORED: &str = "%(name)s cannot be built in unexplored area";
const OBSTRUCTED: &str = "%(name)s cannot be built on another building or resource";
const TERRAIN: &str = "%(name)s cannot be built on invalid terrain";
const TERRITORY: &str = "%(name)s cannot be built in %(territoryType)s territory. Valid territories: %(validTerritories)s";
const SHORE: &str = "%(name)s must be built on a valid shoreline";
const TOO_CLOSE: &str =
    "%(name)s too close to a %(category)s, must be at least %(distance)s meter away";
const TOO_CLOSE_PLURAL: &str =
    "%(name)s too close to a %(category)s, must be at least %(distance)s meters away";
const TOO_FAR: &str = "%(name)s too far from a %(category)s, must be within %(distance)s meter";
const TOO_FAR_PLURAL: &str =
    "%(name)s too far from a %(category)s, must be within %(distance)s meters";

/// `CheckPlacement`, in its order.
pub fn check_placement(world: &World, candidate: &Candidate) -> Result<(), Refusal> {
    let data = candidate.data;
    // The LOS vertex nearest the position, as `ComputeLosVisibility`
    // reads it.
    if !explored(world, candidate.x, candidate.z) {
        return Err(Refusal::new(UNEXPLORED, data));
    }
    let Some((width, depth)) = data.obstruction else {
        return Err(Refusal::new(UNKNOWN, data));
    };
    let class = match data.placement_type.as_str() {
        "shore" => &BUILDING_SHORE,
        "land-shore" => &DEFAULT_TERRAIN_ONLY,
        _ => &BUILDING_LAND,
    };
    let square = Square::new(candidate.x, candidate.z, candidate.angle, width, depth);
    check_foundation(world, candidate, &square, class)?;

    check_territory(world, candidate)?;

    if data.placement_type == "shore" && !shore_placement(world, &square) {
        return Err(Refusal::new(SHORE, data));
    }
    check_distance(world, candidate)
}

/// Whether the seat has explored the spot; a stream with no vision of the
/// seat (an observer's, or an old one) has explored all.
pub(super) fn explored(world: &World, x: f64, z: f64) -> bool {
    let los = &world.scene.los;
    let Some(grid) = usize::try_from(world.seat)
        .ok()
        .and_then(|seat| los.players.get(&(seat as u32)))
        .filter(|_| los.side > 0)
    else {
        return true;
    };
    // `ToInt_RoundToNearest` of the position over `LOS_TILE_SIZE`.
    let i = (x / 4.0 + 0.5).floor() as i64;
    let j = (z / 4.0 + 0.5).floor() as i64;
    let side = i64::from(los.side);
    if i < 0 || j < 0 || i >= side || j >= side {
        return false;
    }
    grid.get((j * side + i) as usize)
        .is_some_and(|state| state & (LOS_EXPLORED | LOS_VISIBLE) != 0)
}

/// `CCmpObstruction::CheckFoundation` through
/// `CCmpPathfinder::CheckBuildingPlacement`.
fn check_foundation(
    world: &World,
    candidate: &Candidate,
    square: &Square,
    class: &PassClass,
) -> Result<(), Refusal> {
    let data = candidate.data;
    if obstructed(world, square, candidate.groups, Mode::Skip) {
        return Err(Refusal::new(OBSTRUCTED, data));
    }
    // The terrain-only grid is a navcell per metre, with no clearance for
    // a building class.
    let size = world.ground.tiles() * 4;
    for (j, i0, i1) in rasterize_rect_with_clearance(square, 0.0) {
        let (j, i0, i1) = (i64::from(j), i64::from(i0), i64::from(i1));
        if i0 < 0 || i1 > size || j < 0 || j > size {
            return Err(Refusal::new(TERRAIN, data));
        }
        // A span on the last row is past the grid, where the engine reads
        // nothing; the border there is closed anyway.
        if j >= size {
            return Err(Refusal::new(TERRAIN, data));
        }
        for i in i0..i1 {
            if !navcell_passable(world.ground, world.shore, class, i, j) {
                return Err(Refusal::new(TERRAIN, data));
            }
        }
    }
    Ok(())
}

/// How the candidate's control groups select the shapes it is tested
/// against.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// `SkipControlGroupsRequireFlagObstructionFilter`: shapes sharing a
    /// group do not count.
    Skip,
    /// `SkipTagRequireControlGroupsAndFlagObstructionFilter`: only shapes
    /// sharing both do.
    Both,
}

/// A control group as the stream's ids give it: a wall piece's is a token,
/// any other entity's its own id, which no token can equal.
const TOKEN: u64 = 1 << 32;

/// An entity's primary and secondary control groups, 0 for none.
fn shape_groups(id: u32, dot: &Dot) -> (u64, u64) {
    if dot.groups[0] != 0 {
        let second = if dot.groups[1] != 0 {
            TOKEN | u64::from(dot.groups[1])
        } else {
            0
        };
        (TOKEN | u64::from(dot.groups[0]), second)
    } else {
        (u64::from(id), 0)
    }
}

/// The pose the stream holds an entity at: where the engine has it for a
/// static obstruction, else the quantised one.
pub fn pose_of(dot: &Dot) -> (f64, f64, f64) {
    match dot.exact {
        Some(pose) => (
            f64::from(pose.x) / 65536.0,
            f64::from(pose.z) / 65536.0,
            f64::from(pose.angle) / 65536.0,
        ),
        None => (
            f64::from(dot.x) / 16.0,
            f64::from(dot.z) / 16.0,
            f64::from(dot.angle) / 256.0 * TAU,
        ),
    }
}

/// What obstructs foundations, among the entities the scene holds: the
/// obstruction, the pose and the groups of each.
pub fn foundation_shapes<'a>(
    scene: &'a Scene,
) -> impl Iterator<Item = (u32, &'a Dot, &'a Obstruction)> + 'a {
    scene.dots.iter().filter_map(|(id, dot)| {
        let obstruction = scene
            .template_details
            .get(dot.template as usize)?
            .as_ref()?
            .obstruction
            .as_ref()?;
        if obstruction.flags & OBSTRUCTION_ACTIVE == 0
            || obstruction.flags & OBSTRUCTION_BLOCK_FOUNDATION == 0
        {
            return None;
        }
        if is_local(scene, dot) {
            return None;
        }
        Some((*id, dot, obstruction))
    })
}

/// Whether the dot is one of the visual or local entities, which have no
/// obstruction of their own and which the range manager never sees: a
/// destroyed foundation's corpse keeps its BuildRestrictions and owner,
/// yet must not count for the distance rules.
fn is_local(scene: &Scene, dot: &Dot) -> bool {
    scene
        .templates
        .get(dot.template as usize)
        .is_some_and(|name| {
            ["construction|", "preview|", "corpse|", "decay|"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
}

/// `TestStaticShape` against the stream's entities.
fn obstructed(world: &World, square: &Square, groups: Groups, mode: Mode) -> bool {
    let size = (world.ground.tiles() * 4) as f64;
    // Every corner must be in the world, which the engine reports as an
    // obstruction.
    let corner1 = [
        square.u[0] * square.hw + square.v[0] * square.hh,
        square.u[1] * square.hw + square.v[1] * square.hh,
    ];
    let corner2 = [
        square.u[0] * square.hw - square.v[0] * square.hh,
        square.u[1] * square.hw - square.v[1] * square.hh,
    ];
    let outside = [
        corner1,
        corner2,
        [-corner1[0], -corner1[1]],
        [-corner2[0], -corner2[1]],
    ]
    .iter()
    .any(|corner| {
        let (x, z) = (square.x + corner[0], square.z + corner[1]);
        x < 0.0 || x > size || z < 0.0 || z > size
    });
    if outside {
        return true;
    }
    let half_box = half_bounding_box(square.u, square.v, square.hw, square.hh);
    let wanted = |id: u32, dot: &Dot| {
        let (group, group2) = shape_groups(id, dot);
        let ours = |group: u64| {
            groups
                .iter()
                .filter(|own| **own != 0)
                .any(|own| group == TOKEN | u64::from(*own))
        };
        match mode {
            // The candidate's second group is its first when it has no
            // second.
            Mode::Skip => !(ours(group) || (group2 != 0 && ours(group2))),
            Mode::Both => {
                groups[0] != 0
                    && groups[1] != 0
                    && ((group == TOKEN | u64::from(groups[0])
                        && group2 == TOKEN | u64::from(groups[1]))
                        || (group2 == TOKEN | u64::from(groups[0])
                            && group == TOKEN | u64::from(groups[1])))
            }
        }
    };
    foundation_shapes(world.scene).any(|(id, dot, obstruction)| {
        let (x, z, angle) = pose_of(dot);
        // Beyond any shape of it a box test could touch.
        let reach =
            obstruction.width.hypot(obstruction.depth) / 2.0 + half_box[0].hypot(half_box[1]) + 1.0;
        if (x - square.x).abs() > reach || (z - square.z).abs() > reach {
            return false;
        }
        if !wanted(id, dot) {
            return false;
        }
        shapes_of(obstruction, x, z, angle)
            .into_iter()
            .any(|shape| match shape {
                Shape::Square(other) => test_square_square(square, &other),
                Shape::Point(point, clearance) => point_is_in_square(
                    [point[0] - square.x, point[1] - square.z],
                    square.u,
                    square.v,
                    square.hw + clearance,
                    square.hh + clearance,
                ),
            })
    })
}

enum Shape {
    Square(Square),
    /// A unit's centre and clearance.
    Point([f64; 2], f64),
}

/// The shapes the engine registers for an entity: a cluster's bounding
/// box and each of its parts (`AddClusterShapes`).
fn shapes_of(obstruction: &Obstruction, x: f64, z: f64, angle: f64) -> Vec<Shape> {
    match obstruction.kind {
        ObstructionKind::Static => vec![Shape::Square(Square::new(
            x,
            z,
            angle,
            obstruction.width,
            obstruction.depth,
        ))],
        ObstructionKind::Unit => vec![Shape::Point([x, z], obstruction.width)],
        ObstructionKind::Cluster => {
            let (s, c) = angle.sin_cos();
            let mut shapes = vec![Shape::Square(Square::new(
                x,
                z,
                angle,
                obstruction.width,
                obstruction.depth,
            ))];
            for part in &obstruction.clusters[..usize::from(obstruction.shapes)] {
                shapes.push(Shape::Square(Square::new(
                    x + part[0] * c + part[1] * s,
                    z + part[1] * c - part[0] * s,
                    angle,
                    part[2],
                    part[3],
                )));
            }
            shapes
        }
    }
}

/// `CCmpObstruction::CheckDuplicateFoundation`: whether no foundation
/// blocking shape already has both of the candidate's groups.
pub fn check_duplicate_foundation(world: &World, candidate: &Candidate) -> bool {
    let Some((width, depth)) = candidate.data.obstruction else {
        return false;
    };
    let square = Square::new(candidate.x, candidate.z, candidate.angle, width, depth);
    !obstructed(world, &square, candidate.groups, Mode::Both)
}

/// The territory rules of `CheckPlacement`.
fn check_territory(world: &World, candidate: &Candidate) -> Result<(), Refusal> {
    let data = candidate.data;
    let territory = &world.scene.territory;
    let tile = if territory.width == 0 || territory.height == 0 {
        0
    } else {
        // `NearestTerritoryTile`.
        let i = ((candidate.x / f64::from(territory::TILE_SIZE)).floor() as i64)
            .clamp(0, i64::from(territory.width) - 1);
        let j = ((candidate.z / f64::from(territory::TILE_SIZE)).floor() as i64)
            .clamp(0, i64::from(territory.height) - 1);
        territory.tiles[(j * i64::from(territory.width) + i) as usize]
    };
    // A tile the seat has not been shown is neutral: the check on the
    // vision has refused any spot there already.
    let tile = if tile & TERRITORY_UNKNOWN != 0 {
        0
    } else {
        tile
    };
    let owner = i32::from(tile & territory::OWNER_MASK);
    let connected = tile & 0x40 == 0;
    let has = |kind: &str| data.territory.iter().any(|token| token == kind);
    let own = owner == world.seat;
    let ally = !own
        && usize::try_from(world.seat)
            .is_ok_and(|seat| mutual_allies(world.scene, seat, owner as usize));
    let invalid = if own {
        if !has("own") {
            Some("own")
        } else if !connected && !has("neutral") {
            Some("unconnected own")
        } else {
            None
        }
    } else if ally {
        if !has("ally") {
            Some("allied")
        } else if !connected && !has("neutral") {
            Some("unconnected allied")
        } else {
            None
        }
    } else if owner == 0 {
        (!has("neutral")).then_some("neutral")
    } else {
        (!has("enemy")).then_some("enemy")
    };
    let Some(kind) = invalid else {
        return Ok(());
    };
    let mut refusal = Refusal::new(TERRITORY, data);
    refusal.parameters.push((
        "territoryType",
        Parameter::Context("Territory type", kind.to_string()),
    ));
    refusal.parameters.push((
        "validTerritories",
        Parameter::List("Territory type list", data.territory.clone()),
    ));
    Err(refusal)
}

/// `CCmpObstruction::CheckShorePlacement`: the ground at the front under
/// more than a metre of water and at the back under less than two.
fn shore_placement(world: &World, square: &Square) -> bool {
    let depth_at = |x: f64, z: f64| {
        let raw = |metres: f64| (metres * 65536.0).round() as i64;
        i64::from(world.ground.water_level) - world.ground.at_fixed(raw(x), raw(z))
    };
    let front = depth_at(
        square.x + square.v[0] * square.hh,
        square.z + square.v[1] * square.hh,
    );
    let back = depth_at(
        square.x - square.v[0] * square.hh,
        square.z - square.v[1] * square.hh,
    );
    front > 65536 && back < 2 * 65536
}

/// The distance rules, which read the seat's own entities with a
/// BuildRestrictions component, within range of the spot.
fn check_distance(world: &World, candidate: &Candidate) -> Result<(), Refusal> {
    let data = candidate.data;
    let Some(distance) = &data.distance else {
        return Ok(());
    };
    let near = |range: f64| {
        world.scene.dots.values().any(|dot| {
            if dot.owner != world.seat || dot.mirage != 0 || is_local(world.scene, dot) {
                return false;
            }
            let Some(details) = world
                .scene
                .template_details
                .get(dot.template as usize)
                .copied()
                .flatten()
            else {
                return false;
            };
            if details.placement_flags & PLACEMENT_BUILD_RESTRICTIONS == 0
                || !pick::has_class(&details, &world.scene.strings, &distance.from_class)
            {
                return false;
            }
            let (x, z, _) = pose_of(dot);
            // `CompareLength(range) > 0` skips what is further.
            (x - candidate.x).hypot(z - candidate.z) <= range
        })
    };
    let refusal = |message, plural, range: f64| {
        let mut refusal = Refusal::new(message, data);
        refusal.plural = Some((plural, range));
        refusal.parameters.push((
            "category",
            Parameter::Translated(distance.from_class.clone()),
        ));
        refusal
            .parameters
            .push(("distance", Parameter::Number(range)));
        refusal
    };
    if let Some(min) = distance.min
        && near(min)
    {
        return Err(refusal(TOO_CLOSE, TOO_CLOSE_PLURAL, min));
    }
    if let Some(max) = distance.max
        && !near(max)
    {
        return Err(refusal(TOO_FAR, TOO_FAR_PLURAL, max));
    }
    Ok(())
}
