//! What a placement check reads of a template, taken once from its kind 18
//! JSON (`GetTemplateData`, plus the hook's `rangeOverlay` and
//! `speculaPieces`).

use wasm_bindgen::JsValue;

use super::GhostRange;
use crate::tips;

#[derive(Clone, Copy, Default, PartialEq)]
pub enum Footprint {
    #[default]
    None,
    Square {
        width: f64,
        depth: f64,
    },
    Circle {
        radius: f64,
    },
}

#[derive(Clone)]
pub struct Distance {
    pub from_class: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Clone, Default)]
pub struct PlaceData {
    /// Identity's generic name, as the template has it.
    pub generic_name: String,
    /// BuildRestrictions' PlacementType.
    pub placement_type: String,
    /// The tokens of BuildRestrictions' Territory.
    pub territory: Vec<String>,
    pub distance: Option<Distance>,
    /// The static obstruction's width and depth.
    pub obstruction: Option<(f64, f64)>,
    pub footprint: Footprint,
    /// The attack range circles the preview draws.
    pub ranges: Vec<GhostRange>,
    /// A wall piece's `wallPiece.length`, 0 for another template.
    pub wall_length: f64,
    /// What the piece costs, by resource code and then population and time.
    pub cost: Vec<(String, f64)>,
    pub wall: Option<WallSetData>,
}

/// One kind of piece of a wall set.
#[derive(Clone)]
pub struct PieceData {
    pub template: String,
    pub actor: String,
    pub data: Box<PlaceData>,
}

#[derive(Clone)]
pub struct WallSetData {
    pub max_overlap: f64,
    pub min_overlap: f64,
    pub tower: PieceData,
    pub long: Option<PieceData>,
    pub medium: Option<PieceData>,
    pub short: Option<PieceData>,
}

impl PlaceData {
    pub fn parse(json: &JsValue) -> Self {
        let restrictions = tips::field(json, "buildRestrictions");
        let distance = tips::field(&restrictions, "distance");
        let shape = tips::field(&tips::field(json, "obstruction"), "shape");
        let footprint = tips::field(json, "footprint");
        let ranged = tips::field(&tips::field(json, "attack"), "Ranged");
        let overlay = tips::field(json, "rangeOverlay");
        let wall_set = tips::field(json, "wallSet");
        let optional = |value: &JsValue, key: &str| {
            let field = tips::field(value, key);
            (!field.is_undefined() && !field.is_null()).then(|| tips::number_field(value, key))
        };
        let mut ranges = Vec::new();
        if overlay.is_object() {
            for radius in [
                tips::number_field(&ranged, "maxRange"),
                tips::number_field(&ranged, "minRange"),
            ] {
                let texture = tips::text_field(&overlay, "texture");
                if radius > 0.0 && !texture.is_empty() {
                    ranges.push(GhostRange {
                        radius: radius as f32,
                        texture,
                        thickness: tips::number_field(&overlay, "thickness") as f32,
                    });
                }
            }
        }
        let cost = tips::field(json, "cost");
        PlaceData {
            generic_name: tips::text_field(&tips::field(json, "name"), "generic"),
            placement_type: tips::text_field(&restrictions, "placementType"),
            territory: tips::text_field(&restrictions, "territory")
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            distance: distance.is_object().then(|| Distance {
                from_class: tips::text_field(&distance, "fromClass"),
                min: optional(&distance, "min"),
                max: optional(&distance, "max"),
            }),
            obstruction: (tips::text_field(&shape, "type") == "static").then(|| {
                (
                    tips::number_field(&shape, "width"),
                    tips::number_field(&shape, "depth"),
                )
            }),
            footprint: if tips::field(&footprint, "square").is_object() {
                let square = tips::field(&footprint, "square");
                Footprint::Square {
                    width: tips::number_field(&square, "width"),
                    depth: tips::number_field(&square, "depth"),
                }
            } else if tips::field(&footprint, "circle").is_object() {
                Footprint::Circle {
                    radius: tips::number_field(&tips::field(&footprint, "circle"), "radius"),
                }
            } else {
                Footprint::None
            },
            ranges,
            wall_length: tips::number_field(&tips::field(json, "wallPiece"), "length"),
            cost: if cost.is_object() {
                js_sys::Object::keys(&js_sys::Object::from(cost.clone()))
                    .iter()
                    .filter_map(|key| key.as_string())
                    .map(|key| {
                        let amount = tips::number_field(&cost, &key);
                        (key, amount)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            wall: wall_set
                .is_object()
                .then(|| WallSetData::parse(json, &wall_set)),
        }
    }
}

impl WallSetData {
    /// A piece the hook did not send is left out; an absent tower has no
    /// length, so no wall is laid.
    fn parse(json: &JsValue, wall_set: &JsValue) -> Self {
        let templates = tips::field(wall_set, "templates");
        let pieces = tips::field(json, "speculaPieces");
        let piece = |kind: &str| {
            let template = tips::text_field(&templates, kind);
            let data = tips::field(&pieces, &template);
            (!template.is_empty() && data.is_object()).then(|| PieceData {
                template,
                actor: tips::text_field(&data, "actor"),
                data: Box::new(PlaceData::parse(&data)),
            })
        };
        WallSetData {
            max_overlap: tips::number_field(wall_set, "maxTowerOverlap"),
            min_overlap: tips::number_field(wall_set, "minTowerOverlap"),
            tower: piece("tower").unwrap_or_else(|| PieceData {
                template: tips::text_field(&templates, "tower"),
                actor: String::new(),
                data: Box::default(),
            }),
            long: piece("long"),
            medium: piece("medium"),
            short: piece("short"),
        }
    }
}
