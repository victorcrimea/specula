//! Asset files kept in the Cache API as well as the HTTP cache, which is
//! shared with every other site and evicted by LRU. Files are named by
//! their content, so a stored copy never goes stale; `prune` drops what
//! the current bake no longer has. Best effort: a page that is not a
//! secure context (plain http to a host other than localhost) has no
//! Cache API, and then every file comes from the network as before.

use std::collections::HashSet;

use specula_assets::files;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::Cache;
use web_sys::Request;
use web_sys::Response;

const CACHE_NAME: &str = "specula-assets";
/// The index the cache was last pruned against, so it is walked once per
/// bake rather than on every visit.
const PRUNED_KEY: &str = "specula-assets-pruned";

/// The file's bytes, and whether they came from the Cache API.
pub async fn fetch(url: &str) -> Result<(Vec<u8>, bool), String> {
    let Some(cache) = open().await else {
        return Ok((crate::fetch_bytes(url).await?, false));
    };
    if let Some(bytes) = stored(&cache, url).await {
        return Ok((bytes, true));
    }
    let bytes = crate::fetch_bytes(url).await?;
    put(&cache, url, &bytes);
    Ok((bytes, false))
}

async fn open() -> Option<Cache> {
    let window = web_sys::window()?;
    if !window.is_secure_context() {
        return None;
    }
    let caches = window.caches().ok()?;
    JsFuture::from(caches.open(CACHE_NAME))
        .await
        .ok()?
        .dyn_into()
        .ok()
}

async fn stored(cache: &Cache, url: &str) -> Option<Vec<u8>> {
    let response: Response = JsFuture::from(cache.match_with_str(url))
        .await
        .ok()?
        .dyn_into()
        .ok()?;
    let buffer = JsFuture::from(response.array_buffer().ok()?).await.ok()?;
    Some(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// In the background: the caller has its bytes, and a full quota only
/// costs the next visit a download.
fn put(cache: &Cache, url: &str, bytes: &[u8]) {
    let array = js_sys::Uint8Array::from(bytes);
    let Ok(response) = Response::new_with_opt_buffer_source(Some(&array)) else {
        return;
    };
    let promise = cache.put_with_str(url, &response);
    let url = url.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if JsFuture::from(promise).await.is_err() {
            crate::log(&format!("assets: cannot store {url}"));
        }
    });
}

/// Asks the browser not to evict stored files under storage pressure.
/// Firefox asks the viewer, so this follows a click.
pub fn persist() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(promise) = window.navigator().storage().persist() else {
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        let granted = JsFuture::from(promise)
            .await
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        crate::log(&format!("assets: persistent storage granted: {granted}"));
    });
}

/// Drops every stored file the bake whose index is `index_name`
/// (`assets/index-<hash>.bin`) does not have.
pub async fn prune(index_name: &str) {
    let Some(cache) = open().await else {
        return;
    };
    let local = web_sys::window().and_then(|window| window.local_storage().ok().flatten());
    if let Some(local) = &local
        && local.get_item(PRUNED_KEY).ok().flatten().as_deref() == Some(index_name)
    {
        return;
    }
    match prune_against(&cache, index_name).await {
        Ok(dropped) => {
            crate::log(&format!(
                "assets: dropped {dropped} stored files of older bakes"
            ));
            if let Some(local) = &local {
                let _ = local.set_item(PRUNED_KEY, index_name);
            }
        }
        Err(error) => crate::log(&format!("assets: not pruned: {error}")),
    }
}

async fn prune_against(cache: &Cache, index_name: &str) -> Result<usize, String> {
    let name = index_name.rsplit('/').next().unwrap_or(index_name);
    let index = files::hash_of_name(name).ok_or("no hash in the index name")?;
    let url = format!("/assets/{}", files::file_list_name(index));
    let list = crate::fetch_bytes(&url)
        .await
        .map_err(|error| format!("{url}: {error}"))?;
    let keep: HashSet<u64> = list
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    let requests: js_sys::Array = JsFuture::from(cache.keys())
        .await
        .map_err(|_| "cannot list the stored files")?
        .dyn_into()
        .map_err(|_| "stored files are not a list")?;
    let mut dropped = 0;
    for request in requests.iter() {
        let Ok(request) = request.dyn_into::<Request>() else {
            continue;
        };
        let url = request.url();
        let name = url.rsplit('/').next().unwrap_or_default();
        if files::hash_of_name(name).is_some_and(|hash| keep.contains(&hash)) {
            continue;
        }
        if JsFuture::from(cache.delete_with_request(&request))
            .await
            .is_ok()
        {
            dropped += 1;
        }
    }
    Ok(dropped)
}
