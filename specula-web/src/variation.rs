//! Which variant of an actor an entity shows, chosen as the engine chooses
//! it: `CUnit::ReloadObject`, `CActorDef::PickSelectionsAtRandom`,
//! `CObjectManager::FindObjectVariation`, and from
//! graphics/ObjectBase.cpp `CalculateRandomRemainingSelections`,
//! `CalculateVariationKey` and `BuildVariation`, then
//! `CObjectEntry::BuildVariation` for the props. The random choices are
//! `specula_assets::variation`'s, which the server also makes for a seat,
//! whose stream carries them instead of the seed.

use std::collections::BTreeMap;
use std::rc::Rc;

use specula_assets::files::Decal;
use specula_assets::files::Definition;
use specula_assets::files::Emitter;
use specula_assets::files::FLAG_PLAYER_COLOR;
use specula_assets::files::FLAG_WIND;
use specula_assets::files::FileRef;
use specula_assets::files::Manifest;
use specula_assets::files::NONE;
use specula_assets::variation::MAX_ACTOR_QUALITY;
use specula_assets::variation::MAX_DEPTH;
pub use specula_assets::variation::Mt19937;
use specula_assets::variation::Selections;
use specula_assets::variation::base;
use specula_assets::variation::chosen;

/// `CObjectEntry::BuildVariation` attaches these to the point after the
/// prefix only while a projectile is loaded; they are not drawn.
const AMMO_PREFIX: &str = "loaded-";
/// `CObjectEntry::BuildVariation` keeps this point's actor as the one to
/// shoot rather than drawing it.
const PROJECTILE_POINT: &str = "projectile";

/// One model of an entity's tree, ready to draw.
pub struct Model {
    pub shape: Shape,
    pub texture: Option<FileRef>,
    /// normTex, specTex and aoTex, where the material samples them.
    pub maps: [Option<FileRef>; 3],
    /// The material's `effectSettings`.
    pub effect: [f32; 4],
    /// The material's `windData`, when `USE_WIND` sways the mesh.
    pub wind: Option<[f32; 2]>,
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
    resolve_chosen(
        manifest,
        object,
        entity,
        &chosen(manifest, object, entity, seed),
    )
}

/// `resolve_object` with the random choices already made, as a seat's
/// stream carries them.
pub fn resolve_chosen(
    manifest: &Manifest,
    object: u32,
    entity: &Selections,
    chosen: &[Selections; 3],
) -> Option<Model> {
    let object = object as usize;
    if object >= manifest.objects.len() {
        return None;
    }
    let complete = [entity, &chosen[0], &chosen[1], &chosen[2]];
    build(manifest, object, MAX_ACTOR_QUALITY, &complete, 0)
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
            wind: None,
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
            wind: None,
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
        wind: (definition.flags & FLAG_WIND != 0).then_some(definition.wind),
        player_color: definition.flags & FLAG_PLAYER_COLOR != 0,
        alpha_ref: definition.alpha_ref,
        animations: model_animations,
        props: model_props,
        projectile,
        ammo_points,
    })
}
