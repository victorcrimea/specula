//! Placing a structure on `/play`, as `gui/session/input.js` does it: a
//! building follows the mouse, a press anchors it and a drag turns it
//! (INPUT_BUILDING_PLACEMENT, _CLICK and _DRAG), a release places it, and
//! Shift keeps placing; a wall goes from a first click to a second
//! (INPUT_BUILDING_WALL_CLICK and _PATHING). The right button and Escape
//! cancel.
//!
//! Whether a spot is valid, the snap to the shore and to nearby edges, and
//! the wall's layout are asked of the engine (`GetFoundationSnapData`,
//! `SetBuildingPlacementPreview`, `SetWallPlacementPreview`) over the
//! uplink. Those read the whole simulation, what the fog hides included,
//! so this is a test only (PLAY-IMPL PL8), to give way to the same checks
//! on the player's own view.

use specula_assets::command::Command;
use specula_assets::command::QUEUED;
use specula_assets::command::QueryKind;
use specula_assets::command::WallPiece;
use wasm_bindgen::JsValue;
use web_sys::HtmlElement;

use crate::View;
use crate::field::DRAG_DELTA;
use crate::lang;
use crate::panel::Place;
use crate::stream::Scene;
use crate::tips;
use crate::tooltip;
use crate::uplink::Uplink;

/// `PlacementSupport.DEFAULT_ANGLE`.
const DEFAULT_ANGLE: f32 = std::f32::consts::PI * 3.0 / 4.0;
/// A query whose reply has not come by then is given up, as the engine
/// may have gone.
const QUERY_TIMEOUT_MS: f64 = 2000.0;
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
}

#[derive(Default)]
pub struct Placement {
    active: Option<Active>,
    /// The query in flight, what it asked, and when.
    pending: Option<(u32, Asked, f64)>,
    /// The canvas, which shows the engine's refusal as its tooltip.
    canvas: Option<HtmlElement>,
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
    /// What the ghost stands on: the point and angle wanted, and where the
    /// snap put it.
    wanted: Option<Spot>,
    snapped: Option<(Spot, Spot)>,
    /// The spot the engine last judged, its verdict and refusal message.
    judged: Option<(Spot, bool, String)>,
    /// A release waiting for the verdict of the spot, with Shift held.
    placing: Option<bool>,
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
    /// The layout last asked for and the engine's answer: `None` when it
    /// cannot build any of it.
    laid: Option<(Ends, Option<Layout>)>,
    /// A press waiting for the layout of the ends, with Shift held.
    placing: Option<bool>,
}

/// A wall's start and, once pathing, its end.
type Ends = ([f32; 2], Option<[f32; 2]>);

#[derive(Clone, Copy, PartialEq)]
enum WallStage {
    Hover,
    /// The start is set; the release starts the pathing.
    Click,
    Pathing,
}

struct Layout {
    pieces: Vec<(WallPiece, String)>,
    start_snapped: u32,
    end_snapped: u32,
}

#[derive(Clone, Copy)]
enum Asked {
    Snap(Spot),
    Preview(Spot),
    Wall([f32; 2], Option<[f32; 2]>),
    /// The end of a preview, whose answer is not read.
    Clear,
}

impl Placement {
    /// `startBuildingPlacement`.
    pub fn start(&mut self, place: Place, uplink: &Uplink) {
        self.stop(uplink);
        self.active = Some(match place.tower {
            Some(_) => Active::Wall(Wall {
                place,
                stage: WallStage::Hover,
                start: None,
                end: None,
                laid: None,
                placing: None,
            }),
            None => Active::Building(Building {
                place,
                seed: random_seed(),
                stage: Stage::Hover,
                angle: DEFAULT_ANGLE,
                wanted: None,
                snapped: None,
                judged: None,
                placing: None,
            }),
        });
        self.set_tooltip(Some(""));
    }

    /// `placementSupport.Reset`: the engine's preview ends too.
    fn stop(&mut self, uplink: &Uplink) {
        let Some(active) = self.active.take() else {
            return;
        };
        let kind = match active {
            Active::Building(_) => QueryKind::Preview {
                template: String::new(),
                x: 0.0,
                z: 0.0,
                angle: 0.0,
                seed: 0,
            },
            Active::Wall(_) => QueryKind::Wall {
                wallset: String::new(),
                start: None,
                end: None,
                snap: Vec::new(),
            },
        };
        self.pending = uplink
            .query(kind)
            .map(|id| (id, Asked::Clear, js_sys::Date::now()));
        self.set_tooltip(None);
    }

    /// Takes the replies, the input and the mouse, asks the engine what
    /// is still to know, gives the orders and draws the previews.
    pub fn update(&mut self, view: &mut dyn View, uplink: &Uplink, scene: &Scene, seat: i32) {
        for (id, result) in uplink.take_replies() {
            if let Some((pending, asked, _)) = self.pending
                && pending == id
            {
                self.pending = None;
                self.answer(asked, &result);
            }
        }
        if self
            .pending
            .is_some_and(|(_, _, since)| js_sys::Date::now() - since > QUERY_TIMEOUT_MS)
        {
            self.pending = None;
        }
        for input in view.take_place_inputs() {
            self.input(input, uplink);
        }
        let pointer = view.place_pointer();
        match &mut self.active {
            Some(Active::Building(building)) => building.follow(pointer),
            Some(Active::Wall(wall)) => wall.follow(pointer),
            None => {}
        }
        self.place(view, uplink);
        if self.pending.is_none() {
            self.ask(uplink, scene, seat);
        }
        let message = match &self.active {
            Some(Active::Building(building)) => building.message(),
            _ => String::new(),
        };
        if self.active.is_some() {
            self.set_tooltip(Some(&message));
        }
        view.set_placing(self.active.is_some());
        view.set_ghosts(self.ghosts(seat));
    }

    fn input(&mut self, input: Input, uplink: &Uplink) {
        if matches!(input, Input::Cancel) {
            self.stop(uplink);
            return;
        }
        match &mut self.active {
            Some(Active::Building(building)) => match input {
                Input::Press(pointer) => {
                    let at = building
                        .snapped
                        .map(|(_, spot)| [spot.x, spot.z])
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
                        building.placing = Some(pointer.shift);
                    }
                }
                Input::Cancel => {}
            },
            Some(Active::Wall(wall)) => match (input, wall.stage) {
                (Input::Press(pointer), WallStage::Hover) => {
                    // A start the engine can build nothing at is no start.
                    let buildable = wall.laid.as_ref().is_some_and(|(ends, layout)| {
                        layout.is_some() && Some(ends.0) == wall.start
                    });
                    if buildable && pointer.ground.is_some() {
                        wall.stage = WallStage::Click;
                    }
                }
                (Input::Release(_), WallStage::Click) => wall.stage = WallStage::Pathing,
                (Input::Press(pointer), WallStage::Pathing) => wall.placing = Some(pointer.shift),
                _ => {}
            },
            None => {}
        }
    }

    /// `tryPlaceBuilding` and `tryPlaceWall`, once the engine has judged
    /// the spot or laid the wall the press was for. Their sounds play from
    /// the first selected entity, as theirs do.
    fn place(&mut self, view: &mut dyn View, uplink: &Uplink) {
        let first = view.selected().first().copied();
        let mut sound = None;
        let mut done = false;
        match &mut self.active {
            Some(Active::Building(building)) => {
                let (Some(queued), Some((_, spot))) = (building.placing, building.snapped) else {
                    return;
                };
                let Some((judged, valid, _)) = &building.judged else {
                    return;
                };
                if *judged != spot {
                    return;
                }
                building.placing = None;
                if !*valid {
                    building.stage = Stage::Hover;
                    if let Some(first) = first {
                        view.queue_sound(first, "invalid_building_placement");
                    }
                    return;
                }
                uplink.send(&Command::Construct {
                    flags: if queued { QUEUED } else { 0 },
                    entities: building.place.builders.clone(),
                    template: building.place.template.clone(),
                    x: spot.x,
                    z: spot.z,
                    angle: spot.angle,
                    seed: building.seed,
                });
                sound = Some("order_build");
                if queued && !building.place.builders.is_empty() {
                    building.seed = random_seed();
                    building.stage = Stage::Hover;
                } else {
                    done = true;
                }
            }
            Some(Active::Wall(wall)) => {
                let (Some(queued), Some(start)) = (wall.placing, wall.start) else {
                    return;
                };
                let Some(((laid_start, laid_end), layout)) = &wall.laid else {
                    return;
                };
                if *laid_start != start || *laid_end != wall.end {
                    return;
                }
                wall.placing = None;
                let tower = wall.place.tower.clone().unwrap_or_default();
                // At least one piece that is no tower, as `tryPlaceWall`
                // wants.
                let Some(layout) = layout.as_ref().filter(|layout| {
                    layout
                        .pieces
                        .iter()
                        .any(|(piece, _)| piece.template != tower)
                }) else {
                    return;
                };
                uplink.send(&Command::ConstructWall {
                    flags: if queued { QUEUED } else { 0 },
                    entities: wall.place.builders.clone(),
                    tower,
                    pieces: layout
                        .pieces
                        .iter()
                        .map(|(piece, _)| piece.clone())
                        .collect(),
                    start_snapped: layout.start_snapped,
                    end_snapped: layout.end_snapped,
                });
                sound = Some("order_build");
                if queued {
                    wall.start = wall.end;
                    wall.end = None;
                    wall.stage = WallStage::Click;
                } else {
                    done = true;
                }
            }
            None => {}
        }
        if let Some(name) = sound
            && let Some(first) = first
        {
            view.queue_sound(first, name);
        }
        if done {
            self.stop(uplink);
        }
    }

    /// Asks the engine the next thing the previews need, if any.
    fn ask(&mut self, uplink: &Uplink, scene: &Scene, seat: i32) {
        let (kind, asked) = match &mut self.active {
            Some(Active::Building(building)) => {
                let Some(wanted) = building.wanted else {
                    return;
                };
                // A snap only moves a dock, or with the snap to edges.
                if !building.place.shore && !wanted.edges {
                    building.snapped = Some((wanted, wanted));
                }
                match building.snapped.filter(|(from, _)| *from == wanted) {
                    None => (
                        QueryKind::Snap {
                            template: building.place.template.clone(),
                            x: wanted.x,
                            z: wanted.z,
                            angle: wanted.angle,
                            edges: wanted.edges,
                        },
                        Asked::Snap(wanted),
                    ),
                    Some((_, spot)) => {
                        if building
                            .judged
                            .as_ref()
                            .is_some_and(|(judged, _, _)| *judged == spot)
                        {
                            return;
                        }
                        (
                            QueryKind::Preview {
                                template: building.place.template.clone(),
                                x: spot.x,
                                z: spot.z,
                                angle: spot.angle,
                                seed: building.seed,
                            },
                            Asked::Preview(spot),
                        )
                    }
                }
            }
            Some(Active::Wall(wall)) => {
                let Some(start) = wall.start else {
                    return;
                };
                if wall
                    .laid
                    .as_ref()
                    .is_some_and(|(ends, _)| *ends == (start, wall.end))
                {
                    return;
                }
                let tower = wall.place.tower.as_deref().unwrap_or_default();
                let foundation = format!("foundation|{tower}");
                // `PickSimilarPlayerEntities` of the tower, foundations
                // included, on the whole map rather than the screen.
                let snap = scene
                    .dots
                    .iter()
                    .filter(|(_, dot)| dot.owner == seat)
                    .filter(|(_, dot)| {
                        scene
                            .templates
                            .get(dot.template as usize)
                            .is_some_and(|name| name == tower || *name == foundation)
                    })
                    .map(|(id, _)| *id)
                    .collect();
                (
                    QueryKind::Wall {
                        wallset: wall.place.template.clone(),
                        start: Some((start[0], start[1])),
                        end: wall.end.map(|[x, z]| (x, z)),
                        snap,
                    },
                    Asked::Wall(start, wall.end),
                )
            }
            None => return,
        };
        self.pending = uplink
            .query(kind)
            .map(|id| (id, asked, js_sys::Date::now()));
    }

    fn answer(&mut self, asked: Asked, result: &JsValue) {
        match (&mut self.active, asked) {
            (Some(Active::Building(building)), Asked::Snap(wanted)) => {
                // False when nothing snaps.
                let spot = if result.is_object() {
                    Spot {
                        x: tips::number_field(result, "x") as f32,
                        z: tips::number_field(result, "z") as f32,
                        angle: tips::number_field(result, "angle") as f32,
                        edges: wanted.edges,
                    }
                } else {
                    wanted
                };
                building.snapped = Some((wanted, spot));
                if building.stage == Stage::Hover {
                    building.angle = spot.angle;
                }
            }
            (Some(Active::Building(building)), Asked::Preview(spot)) => {
                let valid = tips::field(result, "success").as_bool() == Some(true);
                building.judged = Some((spot, valid, refusal(result)));
            }
            (Some(Active::Wall(wall)), Asked::Wall(start, end)) => {
                let layout = result.is_object().then(|| Layout {
                    pieces: js_sys::Array::from(&tips::field(result, "pieces"))
                        .iter()
                        .map(|piece| {
                            (
                                WallPiece {
                                    template: tips::text_field(&piece, "template"),
                                    x: tips::number_field(&piece, "x") as f32,
                                    z: tips::number_field(&piece, "z") as f32,
                                    angle: tips::number_field(&piece, "angle") as f32,
                                },
                                tips::text_field(&piece, "actor"),
                            )
                        })
                        .collect(),
                    start_snapped: tips::number_field(result, "startSnappedEnt") as u32,
                    end_snapped: tips::number_field(result, "endSnappedEnt") as u32,
                });
                wall.laid = Some(((start, end), layout));
            }
            _ => {}
        }
    }

    fn ghosts(&self, seat: i32) -> Vec<Ghost> {
        match &self.active {
            Some(Active::Building(building)) => {
                let Some(spot) = building.snapped.map(|(_, spot)| spot).or(building.wanted) else {
                    return Vec::new();
                };
                vec![Ghost {
                    actor: building.place.actor.clone(),
                    x: spot.x,
                    z: spot.z,
                    angle: spot.angle,
                    seed: building.seed,
                    owner: seat,
                    invalid: building.judged.as_ref().is_some_and(|(_, valid, _)| !valid),
                }]
            }
            Some(Active::Wall(wall)) => match &wall.laid {
                Some((_, Some(layout))) => layout
                    .pieces
                    .iter()
                    .map(|(piece, actor)| Ghost {
                        actor: actor.clone(),
                        x: piece.x,
                        z: piece.z,
                        angle: piece.angle,
                        seed: 0,
                        owner: seat,
                        invalid: false,
                    })
                    .collect(),
                _ => Vec::new(),
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
            Stage::Click { .. } | Stage::Drag { .. } if self.placing.is_some() => return,
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

    /// The engine's refusal of the spot shown, as `placementTooltip`.
    fn message(&self) -> String {
        match &self.judged {
            Some((_, false, message)) => message.clone(),
            _ => String::new(),
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
            WallStage::Pathing if self.placing.is_none() => self.end = Some(ground),
            _ => {}
        }
    }
}

/// `updateBuildingPlacementPreview`'s tooltip of a `CheckPlacement` result
/// that is not a success: the message, translated if it says so, with its
/// parameters.
fn refusal(result: &JsValue) -> String {
    let message = tips::text_field(result, "message");
    if message.is_empty() {
        return String::new();
    }
    let message = if tips::field(result, "translateMessage").as_bool() == Some(true) {
        let plural = tips::text_field(result, "pluralMessage");
        if plural.is_empty() {
            lang::translate(&message)
        } else {
            lang::translate_plural(&message, &plural, tips::number_field(result, "pluralCount"))
        }
    } else {
        message
    };
    let parameters = tips::field(result, "parameters");
    let translated: Vec<String> = js_sys::Array::from(&tips::field(result, "translateParameters"))
        .iter()
        .filter_map(|key| key.as_string())
        .collect();
    let values: Vec<(String, String)> =
        js_sys::Object::keys(&js_sys::Object::from(parameters.clone()))
            .iter()
            .filter_map(|key| key.as_string())
            .map(|key| {
                let value = tips::field(&parameters, &key);
                let translate = translated.contains(&key);
                // `translateObjectKeys`: a string, or {context, _string}.
                let text = if value.is_object() {
                    let text = tips::text_field(&value, "_string");
                    let context = tips::text_field(&value, "context");
                    match (translate, context.is_empty()) {
                        (true, false) => lang::translate_context(&context, &text),
                        (true, true) => lang::translate(&text),
                        (false, _) => text,
                    }
                } else if let Some(number) = value.as_f64() {
                    number.to_string()
                } else {
                    let text = value.as_string().unwrap_or_default();
                    if translate {
                        lang::translate(&text)
                    } else {
                        text
                    }
                };
                (key, text)
            })
            .collect();
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    lang::format(&message, &pairs)
}

/// `RandomizeActorSeed`: below 2^16.
fn random_seed() -> u32 {
    (js_sys::Math::random() * 65536.0) as u32 % 65536
}
