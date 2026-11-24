//! A wall between two points: which pieces, where, and which of them the
//! seat may build.

use specula_assets::command::WallPiece;

use super::data::PieceData;
use super::data::PlaceData;
use super::data::WallSetData;
use super::restrictions::Candidate;
use super::restrictions::World;
use super::restrictions::check_duplicate_foundation;
use super::restrictions::check_placement;
use super::restrictions::explored;
use super::snap::SnapTower;
use super::snap::snap_to_entity;

/// The kinds of piece `GetWallPlacement` picks segments from, longest
/// first.
fn segments(set: &WallSetData) -> Vec<&PieceData> {
    [&set.long, &set.medium, &set.short]
        .into_iter()
        .filter_map(Option::as_ref)
        .collect()
}

/// `GetWallSegmentsRec`: fills `chosen` with indices into the candidates
/// and returns the remaining distance, or `None` when no run of segments
/// fits.
fn segments_rec(
    d: f64,
    candidates: &[&PieceData],
    set: &WallSetData,
    t: f64,
    so_far: f64,
    chosen: &mut Vec<usize>,
) -> Option<f64> {
    for (index, candidate) in candidates.iter().enumerate() {
        chosen.push(index);
        let new_so_far = so_far + candidate.data.wall_length;
        let r = d - new_so_far;
        let count = chosen.len() as f64;
        let lower = (1.0 - 2.0 * set.max_overlap) * count * t;
        let upper = (1.0 - 2.0 * set.min_overlap) * count * t;
        if r < lower {
            chosen.pop();
            continue;
        }
        if r > upper {
            match segments_rec(d, candidates, set, t, new_so_far, chosen) {
                Some(r) => return Some(r),
                None => {
                    chosen.pop();
                    continue;
                }
            }
        }
        return Some(r);
    }
    None
}

/// `GetWallPlacement`: the pieces between two points, with towers between
/// the segments but not at the ends, which the caller adds.
fn wall_placement(
    set: &WallSetData,
    start: [f64; 2],
    end: [f64; 2],
) -> Vec<(PieceKind, f64, f64, f64)> {
    let candidates = segments(set);
    let tower_width = set.tower.data.wall_length;
    let dir = [end[0] - start[0], end[1] - start[1]];
    let length = dir[0].hypot(dir[1]);
    // Room is needed for the starting and ending towers side by side.
    if length <= tower_width || candidates.is_empty() {
        return Vec::new();
    }
    let mut chosen = Vec::new();
    let Some(r) = segments_rec(length, &candidates, set, tower_width, 0.0, &mut chosen) else {
        return Vec::new();
    };
    let spacing = r / (2.0 * chosen.len() as f64);
    let normalized = [dir[0] / length, dir[1] / length];
    let angle = -dir[1].atan2(dir[0]);
    let mut progress = 0.0;
    let mut result = Vec::new();
    for (i, index) in chosen.iter().enumerate() {
        let len = candidates[*index].data.wall_length;
        let at = |distance: f64| {
            (
                start[0] + distance * normalized[0],
                start[1] + distance * normalized[1],
            )
        };
        let (x, z) = at(progress + spacing + len / 2.0);
        result.push((PieceKind::Segment(*index), x, z, angle));
        if i < chosen.len() - 1 {
            let (x, z) = at(progress + len + 2.0 * spacing);
            result.push((PieceKind::Tower, x, z, angle));
        }
        progress += len + 2.0 * spacing;
    }
    result
}

#[derive(Clone, Copy)]
enum PieceKind {
    Tower,
    /// An index into `segments`.
    Segment(usize),
}

/// A preview piece before the checks.
struct Preview {
    kind: PieceKind,
    x: f64,
    z: f64,
    angle: f64,
    /// The control groups it takes from what it snaps to, 0 to 2.
    groups: Vec<u32>,
    /// Drawn but not part of the order: the tower that completes a
    /// snapped foundation.
    exclude: bool,
}

/// One piece of the preview and what the checks found of it.
pub struct Laid {
    pub piece: WallPiece,
    pub actor: String,
    /// Drawn only, as `excludeFromResult`.
    pub exclude: bool,
    /// Every piece up to it is valid, so it is part of what is built.
    pub valid: bool,
    /// The cost of the pieces so far, by resource.
    pub cost: Vec<(String, f64)>,
}

/// What `SetWallPlacementPreview` returns, with the pieces it drew but
/// does not return.
pub struct WallLayout {
    pub pieces: Vec<Laid>,
    /// The snapped entities' ids, 0 for none.
    pub start_snapped: u32,
    pub end_snapped: u32,
}

impl WallLayout {
    /// The pieces to build, `result.pieces`.
    pub fn result(&self) -> impl Iterator<Item = &Laid> {
        self.pieces
            .iter()
            .filter(|laid| laid.valid && !laid.exclude)
    }
}

/// What a wall's end snapped to.
struct Snapped {
    position: [f64; 2],
    tower: Option<SnapTower>,
}

fn snap_end(snap: &[SnapTower], radius: f64, at: [f64; 2]) -> Snapped {
    match snap_to_entity(snap, at[0], at[1], radius) {
        Some(tower) => Snapped {
            position: [tower.x, tower.z],
            tower: Some(tower),
        },
        None => Snapped {
            position: at,
            tower: None,
        },
    }
}

/// `SetWallPlacementPreview`. `None` where the engine answers false: some
/// piece is needed, and none can be built.
pub fn wall_preview(
    world: &World,
    set: &WallSetData,
    start: [f64; 2],
    end: Option<[f64; 2]>,
    snap: &[SnapTower],
    last_angle: &mut f64,
) -> Option<WallLayout> {
    // The same point twice would divide by zero further on.
    let end = end.filter(|end| *end != start);
    // Half a tower, which the engine found by trial.
    let radius = set.tower.data.wall_length * 0.5;
    let start = snap_end(snap, radius, start);
    let end = end.map(|end| snap_end(snap, radius, end));

    let segments = segments(set);
    let mut previews: Vec<Preview> = end
        .as_ref()
        .map(|end| wall_placement(set, start.position, end.position))
        .unwrap_or_default()
        .into_iter()
        .map(|(kind, x, z, angle)| Preview {
            kind,
            x,
            z,
            angle,
            groups: Vec::new(),
            exclude: false,
        })
        .collect();

    // A piece that joins a tower or foundation shares its control group,
    // or it would be found to obstruct it. A snapped one that carries none
    // (a stream of an older server) cannot be, and obstructs.
    let group_of = |tower: &SnapTower| (tower.groups[0] != 0).then_some(tower.groups[0]);
    match &start.tower {
        Some(tower) => {
            let group = group_of(tower);
            if let (Some(first), Some(group)) = (previews.first_mut(), group) {
                first.groups = vec![group];
            }
            if tower.foundation {
                previews.insert(
                    0,
                    Preview {
                        kind: PieceKind::Tower,
                        x: start.position[0],
                        z: start.position[1],
                        angle: tower.angle,
                        groups: group.into_iter().collect(),
                        exclude: true,
                    },
                );
            }
        }
        None => {
            // The angle last seen on a valid piece, so that a queued wall
            // does not turn to 0 and back.
            let angle = previews.first().map_or(*last_angle, |first| first.angle);
            previews.insert(
                0,
                Preview {
                    kind: PieceKind::Tower,
                    x: start.position[0],
                    z: start.position[1],
                    angle,
                    groups: Vec::new(),
                    exclude: false,
                },
            );
        }
    }
    if let Some(end) = &end {
        match &end.tower {
            Some(tower) => {
                let group = group_of(tower);
                if let (Some(last), Some(group)) = (previews.last_mut(), group) {
                    last.groups.push(group);
                }
                if tower.foundation {
                    previews.push(Preview {
                        kind: PieceKind::Tower,
                        x: end.position[0],
                        z: end.position[1],
                        angle: tower.angle,
                        groups: group.into_iter().collect(),
                        exclude: true,
                    });
                }
            }
            None => {
                let angle = previews.last().map_or(*last_angle, |last| last.angle);
                previews.push(Preview {
                    kind: PieceKind::Tower,
                    x: end.position[0],
                    z: end.position[1],
                    angle,
                    groups: Vec::new(),
                    exclude: false,
                });
            }
        }
    }

    let mut all_valid = true;
    let mut required = 0;
    let mut cost: Vec<(String, f64)> = Vec::new();
    let mut pieces = Vec::new();
    for preview in &previews {
        let piece: &PieceData = match preview.kind {
            PieceKind::Tower => &set.tower,
            PieceKind::Segment(index) => segments[index],
        };
        // More than two groups is a programming error of the engine's.
        if preview.groups.len() > 2 {
            break;
        }
        let mut groups = [0; 2];
        for (slot, group) in groups.iter_mut().zip(&preview.groups) {
            *slot = *group;
        }
        let candidate = Candidate {
            data: &piece.data,
            x: preview.x,
            z: preview.z,
            angle: preview.angle,
            groups,
        };
        let mut valid =
            explored(world, preview.x, preview.z) && check_placement(world, &candidate).is_ok();
        // A piece between two towers must not duplicate one already there.
        if valid && preview.groups.len() > 1 {
            valid = check_duplicate_foundation(world, &candidate);
        }
        all_valid = all_valid && valid;
        if !preview.exclude {
            required += 1;
        }
        if all_valid && !preview.exclude {
            *last_angle = preview.angle;
            add_cost(&mut cost, &piece.data);
        }
        pieces.push(Laid {
            piece: WallPiece {
                template: piece.template.clone(),
                x: preview.x as f32,
                z: preview.z as f32,
                angle: preview.angle as f32,
            },
            actor: piece.actor.clone(),
            exclude: preview.exclude,
            valid: all_valid,
            cost: cost.clone(),
        });
    }
    let layout = WallLayout {
        pieces,
        // Only a start the pieces from it can be built at.
        start_snapped: start.tower.map_or(0, |tower| tower.handle),
        end_snapped: end
            .and_then(|end| end.tower)
            .filter(|_| all_valid)
            .map_or(0, |tower| tower.handle),
    };
    if required > 0 && layout.result().next().is_none() {
        return None;
    }
    Some(layout)
}

fn add_cost(total: &mut Vec<(String, f64)>, piece: &PlaceData) {
    for (resource, amount) in &piece.cost {
        match total.iter_mut().find(|(known, _)| known == resource) {
            Some((_, sum)) => *sum += amount,
            None => total.push((resource.clone(), *amount)),
        }
    }
}
