//! A crowd that moves like one in a game, for the unit page. Squads march to
//! random points, wheel to face them on the way, halt for a while and set
//! off again. Each unit walks to its own place in its squad's formation, so
//! a wheeling squad re-forms on foot instead of sliding, and each unit
//! walks or idles on its own, its walk played at its own speed as the
//! engine does.

use std::f64::consts::PI;
use std::f64::consts::TAU;

pub const IDLE: usize = 0;
pub const WALK: usize = 1;

const SQUADS: usize = 100;
/// Formation of 10 files by 10 ranks; 100 squads make 10000 units.
const FILES: usize = 10;
const RANKS: usize = 10;
const FILE_SPACING: f64 = 2.6;
const RANK_SPACING: f64 = 3.2;
/// Off the exact grid, so the ranks do not look stamped out.
const SLOT_JITTER: f64 = 0.3;
/// Squads start on a grid this far apart and pick targets within
/// `FIELD_RADIUS` of the centre.
const SQUAD_SPACING: f64 = 34.0;
const FIELD_RADIUS: f64 = 170.0;
const MIN_LEG: f64 = 40.0;
/// `WalkSpeed` of template_unit_infantry_melee, which the hoplite inherits.
pub const WALK_SPEED: f64 = 9.5;
/// Slower than a unit can walk, so stragglers catch up.
const SQUAD_SPEED: f64 = WALK_SPEED * 0.8;
const SQUAD_TURN_RATE: f64 = 0.5;
/// A squad only moves on once it faces its target this closely, so it
/// wheels in place first.
const SQUAD_FACING: f64 = 0.35;
/// `TurnRate` of `Position` in template_unit.
const UNIT_TURN_RATE: f64 = 14.0;
/// A unit closes on its place at this many times the distance per second,
/// up to walking speed, so it slows down as it arrives.
const CATCH_UP: f64 = 2.0;
/// Below this speed a unit stands; above `WALK_FROM`, once stood, it
/// walks; the gap stops it flickering between the two.
const STAND_BELOW: f64 = 0.4;
const WALK_FROM: f64 = 1.0;
/// Seconds a squad halts at its target, between these.
const HALT: (f64, f64) = (3.0, 10.0);
/// A long frame (a hidden tab) is not simulated in one step.
const MAX_STEP: f64 = 0.1;

pub struct Unit {
    pub x: f64,
    pub z: f64,
    /// Direction of travel as atan2(dx, dz).
    pub heading: f64,
    pub gait: usize,
    /// Position in the gait's animation, in frames.
    pub frame: f64,
    squad: usize,
    /// Place in the formation: x to the right of the squad, z ahead.
    slot: [f64; 2],
}

struct Squad {
    x: f64,
    z: f64,
    heading: f64,
    target: Option<[f64; 2]>,
    /// Seconds left of the halt.
    halt: f64,
}

/// Milliseconds per frame at rate 1 and frame count of each gait's
/// animation.
pub type Timing = [(f64, u32); 2];

pub struct March {
    pub units: Vec<Unit>,
    squads: Vec<Squad>,
    random: Random,
}

impl March {
    pub fn new(timing: Timing) -> Self {
        let mut random = Random(0x2545_F491);
        let side = (SQUADS as f64).sqrt().ceil() as usize;
        let mut squads = Vec::with_capacity(SQUADS);
        let mut units = Vec::with_capacity(SQUADS * FILES * RANKS);
        for index in 0..SQUADS {
            let grid = |i: usize| (i as f64 - (side - 1) as f64 / 2.0) * SQUAD_SPACING;
            let squad = Squad {
                x: grid(index % side),
                z: grid(index / side),
                heading: random.next() * TAU,
                target: None,
                // Staggered, so the squads do not all set off at once.
                halt: random.next() * 3.0,
            };
            for rank in 0..RANKS {
                for file in 0..FILES {
                    let slot = [
                        (file as f64 - (FILES - 1) as f64 / 2.0) * FILE_SPACING
                            + random.signed() * SLOT_JITTER,
                        ((RANKS - 1) as f64 / 2.0 - rank as f64) * RANK_SPACING
                            + random.signed() * SLOT_JITTER,
                    ];
                    let [x, z] = place(&squad, slot);
                    units.push(Unit {
                        x,
                        z,
                        heading: squad.heading,
                        gait: IDLE,
                        frame: random.next() * f64::from(timing[IDLE].1),
                        squad: index,
                        slot,
                    });
                }
            }
            squads.push(squad);
        }
        March {
            units,
            squads,
            random,
        }
    }

    /// Advances `seconds` of real time.
    pub fn update(&mut self, seconds: f64, timing: Timing) {
        let step = seconds.clamp(0.0, MAX_STEP);
        for squad in &mut self.squads {
            match squad.target {
                None => {
                    squad.halt -= step;
                    if squad.halt <= 0.0 {
                        squad.target = Some(pick_target(&mut self.random, squad));
                    }
                }
                Some([tx, tz]) => {
                    let (dx, dz) = (tx - squad.x, tz - squad.z);
                    let distance = dx.hypot(dz);
                    if distance < 1.0 {
                        squad.target = None;
                        squad.halt = HALT.0 + self.random.next() * (HALT.1 - HALT.0);
                        continue;
                    }
                    let wanted = dx.atan2(dz);
                    squad.heading = turn_towards(squad.heading, wanted, SQUAD_TURN_RATE * step);
                    if angle_between(squad.heading, wanted).abs() < SQUAD_FACING {
                        let advance = (SQUAD_SPEED * step).min(distance);
                        squad.x += squad.heading.sin() * advance;
                        squad.z += squad.heading.cos() * advance;
                    }
                }
            }
        }

        for unit in &mut self.units {
            let squad = &self.squads[unit.squad];
            let [px, pz] = place(squad, unit.slot);
            let (dx, dz) = (px - unit.x, pz - unit.z);
            let distance = dx.hypot(dz);
            let speed = (distance * CATCH_UP).min(WALK_SPEED);
            let walking = if unit.gait == WALK {
                speed >= STAND_BELOW
            } else {
                speed >= WALK_FROM
            };
            let gait = if walking { WALK } else { IDLE };
            if gait != unit.gait {
                unit.gait = gait;
                unit.frame = 0.0;
            }
            let rate = if walking {
                let advance = speed * step;
                unit.x += dx / distance * advance;
                unit.z += dz / distance * advance;
                unit.heading = turn_towards(unit.heading, dx.atan2(dz), UNIT_TURN_RATE * step);
                // The engine plays a movement animation at the unit's
                // speed (CCmpUnitMotion passes it to SelectMovementAnimation),
                // so the feet keep pace with the ground.
                speed
            } else {
                unit.heading = turn_towards(unit.heading, squad.heading, UNIT_TURN_RATE * step);
                1.0
            };
            let (frame_ms, frames) = timing[gait];
            if frame_ms > 0.0 && frames > 0 {
                unit.frame = (unit.frame + step * 1000.0 * rate / frame_ms) % f64::from(frames);
            }
        }
    }
}

fn place(squad: &Squad, slot: [f64; 2]) -> [f64; 2] {
    let (sin, cos) = squad.heading.sin_cos();
    // Ahead is (sin, cos); to the right of it is (cos, -sin).
    [
        squad.x + cos * slot[0] + sin * slot[1],
        squad.z - sin * slot[0] + cos * slot[1],
    ]
}

fn pick_target(random: &mut Random, squad: &Squad) -> [f64; 2] {
    loop {
        let angle = random.next() * TAU;
        // Uniform over the disc.
        let radius = random.next().sqrt() * FIELD_RADIUS;
        let target = [radius * angle.sin(), radius * angle.cos()];
        if (target[0] - squad.x).hypot(target[1] - squad.z) >= MIN_LEG {
            return target;
        }
    }
}

/// The signed angle from `from` to `to`, in -PI..PI.
fn angle_between(from: f64, to: f64) -> f64 {
    (to - from + PI).rem_euclid(TAU) - PI
}

fn turn_towards(from: f64, to: f64, most: f64) -> f64 {
    from + angle_between(from, to).clamp(-most, most)
}

/// xorshift32: a fixed sequence, so every load starts the same.
struct Random(u32);

impl Random {
    /// In 0..1.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        f64::from(self.0) / 4_294_967_296.0
    }

    /// In -1..1.
    fn signed(&mut self) -> f64 {
        self.next() * 2.0 - 1.0
    }
}
