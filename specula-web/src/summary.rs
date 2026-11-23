//! The match summary, a port of gui/summary/ (`layout.js`, `counters.js`,
//! `summary.js`) into the page's `#match-summary`: six tables of counters
//! with team totals, and two charts drawn as inline SVG.
//!
//! The numbers are the engine's statistics tracker's, captured by the hook
//! every 30 s and served over HTTP: `/play/summary` to the player of
//! `/play` (the seat, Gaia and the allies it shares LOS with while the game
//! goes on, every player once it is decided or the engine has stopped), and
//! `/match/<id>/summary` to `/` (every player, `--delay` late). Opening the
//! window never asks the engine for anything.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::Document;
use web_sys::Element;
use web_sys::HtmlElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlSelectElement;
use web_sys::Response;

use crate::diplomacy_window::civ_name;
use crate::diplomacy_window::time_to_string;
use crate::lang;
use crate::panel::create;
use crate::panel::css_color;

/// `SUMMARY_INTERVAL_TURNS` of the engine hook: a capture every 30 s of
/// game time.
const CAPTURE_TURNS: u32 = 150;
/// Turns between asks for a capture that is late: 1 s.
const RETRY_TURNS: u32 = 5;
/// `g_PlayerBoxAlpha`.
const ROW_ALPHA: f64 = 50.0 / 255.0;
const SVG: &str = "http://www.w3.org/2000/svg";
const CHART_WIDTH: f64 = 520.0;
const CHART_HEIGHT: f64 = 240.0;
const CHART_LEFT: f64 = 52.0;
const CHART_RIGHT: f64 = 12.0;
const CHART_TOP: f64 = 10.0;
const CHART_BOTTOM: f64 = 26.0;
const TICKS: usize = 5;
/// `g_MapSizes` of `map_sizes.json`, by tiles.
const MAP_SIZES: [(f64, &str); 7] = [
    (128.0, "Tiny"),
    (192.0, "Small"),
    (256.0, "Normal"),
    (320.0, "Medium"),
    (384.0, "Large"),
    (448.0, "Very Large"),
    (512.0, "Giant"),
];
/// `Resources.GetCodes()`, for a payload that names none.
const RESOURCES: [&str; 4] = ["food", "wood", "stone", "metal"];

/// Where the numbers come from.
#[derive(Clone, PartialEq, Eq)]
pub enum Source {
    /// `/play`'s owner: the seat's own copies.
    Play {
        seat: usize,
        /// The engine has not exited, so more captures may come.
        running: bool,
        /// The playhead, when it is behind the live edge.
        turn: Option<u32>,
    },
    /// `/`: a match by id.
    Stream { match_id: u64, turn: Option<u32> },
}

impl Source {
    fn url(&self) -> Option<String> {
        let base = match self {
            Source::Play { .. } => format!("{}/play/summary", crate::match_base()),
            Source::Stream { match_id: 0, .. } => return None,
            Source::Stream { match_id, .. } => {
                format!("{}/match/{match_id:016x}/summary", crate::match_base())
            }
        };
        Some(match self.turn() {
            Some(turn) => format!("{base}?turn={turn}"),
            None => base,
        })
    }

    /// The playhead, when it is behind the live edge.
    fn turn(&self) -> Option<u32> {
        match self {
            Source::Play { turn, .. } | Source::Stream { turn, .. } => *turn,
        }
    }
}

/// `g_SummaryTypes`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Percent,
    Trained,
    Constructed,
    Gathered,
    Count,
    Sent,
    Bought,
    Captured,
    Succeeded,
    Destroyed,
    Killed,
    Lost,
    Used,
    Received,
    Population,
    Sold,
    Failed,
}

impl Kind {
    /// The `%(name)s` of the headings' captions.
    fn key(self) -> &'static str {
        match self {
            Kind::Percent => "percent",
            Kind::Trained => "trained",
            Kind::Constructed => "constructed",
            Kind::Gathered => "gathered",
            Kind::Count => "count",
            Kind::Sent => "sent",
            Kind::Bought => "bought",
            Kind::Captured => "captured",
            Kind::Succeeded => "succeeded",
            Kind::Destroyed => "destroyed",
            Kind::Killed => "killed",
            Kind::Lost => "lost",
            Kind::Used => "used",
            Kind::Received => "received",
            Kind::Population => "population",
            Kind::Sold => "sold",
            Kind::Failed => "failed",
        }
    }

    fn caption(self) -> &'static str {
        match self {
            Kind::Percent => "%",
            Kind::Trained => "Trained",
            Kind::Constructed => "Constructed",
            Kind::Gathered => "Gathered",
            Kind::Count => "Count",
            Kind::Sent => "Sent",
            Kind::Bought => "Bought",
            Kind::Captured => "Captured",
            Kind::Succeeded => "Succeeded",
            Kind::Destroyed => "Destroyed",
            Kind::Killed => "Killed",
            Kind::Lost => "Lost",
            Kind::Used => "Used",
            Kind::Received => "Received",
            Kind::Population => "Population",
            Kind::Sold => "Sold",
            Kind::Failed => "Failed",
        }
    }

    /// `g_TypeColors`.
    fn color(self) -> Option<&'static str> {
        match self {
            Kind::Percent | Kind::Count => None,
            Kind::Trained
            | Kind::Constructed
            | Kind::Gathered
            | Kind::Sent
            | Kind::Bought
            | Kind::Succeeded => Some("rgb(201, 255, 200)"),
            Kind::Captured => Some("rgb(255, 255, 157)"),
            Kind::Destroyed | Kind::Killed => Some("rgb(196, 198, 255)"),
            Kind::Lost
            | Kind::Used
            | Kind::Received
            | Kind::Population
            | Kind::Sold
            | Kind::Failed => Some("rgb(255, 213, 213)"),
        }
    }

    fn postfix(self) -> &'static str {
        match self {
            Kind::Percent => "%",
            Kind::Trained
            | Kind::Constructed
            | Kind::Gathered
            | Kind::Sent
            | Kind::Bought
            | Kind::Captured
            | Kind::Succeeded => " / ",
            Kind::Destroyed | Kind::Killed => "\n",
            _ => "",
        }
    }

    fn hidden(self) -> bool {
        self == Kind::Count
    }
}

/// What a counter shows: a number, or the parts of one in the engine's
/// order.
#[derive(Clone)]
enum Value {
    Number(f64),
    Parts(Vec<(Kind, f64)>),
}

impl Value {
    /// A chart's number: the part of the chosen type, or the number.
    fn pick(&self, index: usize) -> f64 {
        match self {
            Value::Number(value) => *value,
            Value::Parts(parts) => parts.get(index).map_or(0.0, |(_, value)| *value),
        }
    }
}

/// How a chart's Y axis and a heading write a number.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Integer,
    Percentage,
    Decimal2,
}

/// One column of a table, `layout.js`'s heading and its counter.
struct Heading {
    id: String,
    caption: String,
    /// A caption with the colored types, as `headerCaption`.
    header: Option<&'static str>,
    /// Not in the table, though a chart may draw it.
    hidden: bool,
    format: Format,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Score,
    Structures,
    Units,
    Resources,
    Market,
    Miscellaneous,
}

const PANELS: [Panel; 6] = [
    Panel::Score,
    Panel::Structures,
    Panel::Units,
    Panel::Resources,
    Panel::Market,
    Panel::Miscellaneous,
];

impl Panel {
    fn label(self) -> &'static str {
        match self {
            Panel::Score => "Score",
            Panel::Structures => "Structures",
            Panel::Units => "Units",
            Panel::Resources => "Resources",
            Panel::Market => "Market",
            Panel::Miscellaneous => "Miscellaneous",
        }
    }

    /// The caption over the columns, whose `%(name)s` are colored types.
    fn title(self) -> Option<&'static str> {
        match self {
            Panel::Structures => Some(
                "Structure Statistics (%(constructed)s / %(destroyed)s / %(captured)s / %(lost)s)",
            ),
            Panel::Units => Some("Unit Statistics (%(trained)s / %(killed)s / %(lost)s)"),
            Panel::Resources => Some("Resource Statistics (%(gathered)s / %(used)s)"),
            _ => None,
        }
    }

    /// The columns after the player name.
    fn headings(self, resources: &[String]) -> Vec<Heading> {
        let plain = |id: &str, caption: &str, format: Format| Heading {
            id: id.to_string(),
            caption: lang::translate(caption),
            header: None,
            hidden: false,
            format,
        };
        let integer = |id: &str, caption: &str| plain(id, caption, Format::Integer);
        let resource_name = |code: &str| {
            let mut name = code.to_string();
            if let Some(first) = name.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            lang::translate(&name)
        };
        match self {
            Panel::Score => vec![
                integer("totalScore", "Total score"),
                integer("economyScore", "Economy score"),
                integer("militaryScore", "Military score"),
                integer("explorationScore", "Exploration score"),
            ],
            Panel::Structures => vec![
                integer("Structure", "Total"),
                integer("House", "Houses"),
                integer("Economic", "Economic"),
                integer("Outpost", "Outposts"),
                integer("Military", "Military"),
                integer("Fortress", "Fortresses"),
                integer("CivCentre", "Civ centers"),
                integer("Wonder", "Wonders"),
            ],
            Panel::Units => vec![
                integer("Unit", "Total"),
                integer("Infantry", "Infantry"),
                integer("Worker", "Worker"),
                integer("Cavalry", "Cavalry"),
                integer("Champion", "Champion"),
                integer("Hero", "Heroes"),
                integer("Siege", "Siege"),
                integer("Ship", "Navy"),
                integer("Trader", "Traders"),
            ],
            Panel::Resources => {
                let mut headings = vec![integer("total", "Total")];
                for code in resources {
                    headings.push(Heading {
                        caption: resource_name(code),
                        ..integer(code, "")
                    });
                }
                headings.push(Heading {
                    header: Some("Tributes \n(%(sent)s / %(received)s)"),
                    ..integer("tributes", "Tributes")
                });
                headings.push(integer("treasuresCollected", "Treasures collected"));
                headings.push(integer("loot", "Loot"));
                headings.push(integer("livestock", "Livestock bred"));
                headings
            }
            Panel::Market => {
                let mut headings = vec![
                    integer("tradeIncome", "Trade income"),
                    plain("barterEfficency", "Barter efficiency", Format::Percentage),
                ];
                for code in resources {
                    headings.push(Heading {
                        caption: lang::format(
                            &lang::translate("%(resourceFirstWord)s exchanged"),
                            &[("resourceFirstWord", &resource_name(code))],
                        ),
                        ..integer(code, "")
                    });
                }
                headings
            }
            Panel::Miscellaneous => vec![
                plain("killDeath", "Kill / Death ratio", Format::Decimal2),
                Heading {
                    hidden: true,
                    ..integer("population", "Population")
                },
                plain("mapControlPeak", "Map control (peak)", Format::Percentage),
                plain("mapControl", "Map control (finish)", Format::Percentage),
                plain("mapExploration", "Map exploration", Format::Percentage),
                plain("vegetarianRatio", "Vegetarian ratio", Format::Percentage),
                plain("civilianization", "Civilianization", Format::Percentage),
                Heading {
                    header: Some("Bribes\n(%(succeeded)s / %(failed)s)"),
                    ..integer("bribes", "Bribes")
                },
            ],
        }
    }
}

/// One statistics sequence: a series, or one per type.
enum Seq {
    Flat(Vec<f64>),
    Typed(HashMap<String, Vec<f64>>),
}

struct SummaryPlayer {
    id: usize,
    name: String,
    civ: String,
    color: u32,
    team: i32,
    won: bool,
    defeated: bool,
    sequences: HashMap<String, Seq>,
}

impl SummaryPlayer {
    /// `sequences[name][type][index]`, 0 for what the payload lacks.
    fn at(&self, name: &str, kind: Option<&str>, index: usize) -> f64 {
        let series = match (self.sequences.get(name), kind) {
            (Some(Seq::Flat(series)), None) => Some(series),
            (Some(Seq::Typed(map)), Some(kind)) => map.get(kind),
            _ => None,
        };
        series
            .and_then(|series| series.get(index))
            .copied()
            .unwrap_or(0.0)
    }
}

/// A capture, as one page reads it.
struct Data {
    turn: u32,
    is_final: bool,
    elapsed_ms: f64,
    map_name: String,
    map_size: f64,
    map_type: String,
    resources: Vec<String>,
    /// Gaia left out.
    players: Vec<SummaryPlayer>,
    /// Teams in order, with the indices of their players in `players`;
    /// empty when teams are not shown.
    teams: Vec<(i32, Vec<usize>)>,
    /// Clock seconds of each sample, as many as are shown.
    time: Vec<f64>,
}

fn get(object: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn number(object: &JsValue, key: &str) -> f64 {
    get(object, key).as_f64().unwrap_or(0.0)
}

fn string(object: &JsValue, key: &str) -> String {
    get(object, key).as_string().unwrap_or_default()
}

fn numbers(array: &JsValue) -> Vec<f64> {
    js_sys::Array::from(array)
        .iter()
        .map(|value| value.as_f64().unwrap_or(0.0))
        .collect()
}

fn parse_sequences(sequences: &JsValue) -> HashMap<String, Seq> {
    let mut parsed = HashMap::new();
    if !sequences.is_object() {
        return parsed;
    }
    for key in js_sys::Object::keys(sequences.unchecked_ref::<js_sys::Object>()).iter() {
        let Some(name) = key.as_string() else {
            continue;
        };
        let value = get(sequences, &name);
        if js_sys::Array::is_array(&value) {
            parsed.insert(name, Seq::Flat(numbers(&value)));
        } else if value.is_object() {
            let mut typed = HashMap::new();
            for kind in js_sys::Object::keys(value.unchecked_ref::<js_sys::Object>()).iter() {
                if let Some(kind) = kind.as_string() {
                    typed.insert(kind.clone(), numbers(&get(&value, &kind)));
                }
            }
            parsed.insert(name, Seq::Typed(typed));
        }
    }
    parsed
}

impl Data {
    /// The envelope of `/play/summary` and `/match/<id>/summary`.
    fn parse(text: &str) -> Option<Data> {
        let envelope = js_sys::JSON::parse(text).ok()?;
        let metadata = get(&envelope, "metadata");
        let states = js_sys::Array::from(&get(&metadata, "playerStates"));
        let ids_value = get(&metadata, "playerIDs");
        let ids: Option<Vec<usize>> = js_sys::Array::is_array(&ids_value).then(|| {
            js_sys::Array::from(&ids_value)
                .iter()
                .map(|id| id.as_f64().unwrap_or(0.0) as usize)
                .collect()
        });
        let settings = get(&metadata, "mapSettings");
        let mut players = Vec::new();
        for (position, state) in states.iter().enumerate() {
            let id = ids
                .as_ref()
                .and_then(|ids| ids.get(position).copied())
                .unwrap_or(position);
            // Gaia has no row.
            if id == 0 {
                continue;
            }
            let color = get(&state, "color");
            let channel =
                |name: &str| (number(&color, name) * 255.0).floor().clamp(0.0, 255.0) as u32;
            let outcome = string(&state, "state");
            players.push(SummaryPlayer {
                id,
                name: string(&state, "name"),
                civ: string(&state, "civ"),
                color: 0xFF00_0000 | channel("r") << 16 | channel("g") << 8 | channel("b"),
                team: number(&state, "team") as i32,
                won: outcome == "won",
                defeated: outcome == "defeated",
                sequences: parse_sequences(&get(&state, "sequences")),
            });
        }
        let time = players
            .first()
            .and_then(|player| match player.sequences.get("time") {
                Some(Seq::Flat(time)) => Some(time.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let resources: Vec<String> = players
            .first()
            .and_then(|player| match player.sequences.get("resourcesGathered") {
                Some(Seq::Typed(map)) => {
                    let mut codes: Vec<String> = RESOURCES
                        .iter()
                        .filter(|code| map.contains_key(**code))
                        .map(|code| code.to_string())
                        .collect();
                    // A resource the engine adds later still gets a column.
                    let mut extra: Vec<String> = map
                        .keys()
                        .filter(|code| {
                            *code != "vegetarianFood" && !RESOURCES.contains(&code.as_str())
                        })
                        .cloned()
                        .collect();
                    extra.sort();
                    codes.extend(extra);
                    Some(codes)
                }
                _ => None,
            })
            .unwrap_or_else(|| RESOURCES.iter().map(|code| code.to_string()).collect());

        // `initTeamData`.
        let mut teams: Vec<(i32, Vec<usize>)> = Vec::new();
        if get(&settings, "LockTeams").is_truthy() {
            for (index, player) in players.iter().enumerate() {
                if player.team < 0 {
                    continue;
                }
                match teams.iter_mut().find(|(team, _)| *team == player.team) {
                    Some((_, members)) => members.push(index),
                    None => teams.push((player.team, vec![index])),
                }
            }
            teams.sort_by_key(|(team, _)| *team);
        }
        if teams.iter().all(|(_, members)| members.len() < 2) {
            teams.clear();
        }
        if teams.is_empty() {
            for player in &mut players {
                player.team = -1;
            }
        }

        Some(Data {
            turn: number(&envelope, "turn") as u32,
            is_final: get(&envelope, "final").is_truthy(),
            elapsed_ms: number(&metadata, "timeElapsed"),
            map_name: string(&settings, "mapName"),
            map_size: number(&settings, "Size"),
            map_type: string(&settings, "mapType"),
            resources,
            players,
            teams,
            time,
        })
    }

    fn samples(&self) -> usize {
        self.time.len()
    }
}

/// `Math.round`.
fn round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `calculatePercent`.
fn percent(dividend: f64, divisor: f64) -> Value {
    Value::Parts(vec![(
        Kind::Percent,
        if divisor != 0.0 {
            (100.0 * dividend / divisor).floor()
        } else {
            0.0
        },
    )])
}

/// `calculateRatio`.
fn ratio(dividend: f64, divisor: f64) -> Value {
    Value::Number(if dividend != 0.0 {
        (dividend / divisor * 100.0).round() / 100.0
    } else {
        0.0
    })
}

fn score_part(player: &SummaryPlayer, id: &str, resources: &[String], index: usize) -> f64 {
    match id {
        "economyScore" => {
            // The vegetarian food is not in the codes.
            let gathered: f64 = resources
                .iter()
                .map(|code| player.at("resourcesGathered", Some(code), index))
                .sum();
            round((gathered + player.at("tradeIncome", None, index)) / 10.0)
        }
        "militaryScore" => round(
            (player.at("enemyUnitsKilledValue", None, index)
                + player.at("unitsCapturedValue", None, index)
                + player.at("enemyBuildingsDestroyedValue", None, index)
                + player.at("buildingsCapturedValue", None, index))
                / 10.0,
        ),
        _ => player.at("percentMapExplored", None, index) * 10.0,
    }
}

impl Data {
    /// The counter `id` of `panel` for `who`, as of sample `index`: one
    /// player, or a team whose players sum, as `teamCounterFn` does.
    fn value(
        &self,
        panel: Panel,
        id: &str,
        who: &[&SummaryPlayer],
        team: bool,
        index: usize,
    ) -> Value {
        let at = |name: &str, kind: Option<&str>| -> f64 {
            who.iter().map(|player| player.at(name, kind, index)).sum()
        };
        // The tracker's own fields for the team, which the players of a
        // team share.
        let team_or_own = |own: &str, shared: &str| -> f64 {
            if team {
                who.first()
                    .map_or(0.0, |player| player.at(shared, None, index))
            } else {
                at(own, None)
            }
        };
        let resources = &self.resources;
        let parts = |pairs: [(Kind, f64); 3]| Value::Parts(pairs.to_vec());
        match panel {
            Panel::Score => {
                let sum_of = |id: &str| -> f64 {
                    who.iter()
                        .map(|player| score_part(player, id, resources, index))
                        .sum()
                };
                let exploration = if team {
                    team_or_own("percentMapExplored", "teamPercentMapExplored") * 10.0
                } else {
                    sum_of("explorationScore")
                };
                Value::Number(match id {
                    "explorationScore" => exploration,
                    "totalScore" => sum_of("economyScore") + sum_of("militaryScore") + exploration,
                    _ => sum_of(id),
                })
            }
            Panel::Structures => Value::Parts(vec![
                (Kind::Constructed, at("buildingsConstructed", Some(id))),
                (Kind::Destroyed, at("enemyBuildingsDestroyed", Some(id))),
                (Kind::Captured, at("buildingsCaptured", Some(id))),
                (Kind::Lost, at("buildingsLost", Some(id))),
            ]),
            Panel::Units => parts([
                (Kind::Trained, at("unitsTrained", Some(id))),
                (Kind::Killed, at("enemyUnitsKilled", Some(id))),
                (Kind::Lost, at("unitsLost", Some(id))),
            ]),
            Panel::Resources => match id {
                "total" => {
                    let mut total = [0.0; 3];
                    for code in resources {
                        total[0] += at("resourcesCount", Some(code));
                        total[1] += at("resourcesGathered", Some(code));
                        total[2] +=
                            at("resourcesUsed", Some(code)) - at("resourcesSold", Some(code));
                    }
                    parts([
                        (Kind::Count, total[0]),
                        (Kind::Gathered, total[1]),
                        (Kind::Used, total[2]),
                    ])
                }
                "tributes" => Value::Parts(vec![
                    (Kind::Sent, at("tributesSent", None)),
                    (Kind::Received, at("tributesReceived", None)),
                ]),
                "treasuresCollected" => Value::Number(at("treasuresCollected", None)),
                "loot" => Value::Number(at("lootCollected", None)),
                "livestock" => Value::Number(at("unitsTrained", Some("Domestic"))),
                code => parts([
                    (Kind::Count, at("resourcesCount", Some(code))),
                    (Kind::Gathered, at("resourcesGathered", Some(code))),
                    (
                        Kind::Used,
                        at("resourcesUsed", Some(code)) - at("resourcesSold", Some(code)),
                    ),
                ]),
            },
            Panel::Market => match id {
                "tradeIncome" => Value::Number(at("tradeIncome", None)),
                "barterEfficency" => {
                    let (mut bought, mut sold) = (0.0, 0.0);
                    for code in resources {
                        bought += at("resourcesBought", Some(code));
                        sold += at("resourcesSold", Some(code));
                    }
                    percent(bought, sold)
                }
                code => Value::Parts(vec![
                    (Kind::Bought, at("resourcesBought", Some(code))),
                    (Kind::Sold, at("resourcesSold", Some(code))),
                ]),
            },
            Panel::Miscellaneous => match id {
                "killDeath" => ratio(
                    at("enemyUnitsKilled", Some("Unit")),
                    at("unitsLost", Some("Unit")),
                ),
                "population" => Value::Parts(vec![(Kind::Population, at("populationCount", None))]),
                "mapControlPeak" => Value::Parts(vec![(
                    Kind::Percent,
                    team_or_own("peakPercentMapControlled", "teamPeakPercentMapControlled"),
                )]),
                "mapControl" => Value::Parts(vec![(
                    Kind::Percent,
                    team_or_own("percentMapControlled", "teamPercentMapControlled"),
                )]),
                "mapExploration" => Value::Parts(vec![(
                    Kind::Percent,
                    team_or_own("percentMapExplored", "teamPercentMapExplored"),
                )]),
                "vegetarianRatio" => percent(
                    at("resourcesGathered", Some("vegetarianFood")),
                    at("resourcesGathered", Some("food")),
                ),
                "civilianization" => percent(
                    at("unitsTrained", Some("Civilian")),
                    at("unitsTrained", Some("Worker")),
                ),
                _ => Value::Parts(vec![
                    (Kind::Succeeded, at("successfulBribes", None)),
                    (Kind::Failed, at("failedBribes", None)),
                ]),
            },
        }
    }
}

/// What one chart is set to; `category` is a panel, `value` a heading of
/// it after the player name, `kind` a part of its value.
#[derive(Clone, Copy, Default)]
struct ChartSelection {
    category: usize,
    value: usize,
    kind: usize,
}

/// The controls and drawing area of one chart.
struct ChartSlot {
    category: HtmlSelectElement,
    value: HtmlSelectElement,
    kind: HtmlSelectElement,
    kind_label: HtmlElement,
    plot: HtmlElement,
    legend: HtmlElement,
    x_label: HtmlElement,
}

/// `#match-summary`.
pub struct SummaryWindow {
    document: Document,
    root: HtmlElement,
    title: HtmlElement,
    outcome: HtmlElement,
    elapsed: HtmlElement,
    map: HtmlElement,
    tabs: Vec<HtmlElement>,
    table: HtmlElement,
    charts: HtmlElement,
    team_toggle: HtmlInputElement,
    team_text: HtmlElement,
    team_label: HtmlElement,
    slots: [ChartSlot; 2],
    message: HtmlElement,
    close: HtmlElement,
    open: bool,
    /// Drops the answers of requests that were overtaken.
    serial: u32,
    data: Option<Data>,
    source: Option<Source>,
    /// The `turn` of the envelope shown.
    shown_turn: Option<u32>,
    /// The playhead of the last request; the live edge counts as the
    /// playhead at the time.
    asked_turn: Option<u32>,
    /// A request is on its way.
    in_flight: bool,
    tab: usize,
    selections: [ChartSelection; 2],
    team_charts: bool,
}

fn make<T: JsCast>(document: &Document, tag: &str, class: &str) -> Result<T, String> {
    let element: T = create(document, tag)?;
    if !class.is_empty() {
        let _ = element
            .unchecked_ref::<Element>()
            .set_attribute("class", class);
    }
    Ok(element)
}

fn add(
    parent: &impl AsRef<web_sys::Node>,
    child: &impl AsRef<web_sys::Node>,
) -> Result<(), String> {
    parent
        .as_ref()
        .append_child(child.as_ref())
        .map(|_| ())
        .map_err(|_| "cannot build the summary".to_string())
}

fn text_span(document: &Document, text: &str, class: &str) -> Result<HtmlElement, String> {
    let span: HtmlElement = make(document, "span", class)?;
    span.set_text_content(Some(text));
    Ok(span)
}

/// A number as JavaScript writes it.
fn js_number(value: f64) -> String {
    if value.is_infinite() {
        lang::translate("\u{221E}")
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn axis_number(value: f64, format: Format) -> String {
    match format {
        Format::Integer => format!("{:.0}", value.round()),
        Format::Percentage => format!("{:.0}%", value.round()),
        Format::Decimal2 => format!("{value:.2}"),
    }
}

/// Fills `parent` with `template`, translated, whose `%(name)s` are the
/// colored type names.
fn fill_template(document: &Document, parent: &Element, template: &str) -> Result<(), String> {
    let translated = lang::translate(template);
    let mut rest = translated.as_str();
    while let Some(start) = rest.find("%(") {
        let Some(length) = rest[start..].find(")s") else {
            break;
        };
        add(parent, &text_span(document, &rest[..start], "")?)?;
        let key = &rest[start + 2..start + length];
        let kind = [
            Kind::Trained,
            Kind::Constructed,
            Kind::Gathered,
            Kind::Sent,
            Kind::Received,
            Kind::Captured,
            Kind::Succeeded,
            Kind::Failed,
            Kind::Destroyed,
            Kind::Killed,
            Kind::Lost,
            Kind::Used,
        ]
        .into_iter()
        .find(|kind| kind.key() == key);
        match kind {
            Some(kind) => {
                let span = text_span(document, &lang::translate(kind.caption()), "")?;
                if let Some(color) = kind.color() {
                    let _ = span.set_attribute("style", &format!("color: {color}"));
                }
                add(parent, &span)?;
            }
            None => add(
                parent,
                &text_span(document, &rest[start..start + length + 2], "")?,
            )?,
        }
        rest = &rest[start + length + 2..];
    }
    add(parent, &text_span(document, rest, "")?)
}

fn fill_value(document: &Document, parent: &Element, value: &Value) -> Result<(), String> {
    match value {
        Value::Number(value) => add(parent, &text_span(document, &js_number(*value), "")?),
        Value::Parts(parts) => {
            for (kind, value) in parts {
                if kind.hidden() {
                    continue;
                }
                let span = text_span(document, &js_number(*value), "")?;
                if let Some(color) = kind.color() {
                    let _ = span.set_attribute("style", &format!("color: {color}"));
                }
                add(parent, &span)?;
                add(parent, &text_span(document, kind.postfix(), "")?)?;
            }
            Ok(())
        }
    }
}

fn clear(element: &Element) {
    element.set_text_content(None);
}

fn set_hidden(element: &HtmlElement, hidden: bool) {
    if element.hidden() != hidden {
        element.set_hidden(hidden);
    }
}

fn option(document: &Document, select: &HtmlSelectElement, label: &str, value: usize) {
    if let Ok(option) = create::<web_sys::HtmlOptionElement>(document, "option") {
        option.set_text_content(Some(label));
        option.set_value(&value.to_string());
        let _ = select.append_child(&option);
    }
}

impl SummaryWindow {
    pub fn new(document: &Document) -> Result<Rc<RefCell<Self>>, String> {
        let root: HtmlElement = crate::element(document, "match-summary")?;
        let tabs_host: HtmlElement = crate::element(document, "summary-tabs")?;
        let charts: HtmlElement = crate::element(document, "summary-charts")?;
        let mut tabs = Vec::new();
        for _ in 0..=PANELS.len() {
            let tab: HtmlElement = make(document, "button", "tab")?;
            let _ = tab.set_attribute("type", "button");
            let _ = tab.set_attribute("role", "tab");
            add(&tabs_host, &tab)?;
            tabs.push(tab);
        }

        let team_label: HtmlElement = make(document, "label", "team-toggle")?;
        let team_toggle: HtmlInputElement = make(document, "input", "")?;
        team_toggle.set_type("checkbox");
        let team_text: HtmlElement = make(document, "span", "")?;
        add(&team_label, &team_toggle)?;
        add(&team_label, &team_text)?;
        add(&charts, &team_label)?;
        let mut slots = Vec::new();
        for _ in 0..2 {
            let slot: HtmlElement = make(document, "div", "slot")?;
            let controls: HtmlElement = make(document, "div", "controls")?;
            let category: HtmlSelectElement = make(document, "select", "")?;
            let value: HtmlSelectElement = make(document, "select", "")?;
            let kind: HtmlSelectElement = make(document, "select", "")?;
            let kind_label: HtmlElement = make(document, "span", "")?;
            for select in [&category, &value] {
                add(&controls, select)?;
            }
            add(&controls, &kind_label)?;
            add(&controls, &kind)?;
            let plot: HtmlElement = make(document, "div", "plot")?;
            let x_label: HtmlElement = make(document, "div", "x-label")?;
            let legend: HtmlElement = make(document, "div", "legend")?;
            for part in [&controls, &plot, &x_label, &legend] {
                add(&slot, part)?;
            }
            add(&charts, &slot)?;
            slots.push(ChartSlot {
                category,
                value,
                kind,
                kind_label,
                plot,
                legend,
                x_label,
            });
        }
        let Ok(slots) = <[ChartSlot; 2]>::try_from(slots) else {
            return Err("two chart slots".to_string());
        };

        let title: HtmlElement = crate::element(document, "summary-title")?;
        let outcome: HtmlElement = crate::element(document, "summary-outcome")?;
        let elapsed: HtmlElement = crate::element(document, "summary-elapsed")?;
        let map: HtmlElement = crate::element(document, "summary-map")?;
        let table: HtmlElement = crate::element(document, "summary-table")?;
        let message: HtmlElement = crate::element(document, "summary-message")?;
        let close: HtmlElement = crate::element(document, "summary-close")?;
        let window = Rc::new({
            RefCell::new(SummaryWindow {
                document: document.clone(),
                root,
                title,
                outcome,
                elapsed,
                map,
                tabs,
                table,
                charts,
                team_toggle,
                team_text,
                team_label,
                slots,
                message,
                close,
                open: false,
                serial: 0,
                data: None,
                source: None,
                shown_turn: None,
                asked_turn: None,
                in_flight: false,
                tab: 0,
                selections: [
                    ChartSelection {
                        category: 0,
                        value: 0,
                        kind: 0,
                    },
                    ChartSelection {
                        category: 0,
                        value: 1,
                        kind: 0,
                    },
                ],
                team_charts: false,
            })
        });
        window.borrow().root.set_hidden(true);
        SummaryWindow::wire(&window);
        Ok(window)
    }

    pub fn close_button(&self) -> &HtmlElement {
        &self.close
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Shows the window; the caller then starts `refresh`.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.root.set_hidden(!open);
        if open {
            self.render();
        } else {
            // An answer on its way is not for a window that was closed.
            self.serial += 1;
        }
        self.in_flight = false;
    }

    /// Forgets another source's capture, so a stale one is not shown under
    /// the playhead this window opens at.
    pub fn prepare(&mut self, source: &Source) {
        if self.source.as_ref() != Some(source) {
            self.data = None;
            self.shown_turn = None;
        }
    }

    /// The language changed, so every label is made again.
    pub fn invalidate(&mut self) {
        if self.open {
            self.render();
        }
    }

    fn wire(window: &Rc<RefCell<SummaryWindow>>) {
        let tabs = window.borrow().tabs.clone();
        for (index, tab) in tabs.iter().enumerate() {
            let weak = Rc::downgrade(window);
            let handler = Closure::<dyn FnMut()>::new(move || {
                if let Some(window) = weak.upgrade()
                    && let Ok(mut window) = window.try_borrow_mut()
                {
                    window.tab = index;
                    window.render();
                }
            });
            tab.set_onclick(Some(handler.as_ref().unchecked_ref()));
            handler.forget();
        }
        let toggle = window.borrow().team_toggle.clone();
        let weak = Rc::downgrade(window);
        let handler = Closure::<dyn FnMut()>::new(move || {
            if let Some(window) = weak.upgrade()
                && let Ok(mut window) = window.try_borrow_mut()
            {
                window.team_charts = window.team_toggle.checked();
                window.draw_charts();
            }
        });
        toggle.set_onchange(Some(handler.as_ref().unchecked_ref()));
        handler.forget();
        for slot in 0..2 {
            for part in 0..3 {
                let select = {
                    let window = window.borrow();
                    let slot = &window.slots[slot];
                    [&slot.category, &slot.value, &slot.kind][part].clone()
                };
                let weak = Rc::downgrade(window);
                let handler = Closure::<dyn FnMut()>::new(move || {
                    if let Some(window) = weak.upgrade()
                        && let Ok(mut window) = window.try_borrow_mut()
                    {
                        window.chart_changed(slot, part);
                    }
                });
                select.set_onchange(Some(handler.as_ref().unchecked_ref()));
                handler.forget();
            }
        }
    }

    /// A chart's list `part` (category, value, type) was chosen: what
    /// follows it goes back to its first.
    fn chart_changed(&mut self, slot: usize, part: usize) {
        let chosen = |select: &HtmlSelectElement| select.value().parse::<usize>().unwrap_or(0);
        let ui = &self.slots[slot];
        let selection = &mut self.selections[slot];
        match part {
            0 => {
                selection.category = chosen(&ui.category);
                selection.value = 0;
                selection.kind = 0;
            }
            1 => {
                selection.value = chosen(&ui.value);
                selection.kind = 0;
            }
            _ => selection.kind = chosen(&ui.kind),
        }
        self.draw_charts();
    }

    fn set_data(&mut self, data: Option<Data>, source: Source) {
        self.data = data;
        self.source = Some(source);
        self.render();
    }

    /// Everything the open window shows.
    fn render(&mut self) {
        if !self.open {
            return;
        }
        if let Err(error) = self.build() {
            web_sys::console::error_1(&format!("summary: {error}").into());
        }
    }

    fn build(&mut self) -> Result<(), String> {
        let document = self.document.clone();
        lang::set_text(&self.title, "Summary");
        lang::set_text(&self.close, "Close");
        for (index, tab) in self.tabs.iter().enumerate() {
            let label = PANELS.get(index).map_or("Charts", |panel| panel.label());
            tab.set_text_content(Some(&lang::translate(label)));
            let _ = tab.set_attribute(
                "aria-selected",
                if index == self.tab { "true" } else { "false" },
            );
            let _ = tab.set_attribute(
                "class",
                if index == self.tab {
                    "tab selected"
                } else {
                    "tab"
                },
            );
        }
        let Some(data) = self.data.take() else {
            return self.show_message();
        };
        if data.samples() == 0 {
            self.data = Some(data);
            return self.show_message();
        }
        let result = self.build_data(&document, &data);
        self.data = Some(data);
        result
    }

    fn show_message(&self) -> Result<(), String> {
        self.message
            .set_text_content(Some(&lang::translate("No summary data available.")));
        set_hidden(&self.message, false);
        for part in [
            &self.outcome,
            &self.elapsed,
            &self.map,
            &self.table,
            &self.charts,
        ] {
            set_hidden(part, true);
        }
        Ok(())
    }

    fn build_data(&mut self, document: &Document, data: &Data) -> Result<(), String> {
        set_hidden(&self.message, true);
        for part in [&self.outcome, &self.elapsed, &self.map] {
            set_hidden(part, false);
        }

        self.outcome
            .set_text_content(Some(&self.outcome_text(data)));
        self.elapsed.set_text_content(Some(&lang::format(
            &lang::translate("Game time elapsed: %(time)s"),
            &[("time", &time_to_string(data.elapsed_ms))],
        )));
        let size = MAP_SIZES
            .iter()
            .find(|(tiles, _)| *tiles == data.map_size)
            .map(|(_, name)| lang::translate(name));
        let kind = size.unwrap_or_else(|| {
            let mut title = data.map_type.clone();
            if let Some(first) = title.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            lang::translate(&title)
        });
        self.map.set_text_content(Some(&lang::format(
            &lang::translate("%(mapName)s - %(mapType)s"),
            &[
                ("mapName", &lang::translate(&data.map_name)),
                ("mapType", &kind),
            ],
        )));

        let on_charts = self.tab >= PANELS.len();
        set_hidden(&self.table, on_charts);
        set_hidden(&self.charts, !on_charts);
        if on_charts {
            lang::set_text(&self.team_text, "Group by team");
            set_hidden(&self.team_label, data.teams.is_empty());
            self.team_toggle
                .set_checked(self.team_charts && !data.teams.is_empty());
            self.draw_charts_of(data);
        } else {
            self.draw_table(document, data, PANELS[self.tab])?;
        }
        Ok(())
    }

    fn outcome_text(&self, data: &Data) -> String {
        let (seat, running) = match &self.source {
            Some(Source::Play { seat, running, .. }) => (Some(*seat), *running),
            _ => (None, true),
        };
        let local = seat.and_then(|seat| data.players.iter().find(|player| player.id == seat));
        let text = if local.is_some_and(|player| player.won) {
            "You have won the battle!"
        } else if local.is_some_and(|player| player.defeated) {
            "You have been defeated\u{2026}"
        } else if data.is_final {
            "Scores at the end of the game."
        } else if running {
            "Current Scores"
        } else {
            "Last recorded scores"
        };
        lang::translate(text)
    }

    fn draw_table(&self, document: &Document, data: &Data, panel: Panel) -> Result<(), String> {
        clear(&self.table);
        let index = data.samples() - 1;
        let headings = panel.headings(&data.resources);
        let visible: Vec<&Heading> = headings.iter().filter(|heading| !heading.hidden).collect();
        if let Some(title) = panel.title() {
            let caption: HtmlElement = make(document, "div", "caption")?;
            fill_template(document, &caption, title)?;
            add(&self.table, &caption)?;
        }
        let scroll: HtmlElement = make(document, "div", "scroll")?;
        let table: HtmlElement = make(document, "table", "")?;
        let head: HtmlElement = make(document, "tr", "")?;
        let corner: HtmlElement = make(document, "th", "")?;
        corner.set_text_content(Some(&lang::translate("Player name")));
        add(&head, &corner)?;
        for heading in &visible {
            let cell: HtmlElement = make(document, "th", "")?;
            match heading.header {
                Some(template) => fill_template(document, &cell, template)?,
                None => cell.set_text_content(Some(&heading.caption)),
            }
            add(&head, &cell)?;
        }
        add(&table, &head)?;

        let player_row = |player: &SummaryPlayer| -> Result<HtmlElement, String> {
            let row: HtmlElement = make(document, "tr", "player")?;
            let _ = row.set_attribute(
                "style",
                &format!("background: {}", css_color(player.color, ROW_ALPHA)),
            );
            let name_cell: HtmlElement = make(document, "td", "name")?;
            let icons: HtmlElement = make(document, "span", "icons")?;
            let outcome: HtmlElement = make(document, "span", "outcome")?;
            if player.won || player.defeated {
                let (icon, label) = if player.won {
                    ("victory", "Victorious")
                } else {
                    ("defeat", "Defeated")
                };
                let _ =
                    outcome.set_attribute("style", &format!("background-image: var(--gui-{icon})"));
                let _ = outcome.set_attribute("title", &lang::translate(label));
            }
            let emblem: HtmlElement = make(document, "span", "emblem")?;
            let _ = emblem.set_attribute(
                "style",
                &format!("background-image: var(--gui-emblem-{})", player.civ),
            );
            let _ = emblem.set_attribute("title", &civ_name(&player.civ));
            add(&icons, &outcome)?;
            add(&icons, &emblem)?;
            add(&name_cell, &icons)?;
            add(&name_cell, &text_span(document, &player.name, "")?)?;
            add(&row, &name_cell)?;
            for heading in &visible {
                let cell: HtmlElement = make(document, "td", "")?;
                let value = data.value(panel, &heading.id, &[player], false, index);
                fill_value(document, &cell, &value)?;
                add(&row, &cell)?;
            }
            Ok(row)
        };
        for player in data.players.iter().filter(|player| player.team < 0) {
            add(&table, &player_row(player)?)?;
        }
        for (_, members) in &data.teams {
            let players: Vec<&SummaryPlayer> = members
                .iter()
                .filter_map(|&member| data.players.get(member))
                .collect();
            for player in &players {
                add(&table, &player_row(player)?)?;
            }
            let row: HtmlElement = make(document, "tr", "team")?;
            let label: HtmlElement = make(document, "td", "name")?;
            lang::set_text(&label, "Team total");
            add(&row, &label)?;
            for heading in &visible {
                let cell: HtmlElement = make(document, "td", "")?;
                let value = data.value(panel, &heading.id, &players, true, index);
                fill_value(document, &cell, &value)?;
                add(&row, &cell)?;
            }
            add(&table, &row)?;
        }
        add(&scroll, &table)?;
        add(&self.table, &scroll)
    }

    /// Both charts, from the current data and selections.
    fn draw_charts(&mut self) {
        let Some(data) = self.data.take() else {
            return;
        };
        self.draw_charts_of(&data);
        self.data = Some(data);
    }

    fn draw_charts_of(&mut self, data: &Data) {
        for slot in 0..2 {
            if let Err(error) = self.draw_chart(data, slot) {
                web_sys::console::error_1(&format!("summary chart: {error}").into());
            }
        }
    }

    fn draw_chart(&mut self, data: &Data, slot: usize) -> Result<(), String> {
        let document = self.document.clone();
        let teams_on = self.team_charts && !data.teams.is_empty();
        let ui = &self.slots[slot];
        let selection = &mut self.selections[slot];
        selection.category = selection.category.min(PANELS.len() - 1);
        let panel = PANELS[selection.category];
        let headings = panel.headings(&data.resources);
        selection.value = selection.value.min(headings.len().saturating_sub(1));
        let heading = &headings[selection.value];

        // What the lists hold now.
        clear(&ui.category);
        for (index, panel) in PANELS.iter().enumerate() {
            option(
                &document,
                &ui.category,
                &lang::translate(panel.label()),
                index,
            );
        }
        ui.category.set_value(&selection.category.to_string());
        clear(&ui.value);
        for (index, heading) in headings.iter().enumerate() {
            option(&document, &ui.value, &heading.caption, index);
        }
        ui.value.set_value(&selection.value.to_string());

        let Some(first) = data.players.first() else {
            return Ok(());
        };
        let kinds: Vec<Kind> = match data.value(panel, &heading.id, &[first], false, 0) {
            Value::Parts(parts) => parts.iter().map(|(kind, _)| *kind).collect(),
            Value::Number(_) => Vec::new(),
        };
        let typed = kinds.len() >= 2;
        set_hidden(&ui.kind, !typed);
        set_hidden(&ui.kind_label, !typed);
        if typed {
            lang::set_text(&ui.kind_label, "Chart type");
            clear(&ui.kind);
            for (index, kind) in kinds.iter().enumerate() {
                option(&document, &ui.kind, &lang::translate(kind.caption()), index);
            }
            selection.kind = selection.kind.min(kinds.len() - 1);
            ui.kind.set_value(&selection.kind.to_string());
        } else {
            selection.kind = 0;
        }
        let kind_index = if typed { selection.kind } else { 0 };

        // One series per player, or per team.
        struct Series {
            name: String,
            color: u32,
            points: Vec<f64>,
        }
        let mut series = Vec::new();
        if teams_on {
            for (_, members) in &data.teams {
                let players: Vec<&SummaryPlayer> = members
                    .iter()
                    .filter_map(|&member| data.players.get(member))
                    .collect();
                let Some(lead) = players.first() else {
                    continue;
                };
                let names: Vec<&str> = players.iter().map(|player| player.name.as_str()).collect();
                series.push(Series {
                    name: names.join(", "),
                    color: lead.color,
                    points: (0..data.samples())
                        .map(|index| {
                            data.value(panel, &heading.id, &players, true, index)
                                .pick(kind_index)
                        })
                        .collect(),
                });
            }
        } else {
            for player in &data.players {
                series.push(Series {
                    name: player.name.clone(),
                    color: player.color,
                    points: (0..data.samples())
                        .map(|index| {
                            data.value(panel, &heading.id, &[player], false, index)
                                .pick(kind_index)
                        })
                        .collect(),
                });
            }
        }

        // The plot.
        clear(&ui.plot);
        let svg = document
            .create_element_ns(Some(SVG), "svg")
            .map_err(|_| "no SVG".to_string())?;
        let width = CHART_WIDTH - CHART_LEFT - CHART_RIGHT;
        let height = CHART_HEIGHT - CHART_TOP - CHART_BOTTOM;
        let _ = svg.set_attribute("viewBox", &format!("0 0 {CHART_WIDTH} {CHART_HEIGHT}"));
        let _ = svg.set_attribute("class", "chart");
        let _ = svg.set_attribute("role", "img");
        let _ = svg.set_attribute("aria-label", &heading.caption);
        let x_min = data.time.first().copied().unwrap_or(0.0);
        let x_max = data.time.last().copied().unwrap_or(0.0);
        let x_span = if x_max > x_min { x_max - x_min } else { 1.0 };
        let y_max = series
            .iter()
            .flat_map(|series| series.points.iter().copied())
            .filter(|value| value.is_finite())
            .fold(0.0_f64, f64::max);
        let y_max = if y_max > 0.0 { y_max } else { 1.0 };
        let x_of = |seconds: f64| CHART_LEFT + (seconds - x_min) / x_span * width;
        let y_of = |value: f64| CHART_TOP + height - value.clamp(0.0, y_max) / y_max * height;
        let line = |x1: f64, y1: f64, x2: f64, y2: f64, class: &str| -> Result<Element, String> {
            let line = document
                .create_element_ns(Some(SVG), "line")
                .map_err(|_| "no SVG".to_string())?;
            for (name, value) in [("x1", x1), ("y1", y1), ("x2", x2), ("y2", y2)] {
                let _ = line.set_attribute(name, &format!("{value:.1}"));
            }
            let _ = line.set_attribute("class", class);
            Ok(line)
        };
        let label = |x: f64, y: f64, anchor: &str, text: &str| -> Result<Element, String> {
            let label = document
                .create_element_ns(Some(SVG), "text")
                .map_err(|_| "no SVG".to_string())?;
            let _ = label.set_attribute("x", &format!("{x:.1}"));
            let _ = label.set_attribute("y", &format!("{y:.1}"));
            let _ = label.set_attribute("text-anchor", anchor);
            label.set_text_content(Some(text));
            Ok(label)
        };
        for tick in 0..=TICKS {
            let fraction = tick as f64 / TICKS as f64;
            let y = y_of(y_max * fraction);
            add(&svg, &line(CHART_LEFT, y, CHART_LEFT + width, y, "grid")?)?;
            add(
                &svg,
                &label(
                    CHART_LEFT - 4.0,
                    y + 3.0,
                    "end",
                    &axis_number(y_max * fraction, heading.format),
                )?,
            )?;
            let seconds = x_min + x_span * fraction;
            add(
                &svg,
                &label(
                    x_of(seconds),
                    CHART_TOP + height + 14.0,
                    "middle",
                    &time_to_string(seconds * 1000.0),
                )?,
            )?;
        }
        for series in &series {
            // A break in the line wherever the value is not a number.
            let mut run: Vec<String> = Vec::new();
            let flush = |run: &mut Vec<String>| -> Result<(), String> {
                if run.is_empty() {
                    return Ok(());
                }
                let shape = document
                    .create_element_ns(
                        Some(SVG),
                        if run.len() == 1 { "circle" } else { "polyline" },
                    )
                    .map_err(|_| "no SVG".to_string())?;
                if run.len() == 1 {
                    let (x, y) = run[0].split_once(',').unwrap_or(("0", "0"));
                    let _ = shape.set_attribute("cx", x);
                    let _ = shape.set_attribute("cy", y);
                    let _ = shape.set_attribute("r", "2");
                    let _ = shape
                        .set_attribute("style", &format!("fill: {}", css_color(series.color, 1.0)));
                } else {
                    let _ = shape.set_attribute("points", &run.join(" "));
                    let _ = shape.set_attribute("fill", "none");
                    let _ = shape.set_attribute(
                        "style",
                        &format!("stroke: {}", css_color(series.color, 1.0)),
                    );
                }
                add(&svg, &shape)?;
                run.clear();
                Ok(())
            };
            for (index, value) in series.points.iter().enumerate() {
                if value.is_finite() {
                    run.push(format!("{:.1},{:.1}", x_of(data.time[index]), y_of(*value)));
                } else {
                    flush(&mut run)?;
                }
            }
            flush(&mut run)?;
        }
        add(&ui.plot, &svg)?;

        lang::set_text(&ui.x_label, "Time elapsed");
        clear(&ui.legend);
        for series in &series {
            let item: HtmlElement = make(&document, "span", "item")?;
            let swatch = text_span(&document, "\u{25A0}", "swatch")?;
            let _ =
                swatch.set_attribute("style", &format!("color: {}", css_color(series.color, 1.0)));
            add(&item, &swatch)?;
            add(&item, &text_span(&document, &series.name, "")?)?;
            add(&ui.legend, &item)?;
        }
        Ok(())
    }
}

/// Asks for the capture of `source` and shows it, unless the window was
/// closed or asked again meanwhile. `playhead` is what the answer covers.
pub fn refresh(window: &Rc<RefCell<SummaryWindow>>, source: Source, playhead: u32) {
    let serial = {
        let mut window = window.borrow_mut();
        window.serial += 1;
        window.in_flight = true;
        window.asked_turn = Some(playhead);
        window.serial
    };
    let window = Rc::clone(window);
    wasm_bindgen_futures::spawn_local(async move {
        let result = fetch(&source).await;
        let Ok(mut window) = window.try_borrow_mut() else {
            return;
        };
        // Whatever the answer was, the next request may go out; a window
        // closed or asked again meanwhile has its own.
        if window.serial != serial || !window.open {
            return;
        }
        window.in_flight = false;
        match result {
            Ok(data) => {
                window.shown_turn = data.as_ref().map(|data| data.turn);
                window.set_data(data, source);
            }
            Err(error) => web_sys::console::error_1(&format!("summary: {error}").into()),
        }
    });
}

/// Asks for a new capture when the open window's one no longer covers the
/// playhead. There is no timer: a still playhead causes no request.
pub fn follow(window: &Rc<RefCell<SummaryWindow>>, source: Source, playhead: u32, at_end: bool) {
    let due = {
        let state = match window.try_borrow() {
            Ok(state) => state,
            Err(_) => return,
        };
        if !state.open || state.in_flight {
            return;
        }
        let shown = state.shown_turn;
        let asked = state.asked_turn;
        // The next capture is due 150 turns after the one shown, whenever
        // the window asked for it; counting from the request instead made
        // an answer wait up to a whole interval for the playhead.
        let Some(base) = shown.or(asked) else {
            return;
        };
        (at_end && asked.is_none_or(|asked| asked < playhead))
            // Only a playhead behind the live edge is asked for its own
            // capture; the live one is always the newest, which may be
            // ahead of the playhead.
            || (source.turn().is_some() && shown.is_some_and(|shown| playhead < shown))
            // A late capture is asked for again at most once a second.
            || (playhead >= base.saturating_add(CAPTURE_TURNS)
                && (shown.is_none()
                    || asked.is_none_or(|asked| playhead >= asked.saturating_add(RETRY_TURNS))))
    };
    if due {
        refresh(window, source, playhead);
    }
}

/// `None` when the server has no capture at or before the playhead.
async fn fetch(source: &Source) -> Result<Option<Data>, String> {
    let Some(url) = source.url() else {
        return Ok(None);
    };
    let window = web_sys::window().ok_or("no window")?;
    let response: Response = JsFuture::from(window.fetch_with_str(&url))
        .await
        .map_err(|_| "cannot ask for the summary")?
        .dyn_into()
        .map_err(|_| "not a response")?;
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!("the summary: HTTP {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(|_| "no body")?)
        .await
        .map_err(|_| "cannot read the summary")?;
    Ok(Data::parse(&text.as_string().unwrap_or_default()))
}
