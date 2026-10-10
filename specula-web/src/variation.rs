//! Which variant of an actor an entity shows, chosen as the engine chooses
//! it: `CUnit::ReloadObject`, `CActorDef::PickSelectionsAtRandom`,
//! `CObjectManager::FindObjectVariation`, and from
//! graphics/ObjectBase.cpp `CalculateRandomRemainingSelections`,
//! `CalculateVariationKey` and `BuildVariation`, then
//! `CObjectEntry::BuildVariation` for the props. The random choices use
//! `std::mt19937` and libstdc++'s `uniform_int_distribution` (GCC 13), so
//! an entity looks as it does to a player on Linux.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::rc::Rc;

use specula_assets::files::Decal;
use specula_assets::files::Definition;
use specula_assets::files::Emitter;
use specula_assets::files::FLAG_PLAYER_COLOR;
use specula_assets::files::FileRef;
use specula_assets::files::Manifest;
use specula_assets::files::NONE;
use specula_assets::files::Object;

/// default.cfg `max_actor_quality`.
const MAX_ACTOR_QUALITY: u8 = 200;
const MAX_QUALITY: u8 = 255;
/// `CObjectEntry::BuildVariation` attaches these to the point after the
/// prefix only while a projectile is loaded; they are not drawn.
const AMMO_PREFIX: &str = "loaded-";
/// `CObjectEntry::BuildVariation` keeps this point's actor as the one to
/// shoot rather than drawing it.
const PROJECTILE_POINT: &str = "projectile";
/// Props nest a few levels; a cycle in a modded actor must end.
const MAX_DEPTH: usize = 8;

type Selections = BTreeSet<String>;

/// One model of an entity's tree, ready to draw.
pub struct Model {
    pub shape: Shape,
    pub texture: Option<FileRef>,
    /// normTex, specTex and aoTex, where the material samples them.
    pub maps: [Option<FileRef>; 3],
    /// The material's `effectSettings`.
    pub effect: [f32; 4],
    pub player_color: bool,
    pub alpha_ref: f32,
    /// By lower-case name, as `CObjectEntry::m_Animations`.
    pub animations: Vec<ModelAnimation>,
    /// (prop point, model), in the order the engine adds them.
    pub props: Vec<(String, Model)>,
    /// The object in the manifest this model shoots: its own `projectile`
    /// prop, else the first prop's (a rider's), as `m_ProjectileModelName`.
    pub projectile: Option<u32>,
    /// The points a `loaded-` prop names, in the order the engine adds
    /// them; the last one the mesh has holds the ammo, as `AddAmmoProp`.
    pub ammo_points: Vec<String>,
}

/// What `CObjectEntry::BuildVariation` makes of a variation.
#[derive(Clone)]
pub enum Shape {
    Mesh(FileRef),
    /// `CModelDecal`: the texture laid on the terrain; it has no props
    /// and no animations.
    Decal(Decal),
    /// `CModelParticleEmitter`; likewise.
    Particles(Rc<Emitter>),
}

pub struct ModelAnimation {
    pub name: String,
    pub id: String,
    pub frequency: i32,
    pub speed: f32,
    /// Fractions of the duration, -1 for none.
    pub event: f32,
    pub sound: f32,
    pub file: FileRef,
}

/// The model tree an entity shows, or `None` when its variation has
/// no decal, particles or mesh, which the engine makes a dummy model of.
/// `entity` holds the entity's selections, lower case.
pub fn resolve(manifest: &Manifest, entity: &Selections, seed: u32) -> Option<Model> {
    resolve_object(manifest, 0, entity, seed)
}

/// `resolve` for another object of the manifest as the actor, as the
/// engine makes a unit of a projectile actor.
pub fn resolve_object(
    manifest: &Manifest,
    object: u32,
    entity: &Selections,
    seed: u32,
) -> Option<Model> {
    let object = object as usize;
    if object >= manifest.objects.len() {
        return None;
    }
    let actor = random_remaining(manifest, object, MAX_QUALITY, seed, &[]);
    let selections = [entity.clone(), actor];
    let high = random_remaining(manifest, object, MAX_QUALITY, seed, &selections);
    let remaining = random_remaining(manifest, object, MAX_ACTOR_QUALITY, seed, &selections);
    let complete = [&selections[0], &selections[1], &high, &remaining];
    build(manifest, object, MAX_ACTOR_QUALITY, &complete, 0)
}

/// `CActorDef::GetBase`: the first base at or above `quality`.
fn base(object: &Object, quality: u8) -> (u8, &Definition) {
    let (level, definition) = object
        .bases
        .iter()
        .find(|(level, _)| *level >= quality)
        .or(object.bases.last())
        .expect("a manifest object has a base");
    (*level, &object.definitions[*definition as usize])
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

/// `CalculateVariationKey` for the object's own groups: the first variant
/// any selection set names, by the sets' priority, else the first.
fn choices(definition: &Definition, selections: &[&Selections]) -> Vec<usize> {
    definition
        .groups
        .iter()
        .map(|group| {
            if group.len() == 1 {
                return 0;
            }
            selections
                .iter()
                .find_map(|set| group.iter().position(|variant| set.contains(&variant.name)))
                .unwrap_or(0)
        })
        .collect()
}

/// `CObjectBase::BuildVariation` and `CObjectEntry::BuildVariation`.
fn build(
    manifest: &Manifest,
    object: usize,
    quality: u8,
    selections: &[&Selections],
    depth: usize,
) -> Option<Model> {
    let (level, definition) = base(&manifest.objects[object], quality);
    let mut mesh = None;
    let mut decal = None;
    let mut particles = None;
    let mut texture = None;
    let mut maps = [None; 3];
    let mut props: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut animations: BTreeMap<String, Vec<&specula_assets::files::Animation>> = BTreeMap::new();
    for (group, choice) in definition
        .groups
        .iter()
        .zip(choices(definition, selections))
    {
        let Some(variant) = group.get(choice) else {
            continue;
        };
        if variant.mesh.is_some() {
            mesh = variant.mesh;
        }
        if let Some(found) = variant.decal
            && found.size_x != 0.0
            && found.size_z != 0.0
        {
            decal = Some(found);
        }
        if variant.particles.is_some() {
            particles = variant.particles.as_ref();
        }
        if variant.texture.is_some() {
            texture = variant.texture;
        }
        let found = [
            variant.normal_texture,
            variant.specular_texture,
            variant.ao_texture,
        ];
        for (map, found) in maps.iter_mut().zip(found) {
            if found.is_some() {
                *map = found;
            }
        }
        for prop in &variant.props {
            props.remove(&prop.point);
        }
        for prop in &variant.props {
            if prop.object != NONE {
                props
                    .entry(prop.point.clone())
                    .or_default()
                    .push(prop.object);
            }
        }
        for animation in &variant.animations {
            animations.remove(&animation.name);
        }
        for animation in &variant.animations {
            animations
                .entry(animation.name.clone())
                .or_default()
                .push(animation);
        }
    }
    // The engine tries a decal before the mesh.
    if let Some(decal) = decal {
        return Some(Model {
            shape: Shape::Decal(decal),
            texture,
            maps,
            effect: definition.effect,
            player_color: definition.flags & FLAG_PLAYER_COLOR != 0,
            alpha_ref: definition.alpha_ref,
            animations: Vec::new(),
            props: Vec::new(),
            projectile: None,
            ammo_points: Vec::new(),
        });
    }
    // Then particles, which replace the mesh and its props.
    if let Some(emitter) = particles {
        return Some(Model {
            shape: Shape::Particles(Rc::new(emitter.clone())),
            texture: None,
            maps: [None; 3],
            effect: definition.effect,
            player_color: false,
            alpha_ref: 0.0,
            animations: Vec::new(),
            props: Vec::new(),
            projectile: None,
            ammo_points: Vec::new(),
        });
    }
    // Without a mesh the engine makes a dummy model, which has no props.
    let mesh = mesh?;

    let mut model_animations: Vec<ModelAnimation> = animations
        .values()
        .flatten()
        .filter_map(|animation| {
            Some(ModelAnimation {
                name: animation.name.to_lowercase(),
                id: animation.id.clone(),
                frequency: animation.frequency,
                speed: animation.speed,
                event: animation.event,
                sound: animation.sound,
                file: animation.file?,
            })
        })
        .collect();
    // A stable sort keeps equal names in insertion order, as the multimap.
    model_animations.sort_by(|a, b| a.name.cmp(&b.name));

    let mut model_props: Vec<(String, Model)> = Vec::new();
    let mut projectile = props
        .get(PROJECTILE_POINT)
        .and_then(|objects| objects.last().copied());
    let mut ammo_points = Vec::new();
    if depth < MAX_DEPTH {
        for (point, objects) in &props {
            if point == PROJECTILE_POINT {
                continue;
            }
            if let Some(ammo) = point.strip_prefix(AMMO_PREFIX) {
                ammo_points.push(ammo.to_string());
                continue;
            }
            for &prop in objects {
                if let Some(model) = build(manifest, prop as usize, level, selections, depth + 1) {
                    projectile = projectile.or(model.projectile);
                    model_props.push((point.clone(), model));
                }
            }
        }
    }
    Some(Model {
        shape: Shape::Mesh(mesh),
        texture,
        maps,
        effect: definition.effect,
        player_color: definition.flags & FLAG_PLAYER_COLOR != 0,
        alpha_ref: definition.alpha_ref,
        animations: model_animations,
        props: model_props,
        projectile,
        ammo_points,
    })
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
