//! Bakes one actor, with its props, into vertex animation textures for the
//! web client's unit page: every vertex of every frame is skinned here, so
//! the browser only reads positions from a texture.
//!
//! Usage: bake-unit [actor] [output dir]
//! Defaults: units/athenians/infantry_spearman_b.xml (under art/actors)
//! and specula-web. MOD_DIR overrides the mod,
//! ../0ad/binaries/data/mods/public; PYROGENESIS overrides the engine,
//! ../0ad-specula/binaries/system/pyrogenesis, which converts the meshes.
//!
//! The bake is written as `unit-<hash of its content>.bin`, so a cache in
//! front of the server (Cloudflare caches .bin by default) can never serve
//! an old one under the same name. `unit.txt` names the current file; older
//! `unit-*.bin` in the directory are removed.
//!
//! Output, format v1, little-endian:
//! - "SPCU", u8 version, u32 vertex count V, u32 texture width W, f32x3
//!   bounds min, f32x3 bounds max, in metres.
//! - V x f32x2 UV.
//! - u32 index count, that many u32 indices.
//! - u32 part count, part x {u8 level count L, L x {u32 first index, u32
//!   index count}, the full mesh first, then ever coarser ones over the
//!   same vertices, so one VAT serves them all, u8 flags (1:
//!   tint by player colour where texture alpha is low), f32 alpha
//!   reference (discard below it; 0 for none), u32 width, u32 height,
//!   width * height RGBA8, top row first}.
//! - u32 animation count, animation x {u16 name length, UTF-8 name, f32
//!   milliseconds per frame at rate 1 (the engine plays movement animations
//!   at a rate of the unit's speed), u32 frames F, then positions and then
//!   normals
//!   as R x W x f16x4 each, R = ceil(F * V / W)}. Vertex v of frame f is
//!   texel i = f * V + v at (i % W, i / W); the fourth component is 0.
//!
//! The first animation is the bind pose, one frame.

mod actor;
mod bake;
mod convert;
mod lod;
mod math;
mod pmd;
mod psa;
mod texture;
mod unpacker;

use std::fs;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::bake::Baked;
use crate::math::Vec3;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_PYROGENESIS: &str = "../0ad-specula/binaries/system/pyrogenesis";
const DEFAULT_ACTOR: &str = "units/athenians/infantry_spearman_b.xml";
const DEFAULT_OUTPUT: &str = "specula-web";
const POINTER: &str = "unit.txt";
const WORK_DIR: &str = "target/bake-unit";
const ANIMATIONS: [&str; 2] = ["Idle", "Walk"];
const VERSION: u8 = 1;
/// The WebGL2 limit on a texture side, which the client keeps to.
const TEXTURE_WIDTH: usize = 2048;
const FLAG_PLAYER_COLOR: u8 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let actor_name = args.next().unwrap_or_else(|| DEFAULT_ACTOR.into());
    let output_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));
    let mod_dir = PathBuf::from(std::env::var("MOD_DIR").unwrap_or_else(|_| DEFAULT_MOD.into()));
    let pyrogenesis =
        PathBuf::from(std::env::var("PYROGENESIS").unwrap_or_else(|_| DEFAULT_PYROGENESIS.into()));

    let root = actor::load(&mod_dir, &actor_name)?;
    let mut sources = Vec::new();
    root.sources(&ANIMATIONS, &mut sources);
    sources.sort();
    sources.dedup();
    let converted = convert::run(&mod_dir, &pyrogenesis, &PathBuf::from(WORK_DIR), &sources)?;
    let mut baked = bake::bake(&root, &mod_dir, &converted, &ANIMATIONS)?;
    lod::add_levels(&mut baked);

    print_summary(&baked);
    let bytes = encode(&baked)?;
    let name = write_output(&output_dir, &bytes)?;
    println!("wrote {name} ({} bytes)", bytes.len());
    Ok(())
}

/// Returns the file name written. The pointer is written last, so it never
/// names a file that is not there yet.
fn write_output(dir: &Path, bytes: &[u8]) -> Result<String, String> {
    // Only names need to differ when the content does, so a hash that is not
    // stable across Rust versions is fine.
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let name = format!("unit-{:016x}.bin", hasher.finish());
    let path = dir.join(&name);
    fs::write(&path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))?;

    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let old = entry.file_name().to_string_lossy().into_owned();
        if old != name && old.starts_with("unit-") && old.ends_with(".bin") {
            fs::remove_file(entry.path())
                .map_err(|error| format!("cannot remove {old}: {error}"))?;
        }
    }

    let pointer = dir.join(POINTER);
    fs::write(&pointer, format!("{name}\n"))
        .map_err(|error| format!("cannot write {}: {error}", pointer.display()))?;
    Ok(name)
}

fn print_summary(baked: &Baked) {
    println!(
        "{} vertices, {} triangles, bounds {:?} to {:?}",
        baked.uvs.len(),
        baked
            .parts
            .iter()
            .map(|part| part.levels[0].1 as usize / 3)
            .sum::<usize>(),
        baked.min,
        baked.max
    );
    for part in &baked.parts {
        println!(
            "  part {}: {} vertices, {} triangles, texture {} {}x{}, player colour {}, alpha ref {}",
            part.actor,
            part.vertices,
            part.levels
                .iter()
                .map(|(_, count)| (count / 3).to_string())
                .collect::<Vec<_>>()
                .join("/"),
            part.texture,
            part.image.width,
            part.image.height,
            part.player_color,
            part.alpha_ref
        );
    }
    for animation in &baked.animations {
        println!(
            "  animation {} ({}): {} frames, {:.1} ms per frame",
            animation.name, animation.file, animation.frames, animation.frame_ms
        );
    }
}

fn encode(baked: &Baked) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend_from_slice(b"SPCU");
    out.push(VERSION);
    put_u32(&mut out, baked.uvs.len());
    put_u32(&mut out, TEXTURE_WIDTH);
    for bound in [baked.min, baked.max] {
        put_vec3(&mut out, bound);
    }
    for uv in &baked.uvs {
        out.extend_from_slice(&uv[0].to_le_bytes());
        out.extend_from_slice(&uv[1].to_le_bytes());
    }
    put_u32(&mut out, baked.indices.len());
    for index in &baked.indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    put_u32(&mut out, baked.parts.len());
    for part in &baked.parts {
        if part.image.width as usize > TEXTURE_WIDTH || part.image.height as usize > TEXTURE_WIDTH {
            return Err(format!(
                "{}: {}x{} exceeds the WebGL2 texture limit",
                part.texture, part.image.width, part.image.height
            ));
        }
        out.push(part.levels.len() as u8);
        for (first, count) in &part.levels {
            out.extend_from_slice(&first.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
        }
        out.push(if part.player_color {
            FLAG_PLAYER_COLOR
        } else {
            0
        });
        out.extend_from_slice(&part.alpha_ref.to_le_bytes());
        out.extend_from_slice(&part.image.width.to_le_bytes());
        out.extend_from_slice(&part.image.height.to_le_bytes());
        out.extend_from_slice(&part.image.rgba);
    }
    put_u32(&mut out, baked.animations.len());
    for animation in &baked.animations {
        let rows = (animation.frames * baked.uvs.len()).div_ceil(TEXTURE_WIDTH);
        if rows > TEXTURE_WIDTH {
            return Err(format!(
                "{}: too many frames for one texture",
                animation.name
            ));
        }
        let name = animation.name.as_bytes();
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&animation.frame_ms.to_le_bytes());
        put_u32(&mut out, animation.frames);
        for values in [&animation.positions, &animation.normals] {
            for value in values.iter() {
                for component in [value.x, value.y, value.z, 0.0] {
                    out.extend_from_slice(&f16_bits(component).to_le_bytes());
                }
            }
            let padding = rows * TEXTURE_WIDTH - values.len();
            out.resize(out.len() + padding * 8, 0);
        }
    }
    Ok(out)
}

fn put_u32(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u32).to_le_bytes());
}

fn put_vec3(out: &mut Vec<u8>, value: Vec3) {
    for component in [value.x, value.y, value.z] {
        out.extend_from_slice(&component.to_le_bytes());
    }
}

/// IEEE half precision, rounding to nearest even. Unit coordinates are a few
/// metres, where a half keeps about a millimetre.
fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x007F_FFFF;
    if exponent == 0xFF {
        let nan = if mantissa != 0 { 0x0200 } else { 0 };
        return sign | 0x7C00 | nan;
    }
    let half_exponent = exponent - 127 + 15;
    if half_exponent >= 0x1F {
        return sign | 0x7C00;
    }
    if half_exponent <= 0 {
        if half_exponent < -10 {
            return sign;
        }
        let mantissa = mantissa | 0x0080_0000;
        let shift = (14 - half_exponent) as u32;
        let half = mantissa >> shift;
        let remainder = mantissa & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let round = remainder > halfway || (remainder == halfway && half & 1 == 1);
        return sign | (half + u32::from(round)) as u16;
    }
    let half = ((half_exponent as u32) << 10) | (mantissa >> 13);
    let remainder = mantissa & 0x1FFF;
    let round = remainder > 0x1000 || (remainder == 0x1000 && half & 1 == 1);
    // A carry out of the mantissa correctly bumps the exponent.
    sign | (half + u32::from(round)) as u16
}
