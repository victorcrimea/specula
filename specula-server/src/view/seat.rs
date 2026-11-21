//! A seat's stream: the match as the engine shows it to the player a web
//! visitor plays, and nothing more (CONTROL-IMPL C31). It is the
//! observers' stream run through a filter and encoded on its own, with its
//! own tables, so that neither an index nor an id says what the player was
//! not shown:
//! - entities whose `GetLosVisibility` for the player is not HIDDEN, as
//!   `CCmpUnitRenderer` draws them; a mirage only for its own player;
//! - ids are the seat's handles: a unit that leaves sight and comes back
//!   gets a new one, a building and its mirage share one;
//! - no actor seed (by default the engine id): the variants the engine
//!   picks from it are sent instead;
//! - the owner's and a mutual ally's entities have every detail group,
//!   anyone else's what vanilla's session GUI shows of them;
//! - the LOS of the player; the terrain, territory and shore waves where
//!   the LOS texture is not black; sounds the player hears; projectiles
//!   from when they come into sight;
//! - a player who won watches as an observer does, as in vanilla; a defeated
//!   one sees as its lowest-numbered active mutual ally (D47), or only what
//!   it explored once none is left.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::time::Duration;

use specula_assets::shore;
use specula_assets::shore::Heightmap;
use specula_assets::shore::TILE_SIZE;
use specula_assets::shore::Wave;
use specula_assets::variation;
use specula_assets::variation::Selections;

use super::Common;
use super::Dot;
use super::Funds;
use super::LaunchExtra;
use super::Los;
use super::Release;
use super::ShotOrigin;
use super::Shown;
use super::Stream;
use super::Template;
use crate::actors::Actors;
use crate::feed::DataKey;
use crate::feed::Details;
use crate::feed::Entity;
use crate::feed::EntityDetails;
use crate::feed::Garrison;
use crate::feed::HIDDEN;
use crate::feed::LOS_EXPLORED;
use crate::feed::LOS_VISIBLE;
use crate::feed::Launch;
use crate::feed::Mirage;
use crate::feed::PlayerGui;
use crate::feed::Players;
use crate::feed::ProductionItem;
use crate::feed::Sound;
use crate::feed::SoundTarget;
use crate::feed::TemplateDetails;
use crate::feed::Terrain;
use crate::feed::Territory;
use crate::feed::UnitAi;
use crate::feed::VISIBLE;
use crate::feed::Visual;
use crate::feed::visibility_of;
use crate::terrain;
use crate::terrain::Ground;
use crate::wire::Writer;

/// The players record's stances.
const STANCE_ALLY: u8 = 1;
/// A stance between two other players, which the seat is not told.
pub const STANCE_UNKNOWN: u8 = 255;
const PLAYER_ACTIVE: u8 = 0;
const PLAYER_DEFEATED: u8 = 1;
const PLAYER_WON: u8 = 2;
/// A territory tile the seat has not been shown.
pub const TERRITORY_UNKNOWN: u8 = 0x80;
/// Territory tiles per side are this many terrain tiles.
const TERRAIN_TILES_PER_TERRITORY_TILE: usize = 2;
/// `CLOSTexture`'s 7-tap blur, and the texel value its `getLOS` turns
/// black below: 0.03 of 255, so 7 is black and 8 is not.
const BLUR: [u32; 7] = [1, 6, 15, 20, 15, 6, 1];
const BLACK_BELOW: u8 = 8;
const LOS_TEXEL_VISIBLE: u32 = 255;
const LOS_TEXEL_EXPLORED: u32 = 127;
/// `TERRAIN_TILE_SIZE` over `LOS_TILE_SIZE`: both are 4 m, so an LOS
/// vertex is a terrain vertex.
const LOS_TILE_SIZE: f32 = 4.0;
/// Attack.js plays this animation while it shoots, which a projectile's
/// prop is found in (the client's `SHOOTING_ANIMATION`).
const SHOOTING_ANIMATION: &str = "attack_ranged";
/// How finely a projectile's flight is checked against the LOS, in
/// metres.
const FLIGHT_STEP: f64 = 0.5;
const TURN_SECS: f64 = 0.2;
/// `PROJECTILE_DECAY_TIME` and the keep margin `view.rs` gives a shot.
const PROJECTILE_KEEP_SECS: f64 = 35.0;

/// What the observers' encoding has of a turn, which every seat reads.
pub(super) struct SeatInput<'a> {
    pub turn: u32,
    pub entities: &'a [Entity],
    pub details: &'a Details,
    /// The observers' stream before it encodes the turn: its tables are
    /// the feed's, and it holds the turn's players, territory, economy,
    /// sounds and projectiles, and last turn's dots.
    pub feed: &'a Stream,
    pub los: &'a Los,
    pub terrain: Option<&'a Terrain>,
    pub water: Option<&'a SeatWater>,
    pub ground: Option<&'a Ground>,
}

/// What the engine's water makes of the whole heightmap, once per match.
pub(super) struct SeatWater {
    pub wind: Vec<f32>,
    pub waves: Vec<Wave>,
}

impl SeatWater {
    pub fn new(terrain: &Terrain) -> Self {
        let map = Heightmap {
            size: terrain.tiles_per_side,
            heights: &terrain.heights,
        };
        let level = terrain.water_level as f32 / 65536.0;
        let water = &terrain.water;
        SeatWater {
            wind: shore::wind_strength(&map, level, water.wind_angle),
            waves: shore::waves(&map, level, water.waviness, &water.kind),
        }
    }
}

pub(super) struct Seat {
    /// The player index.
    pub player: u32,
    pub shown: Shown,
    stream: Stream,
    handles: Handles,
    local: Local,
    revealed: Revealed,
    /// The `/data` hashes the stream has named.
    named: HashSet<u64>,
    /// Won: from then on the seat watches as an observer, as the engine
    /// reveals the map to a winner (`Player.SetState`'s `SetLosRevealAll`).
    won: bool,
    /// The player whose vision the seat is shown: its own, or a defeated
    /// seat's team proxy (`team_eyes`).
    eyes: u32,
    /// The seat's detail state as the detail encoder reads it: lists, data
    /// and the player table only grow.
    details: Details,
    /// The ids that had groups last turn.
    detailed: BTreeSet<u32>,
    /// Projectiles not yet in sight, and those sent, by the feed's id.
    pending: BTreeMap<u32, Pending>,
    sent_shots: HashSet<u32>,
    /// The choices last worked out for an entity, by the feed's id.
    choices: HashMap<u32, (ChoiceKey, u32)>,
    /// Last turn's mirages and their parents, by the feed's ids.
    held_mirages: HashMap<u32, u32>,
}

/// A projectile launched while out of the seat's sight.
struct Pending {
    launch: Launch,
    launch_turn: u32,
    last_turn: u32,
    origin: Option<ShotOrigin>,
    chosen: u32,
}

/// What an entity's variant choices are worked out from.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ChoiceKey {
    actor: String,
    selections: Selections,
    seed: u32,
}

/// The seat's handles for the feed's ids, both ways. Handles are never
/// reused, so a stale one still names the entity it last stood for.
#[derive(Default)]
struct Handles {
    to_local: HashMap<u32, u32>,
    /// Every handle issued this match, kept after its id loses it: the page
    /// draws behind the newest turn, so its orders may name a unit that
    /// just died or left sight, and the engine filters those as vanilla's
    /// `ProcessCommand` and UnitAI do rather than the whole order going.
    to_feed: HashMap<u32, u32>,
    next: u32,
    /// The ids referred to this turn; the others lose their handles.
    referenced: HashSet<u32>,
}

impl Handles {
    fn get(&self, id: u32) -> Option<u32> {
        self.to_local.get(&id).copied()
    }

    /// The handle of `id`, a new one if it has none; the id counts as
    /// referred to.
    fn of(&mut self, id: u32) -> u32 {
        self.referenced.insert(id);
        if let Some(handle) = self.to_local.get(&id) {
            return *handle;
        }
        self.next += 1;
        self.to_local.insert(id, self.next);
        self.to_feed.insert(self.next, id);
        self.next
    }

    fn transfer(&mut self, from: u32, to: u32) {
        if let Some(handle) = self.to_local.remove(&from) {
            self.to_local.insert(to, handle);
            self.to_feed.insert(handle, to);
            self.referenced.insert(to);
        }
    }

    /// Forgets the ids not referred to this turn.
    fn end_turn(&mut self) {
        let referenced = std::mem::take(&mut self.referenced);
        self.to_local.retain(|id, _| referenced.contains(id));
    }
}

/// The seat's tables against the feed's.
#[derive(Default)]
struct Local {
    strings: HashMap<String, u32>,
    templates: HashMap<u32, u32>,
    variant_sets: HashMap<u32, u32>,
    variable_sets: HashMap<u32, u32>,
    lists: HashMap<u32, u32>,
}

impl Local {
    /// The seat's 0-based index of a text, added if new.
    fn text(&mut self, stream: &mut Stream, text: &str) -> u32 {
        if let Some(index) = self.strings.get(text) {
            return *index;
        }
        let index = stream.strings.items.len() as u32;
        stream.strings.items.push(text.to_string());
        self.strings.insert(text.to_string(), index);
        index
    }

    /// The seat's index of the feed's 0-based string.
    fn string(&mut self, feed: &Stream, stream: &mut Stream, index: u32) -> u32 {
        match feed.strings.items.get(index as usize) {
            Some(text) => self.text(stream, text),
            None => index,
        }
    }

    fn optional(&mut self, feed: &Stream, stream: &mut Stream, index: Option<u32>) -> Option<u32> {
        index.map(|index| self.string(feed, stream, index))
    }

    /// 1-based, 0 for none, as the stream sends it.
    fn one_based(&mut self, feed: &Stream, stream: &mut Stream, index: u32) -> u32 {
        match index.checked_sub(1) {
            Some(index) => self.string(feed, stream, index) + 1,
            None => 0,
        }
    }

    fn template(&mut self, feed: &Stream, stream: &mut Stream, index: u32) -> u32 {
        if let Some(local) = self.templates.get(&index) {
            return *local;
        }
        let Some(template) = feed.templates.items.get(index as usize) else {
            return index;
        };
        let details = template
            .details
            .as_ref()
            .map(|details| self.template_details(feed, stream, details));
        let local = stream.templates.items.len() as u32;
        stream.templates.items.push(Template {
            name: template.name.clone(),
            details,
        });
        self.templates.insert(index, local);
        if let Some(icon) = feed.template_icons.get(&index) {
            let icon = feed.icons.items[*icon as usize - 1].clone();
            let local_icon = stream.intern_icon(icon);
            stream.template_icons.insert(local, local_icon);
        }
        if feed.player_color_templates.contains(&index) {
            stream.player_color_templates.insert(local);
        }
        local
    }

    fn template_details(
        &mut self,
        feed: &Stream,
        stream: &mut Stream,
        details: &TemplateDetails,
    ) -> TemplateDetails {
        let mut local = details.clone();
        for field in [
            &mut local.overlay_texture,
            &mut local.overlay_mask,
            &mut local.generic_name,
            &mut local.specific_name,
            &mut local.icon,
            &mut local.group_name,
            &mut local.rank,
            &mut local.classes,
            &mut local.visible_classes,
            &mut local.select_sound,
            &mut local.lang,
            &mut local.phenotypes,
            &mut local.rally_texture,
            &mut local.rally_mask,
            &mut local.supply_type,
            &mut local.gather_rates,
            &mut local.dropsite_types,
            &mut local.attack_types,
            &mut local.order_sounds,
            &mut local.garrison_classes,
            &mut local.turret_classes,
        ] {
            *field = self.optional(feed, stream, *field);
        }
        local
    }

    fn variant_set(&mut self, feed: &Stream, stream: &mut Stream, index: u32) -> u32 {
        if let Some(local) = self.variant_sets.get(&index) {
            return *local;
        }
        let pairs: Vec<(u32, u32)> = feed
            .variant_sets
            .items
            .get(index as usize)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, selection)| {
                (
                    self.string(feed, stream, key),
                    self.string(feed, stream, selection),
                )
            })
            .collect();
        let local = stream.variant_sets.items.len() as u32;
        stream.variant_sets.items.push(pairs);
        self.variant_sets.insert(index, local);
        local
    }

    fn variable_set(&mut self, feed: &Stream, stream: &mut Stream, index: u32) -> u32 {
        if let Some(local) = self.variable_sets.get(&index) {
            return *local;
        }
        let pairs: Vec<(u32, f32)> = feed
            .variable_sets
            .items
            .get(index as usize)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| (self.string(feed, stream, name), value))
            .collect();
        let local = stream.variable_sets.items.len() as u32;
        stream.variable_sets.items.push(pairs);
        self.variable_sets.insert(index, local);
        local
    }

    fn visual(&mut self, feed: &Stream, stream: &mut Stream, visual: &Visual) -> Visual {
        Visual {
            actor: self.optional(feed, stream, visual.actor),
            variants: visual
                .variants
                .map(|index| self.variant_set(feed, stream, index)),
            variables: visual
                .variables
                .map(|index| self.variable_set(feed, stream, index)),
            animation: self.optional(feed, stream, visual.animation),
            sound_group: self.optional(feed, stream, visual.sound_group),
            // The seat gets the choices the seed makes instead.
            seed: 0,
            ..*visual
        }
    }

    /// A list as the seat's detail stream sends it: its strings the
    /// seat's.
    fn list(
        &mut self,
        feed: &Stream,
        stream: &mut Stream,
        lists: &mut HashMap<u32, Vec<ProductionItem>>,
        details: &Details,
        index: u32,
    ) -> u32 {
        if let Some(local) = self.lists.get(&index) {
            return *local;
        }
        let items = details
            .lists
            .get(&index)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|item| ProductionItem {
                name: self.optional(feed, stream, item.name),
                costs: item
                    .costs
                    .iter()
                    .map(|(resource, cost)| (self.optional(feed, stream, *resource), *cost))
                    .collect(),
                ..item
            })
            .collect();
        let local = lists.len() as u32;
        lists.insert(local, items);
        self.lists.insert(index, local);
        local
    }
}

/// What the seat holds of the terrain.
struct Revealed {
    /// Tiles per side.
    size: usize,
    /// By vertex and by tile: sent.
    vertices: Vec<bool>,
    tiles: Vec<bool>,
    /// The feed's palette index to the seat's.
    palette: HashMap<u16, u16>,
    /// By the feed's wave: the seat's number for it, and the columns sent.
    waves: Vec<Option<(u32, Vec<bool>)>>,
    next_wave: u32,
    /// The LOS the reveal was last worked out from.
    from: Option<Vec<u8>>,
}

impl Revealed {
    fn new() -> Self {
        Revealed {
            size: 0,
            vertices: Vec::new(),
            tiles: Vec::new(),
            palette: HashMap::new(),
            waves: Vec::new(),
            next_wave: 0,
            from: None,
        }
    }

    /// The tiles the engine draws not black with this LOS: a tile's
    /// fragments sample the LOS texture between its corner texels, so one
    /// is drawn when a corner's blurred value is at least `BLACK_BELOW`.
    /// `None`: every tile.
    fn needed(size: usize, los: Option<&[u8]>) -> Vec<bool> {
        let side = size + 1;
        let Some(los) = los.filter(|los| los.len() == side * side) else {
            return vec![true; size * size];
        };
        let texel = |state: u8| {
            if state & LOS_VISIBLE != 0 {
                LOS_TEXEL_VISIBLE
            } else if state & LOS_EXPLORED != 0 {
                LOS_TEXEL_EXPLORED
            } else {
                0
            }
        };
        // `GenerateBitmap`: horizontal then vertical, each pass truncated
        // to a byte, zero outside the grid.
        let mut horizontal = vec![0u32; side * side];
        for j in 0..side {
            for i in 0..side {
                let sum: u32 = BLUR
                    .iter()
                    .enumerate()
                    .filter_map(|(k, weight)| {
                        let x = (i + k).checked_sub(3).filter(|x| *x < side)?;
                        Some(weight * texel(los[j * side + x]))
                    })
                    .sum();
                horizontal[j * side + i] = sum / 64;
            }
        }
        let mut lit = vec![false; side * side];
        for j in 0..side {
            for i in 0..side {
                let sum: u32 = BLUR
                    .iter()
                    .enumerate()
                    .filter_map(|(k, weight)| {
                        let z = (j + k).checked_sub(3).filter(|z| *z < side)?;
                        Some(weight * horizontal[z * side + i])
                    })
                    .sum();
                lit[j * side + i] = sum / 64 >= u32::from(BLACK_BELOW);
            }
        }
        let mut needed = vec![false; size * size];
        for j in 0..size {
            for i in 0..size {
                needed[j * size + i] = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .any(|(di, dj)| lit[(j + dj) * side + i + di]);
            }
        }
        needed
    }

    /// The reveal of what `los` (None: everything) adds, if anything. The
    /// seat gets the needed tiles and a ring round them, for the normals,
    /// the blends and `BuildWater`'s look around a tile, and the wave
    /// columns that lie on tiles it holds.
    fn update(
        &mut self,
        terrain: &Terrain,
        water: &SeatWater,
        los: Option<&[u8]>,
    ) -> Option<Vec<u8>> {
        let size = terrain.tiles_per_side as usize;
        if self.size != size {
            *self = Revealed::new();
            self.size = size;
            self.vertices = vec![false; (size + 1) * (size + 1)];
            self.tiles = vec![false; size * size];
            self.waves = vec![None; water.waves.len()];
        }
        if los.is_some() && self.from.as_deref() == los {
            return None;
        }
        self.from = los.map(<[u8]>::to_vec);
        let needed = Self::needed(size, los);
        let side = size + 1;
        let mut new_vertices = BTreeSet::new();
        let mut new_tiles = BTreeSet::new();
        for j in 0..size {
            for i in 0..size {
                if !needed[j * size + i] {
                    continue;
                }
                for vj in j.saturating_sub(1)..(j + 3).min(side) {
                    for vi in i.saturating_sub(1)..(i + 3).min(side) {
                        if !self.vertices[vj * side + vi] {
                            new_vertices.insert(vj * side + vi);
                        }
                    }
                }
                for tj in j.saturating_sub(1)..(j + 2).min(size) {
                    for ti in i.saturating_sub(1)..(i + 2).min(size) {
                        if !self.tiles[tj * size + ti] {
                            new_tiles.insert(tj * size + ti);
                        }
                    }
                }
            }
        }
        if new_vertices.is_empty() && new_tiles.is_empty() {
            return None;
        }
        for vertex in &new_vertices {
            self.vertices[*vertex] = true;
        }
        for tile in &new_tiles {
            self.tiles[*tile] = true;
        }
        let tiles: Vec<usize> = new_tiles.into_iter().collect();
        let vertices: Vec<usize> = new_vertices.into_iter().collect();
        let mut out = Writer::default();
        self.write(&mut out, terrain, water, &vertices, &tiles);
        Some(out.0)
    }

    /// Everything the seat holds, as one reveal, numbered as the reveals
    /// that brought it, so the later ones read on from it.
    fn snapshot(&self, terrain: &Terrain, water: &SeatWater) -> Vec<u8> {
        let mut out = Writer::default();
        let mut palette: Vec<(u16, u16)> = self
            .palette
            .iter()
            .map(|(feed, local)| (*local, *feed))
            .collect();
        palette.sort_unstable();
        out.varint(palette.len() as u64);
        for (index, texture) in palette {
            let name = &terrain.textures[texture as usize];
            out.varint(u64::from(index));
            out.u32(terrain::color(name).0);
            out.varint(name.len() as u64);
            out.0.extend_from_slice(name.as_bytes());
        }
        let vertices: Vec<usize> = (0..self.vertices.len())
            .filter(|vertex| self.vertices[*vertex])
            .collect();
        let tiles: Vec<usize> = (0..self.tiles.len())
            .filter(|tile| self.tiles[*tile])
            .collect();
        self.write_terrain(&mut out, terrain, water, &vertices, &tiles);
        let mut waves: Vec<(u32, usize)> = self
            .waves
            .iter()
            .enumerate()
            .filter_map(|(index, wave)| Some((wave.as_ref()?.0, index)))
            .collect();
        waves.sort_unstable();
        out.varint(waves.len() as u64);
        for (number, index) in waves {
            let wave = &water.waves[index];
            let sent = &self.waves[index].as_ref().unwrap().1;
            let columns: Vec<usize> = (0..sent.len()).filter(|k| sent[*k]).collect();
            write_wave(&mut out, number, wave, &columns);
        }
        out.0
    }

    /// The vertex and tile runs of a reveal.
    fn write_terrain(
        &self,
        out: &mut Writer,
        terrain: &Terrain,
        water: &SeatWater,
        vertices: &[usize],
        tiles: &[usize],
    ) {
        write_runs(out, vertices, |out, vertex| {
            out.u16(terrain.heights[vertex]);
            out.f32(water.wind.get(vertex).copied().unwrap_or(0.0));
        });
        write_runs(out, tiles, |out, tile| {
            let texture = terrain.tiles[tile];
            out.u16(if texture == u16::MAX {
                u16::MAX
            } else {
                self.palette[&texture]
            });
            out.u32(terrain.priorities[tile]);
        });
    }

    fn write(
        &mut self,
        out: &mut Writer,
        terrain: &Terrain,
        water: &SeatWater,
        vertices: &[usize],
        tiles: &[usize],
    ) {
        let mut palette = Vec::new();
        for tile in tiles {
            let texture = terrain.tiles[*tile];
            if texture != u16::MAX && !self.palette.contains_key(&texture) {
                self.palette.insert(texture, self.palette.len() as u16);
                palette.push(texture);
            }
        }
        out.varint(palette.len() as u64);
        for texture in palette {
            let name = &terrain.textures[texture as usize];
            out.varint(u64::from(self.palette[&texture]));
            out.u32(terrain::color(name).0);
            out.varint(name.len() as u64);
            out.0.extend_from_slice(name.as_bytes());
        }
        self.write_terrain(out, terrain, water, vertices, tiles);
        let size = self.size;
        let held = |x: f32, z: f32| {
            let (i, j) = ((x / TILE_SIZE).floor(), (z / TILE_SIZE).floor());
            i >= 0.0
                && j >= 0.0
                && (i as usize) < size
                && (j as usize) < size
                && self.tiles[j as usize * size + i as usize]
        };
        let mut waves = Writer::default();
        let mut count = 0u64;
        for (index, wave) in water.waves.iter().enumerate() {
            let fresh: Vec<usize> = wave
                .columns
                .iter()
                .enumerate()
                .filter(|(k, column)| {
                    let Some(column) = column else {
                        return false;
                    };
                    let sent = self.waves[index].as_ref().is_some_and(|(_, sent)| sent[*k]);
                    !sent
                        && column.iter().all(|vertex| {
                            (0..4).all(|stage| held(vertex[stage * 3], vertex[stage * 3 + 2]))
                        })
                })
                .map(|(k, _)| k)
                .collect();
            if fresh.is_empty() {
                continue;
            }
            let width = wave.columns.len();
            let next_wave = &mut self.next_wave;
            let (number, sent) = self.waves[index].get_or_insert_with(|| {
                *next_wave += 1;
                (*next_wave - 1, vec![false; width])
            });
            for k in &fresh {
                sent[*k] = true;
            }
            write_wave(&mut waves, *number, wave, &fresh);
            count += 1;
        }
        out.varint(count);
        out.0.extend_from_slice(&waves.0);
    }
}

/// A wave's columns in a reveal.
fn write_wave(out: &mut Writer, number: u32, wave: &Wave, columns: &[usize]) {
    out.varint(u64::from(number));
    out.varint(wave.columns.len() as u64);
    out.varint(columns.len() as u64);
    for k in columns {
        out.varint(*k as u64);
        for value in wave.columns[*k].iter().flatten().flatten() {
            out.f32(*value);
        }
    }
}

/// `{varint skipped, varint count, count x item}` runs of sorted indices.
fn write_runs(out: &mut Writer, indices: &[usize], mut item: impl FnMut(&mut Writer, usize)) {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for index in indices {
        match runs.last_mut() {
            Some(run) if run.1 == *index => run.1 += 1,
            _ => runs.push((*index, index + 1)),
        }
    }
    out.varint(runs.len() as u64);
    let mut end = 0;
    for (start, stop) in runs {
        out.varint((start - end) as u64);
        out.varint((stop - start) as u64);
        for index in start..stop {
            item(out, index);
        }
        end = stop;
    }
}

impl Seat {
    pub fn new(player: u32) -> Self {
        let mut stream = Stream::new();
        // Never whole: revision 0 would send the client to the observers'
        // terrain.
        stream.terrain_revision = 1;
        Seat {
            player,
            shown: Shown::new(),
            stream,
            handles: Handles::default(),
            local: Local::default(),
            revealed: Revealed::new(),
            named: HashSet::new(),
            won: false,
            eyes: player,
            details: Details::default(),
            detailed: BTreeSet::new(),
            pending: BTreeMap::new(),
            sent_shots: HashSet::new(),
            choices: HashMap::new(),
            held_mirages: HashMap::new(),
        }
    }

    /// A new match: everything but what its viewers are subscribed to.
    pub fn reset(&mut self) {
        let shown = std::mem::replace(&mut self.shown, Shown::new());
        *self = Seat::new(self.player);
        self.shown = shown;
    }

    pub fn named_data(&self, hash: u64) -> bool {
        self.named.contains(&hash)
    }

    /// The seat's terrain as served, at its current revision.
    pub fn terrain_resource(&self, terrain: &Terrain, water: &SeatWater) -> Vec<u8> {
        terrain::encode_partial(
            terrain,
            self.stream.terrain_revision,
            &self.revealed.snapshot(terrain, water),
        )
    }

    /// The feed's id the handle last named this match, if the seat ever
    /// issued it.
    pub fn entity(&self, handle: u32) -> Option<u32> {
        self.handles.to_feed.get(&handle).copied()
    }

    /// The feed's id's handle, if the seat holds it.
    pub fn handle(&self, id: u32) -> Option<u32> {
        self.handles.get(id)
    }

    fn sees_all(&self) -> bool {
        self.won
    }

    /// Whether the seat gets every group of an entity of `owner`: its own,
    /// a mutual ally's, or everything once it watches.
    fn full(&self, players: &Players, owner: i32) -> bool {
        if self.sees_all() {
            return true;
        }
        let Ok(owner) = usize::try_from(owner) else {
            return false;
        };
        let me = self.player as usize;
        owner == me || mutual_ally(players, me, owner)
    }

    /// Encodes the turn for the seat and shows it at once.
    pub fn publish(
        &mut self,
        common: &Common,
        input: &SeatInput,
        actors: &mut Actors,
        start: bool,
    ) -> io::Result<()> {
        let feed = input.feed;
        let me = self.player as usize;
        let state = feed.players.players.get(me).map(|player| player.state);
        if state == Some(PLAYER_WON) {
            self.won = true;
        }
        self.eyes = match state {
            Some(PLAYER_DEFEATED) => team_eyes(&feed.players, me).unwrap_or(me),
            _ => me,
        } as u32;
        let eyes = self.eyes as usize;
        let all = self.sees_all();

        // Which of the feed's entities the seat is shown, and their handles.
        let seen = |entity: &Entity| {
            if all {
                return true;
            }
            let state = visibility_of(entity.visibility, eyes);
            match entity.mirage {
                Some(mirage) => mirage.player == eyes as i32 && state != HIDDEN,
                None => state != HIDDEN,
            }
        };
        let view: Vec<&Entity> = input
            .entities
            .iter()
            .filter(|entity| seen(entity))
            .collect();
        let in_view: HashSet<u32> = view.iter().map(|entity| entity.id).collect();
        let mirages: HashMap<u32, Mirage> = view
            .iter()
            .filter_map(|entity| Some((entity.id, entity.mirage?)))
            .collect();
        // A mirage takes over its parent's handle, and the parent takes it
        // back once it is seen again, as the session GUI follows the
        // renames `Fogging.js` and `Mirage.js` post.
        let returning: HashMap<u32, u32> = self
            .held_mirages
            .iter()
            .filter(|(mirage, _)| !in_view.contains(mirage))
            .map(|(mirage, parent)| (*parent, *mirage))
            .collect();
        for entity in &view {
            if self.handles.get(entity.id).is_some() {
                continue;
            }
            if let Some(Mirage {
                parent: Some(parent),
                ..
            }) = entity.mirage
                && self.handles.get(parent).is_some()
                && !in_view.contains(&parent)
            {
                self.handles.transfer(parent, entity.id);
                continue;
            }
            if entity.mirage.is_none()
                && let Some(mirage) = returning.get(&entity.id)
            {
                self.handles.transfer(*mirage, entity.id);
            }
        }
        self.held_mirages = mirages
            .iter()
            .filter_map(|(id, mirage)| Some((*id, mirage.parent?)))
            .collect();

        // The entities as the seat's stream holds them.
        let mut entities = Vec::with_capacity(view.len());
        self.stream.dot_choices.clear();
        self.stream.dot_starts.clear();
        for entity in &view {
            let handle = self.handles.of(entity.id);
            let template = self.local.template(feed, &mut self.stream, entity.template);
            let visual = entity
                .visual
                .map(|visual| self.local.visual(feed, &mut self.stream, &visual));
            let visibility = if all {
                entity.visibility
            } else {
                visibility_of(entity.visibility, eyes) << (2 * me)
            };
            let mirage = entity.mirage.map(|mirage| Mirage {
                player: mirage.player,
                parent: if all {
                    mirage.parent.map(|parent| self.handles.of(parent))
                } else {
                    None
                },
            });
            if let Some(visual) = &entity.visual {
                let selections = selections(feed, visual.variants, visual.animation, None);
                let chosen = self.chosen(feed, actors, entity.id, visual, selections);
                self.stream.dot_choices.insert(handle, chosen);
            }
            // In the feed's tables, as the observers' dots are.
            let dot = Dot::new(entity, 0, false, input.turn, 0, 0);
            let starts = feed.starts(entity.id, &dot, start);
            self.stream.dot_starts.insert(handle, starts);
            entities.push(Entity {
                id: handle,
                template,
                visual,
                visibility,
                mirage,
                ..**entity
            });
        }
        entities.sort_unstable_by_key(|entity| entity.id);

        let details = self.details(feed, input, &view, &mirages);
        self.detailed = details.entities.keys().copied().collect();

        self.players(feed);
        self.economy(feed);
        self.los(input);
        if let (Some(terrain), Some(water)) = (input.terrain, input.water) {
            let los = (!all)
                .then(|| input.los.players.get(&self.eyes))
                .flatten()
                .map(Vec::as_slice);
            // Before the first LOS, nothing is revealed.
            let reveal = if all || los.is_some() {
                self.revealed.update(terrain, water, los)
            } else {
                None
            };
            if !start && let Some(reveal) = reveal {
                self.stream.terrain_revision += 1;
                self.stream.reveal = Some(reveal);
            } else if start && reveal.is_some() {
                self.stream.terrain_revision += 1;
            }
            self.territory(feed, terrain);
        }
        self.sounds(feed);
        self.projectiles(feed, input, actors, &in_view);

        let seat_common = Common {
            delay: Duration::ZERO,
            ..*common
        };
        let (visual, detail) = self.stream.encode_turn(
            &seat_common,
            input.turn,
            &entities,
            &details,
            input.ground,
            start,
        )?;
        self.shown.apply(
            Release::Turn {
                start: start.then_some(common.match_id),
                visual,
                detail,
            },
            None,
            common.match_id,
        );
        self.handles.end_turn();
        Ok(())
    }

    /// The 1-based choice table index for an entity's look.
    fn chosen(
        &mut self,
        feed: &Stream,
        actors: &mut Actors,
        id: u32,
        visual: &Visual,
        selections: Selections,
    ) -> u32 {
        let Some(actor) = visual
            .actor
            .and_then(|actor| feed.strings.items.get(actor as usize))
        else {
            return 0;
        };
        let key = ChoiceKey {
            actor: actor.clone(),
            selections,
            seed: visual.seed,
        };
        if let Some((last, chosen)) = self.choices.get(&id)
            && *last == key
        {
            return *chosen;
        }
        let chosen = self.choice_index(actors, &key);
        self.choices.insert(id, (key, chosen));
        chosen
    }

    fn choice_index(&mut self, actors: &mut Actors, key: &ChoiceKey) -> u32 {
        let sets = match actors.get(&key.actor) {
            Some(manifest) => variation::chosen(&manifest, 0, &key.selections, key.seed),
            None => Default::default(),
        };
        let local = sets.map(|set| {
            let mut indices: Vec<u32> = set
                .iter()
                .map(|name| self.local.text(&mut self.stream, name))
                .collect();
            indices.sort_unstable();
            indices
        });
        let next = self.stream.choices.items.len() as u32 + 1;
        let index = *self.stream.choice_ids.entry(local.clone()).or_insert(next);
        if index == next {
            self.stream.choices.items.push(local);
        }
        index
    }

    /// The seat's detail state for this turn.
    fn details(
        &mut self,
        feed: &Stream,
        input: &SeatInput,
        view: &[&Entity],
        mirages: &HashMap<u32, Mirage>,
    ) -> Details {
        let source = input.details;
        let players = &feed.players;
        let mut details = Details {
            lists: std::mem::take(&mut self.details.lists),
            data: std::mem::take(&mut self.details.data),
            ..Details::default()
        };
        // The templates, technologies and formations the seat is shown, for
        // the data table: (template name, player) as the tooltips read them.
        let mut templates: HashSet<(String, i32)> = HashSet::new();
        let mut technologies: HashSet<u32> = HashSet::new();
        let mut detailed: Vec<(u32, i32, bool)> = view
            .iter()
            .map(|entity| (entity.id, entity.owner, mirages.contains_key(&entity.id)))
            .collect();
        let mut index = 0;
        while index < detailed.len() {
            let (id, owner, mirage) = detailed[index];
            index += 1;
            let Some(groups) = source.entities.get(&id) else {
                continue;
            };
            let full = self.full(players, owner) && !mirage;
            if full && let Some(garrison) = &groups.garrison {
                let owners = groups.garrison_owners.as_deref().unwrap_or_default();
                for (k, (member, _)) in garrison.entities.iter().enumerate() {
                    let member_owner = owners
                        .get(k)
                        .filter(|_| owners.len() == garrison.entities.len())
                        .map_or(-1, |owner| i32::from(*owner));
                    if !detailed.iter().any(|(known, _, _)| known == member) {
                        detailed.push((*member, member_owner, false));
                    }
                }
            }
            let local = self.groups(feed, source, &mut details, groups, full, mirage);
            if full {
                for list in [&groups.trainable, &groups.buildable, &groups.upgrades]
                    .into_iter()
                    .flatten()
                {
                    for item in source.lists.get(list).into_iter().flatten() {
                        if let Some(name) = item
                            .name
                            .and_then(|name| feed.strings.items.get(name as usize))
                        {
                            templates.insert((name.clone(), owner));
                        }
                    }
                }
                for item in source
                    .lists
                    .get(groups.researchable.as_ref().unwrap_or(&u32::MAX))
                    .into_iter()
                    .flatten()
                {
                    if let Some(name) = item.name {
                        technologies.insert(name);
                    }
                }
                if let Some(queue) = &groups.queue {
                    for item in &queue.items {
                        match (item.technology, item.name) {
                            (true, Some(name)) => {
                                technologies.insert(name);
                            }
                            (false, Some(name)) => {
                                if let Some(text) = feed.strings.items.get(name as usize) {
                                    templates.insert((text.clone(), owner));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if let Some(stats) = local.stats {
                self.named.insert(stats);
            }
            let handle = self.handles.of(id);
            details.entities.insert(handle, local);
        }
        for entity in view {
            if let Some(template) = feed.templates.items.get(entity.template as usize) {
                templates.insert((template.name.clone(), entity.owner));
            }
        }
        // The data the tooltips read for what the seat is shown.
        let all = self.sees_all();
        for (key, hash) in &source.data {
            let local = match *key {
                DataKey::Template { name, player } => {
                    let Some(text) = feed.strings.items.get(name as usize) else {
                        continue;
                    };
                    if !all && !templates.contains(&(text.clone(), player)) {
                        continue;
                    }
                    DataKey::Template {
                        name: self.local.text(&mut self.stream, text),
                        player,
                    }
                }
                DataKey::Technology { name, civ } => {
                    if !all && !technologies.contains(&name) {
                        continue;
                    }
                    DataKey::Technology {
                        name: self.local.string(feed, &mut self.stream, name),
                        civ: self.local.string(feed, &mut self.stream, civ),
                    }
                }
                // Formations are the civs', which every player knows.
                DataKey::Formation { name } => DataKey::Formation {
                    name: self.local.string(feed, &mut self.stream, name),
                },
            };
            details.data.insert(local, *hash);
            self.named.insert(*hash);
        }
        let me = self.player as usize;
        details.player_gui = source
            .player_gui
            .iter()
            .enumerate()
            .map(|(index, player)| PlayerGui {
                formations: player
                    .formations
                    .iter()
                    .map(|formation| self.local.optional(feed, &mut self.stream, *formation))
                    .collect(),
                prices: player
                    .prices
                    .iter()
                    .map(|(resource, buy, sell)| {
                        (
                            self.local.optional(feed, &mut self.stream, *resource),
                            *buy,
                            *sell,
                        )
                    })
                    .collect(),
                can_barter: player.can_barter,
                // The engine shows a player only its own traders.
                trading_goods: if all || index == me {
                    player
                        .trading_goods
                        .iter()
                        .map(|(resource, percent)| {
                            (
                                self.local.optional(feed, &mut self.stream, *resource),
                                *percent,
                            )
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                traders: if all || index == me {
                    player.traders
                } else {
                    [0; 5]
                },
            })
            .collect();
        // Every group is compared with what the seat holds, so all count as
        // changed; those gone are the ones that had groups last turn.
        details.changed = details.entities.keys().copied().collect();
        details.gone = self
            .detailed
            .iter()
            .copied()
            .filter(|handle| !details.entities.contains_key(handle))
            .collect();
        details.renames = self.renames(source);
        self.details.lists = std::mem::take(&mut details.lists);
        self.details.data = details.data.clone();
        details.lists = self.details.lists.clone();
        details
    }

    /// One entity's groups as the seat may have them, its strings, lists
    /// and ids the seat's.
    fn groups(
        &mut self,
        feed: &Stream,
        source: &Details,
        details: &mut Details,
        groups: &EntityDetails,
        full: bool,
        mirage: bool,
    ) -> EntityDetails {
        let stream = &mut self.stream;
        let local = &mut self.local;
        let mut out = EntityDetails {
            health: groups.health,
            capture: groups.capture.clone(),
            supply: groups.supply,
            gatherers: groups.gatherers,
            foundation: groups.foundation.clone(),
            repair: groups.repair.clone(),
            stats: groups.stats,
            ranges: groups.ranges.clone().map(|ranges| {
                ranges
                    .into_iter()
                    .map(|mut overlay| {
                        overlay.texture = local.optional(feed, stream, overlay.texture);
                        overlay.mask = local.optional(feed, stream, overlay.mask);
                        overlay
                    })
                    .collect()
            }),
            ..EntityDetails::default()
        };
        // A mirage has what Fogging.js copied, which the hook sends.
        if mirage {
            return out;
        }
        out.carrying = groups.carrying.clone().map(|carrying| {
            carrying
                .into_iter()
                .map(|mut carried| {
                    carried.kind = local.optional(feed, stream, carried.kind);
                    carried
                })
                .collect()
        });
        out.experience = groups.experience;
        out.market_bonus = groups.market_bonus;
        out.trade_garrison = groups.trade_garrison.filter(|_| full);
        out.trader = groups.trader.clone().map(|mut trader| {
            trader.goods = local.optional(feed, stream, trader.goods);
            trader
        });
        out.progress = groups.progress.clone().map(|mut progress| {
            progress.upgrade_target = if full {
                local.optional(feed, stream, progress.upgrade_target)
            } else {
                None
            };
            progress
        });
        if !full {
            // `getGarrisonTooltip` reads the capacity and the occupied
            // slots; the members are the Garrison panel's, which only the
            // owner and mutual allies get.
            out.garrison = groups.garrison.as_ref().map(|garrison| Garrison {
                capacity: garrison.capacity,
                occupied: garrison.occupied,
                entities: Vec::new(),
            });
            out.garrison_owners = groups.garrison.as_ref().map(|_| Vec::new());
            // `addFormationMembers` reads the formation of any entity.
            out.unit_ai = groups.unit_ai.as_ref().map(|unit_ai| UnitAi {
                state: None,
                idle: false,
                stance: None,
                formation_controller: if unit_ai.formation_controller == 0 {
                    0
                } else {
                    self.handles.of(unit_ai.formation_controller)
                },
            });
            return out;
        }
        out.queue = groups.queue.clone().map(|mut queue| {
            for item in &mut queue.items {
                item.name = local.optional(feed, stream, item.name);
            }
            queue
        });
        out.garrison = groups.garrison.as_ref().map(|garrison| Garrison {
            capacity: garrison.capacity,
            occupied: garrison.occupied,
            entities: garrison
                .entities
                .iter()
                .map(|(member, template)| {
                    (
                        self.handles.of(*member),
                        local.template(feed, stream, *template),
                    )
                })
                .collect(),
        });
        out.garrison_owners = groups.garrison_owners.clone();
        out.unit_ai = groups.unit_ai.as_ref().map(|unit_ai| UnitAi {
            state: None,
            idle: unit_ai.idle,
            stance: local.optional(feed, stream, unit_ai.stance),
            formation_controller: if unit_ai.formation_controller == 0 {
                0
            } else {
                self.handles.of(unit_ai.formation_controller)
            },
        });
        let mut list = |index: &Option<u32>| {
            index.map(|index| local.list(feed, stream, &mut details.lists, source, index))
        };
        out.trainable = list(&groups.trainable);
        out.researchable = list(&groups.researchable);
        out.buildable = list(&groups.buildable);
        out.upgrades = list(&groups.upgrades);
        out.stances = groups.stances.as_ref().map(|stances| {
            stances
                .iter()
                .map(|stance| local.optional(feed, stream, *stance))
                .collect()
        });
        out.formations = groups.formations.clone().map(|mut formations| {
            formations.current = local.optional(feed, stream, formations.current);
            for formation in &mut formations.list {
                *formation = local.optional(feed, stream, *formation);
            }
            formations
        });
        out.commands = groups.commands.map(|mut commands| {
            commands.alert_classes = local.optional(feed, stream, commands.alert_classes);
            commands
        });
        out.holder = groups.holder.map(|holder| {
            if holder == 0 {
                0
            } else {
                self.handles.of(holder)
            }
        });
        // Another player's route is not the seat's to see.
        out.trade_route = groups
            .trade_route
            .clone()
            .filter(|_| full)
            .map(|mut route| {
                for market in [&mut route.first, &mut route.second] {
                    if *market != 0 {
                        *market = self.handles.of(*market);
                    }
                }
                route
            });
        out.rally = groups.rally.as_ref().map(|rally| crate::feed::Rally {
            marker: local.optional(feed, stream, rally.marker),
            points: rally.points.clone(),
        });
        out
    }

    /// The feed's renames the seat can follow: from an entity it held to
    /// one it holds now. A mirage's are its handle's (`publish`).
    fn renames(&mut self, source: &Details) -> Vec<(u32, u32)> {
        source
            .renames
            .iter()
            .filter_map(|(old, new)| {
                let old_handle = self.handles.get(*old)?;
                let new_handle = self.handles.get(*new)?;
                (old_handle != new_handle).then_some((old_handle, new_handle))
            })
            .collect()
    }

    /// Names, colours, teams, states, civs and the ceasefire are public;
    /// of the diplomacy, the seat's own stances and the others' towards it;
    /// the battle state only its own.
    fn players(&mut self, feed: &Stream) {
        let me = self.player as usize;
        let all = self.sees_all();
        let mut players = feed.players.clone();
        for (index, player) in players.players.iter_mut().enumerate() {
            if all || index == me {
                continue;
            }
            for (other, stance) in player.stances.iter_mut().enumerate() {
                if other != me {
                    *stance = STANCE_UNKNOWN;
                }
            }
            player.battle = false;
            player.shared_los = false;
        }
        self.stream.set_players(players);
    }

    /// The seat's own top panel; a mutual ally's resources while the seat
    /// has shared LOS and the ally is not defeated, and its phase always
    /// (`CounterManager.js`, `selection_details.js`).
    fn economy(&mut self, feed: &Stream) {
        let me = self.player as usize;
        let all = self.sees_all();
        let players = &feed.players;
        let shared = players
            .players
            .get(me)
            .is_some_and(|player| player.shared_los);
        let economy: Vec<Funds> = feed
            .economy
            .iter()
            .enumerate()
            .map(|(index, funds)| {
                let full = all || index == me;
                let ally = mutual_ally(players, me, index);
                let defeated = players
                    .players
                    .get(index)
                    .is_some_and(|player| player.state == PLAYER_DEFEATED);
                let shown = full || (ally && shared && !defeated);
                let mut local = if shown {
                    funds.clone()
                } else {
                    Funds::default()
                };
                if !shown && ally {
                    local.phase = funds.phase;
                }
                if !full {
                    local.panel_entities.clear();
                }
                local.resources = local
                    .resources
                    .iter()
                    .map(|(code, amount, gatherers)| {
                        (
                            self.local.one_based(feed, &mut self.stream, *code),
                            *amount,
                            *gatherers,
                        )
                    })
                    .collect();
                local.phase = self.local.one_based(feed, &mut self.stream, local.phase);
                local.panel_entities = local
                    .panel_entities
                    .iter()
                    .map(|id| self.handles.of(*id))
                    .collect();
                local
            })
            .collect();
        self.stream.economy = economy;
    }

    fn los(&mut self, input: &SeatInput) {
        if self.sees_all() {
            self.stream.los = input.los.clone();
            return;
        }
        let mut los = Los {
            side: input.los.side,
            players: BTreeMap::new(),
        };
        if let Some(grid) = input.los.players.get(&self.eyes) {
            los.players.insert(self.player, grid.clone());
        }
        self.stream.los = los;
    }

    /// The territory where the seat holds the terrain under it: the engine
    /// draws it, the live one, darkened by the LOS. The rest is unknown, so
    /// no border is traced along it.
    fn territory(&mut self, feed: &Stream, terrain: &Terrain) {
        let Some(territory) = &feed.territory else {
            return;
        };
        if self.sees_all() {
            self.stream.set_territory(territory.clone());
            return;
        }
        let size = terrain.tiles_per_side as usize;
        let held = |ti: usize, tj: usize| {
            (0..TERRAIN_TILES_PER_TERRITORY_TILE).any(|dj| {
                (0..TERRAIN_TILES_PER_TERRITORY_TILE).any(|di| {
                    let (i, j) = (
                        ti * TERRAIN_TILES_PER_TERRITORY_TILE + di,
                        tj * TERRAIN_TILES_PER_TERRITORY_TILE + dj,
                    );
                    i < size && j < size && self.revealed.tiles.get(j * size + i) == Some(&true)
                })
            })
        };
        let width = territory.width as usize;
        let tiles = territory
            .tiles
            .iter()
            .enumerate()
            .map(|(index, tile)| {
                if held(index % width, index / width) {
                    *tile
                } else {
                    TERRITORY_UNKNOWN
                }
            })
            .collect();
        self.stream.set_territory(Territory {
            width: territory.width,
            height: territory.height,
            tiles,
        });
    }

    /// The sounds the engine plays for the seat's player: an entity's when
    /// the player sees it (`CCmpSoundManager::PlaySoundGroup`), one at a
    /// position always, a player's when it is the seat's.
    fn sounds(&mut self, feed: &Stream) {
        let me = self.player as usize;
        let eyes = self.eyes as usize;
        let all = self.sees_all();
        let mut sounds = Vec::new();
        for sound in &feed.sounds {
            let target = match sound.target {
                SoundTarget::Entity { id, owner } => {
                    if !all && visibility_of(sound.visibility, eyes) != VISIBLE {
                        continue;
                    }
                    SoundTarget::Entity {
                        id: self.handles.of(id),
                        owner,
                    }
                }
                SoundTarget::Position => SoundTarget::Position,
                SoundTarget::Player(player) => {
                    if !all && player != me as i32 {
                        continue;
                    }
                    SoundTarget::Player(player)
                }
            };
            sounds.push(Sound {
                group: self.local.string(feed, &mut self.stream, sound.group),
                target,
                visibility: if all {
                    sound.visibility
                } else {
                    visibility_of(sound.visibility, eyes) << (2 * me)
                },
                ..*sound
            });
        }
        self.stream.sounds = sounds;
    }

    /// The projectiles the engine draws for the seat's player: each frame
    /// where its LOS is visible (`CCmpProjectileManager::Interpolate`). One
    /// is sent on the first turn it is in sight, with how long it has
    /// flown, and its shooter only when the seat holds it.
    fn projectiles(
        &mut self,
        feed: &Stream,
        input: &SeatInput,
        actors: &mut Actors,
        in_view: &HashSet<u32>,
    ) {
        let turn = input.turn;
        let by_id: HashMap<u32, &Entity> = input
            .entities
            .iter()
            .map(|entity| (entity.id, entity))
            .collect();
        for launch in &feed.launches {
            if feed.removals.contains(&launch.id) {
                continue;
            }
            let shooter = by_id
                .get(&launch.shooter)
                .filter(|_| launch.shooter != 0)
                .map(|entity| **entity);
            let (origin, chosen) =
                match shooter.and_then(|shooter| Some((shooter, shooter.visual?))) {
                    Some((shooter, visual)) => {
                        let local_visual = self.local.visual(feed, &mut self.stream, &visual);
                        let selections =
                            selections(feed, visual.variants, None, Some(SHOOTING_ANIMATION));
                        let key = ChoiceKey {
                            actor: visual
                                .actor
                                .and_then(|actor| feed.strings.items.get(actor as usize))
                                .cloned()
                                .unwrap_or_default(),
                            selections,
                            seed: visual.seed,
                        };
                        let chosen = if key.actor.is_empty() {
                            0
                        } else {
                            self.choice_index(actors, &key)
                        };
                        let origin = ShotOrigin {
                            owner: shooter.owner,
                            actor: local_visual.actor.map_or(0, |actor| actor + 1),
                            variants: local_visual.variants.map_or(0, |set| set + 1),
                        };
                        (Some(origin), chosen)
                    }
                    None => (None, 0),
                };
            let flight = flight_secs(launch);
            let impact = f64::from(launch.impact_lifetime) / 65536.0;
            let keep =
                ((flight + PROJECTILE_KEEP_SECS + impact.max(0.0)) / TURN_SECS).ceil() as u32;
            self.pending.insert(
                launch.id,
                Pending {
                    launch: *launch,
                    launch_turn: turn,
                    last_turn: turn.saturating_add(keep),
                    origin,
                    chosen,
                },
            );
        }
        let mut removals = Vec::new();
        for id in &feed.removals {
            self.pending.remove(id);
            if self.sent_shots.remove(id) {
                removals.push(*id);
            }
        }
        self.pending.retain(|_, pending| pending.last_turn >= turn);
        let all = self.sees_all();
        let grid = input.los.players.get(&self.eyes);
        let side = input.los.side as usize;
        let mut launches = Vec::new();
        let ids: Vec<u32> = self.pending.keys().copied().collect();
        for id in ids {
            let pending = &self.pending[&id];
            let age = turn - pending.launch_turn;
            let in_sight =
                all || grid.is_some_and(|grid| in_flight_sight(&pending.launch, age, grid, side));
            if !in_sight {
                continue;
            }
            let pending = self.pending.remove(&id).unwrap();
            let shooter_handle =
                if pending.launch.shooter != 0 && in_view.contains(&pending.launch.shooter) {
                    self.handles.of(pending.launch.shooter)
                } else {
                    0
                };
            let launch = Launch {
                shooter: shooter_handle,
                actor: self
                    .local
                    .optional(feed, &mut self.stream, pending.launch.actor),
                impact_actor: self.local.optional(
                    feed,
                    &mut self.stream,
                    pending.launch.impact_actor,
                ),
                ..pending.launch
            };
            self.stream.launch_extras.insert(
                id,
                LaunchExtra {
                    age,
                    origin: if shooter_handle == 0 {
                        pending.origin
                    } else {
                        None
                    },
                    chosen: pending.chosen,
                },
            );
            self.sent_shots.insert(id);
            launches.push(launch);
        }
        self.stream.launches = launches;
        self.stream.removals = removals;
    }

    /// Tells the seat's viewers who has the game paused.
    pub fn pause(&mut self, turn: u32, players: &[i8]) -> io::Result<()> {
        let visual = self.stream.pause(turn, players)?;
        self.shown
            .apply(Release::Pause(visual), None, self.shown.match_id);
        Ok(())
    }

    pub fn end(&mut self, common: &Common, turn: u32) {
        let seat_common = Common {
            delay: Duration::ZERO,
            ..*common
        };
        let (ended, visual_tail, detail_tail) = self.stream.finish(&seat_common, turn);
        self.shown.apply(
            Release::End {
                match_id: common.match_id,
                visual_tail,
                detail_tail,
                ended,
                terrain: None,
                data_files: HashMap::new(),
            },
            None,
            common.match_id,
        );
    }
}

/// Both players are allies of each other (`Diplomacy.IsMutualAlly`).
fn mutual_ally(players: &Players, a: usize, b: usize) -> bool {
    let stance = |from: usize, to: usize| {
        players
            .players
            .get(from)
            .and_then(|player| player.stances.get(to))
            .copied()
    };
    a != 0 && b != 0 && stance(a, b) == Some(STANCE_ALLY) && stance(b, a) == Some(STANCE_ALLY)
}

/// Whose vision a defeated `me` is shown (D47): its lowest-numbered mutual
/// ally still in the game. None once none is.
pub(super) fn team_eyes(players: &Players, me: usize) -> Option<usize> {
    (1..players.players.len()).find(|&other| {
        other != me
            && players.players[other].state == PLAYER_ACTIVE
            && mutual_ally(players, me, other)
    })
}

/// The lower-case selections the client resolves an entity's look with:
/// its variant selections and its animation, or the shooting animation.
fn selections(
    feed: &Stream,
    variants: Option<u32>,
    animation: Option<u32>,
    extra: Option<&str>,
) -> Selections {
    let strings = &feed.strings.items;
    let mut selections: Selections = variants
        .and_then(|set| feed.variant_sets.items.get(set as usize))
        .into_iter()
        .flatten()
        .filter_map(|(_, selection)| strings.get(*selection as usize))
        .map(|selection| selection.to_lowercase())
        .collect();
    if let Some(animation) = animation.and_then(|animation| strings.get(animation as usize)) {
        selections.insert(animation.to_lowercase());
    }
    if let Some(extra) = extra {
        selections.insert(extra.to_string());
    }
    selections
}

/// Seconds of horizontal flight, as `Shot::new` reckons it.
fn flight_secs(launch: &Launch) -> f64 {
    let metres = |raw: i32| f64::from(raw) / 65536.0;
    let dx = metres(launch.target[0]) - metres(launch.launch[0]);
    let dz = metres(launch.target[2]) - metres(launch.launch[2]);
    let speed = metres(launch.speed);
    if speed > 0.0 {
        dx.hypot(dz) / speed
    } else {
        0.0
    }
}

/// Whether the projectile is over a visible LOS vertex at some point of
/// the turn the client draws with this turn's LOS: from `age - 1` to `age`
/// turns after its launch, in flight or lying at its target.
fn in_flight_sight(launch: &Launch, age: u32, grid: &[u8], side: usize) -> bool {
    let metres = |raw: i32| f64::from(raw) / 65536.0;
    let (x0, z0) = (metres(launch.launch[0]), metres(launch.launch[2]));
    let (x1, z1) = (metres(launch.target[0]), metres(launch.target[2]));
    let flight = flight_secs(launch);
    let distance = (x1 - x0).hypot(z1 - z0);
    let visible = |x: f64, z: f64| {
        let i = (x / f64::from(LOS_TILE_SIZE)).round();
        let j = (z / f64::from(LOS_TILE_SIZE)).round();
        i >= 0.0
            && j >= 0.0
            && (i as usize) < side
            && (j as usize) < side
            && grid[j as usize * side + i as usize] & LOS_VISIBLE != 0
    };
    let from = (f64::from(age) - 1.0).max(0.0) * TURN_SECS;
    let to = f64::from(age) * TURN_SECS;
    let at = |t: f64| {
        let fraction = if flight > 0.0 {
            (t / flight).min(1.0)
        } else {
            1.0
        };
        (x0 + (x1 - x0) * fraction, z0 + (z1 - z0) * fraction)
    };
    let span = if flight > 0.0 {
        (to.min(flight) - from.min(flight)).max(0.0) / flight * distance
    } else {
        0.0
    };
    let steps = (span / FLIGHT_STEP).ceil().max(1.0) as u32;
    (0..=steps).any(|step| {
        let t = from + (to - from) * f64::from(step) / f64::from(steps);
        let (x, z) = at(t);
        visible(x, z)
    })
}
