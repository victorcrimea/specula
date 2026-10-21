//! Packs the textures the engine draws territory borders with
//! (`CCmpTerritoryManager::UpdateBoundaryLines`) into one file the web
//! client loads, so the browser needs no PNG decoder.
//!
//! Usage: territory-border [mod dir] [output]
//! Defaults: ../0ad/binaries/data/mods/mod and
//! specula-web/territory-border.bin.
//!
//! Output, format v0, little-endian: u8 version, u32 width, u32 height,
//! then width * height RGBA8 pixels of territory_border.png and as many of
//! territory_border_mask.png, each row-major.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use specula_tools::image::Image;
use specula_tools::image::decode_png;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/mod";
const DEFAULT_OUTPUT: &str = "specula-web/territory-border.bin";
const VERSION: u8 = 0;
const BASE: &str = "art/textures/misc/territory_border.png";
const MASK: &str = "art/textures/misc/territory_border_mask.png";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mod_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_MOD.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));

    let (base, mask) = match (load(&mod_dir.join(BASE)), load(&mod_dir.join(MASK))) {
        (Ok(base), Ok(mask)) => (base, mask),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    // The shader samples both at the same coordinates.
    if (base.width, base.height) != (mask.width, mask.height) {
        eprintln!(
            "the base is {}x{} but the mask {}x{}",
            base.width, base.height, mask.width, mask.height
        );
        return ExitCode::FAILURE;
    }

    let mut bytes = vec![VERSION];
    bytes.extend_from_slice(&base.width.to_le_bytes());
    bytes.extend_from_slice(&base.height.to_le_bytes());
    bytes.extend_from_slice(&base.rgba);
    bytes.extend_from_slice(&mask.rgba);
    if let Err(error) = fs::write(&output, &bytes) {
        eprintln!("cannot write {}: {error}", output.display());
        return ExitCode::FAILURE;
    }
    println!(
        "{}x{} border, written to {}",
        base.width,
        base.height,
        output.display()
    );
    ExitCode::SUCCESS
}

fn load(path: &Path) -> Result<Image, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    decode_png(&bytes).map_err(|error| format!("cannot decode {}: {error}", path.display()))
}
