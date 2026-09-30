//! The viewer stream, wire format v2: one binary WebSocket message per
//! keyframe or turn, varints and zigzag as in `wire.rs`, positions in 1/16 m.
//!
//! - kind 1 keyframe: varint turn, varint map size in metres, u64 terrain
//!   id (0: none yet, else fetch `/terrain/<id as 16 hex digits>`), varint
//!   icon count, count x icon, varint count, then count x {varint id
//!   delta, zigzag x, zigzag z, u32 colour, varint icon}.
//! - kind 2 delta: varint turn, varint count of icons added to the table,
//!   count x icon, varint spawned, destroyed and changed counts. Spawned
//!   entries are keyframe entries, destroyed entries are id deltas, changed
//!   entries are {varint id delta, u8 mask, zigzag dx and dz if mask & 1,
//!   u32 colour if mask & 2, varint icon if mask & 4}.
//!
//! An icon is {varint name length, UTF-8 name, f32 size in CSS pixels}; the
//! name is an entry of `/icons.bin`. An entity's icon is a 1-based index
//! into the icon table, or 0 for none.
//! Colour is 0xFFRRGGBB, or 0 for an entity that is not on the minimap.
//! Ids in each list are sorted and delta-coded from 0.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use rocket::tokio::sync::broadcast;

use crate::feed::Entity;
use crate::feed::Icon;
use crate::wire::Writer;
use crate::wire::diff;
use crate::wire::quantize_position;

const MESSAGE_KEYFRAME: u8 = 1;
const MESSAGE_DELTA: u8 = 2;
const CHANGED_POSITION: u8 = 1;
const CHANGED_COLOR: u8 = 2;
const CHANGED_ICON: u8 = 4;
/// About 50 s of turns; a viewer further behind is resent a keyframe.
const BACKLOG_MESSAGES: usize = 256;

pub type Message = Arc<[u8]>;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Dot {
    x: i32,
    z: i32,
    color: u32,
    icon: u32,
}

impl Dot {
    fn new(entity: &Entity, icon: u32) -> Self {
        Dot {
            x: quantize_position(entity.x),
            z: quantize_position(entity.z),
            color: entity.color.map_or(0, |rgb| 0xFF00_0000 | rgb),
            icon,
        }
    }
}

pub struct Hub {
    state: Mutex<State>,
}

struct State {
    turn: u32,
    map_size: u32,
    terrain: Option<(u64, Message)>,
    dots: BTreeMap<u32, Dot>,
    /// Append-only per feed, so indices already sent stay valid.
    icons: Vec<Icon>,
    /// How many icons viewers have been sent; later ones go out with the
    /// next turn.
    icons_sent: usize,
    /// Template index to 1-based icon index.
    template_icons: HashMap<u32, u32>,
    has_turn: bool,
    /// Set when a new observer connects: its entity ids belong to another
    /// match, so its first turn must replace, not patch, what viewers have.
    new_feed: bool,
    sender: broadcast::Sender<Message>,
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            state: Mutex::new(State {
                turn: 0,
                map_size: 0,
                terrain: None,
                dots: BTreeMap::new(),
                icons: Vec::new(),
                icons_sent: 0,
                template_icons: HashMap::new(),
                has_turn: false,
                new_feed: true,
                sender: broadcast::channel(BACKLOG_MESSAGES).0,
            }),
        }
    }

    pub fn start_feed(&self) {
        let mut state = self.state.lock().unwrap();
        state.new_feed = true;
        state.terrain = None;
        state.icons.clear();
        state.icons_sent = 0;
        state.template_icons.clear();
    }

    pub fn set_template_icon(&self, template: u32, icon: Icon) {
        let state = &mut *self.state.lock().unwrap();
        let index = match state
            .icons
            .iter()
            .position(|known| known.name == icon.name && known.size == icon.size)
        {
            Some(index) => index,
            None => {
                state.icons.push(icon);
                state.icons.len() - 1
            }
        };
        state.template_icons.insert(template, index as u32 + 1);
    }

    pub fn set_terrain(&self, id: u64, bytes: Vec<u8>) {
        self.state.lock().unwrap().terrain = Some((id, bytes.into()));
    }

    /// Only the current match's terrain is kept; an old id is gone.
    pub fn terrain(&self, id: u64) -> Option<Message> {
        let state = self.state.lock().unwrap();
        let (current, bytes) = state.terrain.as_ref()?;
        (*current == id).then(|| Arc::clone(bytes))
    }

    pub fn set_map_size(&self, size: u32) {
        self.state.lock().unwrap().map_size = size;
    }

    pub fn publish(&self, turn: u32, entities: &[Entity]) {
        let mut state = self.state.lock().unwrap();
        let dots: BTreeMap<u32, Dot> = entities
            .iter()
            .map(|entity| {
                let icon = state
                    .template_icons
                    .get(&entity.template)
                    .copied()
                    .unwrap_or(0);
                (entity.id, Dot::new(entity, icon))
            })
            .collect();
        let message = if state.new_feed {
            encode_keyframe(
                turn,
                state.map_size,
                terrain_id(&state),
                &state.icons,
                &dots,
            )
        } else {
            encode_delta(turn, &state.icons[state.icons_sent..], &state.dots, &dots)
        };
        state.icons_sent = state.icons.len();
        state.turn = turn;
        state.dots = dots;
        state.has_turn = true;
        state.new_feed = false;
        // No receivers is not an error: nobody is watching yet.
        let _ = state.sender.send(message.into());
    }

    /// The keyframe (none before the first turn) and the receiver are taken
    /// under one lock, so the first delta a viewer receives applies to
    /// exactly that keyframe.
    pub fn subscribe(&self) -> (Option<Message>, broadcast::Receiver<Message>) {
        let state = self.state.lock().unwrap();
        // Icons not sent yet are not used by these dots, and the next delta
        // will add them, so they must not be in this table too.
        let keyframe = state.has_turn.then(|| {
            encode_keyframe(
                state.turn,
                state.map_size,
                terrain_id(&state),
                &state.icons[..state.icons_sent],
                &state.dots,
            )
            .into()
        });
        (keyframe, state.sender.subscribe())
    }
}

fn terrain_id(state: &State) -> u64 {
    state.terrain.as_ref().map_or(0, |(id, _)| *id)
}

fn encode_keyframe(
    turn: u32,
    map_size: u32,
    terrain: u64,
    icons: &[Icon],
    dots: &BTreeMap<u32, Dot>,
) -> Vec<u8> {
    let mut out = Writer::default();
    out.u8(MESSAGE_KEYFRAME);
    out.varint(u64::from(turn));
    out.varint(u64::from(map_size));
    out.u64(terrain);
    write_icons(&mut out, icons);
    out.varint(dots.len() as u64);
    let mut last_id = 0;
    for (id, dot) in dots {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, dot);
    }
    out.0
}

fn encode_delta(
    turn: u32,
    new_icons: &[Icon],
    previous: &BTreeMap<u32, Dot>,
    current: &BTreeMap<u32, Dot>,
) -> Vec<u8> {
    let diff = diff(previous, current);
    let mut out = Writer::default();
    out.u8(MESSAGE_DELTA);
    out.varint(u64::from(turn));
    write_icons(&mut out, new_icons);
    out.varint(diff.spawned.len() as u64);
    out.varint(diff.destroyed.len() as u64);
    out.varint(diff.changed.len() as u64);
    let mut last_id = 0;
    for (id, dot) in &diff.spawned {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        write_dot(&mut out, dot);
    }
    let mut last_id = 0;
    for id in &diff.destroyed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
    }
    let mut last_id = 0;
    for (id, before, now) in &diff.changed {
        out.varint(u64::from(id - last_id));
        last_id = *id;
        let position = before.x != now.x || before.z != now.z;
        let color = before.color != now.color;
        let icon = before.icon != now.icon;
        let mut mask = 0;
        if position {
            mask |= CHANGED_POSITION;
        }
        if color {
            mask |= CHANGED_COLOR;
        }
        if icon {
            mask |= CHANGED_ICON;
        }
        out.u8(mask);
        if position {
            out.zigzag(i64::from(now.x) - i64::from(before.x));
            out.zigzag(i64::from(now.z) - i64::from(before.z));
        }
        if color {
            out.u32(now.color);
        }
        if icon {
            out.varint(u64::from(now.icon));
        }
    }
    out.0
}

fn write_icons(out: &mut Writer, icons: &[Icon]) {
    out.varint(icons.len() as u64);
    for icon in icons {
        out.varint(icon.name.len() as u64);
        out.0.extend_from_slice(icon.name.as_bytes());
        out.f32(icon.size);
    }
}

fn write_dot(out: &mut Writer, dot: &Dot) {
    out.zigzag(i64::from(dot.x));
    out.zigzag(i64::from(dot.z));
    out.u32(dot.color);
    out.varint(u64::from(dot.icon));
}
