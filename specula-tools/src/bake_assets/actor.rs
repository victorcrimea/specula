//! Loads actors whole, every group and variant at every quality level,
//! porting `CActorDef::Load`, `CObjectBase::Load`, `LoadVariant` and
//! `GetQualitySplits` (graphics/ObjectBase.cpp), so the client can make the
//! engine's per-entity variant choice itself.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use roxmltree::Node;

const MAX_QUALITY: u8 = 255;
/// `MAX_LEVELS_PER_ACTOR_DEF`.
const MAX_LEVELS: usize = 5;
/// Variant files include variant files; a cycle would never end.
const MAX_INCLUDE_DEPTH: usize = 8;
const DEFAULT_MATERIAL: &str = "default.xml";

pub struct ActorDef {
    /// (quality, index into `definitions`), rising, the last at 255, as
    /// `CActorDef::m_ObjectBases` after the prop splits; a split shares
    /// the definition of the base it was copied from.
    pub bases: Vec<(u8, usize)>,
    pub definitions: Vec<Definition>,
}

pub struct Definition {
    pub material: Material,
    pub groups: Vec<Vec<Variant>>,
}

#[derive(Clone, Copy, Default)]
pub struct Material {
    pub player_color: bool,
    pub transparent: bool,
}

#[derive(Default)]
pub struct Variant {
    /// Lower case, as the engine compares selections.
    pub name: String,
    pub frequency: i32,
    /// Paths under the mod.
    pub mesh: Option<String>,
    pub base_texture: Option<String>,
    /// (attach point, actor); an empty actor only removes.
    pub props: Vec<(String, String)>,
    pub animations: Vec<Animation>,
}

pub struct Animation {
    pub name: String,
    pub id: String,
    pub frequency: i32,
    /// A path under the mod.
    pub file: Option<String>,
    pub speed: f32,
    /// Fractions of the duration, -1 for none.
    pub event: f32,
    pub load: f32,
}

/// `CObjectManager::FindActorDef`: each actor is loaded once, and a failed
/// one stays failed.
pub struct Loader<'a> {
    mod_dir: &'a Path,
    actors: BTreeMap<String, Result<Rc<ActorDef>, String>>,
    /// Actors being loaded, so a prop cycle fails instead of recursing.
    loading: HashSet<String>,
    materials: BTreeMap<String, Material>,
}

impl<'a> Loader<'a> {
    pub fn new(mod_dir: &'a Path) -> Self {
        Loader {
            mod_dir,
            actors: BTreeMap::new(),
            loading: HashSet::new(),
            materials: BTreeMap::new(),
        }
    }

    /// `name` is relative to art/actors.
    pub fn load(&mut self, name: &str) -> Result<Rc<ActorDef>, String> {
        if let Some(result) = self.actors.get(name) {
            return result.clone();
        }
        if !self.loading.insert(name.to_string()) {
            return Err(format!("{name}: props itself"));
        }
        let result = self.load_actor(name).map(Rc::new);
        self.loading.remove(name);
        self.actors.insert(name.to_string(), result.clone());
        result
    }

    /// Every actor tried so far, failed ones included.
    pub fn actors(&self) -> &BTreeMap<String, Result<Rc<ActorDef>, String>> {
        &self.actors
    }

    fn load_actor(&mut self, name: &str) -> Result<ActorDef, String> {
        let xml = read(self.mod_dir, &format!("art/actors/{name}"))?;
        let document =
            roxmltree::Document::parse(&xml).map_err(|error| format!("{name}: {error}"))?;
        let root = document.root_element();
        let mut actor = ActorDef {
            bases: Vec::new(),
            definitions: Vec::new(),
        };
        match root.tag_name().name() {
            "actor" => {
                let definition = self.load_base(name, root, MAX_QUALITY)?;
                actor.definitions.push(definition);
                actor.bases.push((MAX_QUALITY, 0));
            }
            "qualitylevels" => {
                if let Some(version) = root.attribute("version")
                    && to_int(version) != 1
                {
                    return Err(format!("{name}: qualitylevels version {version}"));
                }
                let inline = root.children().find(|node| node.has_tag_name("inline"));
                let mut quality: u8 = 0;
                for node in root.children().filter(|node| node.has_tag_name("actor")) {
                    let mut file = None;
                    let mut use_inline = false;
                    let mut found_quality = false;
                    for attribute in node.attributes() {
                        match attribute.name() {
                            "quality" => {
                                let value = get_quality(attribute.value());
                                if value > i32::from(MAX_QUALITY) {
                                    return Err(format!("{name}: quality above 255"));
                                }
                                if value <= i32::from(quality) {
                                    return Err(format!("{name}: qualities not rising"));
                                }
                                quality = value as u8;
                                found_quality = true;
                            }
                            "file" => file = Some(attribute.value().to_string()),
                            "inline" => use_inline = true,
                            _ => {}
                        }
                    }
                    if !found_quality {
                        quality = MAX_QUALITY;
                    }
                    let definition = if use_inline {
                        let inline = inline.ok_or_else(|| format!("{name}: no inline actor"))?;
                        self.load_base(name, inline, quality)?
                    } else if let Some(file) = file.filter(|file| !file.is_empty()) {
                        let included = read(self.mod_dir, &format!("art/actors/{file}"))?;
                        let document = roxmltree::Document::parse(&included)
                            .map_err(|error| format!("{file}: {error}"))?;
                        let root = document.root_element();
                        if root.has_tag_name("qualitylevels") {
                            return Err(format!("{name}: included {file} has quality levels"));
                        }
                        self.load_base(name, root, quality)?
                    } else {
                        self.load_base(name, node, quality)?
                    };
                    actor.bases.push((quality, actor.definitions.len()));
                    actor.definitions.push(definition);
                }
                if quality != MAX_QUALITY {
                    return Err(format!("{name}: the highest quality is {quality}, not 255"));
                }
            }
            other => return Err(format!("{name}: unexpected root element {other}")),
        }
        self.split_qualities(name, &mut actor)?;
        Ok(actor)
    }

    /// The end of `CActorDef::Load`: a base gains copies at the quality
    /// levels its props split at, so `GetBase` hands each prop the right
    /// level.
    fn split_qualities(&mut self, name: &str, actor: &mut ActorDef) -> Result<(), String> {
        let mut splits: Vec<u8> = actor.bases.iter().map(|(quality, _)| *quality).collect();
        for &(quality, definition) in &actor.bases.clone() {
            // GetQualitySplits.
            let at = splits.partition_point(|split| *split < quality);
            if splits.get(at) != Some(&quality) {
                splits.insert(at, quality);
            }
            let props: Vec<String> = actor.definitions[definition]
                .groups
                .iter()
                .flatten()
                .flat_map(|variant| variant.props.iter())
                .filter(|(_, prop)| !prop.is_empty())
                .map(|(_, prop)| prop.clone())
                .collect();
            for prop in props {
                let Ok(prop) = self.load(&prop) else {
                    continue;
                };
                let levels: Vec<u8> = prop.bases.iter().map(|(quality, _)| *quality).collect();
                if levels.len() <= 1 {
                    continue;
                }
                for level in levels {
                    let at = splits.partition_point(|split| *split < level);
                    if splits.get(at) != Some(&level) {
                        splits.insert(at, level);
                    }
                }
            }
        }
        if splits.len() > MAX_LEVELS {
            return Err(format!("{name}: {} quality levels", splits.len()));
        }
        let mut bases = Vec::with_capacity(splits.len());
        let mut split = splits.iter().peekable();
        for &(quality, definition) in &actor.bases {
            while let Some(&&level) = split.peek()
                && level < quality
            {
                bases.push((level, definition));
                split.next();
            }
            if split.peek() == Some(&&quality) {
                split.next();
            }
            bases.push((quality, definition));
        }
        actor.bases = bases;
        Ok(())
    }

    /// `CObjectBase::Load` at `quality`.
    fn load_base(&mut self, actor: &str, node: Node, quality: u8) -> Result<Definition, String> {
        let mut material = DEFAULT_MATERIAL.to_string();
        let mut groups = Vec::new();
        for child in node.children().filter(Node::is_element) {
            if should_skip(child, quality) {
                continue;
            }
            match child.tag_name().name() {
                "group" => {
                    let mut group = Vec::new();
                    for element in child.children().filter(Node::is_element) {
                        if should_skip(element, quality) {
                            continue;
                        }
                        let mut variant = Variant::default();
                        self.load_variant(element, &mut variant, 0)?;
                        group.push(variant);
                    }
                    if group.is_empty() {
                        return Err(format!("{actor}: a group has no variants"));
                    }
                    groups.push(group);
                }
                "material" => material = text(child),
                _ => {}
            }
        }
        Ok(Definition {
            material: self.material(&material),
            groups,
        })
    }

    fn load_variant(
        &mut self,
        node: Node,
        variant: &mut Variant,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_INCLUDE_DEPTH {
            return Err("variant files nest too deeply".into());
        }
        if !node.has_tag_name("variant") {
            return Err(format!(
                "unexpected variant element {}",
                node.tag_name().name()
            ));
        }
        // The included file loads first, so the element can override it.
        if let Some(file) = node.attribute("file") {
            let xml = read(self.mod_dir, &format!("art/variants/{file}"))?;
            let document =
                roxmltree::Document::parse(&xml).map_err(|error| format!("{file}: {error}"))?;
            self.load_variant(document.root_element(), variant, depth + 1)?;
        }
        if let Some(name) = node.attribute("name") {
            variant.name = name.to_lowercase();
        }
        if let Some(frequency) = node.attribute("frequency") {
            variant.frequency = to_int(frequency);
        }
        for option in node.children().filter(Node::is_element) {
            match option.tag_name().name() {
                "mesh" => variant.mesh = Some(format!("art/meshes/{}", text(option))),
                "textures" => {
                    for texture in option.children().filter(Node::is_element) {
                        if !texture.has_tag_name("texture") {
                            return Err("<textures> holds something else".into());
                        }
                        // Only the colour map is drawn. The last one wins,
                        // as ModelRenderer binds the samplers in order.
                        if texture.attribute("name") == Some("baseTex")
                            && let Some(file) = texture.attribute("file")
                        {
                            variant.base_texture = Some(format!("art/textures/skins/{file}"));
                        }
                    }
                }
                "animations" => {
                    for element in option.children().filter(Node::is_element) {
                        if !element.has_tag_name("animation") {
                            return Err("<animations> holds something else".into());
                        }
                        variant.animations.push(load_animation(element));
                    }
                }
                "props" => {
                    for prop in option.children().filter(Node::is_element) {
                        let point = prop.attribute("attachpoint").unwrap_or_default();
                        let actor = prop.attribute("actor").unwrap_or_default();
                        variant.props.push((point.to_string(), actor.to_string()));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The shader defines the client follows; a missing material falls
    /// back to the defaults, as `CMaterialManager::LoadMaterial` does.
    fn material(&mut self, name: &str) -> Material {
        if let Some(material) = self.materials.get(name) {
            return *material;
        }
        let material = match read(self.mod_dir, &format!("art/materials/{name}")) {
            Ok(xml) => match roxmltree::Document::parse(&xml) {
                Ok(document) => {
                    let defined = |define: &str| {
                        document.root_element().children().any(|node| {
                            node.has_tag_name("define")
                                && node.attribute("name") == Some(define)
                                && node.attribute("value") != Some("0")
                        })
                    };
                    Material {
                        player_color: defined("USE_PLAYERCOLOR"),
                        transparent: defined("USE_TRANSPARENT"),
                    }
                }
                Err(error) => {
                    eprintln!("material {name}: {error}");
                    Material::default()
                }
            },
            Err(error) => {
                eprintln!("material {error}");
                Material::default()
            }
        };
        self.materials.insert(name.to_string(), material);
        material
    }
}

fn load_animation(node: Node) -> Animation {
    let mut animation = Animation {
        name: String::new(),
        id: String::new(),
        frequency: 0,
        file: None,
        speed: 1.0,
        event: -1.0,
        load: -1.0,
    };
    for attribute in node.attributes() {
        let value = attribute.value();
        match attribute.name() {
            "name" => animation.name = value.to_string(),
            "id" => animation.id = value.to_string(),
            "frequency" => animation.frequency = to_int(value),
            "file" => animation.file = Some(format!("art/animation/{value}")),
            "speed" => {
                let speed = to_int(value);
                animation.speed = if speed > 0 { speed as f32 / 100.0 } else { 1.0 };
            }
            "event" => animation.event = to_float(value).clamp(0.0, 1.0),
            "load" => animation.load = to_float(value).clamp(0.0, 1.0),
            _ => {}
        }
    }
    animation
}

/// `shouldSkip` in `CObjectBase::Load`.
fn should_skip(node: Node, quality: u8) -> bool {
    node.attributes().any(|attribute| match attribute.name() {
        "minquality" => get_quality(attribute.value()) > i32::from(quality),
        "maxquality" => get_quality(attribute.value()) <= i32::from(quality),
        _ => false,
    })
}

fn get_quality(value: &str) -> i32 {
    match value {
        "low" => 100,
        "medium" => 150,
        "high" => 200,
        _ => to_int(value),
    }
}

/// `CStr::ToInt`: the leading number, 0 if there is none.
fn to_int(value: &str) -> i32 {
    let value = value.trim_start();
    let end = value
        .char_indices()
        .find(|&(at, c)| !(c.is_ascii_digit() || (at == 0 && (c == '-' || c == '+'))))
        .map_or(value.len(), |(at, _)| at);
    value[..end].parse().unwrap_or(0)
}

/// `CStr::ToFloat`: the leading number, 0 if there is none.
fn to_float(value: &str) -> f32 {
    let value = value.trim_start();
    (1..=value.len())
        .rev()
        .filter(|end| value.is_char_boundary(*end))
        .find_map(|end| value[..end].parse().ok())
        .unwrap_or(0.0)
}

fn text(node: Node) -> String {
    node.text().unwrap_or_default().trim().to_string()
}

fn read(mod_dir: &Path, path: &str) -> Result<String, String> {
    fs::read_to_string(mod_dir.join(path)).map_err(|error| format!("{path}: {error}"))
}
