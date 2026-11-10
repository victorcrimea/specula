//! The random half of the engine's variant choice, which both the client
//! and the server need: `CActorDef::PickSelectionsAtRandom` and
//! `CObjectBase::CalculateRandomRemainingSelections`, from a seed, with
//! `std::mt19937` and libstdc++'s `uniform_int_distribution` (GCC 13), so an
//! entity looks as it does to a player on Linux. The server works the
//! choices out for a seat, which is never sent the seed: an actor's seed is
//! by default its entity id. The client builds the model from them
//! (`specula-web/src/variation.rs`).

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::files::Definition;
use crate::files::Manifest;
use crate::files::NONE;
use crate::files::Object;

/// default.cfg `max_actor_quality`.
pub const MAX_ACTOR_QUALITY: u8 = 200;
pub const MAX_QUALITY: u8 = 255;
/// Props nest a few levels; a cycle in a modded actor must end.
pub const MAX_DEPTH: usize = 8;

pub type Selections = BTreeSet<String>;

/// `CActorDef::GetBase`: the first base at or above `quality`.
pub fn base(object: &Object, quality: u8) -> (u8, &Definition) {
    let (level, definition) = object
        .bases
        .iter()
        .find(|(level, _)| *level >= quality)
        .or(object.bases.last())
        .expect("a manifest object has a base");
    (*level, &object.definitions[*definition as usize])
}

/// What `CUnit::ReloadObject` and `CObjectManager::FindObjectVariation`
/// pick at random for an object of `manifest` with `entity`'s selections
/// (lower case): the actor's selections, then the remaining ones at the
/// highest quality and at the configured one. `CalculateVariationKey`
/// looks them up after `entity`, in this order; a name an earlier set
/// already holds is left out of a later one, which cannot change what the
/// lookup finds.
pub fn chosen(manifest: &Manifest, object: u32, entity: &Selections, seed: u32) -> [Selections; 3] {
    let object = object as usize;
    if object >= manifest.objects.len() {
        return Default::default();
    }
    let actor = random_remaining(manifest, object, MAX_QUALITY, seed, &[]);
    let selections = [entity.clone(), actor];
    let mut high = random_remaining(manifest, object, MAX_QUALITY, seed, &selections);
    let mut remaining = random_remaining(manifest, object, MAX_ACTOR_QUALITY, seed, &selections);
    let [entity, mut actor] = selections;
    actor.retain(|name| !entity.contains(name));
    high.retain(|name| !entity.contains(name) && !actor.contains(name));
    remaining
        .retain(|name| !entity.contains(name) && !actor.contains(name) && !high.contains(name));
    [actor, high, remaining]
}

/// The overload taking a seed: a fresh generator, and the initial
/// selections added to the result.
fn random_remaining(
    manifest: &Manifest,
    object: usize,
    quality: u8,
    seed: u32,
    initial: &[Selections],
) -> Selections {
    let mut rng = Mt19937::new(seed);
    let mut remaining = random_remaining_with(manifest, object, quality, &mut rng, initial, 0);
    for set in initial {
        remaining.extend(set.iter().cloned());
    }
    remaining
}

fn random_remaining_with(
    manifest: &Manifest,
    object: usize,
    quality: u8,
    rng: &mut Mt19937,
    initial: &[Selections],
    depth: usize,
) -> Selections {
    let (level, definition) = base(&manifest.objects[object], quality);
    let mut remaining = Selections::new();
    // A multimap by point name: ordered, equal points in insertion order.
    let mut chosen_props: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for group in &definition.groups {
        if group.is_empty() {
            continue;
        }
        let mut chosen = None;
        if group.len() == 1 {
            chosen = Some(0);
        } else {
            for set in initial {
                if let Some(index) = group.iter().position(|variant| set.contains(&variant.name)) {
                    chosen = Some(index);
                    break;
                }
            }
            if chosen.is_none() {
                let total: i32 = group.iter().map(|variant| variant.frequency).sum();
                let all_zero = total == 0;
                let total = if all_zero { group.len() as i32 } else { total };
                let mut pick = rng.uniform_int(0, total - 1);
                let mut index = 0;
                for (i, variant) in group.iter().enumerate() {
                    pick -= if all_zero { 1 } else { variant.frequency };
                    if pick < 0 {
                        index = i;
                        break;
                    }
                }
                remaining.insert(group[index].name.clone());
                chosen = Some(index);
            }
        }
        let variant = &group[chosen.unwrap_or(0)];
        for prop in &variant.props {
            chosen_props.remove(&prop.point);
        }
        for prop in &variant.props {
            if prop.object != NONE {
                chosen_props
                    .entry(prop.point.clone())
                    .or_default()
                    .push(prop.object);
            }
        }
    }
    if depth >= MAX_DEPTH {
        return remaining;
    }
    for objects in chosen_props.values() {
        for &prop in objects {
            let mut prop_initial = initial.to_vec();
            if !remaining.is_empty() {
                prop_initial.push(remaining.clone());
            }
            let prop_remaining = random_remaining_with(
                manifest,
                prop as usize,
                level,
                rng,
                &prop_initial,
                depth + 1,
            );
            remaining.extend(prop_remaining);
        }
    }
    remaining
}

/// `std::mt19937`.
pub struct Mt19937 {
    state: [u32; 624],
    index: usize,
}

impl Mt19937 {
    pub fn new(seed: u32) -> Self {
        let mut state = [0u32; 624];
        state[0] = seed;
        for i in 1..624 {
            let previous = state[i - 1];
            state[i] = 1_812_433_253u32
                .wrapping_mul(previous ^ (previous >> 30))
                .wrapping_add(i as u32);
        }
        Mt19937 { state, index: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7FFF_FFFF);
                let mut value = self.state[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    value ^= 0x9908_B0DF;
                }
                self.state[i] = value;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^= y >> 18;
        y
    }

    /// `std::uniform_int_distribution<int>(low, high)` in libstdc++: the
    /// generator spans exactly 32 bits, so `_S_nd` downscales with a 64-bit
    /// product (Lemire's method).
    fn uniform_int(&mut self, low: i32, high: i32) -> i32 {
        let range = (high as u32).wrapping_sub(low as u32).wrapping_add(1);
        if range == 0 {
            // The whole 32-bit range.
            return low.wrapping_add(self.next() as i32);
        }
        let mut product = u64::from(self.next()) * u64::from(range);
        let mut low_bits = product as u32;
        if low_bits < range {
            let threshold = range.wrapping_neg() % range;
            while low_bits < threshold {
                product = u64::from(self.next()) * u64::from(range);
                low_bits = product as u32;
            }
        }
        low.wrapping_add((product >> 32) as i32)
    }

    /// `std::uniform_real_distribution<float>(low, high)` in libstdc++:
    /// `generate_canonical<float, 24>` takes one 32-bit draw, kept below 1.
    pub fn uniform_real(&mut self, low: f32, high: f32) -> f32 {
        let canonical = (self.next() as f32 / 4_294_967_296.0).min(1.0 - f32::EPSILON / 2.0);
        canonical * (high - low) + low
    }
}
