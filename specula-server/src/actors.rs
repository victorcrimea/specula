//! The baked actor manifests, read from `specula-web/assets/` as a client
//! fetches them, for the variant choices a seat is sent instead of the
//! actor seed (`specula_assets::variation`). Only read: the bake is not
//! touched.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use specula_assets::files::FileRef;
use specula_assets::files::Index;
use specula_assets::files::Kind;
use specula_assets::files::Manifest;

use crate::WEB_ROOT;

pub struct Actors {
    /// `None` until first asked; empty when there is no bake.
    index: Option<HashMap<String, FileRef>>,
    /// By actor name; `None` when its file cannot be read.
    loaded: HashMap<String, Option<Arc<Manifest>>>,
}

impl Actors {
    pub fn new() -> Self {
        Actors {
            index: None,
            loaded: HashMap::new(),
        }
    }

    /// The manifest of an actor as the stream names it.
    pub fn get(&mut self, name: &str) -> Option<Arc<Manifest>> {
        if let Some(manifest) = self.loaded.get(name) {
            return manifest.clone();
        }
        let index = self.index.get_or_insert_with(read_index);
        let manifest = index.get(name).and_then(|file| {
            let path = Path::new(WEB_ROOT)
                .join("assets")
                .join(Kind::Actor.file_name(file.hash));
            let bytes = std::fs::read(&path)
                .map_err(|error| eprintln!("cannot read {}: {error}", path.display()))
                .ok()?;
            Manifest::decode(&bytes)
                .map_err(|error| eprintln!("cannot decode {}: {error}", path.display()))
                .ok()
                .map(Arc::new)
        });
        if manifest.is_none() {
            eprintln!("actor {name} is not in the bake; a seat sees its first variants");
        }
        self.loaded.insert(name.to_string(), manifest.clone());
        manifest
    }
}

/// The bake `assets.txt` points at, or nothing.
fn read_index() -> HashMap<String, FileRef> {
    let read = || -> Result<HashMap<String, FileRef>, String> {
        let pointer = Path::new(WEB_ROOT).join("assets.txt");
        let name = std::fs::read_to_string(&pointer).map_err(|error| error.to_string())?;
        let bytes = std::fs::read(Path::new(WEB_ROOT).join(name.trim()))
            .map_err(|error| error.to_string())?;
        let index = Index::decode(&bytes).map_err(str::to_string)?;
        Ok(index.actors.into_iter().collect())
    };
    read().unwrap_or_else(|error| {
        eprintln!("no baked actors for the seats' variants: {error}");
        HashMap::new()
    })
}
