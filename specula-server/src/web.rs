//! The browser client's files where the plain file server is not enough:
//! baked assets with the brotli copy bake-assets writes next to each, the
//! wasm and its glue compressed here once per build, and the pages with
//! the current glue and wasm versions in their URLs, and the match terrain
//! with its brotli copy.
//!
//! A page names `./pkg/specula_web.js?v=GLUE` and
//! `./pkg/specula_web_bg.wasm?v=WASM`; the placeholders become a hash of
//! the file, so a cache in front of the server cannot pair a new wasm with
//! an old glue, and a `pkg/` file asked for by its current hash never
//! changes. A server that does not fill them in still serves working pages.
//!
//! `/` also names `ASSETS_SIZE`, which becomes how much the current bake
//! can store in a viewer's browser, worked out once per bake.

use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

use rocket::Request;
use rocket::Response;
use rocket::State;
use rocket::fs::NamedFile;
use rocket::http::ContentType;
use rocket::request::FromParam;
use rocket::request::FromRequest;
use rocket::request::Outcome;
use rocket::response;
use rocket::response::Responder;
use rocket::response::content::RawHtml;

use crate::WEB_ROOT;
use crate::view::Hub;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";
const GLUE: &str = "specula_web.js";
const WASM: &str = "specula_web_bg.wasm";
const GLUE_PLACEHOLDER: &str = "specula_web.js?v=GLUE";
const WASM_PLACEHOLDER: &str = "specula_web_bg.wasm?v=WASM";
const ASSETS_SIZE_PLACEHOLDER: &str = "ASSETS_SIZE";
/// Highest quality: a build or a terrain is compressed once and served
/// many times.
const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW_BITS: u32 = 22;

/// Whether the request takes `Content-Encoding: br`.
pub struct AcceptsBrotli(bool);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AcceptsBrotli {
    type Error = std::convert::Infallible;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let accepts = request.headers().get("Accept-Encoding").any(|value| {
            value.split(',').any(|coding| {
                let name = coding.split(';').next().unwrap_or_default();
                name.trim().eq_ignore_ascii_case("br")
            })
        });
        Outcome::Success(AcceptsBrotli(accepts))
    }
}

/// A page whose glue and wasm URLs are filled in; any other name forwards
/// to the file server.
pub struct Page(&'static str);

impl<'r> FromParam<'r> for Page {
    type Error = &'r str;

    fn from_param(param: &'r str) -> Result<Self, Self::Error> {
        ["index.html", "unit.html", "map.html"]
            .into_iter()
            .find(|page| *page == param)
            .map(Page)
            .ok_or(param)
    }
}

/// A body with the headers that say how to decode and keep it.
pub struct Encoded<B> {
    body: B,
    content_type: ContentType,
    brotli: bool,
    cache: &'static str,
}

impl<'r, B: Responder<'r, 'static>> Responder<'r, 'static> for Encoded<B> {
    fn respond_to(self, request: &'r Request<'_>) -> response::Result<'static> {
        let mut response = self.body.respond_to(request)?;
        response.set_header(self.content_type);
        if self.brotli {
            response.set_raw_header("Content-Encoding", "br");
        }
        // Caches in between must not hand a brotli body to a client that
        // did not ask for one.
        response.set_raw_header("Vary", "Accept-Encoding");
        response.set_raw_header("Cache-Control", self.cache);
        Ok(response)
    }
}

/// Shared bytes as a response body, so a request does not copy the wasm.
#[derive(Clone)]
pub struct Shared(Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<'r> Responder<'r, 'static> for Shared {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        Response::build()
            .sized_body(self.0.len(), Cursor::new(self))
            .ok()
    }
}

/// One `pkg/` file as last read, with its hash and, once compressed, its
/// brotli copy.
struct Build {
    modified: SystemTime,
    len: u64,
    raw: Shared,
    hash: String,
    brotli: OnceLock<Shared>,
    compressing: AtomicBool,
}

/// `pkg/` files by name, reread when they change on disk.
#[derive(Default)]
pub struct Pkg {
    builds: Mutex<HashMap<String, Arc<Build>>>,
}

impl Pkg {
    async fn get(&self, name: &str) -> Option<Arc<Build>> {
        let path = Path::new(WEB_ROOT).join("pkg").join(name);
        let metadata = rocket::tokio::fs::metadata(&path).await.ok()?;
        let modified = metadata.modified().ok()?;
        if let Some(build) = self.builds.lock().unwrap().get(name)
            && build.modified == modified
            && build.len == metadata.len()
        {
            return Some(Arc::clone(build));
        }
        let raw = rocket::tokio::fs::read(&path).await.ok()?;
        let mut hasher = DefaultHasher::new();
        raw.hash(&mut hasher);
        let build = Arc::new(Build {
            modified,
            len: raw.len() as u64,
            raw: Shared(Arc::new(raw)),
            hash: format!("{:016x}", hasher.finish()),
            brotli: OnceLock::new(),
            compressing: AtomicBool::new(false),
        });
        self.builds
            .lock()
            .unwrap()
            .insert(name.to_string(), Arc::clone(&build));
        Some(build)
    }

    /// Empty when the file is missing, and `?v=` never matches a hash, so
    /// nothing is cached under it.
    async fn version(&self, name: &str) -> String {
        self.get(name)
            .await
            .map_or_else(String::new, |build| build.hash.clone())
    }
}

/// The brotli copy if it is ready; otherwise starts it, and the raw file
/// goes out meanwhile rather than holding the request for seconds.
fn compressed(build: &Arc<Build>) -> Option<Shared> {
    if let Some(brotli) = build.brotli.get() {
        return Some(brotli.clone());
    }
    if !build.compressing.swap(true, Ordering::Relaxed) {
        let build = Arc::clone(build);
        rocket::tokio::task::spawn_blocking(move || {
            let _ = build.brotli.set(Shared(Arc::new(compress(&build.raw.0))));
        });
    }
    None
}

pub fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut writer =
        brotli::CompressorWriter::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS);
    // Writing to a Vec cannot fail.
    writer.write_all(bytes).unwrap();
    writer.into_inner()
}

/// The bytes the current bake's files take uncompressed, which is what
/// the client keeps in the Cache API, for the `assets.txt` it was summed
/// against.
#[derive(Default)]
pub struct AssetsSize {
    last: Mutex<Option<(String, u64)>>,
}

impl AssetsSize {
    /// Rereads only the pointer on each request, so a rebake under a
    /// running server is summed again.
    async fn get(&self) -> Option<u64> {
        let pointer = Path::new(WEB_ROOT).join("assets.txt");
        let pointer = rocket::tokio::fs::read_to_string(pointer).await.ok()?;
        let pointer = pointer.trim().to_string();
        if let Some((summed, size)) = &*self.last.lock().unwrap()
            && *summed == pointer
        {
            return Some(*size);
        }
        let index = pointer.clone();
        let size = rocket::tokio::task::spawn_blocking(move || bake_size(&index))
            .await
            .ok()??;
        *self.last.lock().unwrap() = Some((pointer, size));
        Some(size)
    }
}

/// `index` is `assets/index-<hash>.bin`; its `files-<hash>.bin` lists the
/// hash of every file of the bake, so files of older bakes left in the
/// directory are not counted.
fn bake_size(index: &str) -> Option<u64> {
    let hash = asset_hash(index.rsplit('/').next()?)?;
    let dir = Path::new(WEB_ROOT).join("assets");
    let list = std::fs::read(dir.join(format!("files-{hash:016x}.bin"))).ok()?;
    let keep: HashSet<u64> = list
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    let mut size = 0;
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        if asset_hash(&entry.file_name().to_string_lossy()).is_some_and(|hash| keep.contains(&hash))
            && let Ok(metadata) = entry.metadata()
        {
            size += metadata.len();
        }
    }
    Some(size)
}

/// The hash in `<kind>-<hash>.bin`; brotli copies are left out, since the
/// browser keeps the decoded bytes.
fn asset_hash(name: &str) -> Option<u64> {
    let (_, hash) = name.strip_suffix(".bin")?.rsplit_once('-')?;
    u64::from_str_radix(hash, 16).ok()
}

/// Rounded up, so the page never promises less than is stored.
fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", (bytes as f64 / 1e8).ceil() / 10.0)
    } else {
        format!("{} MB", bytes.div_ceil(1_000_000))
    }
}

#[rocket::get("/")]
pub async fn index(pkg: &State<Pkg>, assets: &State<AssetsSize>) -> Option<RawHtml<String>> {
    page(Page("index.html"), pkg, assets).await
}

#[rocket::get("/<page>")]
pub async fn page(
    page: Page,
    pkg: &State<Pkg>,
    assets: &State<AssetsSize>,
) -> Option<RawHtml<String>> {
    let path = Path::new(WEB_ROOT).join(page.0);
    let html = rocket::tokio::fs::read_to_string(path).await.ok()?;
    let glue = pkg.version(GLUE).await;
    let wasm = pkg.version(WASM).await;
    let mut html = html
        .replace(GLUE_PLACEHOLDER, &format!("{GLUE}?v={glue}"))
        .replace(WASM_PLACEHOLDER, &format!("{WASM}?v={wasm}"));
    if html.contains(ASSETS_SIZE_PLACEHOLDER) {
        // Without a bake the 3D page has nothing to store, but the text
        // must still read.
        let size = assets
            .get()
            .await
            .map_or_else(|| "a few GB".to_string(), format_size);
        html = html.replace(ASSETS_SIZE_PLACEHOLDER, &size);
    }
    Some(RawHtml(html))
}

#[rocket::get("/pkg/<name>?<v>")]
pub async fn pkg_file(
    name: &str,
    v: Option<&str>,
    accepts: AcceptsBrotli,
    pkg: &State<Pkg>,
) -> Option<Encoded<Shared>> {
    if !plain_name(name) {
        return None;
    }
    let build = pkg.get(name).await?;
    let brotli = if accepts.0 { compressed(&build) } else { None };
    // A stale version in the URL still gets the current file, but must
    // not be cached under that name. Nor must the raw file sent while the
    // brotli copy is still being made, or it would be kept for good.
    let compressing = accepts.0 && brotli.is_none();
    let cache = if v == Some(build.hash.as_str()) && !compressing {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    let content_type = Path::new(name)
        .extension()
        .and_then(|ext| ContentType::from_extension(&ext.to_string_lossy()))
        .unwrap_or(ContentType::Binary);
    Some(Encoded {
        brotli: brotli.is_some(),
        body: brotli.unwrap_or_else(|| build.raw.clone()),
        content_type,
        cache,
    })
}

#[rocket::get("/terrain/<id>")]
pub fn terrain(
    id: &str,
    accepts: AcceptsBrotli,
    hub: &State<Arc<Hub>>,
) -> Option<Encoded<Vec<u8>>> {
    let file = hub.terrain(u64::from_str_radix(id, 16).ok()?)?;
    let brotli = accepts.0.then(|| file.brotli.get()).flatten();
    // The id is a hash of the content, so a copy never goes stale, but the
    // raw file sent while the brotli copy is still being made must not be
    // kept for good.
    let cache = if accepts.0 && brotli.is_none() {
        REVALIDATE
    } else {
        IMMUTABLE
    };
    Some(Encoded {
        brotli: brotli.is_some(),
        body: brotli.unwrap_or(&file.raw).to_vec(),
        content_type: ContentType::Binary,
        cache,
    })
}

#[rocket::get("/assets/<name>")]
pub async fn asset(name: &str, accepts: AcceptsBrotli) -> Option<Encoded<NamedFile>> {
    if !plain_name(name) {
        return None;
    }
    let path = Path::new(WEB_ROOT).join("assets").join(name);
    // bake-assets leaves the brotli copy out when it would not be smaller.
    let compressed = if accepts.0 {
        NamedFile::open(brotli_path(&path)).await.ok()
    } else {
        None
    };
    let brotli = compressed.is_some();
    let body = match compressed {
        Some(file) => file,
        None => NamedFile::open(&path).await.ok()?,
    };
    Some(Encoded {
        body,
        content_type: ContentType::Binary,
        brotli,
        // Baked assets are named by their content, so a copy never goes
        // stale.
        cache: IMMUTABLE,
    })
}

fn brotli_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".br");
    PathBuf::from(name)
}

/// One file name that stays inside its directory.
fn plain_name(name: &str) -> bool {
    !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}
