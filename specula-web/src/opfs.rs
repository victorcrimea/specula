//! Files in the origin private file system, for what the page derives at
//! some cost and would rather not redo on the next visit. Best effort: a
//! browser without it, without `createWritable` or out of quota only loses
//! the saving.

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::Blob;
use web_sys::FileSystemDirectoryHandle;
use web_sys::FileSystemFileHandle;
use web_sys::FileSystemGetDirectoryOptions;
use web_sys::FileSystemGetFileOptions;
use web_sys::FileSystemRemoveOptions;
use web_sys::FileSystemWritableFileStream;

const VERSION_FILE: &str = "version.txt";

/// The directory `name`, emptied first unless it was made for `version`,
/// so files derived from older inputs do not pile up.
pub async fn directory(name: &str, version: &str) -> Result<FileSystemDirectoryHandle, String> {
    let storage = web_sys::window().ok_or("no window")?.navigator().storage();
    supports(&storage, "getDirectory")?;
    let root: FileSystemDirectoryHandle = cast(wait(storage.get_directory()).await?)?;
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(true);
    let dir: FileSystemDirectoryHandle =
        cast(wait(root.get_directory_handle_with_options(name, &options)).await?)?;
    let current = read(&dir, VERSION_FILE).await;
    if current.as_deref() == Some(version.as_bytes()) {
        return Ok(dir);
    }
    let remove = FileSystemRemoveOptions::new();
    remove.set_recursive(true);
    wait(root.remove_entry_with_options(name, &remove)).await?;
    let dir: FileSystemDirectoryHandle =
        cast(wait(root.get_directory_handle_with_options(name, &options)).await?)?;
    write(&dir, VERSION_FILE, version.as_bytes()).await?;
    Ok(dir)
}

/// The file's bytes, or `None` when it is not there or cannot be read.
pub async fn read(dir: &FileSystemDirectoryHandle, name: &str) -> Option<Vec<u8>> {
    let handle: FileSystemFileHandle = cast(wait(dir.get_file_handle(name)).await.ok()?).ok()?;
    let file: Blob = cast(wait(handle.get_file()).await.ok()?).ok()?;
    let buffer = wait(file.array_buffer()).await.ok()?;
    Some(js_sys::Uint8Array::new(&buffer).to_vec())
}

pub async fn write(
    dir: &FileSystemDirectoryHandle,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let options = FileSystemGetFileOptions::new();
    options.set_create(true);
    let handle: FileSystemFileHandle =
        cast(wait(dir.get_file_handle_with_options(name, &options)).await?)?;
    supports(&handle, "createWritable")?;
    let stream: FileSystemWritableFileStream = cast(wait(handle.create_writable()).await?)?;
    wait(stream.write_with_u8_array(bytes).map_err(describe)?).await?;
    wait(stream.close()).await?;
    Ok(())
}

/// The bindings call a missing method as if it were there and throw
/// through the caller, so check first.
fn supports(object: &JsValue, method: &str) -> Result<(), String> {
    let found = js_sys::Reflect::get(object, &method.into()).is_ok_and(|value| value.is_function());
    if found {
        Ok(())
    } else {
        Err(format!("this browser has no {method}"))
    }
}

async fn wait(promise: js_sys::Promise) -> Result<JsValue, String> {
    JsFuture::from(promise).await.map_err(describe)
}

fn cast<T: JsCast>(value: JsValue) -> Result<T, String> {
    value
        .dyn_into()
        .map_err(|_| "unexpected OPFS value".to_string())
}

fn describe(error: JsValue) -> String {
    error.dyn_ref::<js_sys::Error>().map_or_else(
        || format!("{error:?}"),
        |error| String::from(error.message()),
    )
}
