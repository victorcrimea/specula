//! What each civ's entity templates can show and play: their actors and
//! sound groups, named as the stream names them, so a viewer can fetch a
//! match's assets before the entities appear. Templates are resolved as
//! `CTemplateLoader::LoadTemplateFile` and `CParamNode::ApplyLayer` do,
//! keeping only what is read here.
//!
//! Technologies that swap an actor through `VisualActor/Actor` modifiers
//! are not followed; those actors are still fetched when first seen.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use specula_assets::files::Sinking;

const TEMPLATE_ROOT: &str = "simulation/templates";
const CIV_DIR: &str = "simulation/data/civs";
/// `LoadTemplateFile`'s guard against inheritance loops.
const MAX_DEPTH: u32 = 100;
/// `Identity::GetLang`'s fallback.
const DEFAULT_LANG: &str = "greek";
/// `Identity::Init`'s phenotype without a `Phenotype` element.
const DEFAULT_PHENOTYPE: &str = "default";

#[derive(Default)]
pub struct CivRefs {
    /// Paths under art/actors.
    pub actors: BTreeSet<String>,
    /// Paths under audio/.
    pub sound_groups: BTreeSet<String>,
}

#[derive(Clone, Default)]
struct Node {
    value: String,
    children: BTreeMap<String, Node>,
}

impl Node {
    fn get(&self, path: &str) -> Option<&Node> {
        path.split('/')
            .try_fold(self, |node, name| node.children.get(name))
    }

    fn text(&self, path: &str) -> Option<&str> {
        self.get(path)
            .map(|node| node.value.as_str())
            .filter(|value| !value.is_empty())
    }
}

struct Loader {
    root: PathBuf,
    /// Template name -> its file's text, `None` when there is none.
    files: HashMap<String, Option<String>>,
}

impl Loader {
    /// The file `LoadTemplateFile` reads for a single name: filters first,
    /// then mixins, then the root.
    fn file(&mut self, name: &str) -> Option<&str> {
        if !self.files.contains_key(name) {
            let file = format!("{name}.xml");
            let text = ["special/filter", "mixins", ""]
                .iter()
                .map(|dir| self.root.join(dir).join(&file))
                .find(|path| path.is_file())
                .and_then(|path| fs::read_to_string(path).ok());
            self.files.insert(name.to_string(), text);
        }
        self.files[name].as_deref()
    }

    fn load(&mut self, node: &mut Node, name: &str, depth: u32) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!("{name}: probable inheritance loop"));
        }
        // An actor-only template has no Identity, so no civ.
        if name.starts_with("actor|") {
            return Ok(());
        }
        if let Some((layer, parent)) = name.split_once('|') {
            self.load(node, parent, depth + 1)?;
            return self.load(node, layer, depth + 1);
        }
        let text = self
            .file(name)
            .ok_or_else(|| format!("{name}: no such template"))?
            .to_string();
        let document =
            roxmltree::Document::parse(&text).map_err(|error| format!("{name}: {error}"))?;
        let root = document.root_element();
        if let Some(parent) = root.attribute("parent").filter(|parent| !parent.is_empty()) {
            self.load(node, parent, depth + 1)?;
        }
        apply_layer(node, root);
        Ok(())
    }

    fn resolve(&mut self, name: &str) -> Result<Node, String> {
        let mut node = Node::default();
        self.load(&mut node, name, 0)?;
        node.children
            .remove("Entity")
            .ok_or_else(|| format!("{name}: no Entity element"))
    }
}

/// `CParamNode::ApplyLayer`. `op` is only kept from overwriting the value:
/// the numbers it changes are not read here.
fn apply_layer(parent: &mut Node, element: roxmltree::Node) {
    let name = element.tag_name().name();
    let value = element_text(element);
    let mut replacing = false;
    let mut filtering = false;
    let mut merging = false;
    let mut has_op = false;
    for attribute in element.attributes() {
        match attribute.name() {
            "disable" => {
                parent.children.remove(name);
                return;
            }
            "replace" => {
                parent.children.remove(name);
                replacing = true;
            }
            "filtered" => filtering = true,
            "merge" => {
                if !parent.children.contains_key(name) {
                    return;
                }
                merging = true;
            }
            "op" => has_op = true,
            _ => {}
        }
    }
    let node = parent.children.entry(name.to_string()).or_default();
    let mut has_set_value = has_op;
    if element.attribute("datatype") == Some("tokens") {
        let old: Vec<&str> = if replacing {
            Vec::new()
        } else {
            node.value.split_whitespace().collect()
        };
        let mut tokens: Vec<String> = old.iter().map(|token| token.to_string()).collect();
        for token in value.split_whitespace() {
            if let Some(removed) = token.strip_prefix('-') {
                if let Some(at) = tokens.iter().position(|kept| kept == removed) {
                    tokens.remove(at);
                }
            } else if !old.contains(&token) {
                tokens.push(token.to_string());
            }
        }
        node.value = tokens.join(" ");
        has_set_value = true;
    }
    if !has_set_value && !merging {
        node.value = value;
    }
    let mut kept = BTreeMap::new();
    for child in element.children().filter(roxmltree::Node::is_element) {
        apply_layer(node, child);
        if filtering {
            let child_name = child.tag_name().name();
            if let Some(child_node) = node.children.remove(child_name) {
                kept.insert(child_name.to_string(), child_node);
            }
        }
    }
    if filtering {
        node.children = kept;
    }
    for attribute in element.attributes() {
        if ["replace", "op", "merge", "filtered"].contains(&attribute.name()) {
            continue;
        }
        node.children.insert(
            format!("@{}", attribute.name()),
            Node {
                value: attribute.value().to_string(),
                children: BTreeMap::new(),
            },
        );
    }
}

/// Xeromyces keeps an element's text trimmed.
fn element_text(element: roxmltree::Node) -> String {
    let text: String = element
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|child| child.text())
        .collect();
    text.trim().to_string()
}

/// What the templates give the bake.
pub struct Templates {
    /// Civ code -> what its templates use.
    pub civs: BTreeMap<String, CivRefs>,
    /// Every template's `Decay`.
    pub sinking: Vec<(String, Sinking)>,
    /// The templates that could not be resolved.
    pub errors: Vec<String>,
}

pub fn load(mod_dir: &Path) -> Templates {
    let mut errors = Vec::new();
    let mut civs: BTreeMap<String, CivRefs> = BTreeMap::new();
    match fs::read_dir(mod_dir.join(CIV_DIR)) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                    && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
                {
                    civs.insert(stem.to_string(), CivRefs::default());
                }
            }
        }
        Err(error) => errors.push(format!("{CIV_DIR}: {error}")),
    }
    let mut loader = Loader {
        root: mod_dir.join(TEMPLATE_ROOT),
        files: HashMap::new(),
    };
    let mut names = Vec::new();
    list_templates(&loader.root.clone(), "", &mut names);
    names.sort();
    let mut sinking = Vec::new();
    for name in &names {
        let template = match loader.resolve(name) {
            Ok(template) => template,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if let Some(decay) = template.get("Decay") {
            // `CParamNode::ToFixed` and `ToBool`: 0 and false when absent.
            let number = |name: &str| {
                decay
                    .text(name)
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0.0)
            };
            sinking.push((
                name.clone(),
                Sinking {
                    delay: number("DelayTime"),
                    rate: number("SinkRate"),
                    acceleration: number("SinkAccel"),
                    ship: decay
                        .text("SinkingAnim")
                        .is_some_and(|value| value.trim() == "true"),
                },
            ));
        }
        let Some(refs) = template
            .text("Identity/Civ")
            .and_then(|civ| civs.get_mut(civ))
        else {
            continue;
        };
        add_refs(&template, refs);
        // Rubble shows for the civ's buildings, though it belongs to none.
        if let Some(spawned) = template.text("Health/SpawnEntityOnDeath") {
            match loader.resolve(spawned) {
                Ok(spawned) => add_refs(&spawned, refs),
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
    }
    errors.sort();
    errors.dedup();
    Templates {
        civs,
        sinking,
        errors,
    }
}

/// The template's actors with `{civ}` and `{phenotype}` filled as
/// `CCmpVisualActor` does, and its sound groups with `{lang}` and
/// `{phenotype}` filled as `Sound.js` does, for every phenotype.
fn add_refs(template: &Node, refs: &mut CivRefs) {
    let civ = template.text("Identity/Civ").unwrap_or("");
    let lang = template.text("Identity/Lang").unwrap_or(DEFAULT_LANG);
    let phenotypes: Vec<&str> = match template.get("Identity/Phenotype") {
        Some(node) => node.value.split_whitespace().collect(),
        None => vec![DEFAULT_PHENOTYPE],
    };
    for path in ["VisualActor/Actor", "VisualActor/FoundationActor"] {
        let Some(actor) = template.text(path) else {
            continue;
        };
        for phenotype in &phenotypes {
            refs.actors.insert(
                actor
                    .replace("{phenotype}", phenotype)
                    .replace("{civ}", civ),
            );
        }
    }
    let Some(groups) = template.get("Sound/SoundGroups") else {
        return;
    };
    for (key, group) in &groups.children {
        if key.starts_with('@') || group.value.is_empty() {
            continue;
        }
        for phenotype in &phenotypes {
            refs.sound_groups.insert(
                group
                    .value
                    .replace("{lang}", lang)
                    .replace("{phenotype}", phenotype),
            );
        }
    }
}

/// Template names as `FindTemplates` lists them: no `template_*` at the
/// root, nothing under special/ or mixins/.
fn list_templates(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name = format!("{prefix}{file_name}");
        if path.is_dir() {
            if name != "special" && name != "mixins" {
                list_templates(&path, &format!("{name}/"), out);
            }
        } else if let Some(stem) = name.strip_suffix(".xml")
            && !stem.starts_with("template_")
        {
            out.push(stem.to_string());
        }
    }
}
