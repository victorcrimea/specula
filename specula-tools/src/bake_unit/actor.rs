//! Resolves an actor into the tree of models the engine would build for it,
//! following `CActorDef::Load`, `CObjectBase::Load`, `LoadVariant` and
//! `BuildVariation` (graphics/ObjectBase.cpp) at the highest quality level.
//!
//! The engine picks one variant per group at random by frequency; here it
//! is always the first variant with a non-zero frequency, so a bake is
//! reproducible.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use roxmltree::Node;

/// Prop actors can carry props of their own; a cycle would never end.
const MAX_DEPTH: usize = 8;
const DEFAULT_MATERIAL: &str = "default.xml";

pub struct Part {
    /// The actor it came from, relative to art/actors.
    pub actor: String,
    /// Relative to art/meshes.
    pub mesh: String,
    /// The `baseTex`, relative to art/textures/skins.
    pub texture: Option<String>,
    pub material: Material,
    pub props: Vec<Prop>,
    /// By animation name, in the order the actor lists them.
    pub animations: BTreeMap<String, Vec<Animation>>,
}

pub struct Prop {
    pub point: String,
    pub part: Part,
}

#[derive(Clone)]
pub struct Animation {
    /// Relative to art/animation.
    pub file: String,
    /// Playback rate; `speed="80"` in the actor is 0.8.
    pub speed: f32,
}

pub struct Material {
    pub player_color: bool,
    pub transparent: bool,
}

#[derive(Default, Clone)]
struct Variant {
    frequency: u32,
    mesh: Option<String>,
    textures: Vec<(String, String)>,
    props: Vec<(String, String)>,
    animations: Vec<(String, Option<Animation>)>,
}

pub fn load(mod_dir: &Path, actor: &str) -> Result<Part, String> {
    load_part(mod_dir, actor, 0)
}

fn load_part(mod_dir: &Path, actor: &str, depth: usize) -> Result<Part, String> {
    if depth > MAX_DEPTH {
        return Err(format!("props nest deeper than {MAX_DEPTH} at {actor}"));
    }
    let xml = read(mod_dir, &format!("art/actors/{actor}"))?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| format!("{actor}: {error}"))?;
    let root = document.root_element();
    // An actor file can hold several quality levels; take the highest.
    let actor_node = match root.tag_name().name() {
        "actor" => root,
        "qualitylevels" => {
            let best = root
                .children()
                .rfind(|node| node.has_tag_name("actor"))
                .ok_or_else(|| format!("{actor}: no actor in qualitylevels"))?;
            if best.attribute("inline").is_some() {
                root.children()
                    .find(|node| node.has_tag_name("inline"))
                    .ok_or_else(|| format!("{actor}: inline actor missing"))?
            } else if let Some(file) = best.attribute("file") {
                let included = read(mod_dir, &format!("art/actors/{file}"))?;
                let document = roxmltree::Document::parse(&included)
                    .map_err(|error| format!("{file}: {error}"))?;
                return resolve(mod_dir, actor, document.root_element(), depth);
            } else {
                best
            }
        }
        other => return Err(format!("{actor}: unexpected root element {other}")),
    };
    resolve(mod_dir, actor, actor_node, depth)
}

fn resolve(mod_dir: &Path, actor: &str, node: Node, depth: usize) -> Result<Part, String> {
    let mut material = DEFAULT_MATERIAL.to_string();
    let mut chosen = Vec::new();
    for child in node.children().filter(Node::is_element) {
        if skipped_at_highest_quality(child) {
            continue;
        }
        match child.tag_name().name() {
            "group" => {
                let mut variants = Vec::new();
                for element in child.children().filter(Node::is_element) {
                    if skipped_at_highest_quality(element) {
                        continue;
                    }
                    let mut variant = Variant::default();
                    load_variant(mod_dir, element, &mut variant, 0)?;
                    variants.push(variant);
                }
                let pick = variants
                    .iter()
                    .position(|variant| variant.frequency > 0)
                    .unwrap_or(0);
                if let Some(variant) = variants.into_iter().nth(pick) {
                    chosen.push(variant);
                }
            }
            "material" => {
                if let Some(text) = child.text() {
                    material = text.trim().to_string();
                }
            }
            _ => {}
        }
    }

    // BuildVariation: later groups override earlier ones by key.
    let mut mesh = None;
    let mut textures: BTreeMap<String, String> = BTreeMap::new();
    let mut props: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut animations: BTreeMap<String, Vec<Animation>> = BTreeMap::new();
    for variant in chosen {
        if variant.mesh.is_some() {
            mesh = variant.mesh;
        }
        for (name, _) in &variant.props {
            props.remove(name);
        }
        for (name, actor) in variant.props {
            // An empty actor only removes what an earlier group attached.
            if !actor.is_empty() {
                props.entry(name).or_default().push(actor);
            }
        }
        for (name, _) in &variant.animations {
            animations.remove(name);
        }
        for (name, animation) in variant.animations {
            let list = animations.entry(name).or_default();
            if let Some(animation) = animation {
                list.push(animation);
            }
        }
        for (name, file) in variant.textures {
            textures.insert(name, file);
        }
    }
    animations.retain(|_, list| !list.is_empty());

    let mesh = mesh.ok_or_else(|| format!("{actor}: no mesh"))?;
    let mut parts = Vec::new();
    for (point, actors) in props {
        for prop_actor in actors {
            let part = load_part(mod_dir, &prop_actor, depth + 1)?;
            parts.push(Prop {
                point: point.clone(),
                part,
            });
        }
    }
    Ok(Part {
        actor: actor.to_string(),
        mesh,
        texture: textures.remove("baseTex"),
        material: load_material(mod_dir, &material)?,
        props: parts,
        animations,
    })
}

/// `shouldSkip` in `CObjectBase::Load` at MAX_QUALITY: any `maxquality`
/// is at most the maximum, and no `minquality` exceeds it.
fn skipped_at_highest_quality(node: Node) -> bool {
    node.attribute("maxquality").is_some()
}

fn load_variant(
    mod_dir: &Path,
    node: Node,
    variant: &mut Variant,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("variant files nest too deeply".into());
    }
    // The included file loads first, so the element can override it.
    if let Some(file) = node.attribute("file") {
        let xml = read(mod_dir, &format!("art/variants/{file}"))?;
        let document =
            roxmltree::Document::parse(&xml).map_err(|error| format!("{file}: {error}"))?;
        load_variant(mod_dir, document.root_element(), variant, depth + 1)?;
    }
    if let Some(frequency) = node.attribute("frequency") {
        variant.frequency = frequency.trim().parse().unwrap_or(0);
    }
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "mesh" => {
                if let Some(text) = child.text() {
                    variant.mesh = Some(text.trim().to_string());
                }
            }
            "textures" => {
                for texture in child.children().filter(|node| node.has_tag_name("texture")) {
                    if let (Some(name), Some(file)) =
                        (texture.attribute("name"), texture.attribute("file"))
                    {
                        variant.textures.push((name.to_string(), file.to_string()));
                    }
                }
            }
            "props" => {
                for prop in child.children().filter(|node| node.has_tag_name("prop")) {
                    if let Some(point) = prop.attribute("attachpoint") {
                        let actor = prop.attribute("actor").unwrap_or_default();
                        variant.props.push((point.to_string(), actor.to_string()));
                    }
                }
            }
            "animations" => {
                for animation in child
                    .children()
                    .filter(|node| node.has_tag_name("animation"))
                {
                    let Some(name) = animation.attribute("name") else {
                        continue;
                    };
                    // ObjectBase.cpp: a positive integer percentage, else 1.
                    let speed = animation
                        .attribute("speed")
                        .and_then(|speed| speed.trim().parse::<i32>().ok())
                        .filter(|speed| *speed > 0)
                        .map_or(1.0, |speed| speed as f32 / 100.0);
                    let file = animation.attribute("file").map(|file| Animation {
                        file: file.to_string(),
                        speed,
                    });
                    variant.animations.push((name.to_string(), file));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn load_material(mod_dir: &Path, name: &str) -> Result<Material, String> {
    let xml = read(mod_dir, &format!("art/materials/{name}"))?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| format!("{name}: {error}"))?;
    let defined = |define: &str| {
        document.root_element().children().any(|node| {
            node.has_tag_name("define")
                && node.attribute("name") == Some(define)
                && node.attribute("value") != Some("0")
        })
    };
    Ok(Material {
        player_color: defined("USE_PLAYERCOLOR"),
        transparent: defined("USE_TRANSPARENT"),
    })
}

fn read(mod_dir: &Path, path: &str) -> Result<String, String> {
    fs::read_to_string(mod_dir.join(path)).map_err(|error| format!("{path}: {error}"))
}

impl Part {
    /// Every mesh in the tree and every file of the named animations, as
    /// paths under the mod.
    pub fn sources(&self, animations: &[&str], out: &mut Vec<String>) {
        out.push(format!("art/meshes/{}", self.mesh));
        for name in animations {
            if let Some(animation) = self.animations.get(*name).and_then(|list| list.first()) {
                out.push(format!("art/animation/{}", animation.file));
            }
        }
        for prop in &self.props {
            prop.part.sources(&[], out);
        }
    }
}
