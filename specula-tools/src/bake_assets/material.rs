//! The parts of a material (`CMaterialManager::LoadMaterial`) the client
//! follows: its shader defines, those its required textures add, and its
//! `effectSettings` uniform.

use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Default)]
pub struct Material {
    pub player_color: bool,
    pub transparent: bool,
    pub normal_map: bool,
    pub specular_map: bool,
    pub ao_map: bool,
    /// Components the uniform does not name stay 0, as an unset uniform.
    pub effect: [f32; 4],
}

/// The engine mod that `public` is mounted over; the terrain materials
/// live in it.
const BASE_MOD: &str = "../mod";

/// `name` is relative to art/materials, looked up in the mod and then in
/// the base mod under it, as the engine's VFS mounts them. A missing or
/// broken material falls back to the defaults, as the engine does.
pub fn load(mod_dir: &Path, name: &str) -> Material {
    let path = format!("art/materials/{name}");
    let read = |dir: &Path| fs::read_to_string(dir.join(&path));
    let xml = match read(mod_dir).or_else(|_| read(&mod_dir.join(BASE_MOD))) {
        Ok(xml) => xml,
        Err(error) => {
            eprintln!("material {path}: {error}");
            return Material::default();
        }
    };
    let document = match roxmltree::Document::parse(&xml) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("material {name}: {error}");
            return Material::default();
        }
    };
    let root = document.root_element();
    let defined = |define: &str| {
        root.children().any(|node| {
            (node.has_tag_name("define")
                && node.attribute("name") == Some(define)
                && node.attribute("value") != Some("0"))
                || (node.has_tag_name("required_texture")
                    && node.attribute("define") == Some(define))
        })
    };
    let mut effect = [0.0; 4];
    if let Some(value) = root
        .children()
        .find(|node| {
            node.has_tag_name("uniform") && node.attribute("name") == Some("effectSettings")
        })
        .and_then(|node| node.attribute("value"))
    {
        for (slot, number) in effect.iter_mut().zip(value.split_whitespace()) {
            *slot = number.parse().unwrap_or(0.0);
        }
    }
    Material {
        player_color: defined("USE_PLAYERCOLOR"),
        transparent: defined("USE_TRANSPARENT"),
        normal_map: defined("USE_NORMAL_MAP"),
        specular_map: defined("USE_SPECULAR_MAP"),
        ao_map: defined("USE_AO"),
        effect,
    }
}
