//! The "Warm up" button: fetches every asset file the match's civs and
//! map can need, and those of the entities on the map when it is
//! pressed (trees, rocks, animals and other gaia objects, which belong
//! to no civ), so the asset store finds them stored in the browser when
//! they first come on screen, and asks the browser to keep them. Bodies are dropped once read;
//! only the files that name others are decoded, to follow those names.

use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::rc::Rc;

use specula_assets::files::FileRef;
use specula_assets::files::Kind;
use specula_assets::files::Manifest;
use specula_assets::files::Mesh;
use specula_assets::files::SoundGroup;
use specula_assets::files::Texture;
use web_sys::HtmlButtonElement;

use crate::assets::Store;

/// Fewer than the store's own requests, which are for what is on screen
/// now, and the stream's segments.
const MAX_IN_FLIGHT: usize = 3;

pub struct Warmup {
    button: HtmlButtonElement,
    /// Civ codes of the match, sorted, gaia left out.
    civs: Vec<String>,
    /// The map's terrain names, once its terrain has arrived.
    terrains: Vec<String>,
    run: Option<Rc<RefCell<Run>>>,
    /// The (civs, terrains) the last run was for.
    ran_for: Option<(Vec<String>, Vec<String>)>,
    shown: String,
    /// Asked for before the button could start a run: the storage notice
    /// can be answered before the match's civs are known.
    requested: bool,
}

#[derive(Default)]
struct Run {
    queue: VecDeque<(Kind, FileRef)>,
    seen: HashSet<u64>,
    in_flight: usize,
    done_bytes: u64,
    total_bytes: u64,
    failed: u32,
    /// The store's texture tiers when the run started.
    texture_tiers: usize,
}

impl Run {
    fn add(&mut self, kind: Kind, file: FileRef) {
        if self.seen.insert(file.hash) {
            self.total_bytes += u64::from(file.size);
            self.queue.push_back((kind, file));
        }
    }

    fn finished(&self) -> bool {
        self.queue.is_empty() && self.in_flight == 0
    }
}

impl Warmup {
    pub fn new(button: HtmlButtonElement) -> Self {
        Warmup {
            button,
            civs: Vec::new(),
            terrains: Vec::new(),
            run: None,
            ran_for: None,
            shown: String::new(),
            requested: false,
        }
    }

    pub fn button(&self) -> &HtmlButtonElement {
        &self.button
    }

    pub fn set_civs(&mut self, mut civs: Vec<String>) {
        civs.sort();
        civs.dedup();
        self.civs = civs;
    }

    pub fn set_terrains(&mut self, terrains: Vec<String>) {
        self.terrains = terrains;
    }

    /// From a click on the button; `actors` are those of the entities
    /// on the map now.
    pub fn start(&mut self, store: &Store, actors: &[&str]) {
        if self.running() || self.civs.is_empty() {
            return;
        }
        let civs: Vec<&str> = self.civs.iter().map(String::as_str).collect();
        let Some(roots) = store.warmup_roots(&civs, actors, &self.terrains) else {
            return;
        };
        let mut run = Run {
            texture_tiers: store.texture_tiers(),
            ..Run::default()
        };
        for (kind, file) in roots {
            run.add(kind, file);
        }
        let run = Rc::new(RefCell::new(run));
        pump(&run);
        self.run = Some(run);
        self.ran_for = Some((self.civs.clone(), self.terrains.clone()));
    }

    pub fn request(&mut self) {
        self.requested = true;
    }

    /// Whether a request is waiting and the button would now start it;
    /// clears the request when it does.
    pub fn take_request(&mut self) -> bool {
        let ready = self.requested && !self.button.disabled();
        if ready {
            self.requested = false;
        }
        ready
    }

    fn running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|run| !run.borrow().finished())
    }

    /// Shows the run's progress, or whether there is anything to warm up.
    pub fn frame(&mut self, index_loaded: bool) {
        let ran_for_target = self
            .ran_for
            .as_ref()
            .is_some_and(|(civs, terrains)| *civs == self.civs && *terrains == self.terrains);
        let (label, enabled) = match &self.run {
            Some(run) if !run.borrow().finished() => {
                let run = run.borrow();
                (
                    format!(
                        "Warming up {:.1} / {:.1} MB",
                        run.done_bytes as f64 / 1e6,
                        run.total_bytes as f64 / 1e6
                    ),
                    false,
                )
            }
            Some(run) if ran_for_target => {
                let failed = run.borrow().failed;
                if failed == 0 {
                    ("Warmed up".to_string(), false)
                } else {
                    (format!("Warm up ({failed} failed)"), true)
                }
            }
            _ => ("Warm up".to_string(), index_loaded && !self.civs.is_empty()),
        };
        let key = format!("{label}|{enabled}");
        if key == self.shown {
            return;
        }
        self.shown = key;
        self.button.set_text_content(Some(&label));
        self.button.set_disabled(!enabled);
    }
}

fn pump(run: &Rc<RefCell<Run>>) {
    let mut started = Vec::new();
    {
        let mut state = run.borrow_mut();
        while state.in_flight < MAX_IN_FLIGHT {
            let Some(job) = state.queue.pop_front() else {
                break;
            };
            state.in_flight += 1;
            started.push(job);
        }
    }
    for (kind, file) in started {
        wasm_bindgen_futures::spawn_local(fetch(Rc::clone(run), kind, file));
    }
}

/// The same URL the store fetches, so its request hits the cache.
async fn fetch(run: Rc<RefCell<Run>>, kind: Kind, file: FileRef) {
    let url = format!("/assets/{}", kind.file_name(file.hash));
    let named = crate::stored::fetch(&url).await.and_then(|(bytes, _)| {
        names(kind, &bytes, run.borrow().texture_tiers).map_err(String::from)
    });
    {
        let mut state = run.borrow_mut();
        state.in_flight -= 1;
        state.done_bytes += u64::from(file.size);
        match named {
            Ok(named) => {
                for (kind, file) in named {
                    state.add(kind, file);
                }
            }
            Err(error) => {
                state.failed += 1;
                web_sys::console::error_1(&format!("warm-up {url}: {error}").into());
            }
        }
    }
    pump(&run);
}

/// The files this one names, a texture's detail up to `texture_tiers`.
fn names(
    kind: Kind,
    bytes: &[u8],
    texture_tiers: usize,
) -> Result<Vec<(Kind, FileRef)>, &'static str> {
    let mut out = Vec::new();
    match kind {
        Kind::Actor => {
            let manifest = Manifest::decode(bytes)?;
            let variants = manifest
                .objects
                .iter()
                .flat_map(|object| &object.definitions)
                .flat_map(|definition| &definition.groups)
                .flatten();
            for variant in variants {
                out.extend(variant.mesh.map(|file| (Kind::Mesh, file)));
                out.extend(variant.texture.map(|file| (Kind::Texture, file)));
                out.extend(
                    variant
                        .animations
                        .iter()
                        .filter_map(|animation| animation.file)
                        .map(|file| (Kind::Animation, file)),
                );
            }
        }
        Kind::Mesh => {
            let mesh = Mesh::decode(bytes)?;
            out.extend(mesh.skeleton.map(|file| (Kind::Skeleton, file)));
            out.extend(mesh.detail.map(|file| (Kind::MeshDetail, file)));
        }
        Kind::Texture => {
            let texture = Texture::decode(bytes)?;
            out.extend(
                texture.details[..texture_tiers]
                    .iter()
                    .flatten()
                    .map(|file| (Kind::TextureDetail, *file)),
            );
        }
        Kind::SoundGroup => {
            let group = SoundGroup::decode(bytes)?;
            out.extend(group.sounds.iter().map(|file| (Kind::Sound, *file)));
        }
        _ => {}
    }
    Ok(out)
}
