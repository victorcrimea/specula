//! Placing a structure on `/play`, as `gui/session/input.js` does it: a
//! building follows the mouse, a press anchors it and a drag turns it
//! (INPUT_BUILDING_PLACEMENT, _CLICK and _DRAG), a release places it, and
//! Shift keeps placing; a wall goes from a first click to a second
//! (INPUT_BUILDING_WALL_CLICK and _PATHING). The right button and Escape
//! cancel.
//!
//! Whether a spot is valid, the snap to the shore and to nearby edges, and
//! the wall's layout are worked out here, on the player's own view (what
//! the seat's stream holds, mirages included), as the engine's
//! `BuildRestrictions.CheckPlacement` and `SetWallPlacementPreview` do on
//! the whole simulation (CONTROL-IMPL). The engine judges the command again
//! when it runs it.

mod data;
mod geometry;
mod passability;
mod restrictions;
mod snap;
mod walls;

use specula_assets::command::Command;
use specula_assets::command::QUEUED;
use specula_assets::command::WallPiece;
use web_sys::HtmlElement;

use self::passability::ShoreGrid;
use self::restrictions::Candidate;
use self::restrictions::Refusal;
use self::restrictions::World;
use self::restrictions::check_placement;
use self::restrictions::pose_of;
use self::snap::SnapTower;
use self::snap::foundation_snap;
use self::walls::WallLayout;
use self::walls::wall_preview;
use crate::View;
use crate::field::DRAG_DELTA;
use crate::panel::Place;
use crate::panel::mutual_allies;
use crate::stream::Scene;
use crate::tooltip;
use crate::uplink::Uplink;

pub use self::data::PlaceData;

/// `PlacementSupport.DEFAULT_ANGLE`.
const DEFAULT_ANGLE: f32 = std::f32::consts::PI * 3.0 / 4.0;
const TOOLTIP_STYLE: &str = "sessionToolTipInstantly";

/// The mouse over the ground while placing: the point under it, x and z in
/// metres (none off the map), in client pixels, and the modifiers held.
#[derive(Clone, Copy)]
pub struct Pointer {
    pub ground: Option<[f32; 2]>,
    pub client: [f32; 2],
    /// session.snaptoedges.
    pub ctrl: bool,
    /// session.queue.
    pub shift: bool,
}

/// What the left and right buttons and Escape did while placing.
pub enum Input {
    Press(Pointer),
    Release(Pointer),
    Cancel,
}

/// A preview to draw: the actor at a point of the ground, turned by
/// `angle` (radians, as the engine's rotation Y), in the owner's colour,
/// red where the engine refuses it.
#[derive(Clone)]
pub struct Ghost {
    pub actor: String,
    pub x: f32,
    pub z: f32,
    pub angle: f32,
    pub seed: u32,
    pub owner: i32,
    pub invalid: bool,
    /// The preview's attack range circles.
    pub ranges: Vec<GhostRange>,
    /// The players whose foundations show their future attack range with
    /// this ghost: its owner and the owner's mutual allies. Empty when the
    /// ghost has no ranges.
    pub range_allies: Vec<i32>,
}

/// One circle of the preview's `Attack.GetRangeOverlays()`.
#[derive(Clone)]
pub struct GhostRange {
    pub radius: f32,
    /// Relative to art/textures/selection/.
    pub texture: String,
    pub thickness: f32,
}

#[derive(Default)]
pub struct Placement {
    active: Option<Active>,
    /// The canvas, which shows the refusal as its tooltip.
    canvas: Option<HtmlElement>,
    /// The terrain's distances from the shore, for the checks.
    shore: Option<ShoreGrid>,
}

enum Active {
    Building(Building),
    Wall(Wall),
}

struct Building {
    place: Place,
    seed: u32,
    stage: Stage,
    /// Kept between moves: the last snap's or drag's.
    angle: f32,
    /// The point and angle the mouse wants.
    wanted: Option<Spot>,
    /// Where the ghost stands, the snap's outcome, and whether it may be
    /// built there, as of the last update.
    spot: Option<Spot>,
    verdict: Option<Result<(), Refusal>>,
    /// The attack range circles, which do not depend on the position.
    ranges: Vec<GhostRange>,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Hover,
    /// Pressed at `client`, the ghost anchored at `at`.
    Click {
        client: [f32; 2],
        at: [f32; 2],
    },
    Drag {
        at: [f32; 2],
        client: [f32; 2],
    },
}

#[derive(Clone, Copy, PartialEq)]
struct Spot {
    x: f32,
    z: f32,
    angle: f32,
    /// Whether the snap to edges was wanted.
    edges: bool,
}

struct Wall {
    place: Place,
    stage: WallStage,
    start: Option<[f32; 2]>,
    end: Option<[f32; 2]>,
    /// The layout of the ends as of the last update; `None` when none of
    /// it can be built.
    layout: Option<WallLayout>,
    /// `placementWallLastAngle`.
    last_angle: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum WallStage {
    Hover,
    /// The start is set; the release starts the pathing.
    Click,
    Pathing,
}

impl Placement {
    /// `startBuildingPlacement`.
    pub fn start(&mut self, place: Place) {
        self.stop();
        self.active = Some(match place.tower {
            Some(_) => Active::Wall(Wall {
                place,
                stage: WallStage::Hover,
                start: None,
                end: None,
                layout: None,
                last_angle: 0.0,
            }),
            None => Active::Building(Building {
                ranges: place.data.ranges.clone(),
                place,
                seed: random_seed(),
                stage: Stage::Hover,
                angle: DEFAULT_ANGLE,
                wanted: None,
                spot: None,
                verdict: None,
            }),
        });
        self.set_tooltip(Some(""));
    }

    /// Whether a structure is being placed.
    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    /// `placementSupport.Reset`.
    fn stop(&mut self) {
        if self.active.take().is_some() {
            self.set_tooltip(None);
        }
    }

    /// Takes the input and the mouse, judges the spot or lays the wall,
    /// gives the orders and draws the previews.
    pub fn update(&mut self, view: &mut dyn View, uplink: &Uplink, scene: &Scene, seat: i32) {
        self.refresh_shore(scene);
        for input in view.take_place_inputs() {
            self.input(input, view, uplink);
        }
        let pointer = view.place_pointer();
        // The mouse motion of INPUT_BUILDING_PLACEMENT ends a building's
        // placement once its cost cannot be paid, which is how a queued
        // run of buildings stops when the stockpile runs out.
        if pointer.is_some()
            && let Some(Active::Building(building)) = &self.active
            && !building.place.costs.is_empty()
            && short_of(
                scene,
                seat,
                building
                    .place
                    .costs
                    .iter()
                    .map(|&(resource, cost)| (resource, f64::from(cost / 100))),
            )
        {
            self.stop();
        }
        let world = match (&scene.ground, &self.shore) {
            (Some(ground), Some(shore)) => Some(World {
                scene,
                seat,
                ground,
                shore,
            }),
            _ => None,
        };
        match &mut self.active {
            Some(Active::Building(building)) => {
                building.follow(pointer);
                building.judge(world.as_ref());
            }
            Some(Active::Wall(wall)) => {
                wall.follow(pointer);
                wall.judge(world.as_ref(), scene, seat);
            }
            None => {}
        }
        let message = match &self.active {
            Some(Active::Building(building)) => building.message(),
            _ => String::new(),
        };
        if self.active.is_some() {
            self.set_tooltip(Some(&message));
        }
        view.set_placing(self.active.is_some());
        view.set_ghosts(self.ghosts(scene, seat));
    }

    /// The shore distances of the terrain the scene holds, computed again
    /// when a reveal changes it.
    fn refresh_shore(&mut self, scene: &Scene) {
        let Some(ground) = &scene.ground else {
            self.shore = None;
            return;
        };
        if self.active.is_some()
            && self.shore.as_ref().map(ShoreGrid::key) != Some((ground.id, ground.revision))
        {
            self.shore = Some(ShoreGrid::compute(ground));
        }
    }

    fn input(&mut self, input: Input, view: &mut dyn View, uplink: &Uplink) {
        if matches!(input, Input::Cancel) {
            self.stop();
            return;
        }
        let first = view.selected().first().copied();
        let mut sound = None;
        let mut done = false;
        match &mut self.active {
            Some(Active::Building(building)) => match input {
                Input::Press(pointer) => {
                    let at = building
                        .spot
                        .map(|spot| [spot.x, spot.z])
                        .or(pointer.ground);
                    if let Some(at) = at {
                        building.stage = Stage::Click {
                            client: pointer.client,
                            at,
                        };
                    }
                }
                Input::Release(pointer) => {
                    if building.stage != Stage::Hover {
                        (sound, done) = building.place(pointer.shift, uplink, first, view);
                    }
                }
                Input::Cancel => {}
            },
            Some(Active::Wall(wall)) => match (input, wall.stage) {
                (Input::Press(pointer), WallStage::Hover) => {
                    // A start nothing can be built at is no start.
                    if wall.layout.is_some() && pointer.ground.is_some() {
                        wall.stage = WallStage::Click;
                    }
                }
                (Input::Release(_), WallStage::Click) => wall.stage = WallStage::Pathing,
                (Input::Press(pointer), WallStage::Pathing) => {
                    (sound, done) = wall.place(pointer.shift, uplink);
                }
                _ => {}
            },
            None => {}
        }
        if let Some(name) = sound
            && let Some(first) = first
        {
            view.queue_sound(first, name);
        }
        if done {
            self.stop();
        }
    }

    fn ghosts(&self, scene: &Scene, seat: i32) -> Vec<Ghost> {
        match &self.active {
            Some(Active::Building(building)) => {
                let Some(spot) = building.spot.or(building.wanted) else {
                    return Vec::new();
                };
                vec![Ghost {
                    actor: building.place.actor.clone(),
                    x: spot.x,
                    z: spot.z,
                    angle: spot.angle,
                    seed: building.seed,
                    owner: seat,
                    invalid: matches!(building.verdict, Some(Err(_))),
                    ranges: building.ranges.clone(),
                    range_allies: if building.ranges.is_empty() {
                        Vec::new()
                    } else {
                        (1..scene.players.len())
                            .filter(|&p| {
                                p as i32 == seat
                                    || usize::try_from(seat)
                                        .is_ok_and(|s| mutual_allies(scene, s, p))
                            })
                            .map(|p| p as i32)
                            .collect()
                    },
                }]
            }
            Some(Active::Wall(wall)) => match &wall.layout {
                // Every piece is drawn, red from the first that cannot be
                // built or paid for on.
                Some(layout) => layout
                    .pieces
                    .iter()
                    .map(|laid| Ghost {
                        actor: laid.actor.clone(),
                        x: laid.piece.x,
                        z: laid.piece.z,
                        angle: laid.piece.angle,
                        seed: 0,
                        owner: seat,
                        invalid: !laid.valid || short_of(scene, seat, priced(scene, &laid.cost)),
                        ranges: Vec::new(),
                        range_allies: Vec::new(),
                    })
                    .collect(),
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// The canvas's tooltip: `Some` while placing, the refusal or nothing.
    fn set_tooltip(&mut self, markup: Option<&str>) {
        if self.canvas.is_none() {
            self.canvas = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| crate::element(&document, "view").ok());
        }
        let Some(canvas) = &self.canvas else {
            return;
        };
        match markup {
            Some(markup) => {
                if canvas.get_attribute(tooltip::ATTRIBUTE).as_deref() != Some(markup) {
                    let _ = canvas.set_attribute(tooltip::ATTRIBUTE, markup);
                    let _ = canvas.set_attribute(tooltip::STYLE_ATTRIBUTE, TOOLTIP_STYLE);
                }
            }
            None => {
                let _ = canvas.remove_attribute(tooltip::ATTRIBUTE);
                let _ = canvas.remove_attribute(tooltip::STYLE_ATTRIBUTE);
            }
        }
    }
}

impl Building {
    /// What the mouse wants of the ghost: in the hover, to stand under it;
    /// pressed, to stay and turn towards it once dragged.
    fn follow(&mut self, pointer: Option<Pointer>) {
        let Some(pointer) = pointer else {
            return;
        };
        let edges = pointer.ctrl;
        let wanted = match self.stage {
            Stage::Hover => {
                let Some([x, z]) = pointer.ground else {
                    return;
                };
                // The snap to edges starts from the default angle, to end
                // near it.
                let angle = if edges { DEFAULT_ANGLE } else { self.angle };
                Spot { x, z, angle, edges }
            }
            Stage::Click { client, at } | Stage::Drag { at, client } => {
                let dragged = matches!(self.stage, Stage::Drag { .. })
                    || (pointer.client[0] - client[0]).hypot(pointer.client[1] - client[1])
                        >= DRAG_DELTA;
                if !dragged {
                    return;
                }
                self.stage = Stage::Drag { at, client };
                let angle = match pointer.ground {
                    // `horizAngleTo`; near the anchor, the default.
                    Some(to) if (to[0] - at[0]).hypot(to[1] - at[1]) > 0.0 => {
                        (to[0] - at[0]).atan2(to[1] - at[1])
                    }
                    _ => DEFAULT_ANGLE,
                };
                self.angle = angle;
                Spot {
                    x: at[0],
                    z: at[1],
                    angle,
                    edges,
                }
            }
        };
        self.wanted = Some(wanted);
    }

    /// Where the snap puts the ghost and whether it may be built there.
    fn judge(&mut self, world: Option<&World>) {
        let Some(wanted) = self.wanted else {
            return;
        };
        let data = &*self.place.data;
        let snapped = world.and_then(|world| {
            foundation_snap(
                world,
                data,
                f64::from(wanted.x),
                f64::from(wanted.z),
                f64::from(wanted.angle),
                wanted.edges,
            )
        });
        let spot = match snapped {
            Some((x, z, angle)) => Spot {
                x: x as f32,
                z: z as f32,
                angle: angle as f32,
                edges: wanted.edges,
            },
            None => wanted,
        };
        if self.stage == Stage::Hover {
            self.angle = spot.angle;
        }
        self.spot = Some(spot);
        self.verdict = Some(match world {
            Some(world) => check_placement(
                world,
                &Candidate {
                    data,
                    x: f64::from(spot.x),
                    z: f64::from(spot.z),
                    angle: f64::from(spot.angle),
                    groups: [0, 0],
                },
            ),
            None => Err(Refusal::unknown(data)),
        });
    }

    /// The refusal of the spot shown, as `placementTooltip`.
    fn message(&self) -> String {
        match &self.verdict {
            Some(Err(refusal)) => refusal.to_markup(),
            _ => String::new(),
        }
    }

    /// `tryPlaceBuilding`, on the verdict of the last update. Returns the
    /// sound to play and whether the placement is over.
    fn place(
        &mut self,
        queued: bool,
        uplink: &Uplink,
        first: Option<u32>,
        view: &mut dyn View,
    ) -> (Option<&'static str>, bool) {
        let (Some(spot), Some(verdict)) = (self.spot, &self.verdict) else {
            return (None, false);
        };
        if verdict.is_err() {
            self.stage = Stage::Hover;
            if let Some(first) = first {
                view.queue_sound(first, "invalid_building_placement");
            }
            return (None, false);
        }
        uplink.send(&Command::Construct {
            flags: if queued { QUEUED } else { 0 },
            entities: self.place.builders.clone(),
            template: self.place.template.clone(),
            x: spot.x,
            z: spot.z,
            angle: spot.angle,
            seed: self.seed,
        });
        if queued && !self.place.builders.is_empty() {
            self.seed = random_seed();
            self.stage = Stage::Hover;
            (Some("order_build"), false)
        } else {
            (Some("order_build"), true)
        }
    }
}

impl Wall {
    fn follow(&mut self, pointer: Option<Pointer>) {
        let Some(ground) = pointer.and_then(|pointer| pointer.ground) else {
            return;
        };
        match self.stage {
            WallStage::Hover => self.start = Some(ground),
            WallStage::Pathing => self.end = Some(ground),
            WallStage::Click => {}
        }
    }

    /// `SetWallPlacementPreview` on the scene's towers and foundations.
    fn judge(&mut self, world: Option<&World>, scene: &Scene, seat: i32) {
        let (Some(start), Some(world), Some(set)) = (self.start, world, &self.place.data.wall)
        else {
            self.layout = None;
            return;
        };
        let tower = self.place.tower.as_deref().unwrap_or_default();
        let foundation = format!("foundation|{tower}");
        // `PickSimilarPlayerEntities` of the tower, foundations included,
        // on the whole map rather than the screen.
        let snap: Vec<SnapTower> = scene
            .dots
            .iter()
            .filter(|(_, dot)| dot.owner == seat)
            .filter_map(|(id, dot)| {
                let name = scene.templates.get(dot.template as usize)?;
                if name != tower && *name != foundation {
                    return None;
                }
                let (x, z, angle) = pose_of(dot);
                Some(SnapTower {
                    handle: *id,
                    x,
                    z,
                    angle,
                    groups: dot.groups,
                    foundation: *name == foundation,
                })
            })
            .collect();
        self.layout = wall_preview(
            world,
            set,
            [f64::from(start[0]), f64::from(start[1])],
            self.end.map(|end| [f64::from(end[0]), f64::from(end[1])]),
            &snap,
            &mut self.last_angle,
        );
    }

    /// `tryPlaceWall`, on the layout of the last update.
    fn place(&mut self, queued: bool, uplink: &Uplink) -> (Option<&'static str>, bool) {
        let Some(layout) = &self.layout else {
            return (None, false);
        };
        let tower = self.place.tower.clone().unwrap_or_default();
        // At least one piece that is no tower, as `tryPlaceWall` wants.
        if !layout.result().any(|laid| laid.piece.template != tower) {
            return (None, false);
        }
        uplink.send(&Command::ConstructWall {
            flags: if queued { QUEUED } else { 0 },
            entities: self.place.builders.clone(),
            tower,
            pieces: layout
                .result()
                .map(|laid| WallPiece {
                    template: laid.piece.template.clone(),
                    x: laid.piece.x,
                    z: laid.piece.z,
                    angle: laid.piece.angle,
                })
                .collect(),
            start_snapped: layout.start_snapped,
            end_snapped: layout.end_snapped,
        });
        if queued {
            self.start = self.end;
            self.end = None;
            self.stage = WallStage::Click;
            (Some("order_build"), false)
        } else {
            (Some("order_build"), true)
        }
    }
}

/// Whether the seat's stockpile is short of any of `costs`, by resource
/// code (1-based string index) and amount.
fn short_of(scene: &Scene, seat: i32, costs: impl IntoIterator<Item = (u32, f64)>) -> bool {
    let Some(economy) = usize::try_from(seat)
        .ok()
        .and_then(|seat| scene.economy.get(seat))
    else {
        return false;
    };
    costs.into_iter().any(|(resource, amount)| {
        economy
            .resources
            .iter()
            .find(|stock| stock.code == resource)
            .is_some_and(|stock| amount > f64::from(stock.amount))
    })
}

/// A cost by resource name as `short_of` takes it; what the player has no
/// count of (population, time) is dropped, as it is never needed.
fn priced<'a>(
    scene: &'a Scene,
    cost: &'a [(String, f64)],
) -> impl Iterator<Item = (u32, f64)> + 'a {
    cost.iter().filter_map(|(name, amount)| {
        let index = scene.strings.iter().position(|string| string == name)?;
        Some((index as u32 + 1, *amount))
    })
}

/// `RandomizeActorSeed`: below 2^16.
fn random_seed() -> u32 {
    (js_sys::Math::random() * 65536.0) as u32 % 65536
}
