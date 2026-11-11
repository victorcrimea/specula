//! The textures and fonts the session GUI's panels are drawn with, for the
//! page's DOM panels: each file named by its content, so it is kept for
//! good, and `session.css`, which names them in custom properties
//! (`--gui-<key>`), declares the engine's fonts and gives each of the
//! engine's cursors a `[data-cursor="<name>"]` rule, with the default
//! arrow as `--gui-cursor-default` too.
//!
//! Usage: session-gui [mod dir] [output dir] [civs file]
//! Defaults: ../0ad/binaries/data/mods/public, specula-web/gui and
//! specula-web/civs.txt. The fonts and the dialog frame come from the
//! engine's own `mod` next to the mod dir, as the VFS mounts it under the
//! public mod. The civs file names each civ, for the panels that show one.
//!
//! Most textures are copied as they are, since a browser decodes PNG. Only
//! what the engine changes when it draws, or what a browser cannot read,
//! is converted: civ emblems turned grey as `canvas2d.fs` does for a
//! `grayscale:` sprite, the DDS portrait background as PNG, and the
//! minimap's civ backgrounds, 512 px squares drawn in under 200 px, scaled
//! to twice that as lossy WebP.
//!
//! Old files are left in the output directory, so a page that still names
//! them keeps working until the next deploy clears it.

use std::collections::BTreeMap;
use std::fs;
use std::hash::DefaultHasher;
use std::hash::Hasher;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use fast_image_resize::PixelType;
use fast_image_resize::Resizer;
use fast_image_resize::images::Image;
use specula_assets::bc;
use specula_tools::dds;

const DEFAULT_MOD: &str = "../0ad/binaries/data/mods/public";
const DEFAULT_OUTPUT: &str = "specula-web/gui";
/// The civ names, which the client embeds; committed, unlike the output
/// dir, as it changes only with the game data.
const DEFAULT_CIVS: &str = "specula-web/civs.txt";
const UI: &str = "art/textures/ui";
/// `CVideoMode::CCursor`'s, each a PNG with a `.txt` of its hotspot.
const CURSORS: &str = "art/textures/cursors";
/// What `Engine.ResetCursor` shows.
const DEFAULT_CURSOR: &str = "default-arrow";
const PLAYERS: &str = "simulation/templates/special/players";
const CSS: &str = "session.css";
/// Twice the minimap's 192 px background, for high density screens.
const MINIMAP_BACKGROUND_SIDE: u32 = 384;
const WEBP_QUALITY: f32 = 85.0;
const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW_BITS: u32 = 22;

/// (custom property key, path under `UI`), copied as they are.
const TEXTURES: [(&str, &str); 114] = [
    ("hud-panels", "session/hud_panels.png"),
    ("line-horiz", "global/border/line_horiz.png"),
    ("line-vert", "global/border/line_vert.png"),
    (
        "line-corner-top-left",
        "global/border/line_corner_top_left.png",
    ),
    (
        "line-corner-top-right",
        "global/border/line_corner_top_right.png",
    ),
    (
        "line-corner-bottom-left",
        "global/border/line_corner_bottom_left.png",
    ),
    (
        "line-corner-bottom-right",
        "global/border/line_corner_bottom_right.png",
    ),
    (
        "line-corner-middle-left",
        "global/border/line_corner_middle_left.png",
    ),
    (
        "line-corner-middle-right",
        "global/border/line_corner_middle_right.png",
    ),
    (
        "line-corner-bottom-middle",
        "global/border/line_corner_bottom_middle.png",
    ),
    ("panel-shader", "session/panel_shader.png"),
    ("panel-shader-top-edge", "session/panel_shader_top_edge.png"),
    (
        "panel-shader-bottom-edge",
        "session/panel_shader_bottom_edge.png",
    ),
    (
        "bar-lighting-horizontal",
        "session/bar_lighting_horizontal.png",
    ),
    ("bar-lighting-vertical", "session/bar_lighting_vertical.png"),
    ("groups", "session/icons/groups.png"),
    ("resource-food", "session/icons/resources/food.png"),
    ("resource-wood", "session/icons/resources/wood.png"),
    ("resource-stone", "session/icons/resources/stone.png"),
    ("resource-metal", "session/icons/resources/metal.png"),
    (
        "resource-population",
        "session/icons/resources/population.png",
    ),
    ("ribbon-bg", "session/ribbon_bg.png"),
    ("repair", "session/icons/repair.png"),
    ("production", "session/icons/production.png"),
    ("stop", "session/icons/stop.png"),
    ("vertical-pair", "session/icons/vertical_pair.png"),
    (
        "tech-pair-unavailable",
        "session/icons/tech_pair_would_be_unavailable.png",
    ),
    ("rank-basic", "session/icons/ranks/Basic.png"),
    ("rank-advanced", "session/icons/ranks/Advanced.png"),
    ("rank-elite", "session/icons/ranks/Elite.png"),
    ("phase-village", "session/panel_phase_emblems_village.png"),
    ("phase-town", "session/panel_phase_emblems_town.png"),
    ("phase-city", "session/panel_phase_emblems_city.png"),
    ("phase-hidden", "session/panel_phase_emblems_hidden.png"),
    ("minimap-circle", "session/minimap_circle_modern.png"),
    ("minimap-diplomacy-off", "session/minimap-diplomacy-off.png"),
    (
        "minimap-diplomacy-off-highlight",
        "session/minimap-diplomacy-off-highlight.png",
    ),
    ("minimap-diplomacy-on", "session/minimap-diplomacy-on.png"),
    (
        "minimap-diplomacy-on-highlight",
        "session/minimap-diplomacy-on-highlight.png",
    ),
    ("minimap-idle", "session/minimap-idle.png"),
    (
        "minimap-idle-highlight",
        "session/minimap-idle-highlight.png",
    ),
    ("minimap-idle-disabled", "session/minimap-idle-disabled.png"),
    ("stance-defensive", "session/icons/stances/defensive.png"),
    // The `<icon>`s of gui/common/resources/, which tooltips name as
    // `[icon="icon_<code>"]`.
    ("icon-food", "session/icons/resources/food_small.png"),
    ("icon-wood", "session/icons/resources/wood_small.png"),
    ("icon-stone", "session/icons/resources/stone_small.png"),
    ("icon-metal", "session/icons/resources/metal_small.png"),
    (
        "icon-population",
        "session/icons/resources/population_small.png",
    ),
    ("icon-time", "session/icons/resources/time_small.png"),
    ("icon-xp", "session/icons/promote.png"),
    ("icon-food_fish", "session/icons/resources/fish_small.png"),
    ("icon-food_fruit", "session/icons/resources/fruit_small.png"),
    ("icon-food_grain", "session/icons/resources/grain_small.png"),
    ("icon-food_meat", "session/icons/resources/meat_small.png"),
    ("icon-food_rice", "session/icons/resources/rice_small.png"),
    ("icon-metal_ore", "session/icons/resources/metal_small.png"),
    ("icon-stone_rock", "session/icons/resources/stone_small.png"),
    ("icon-wood_tree", "session/icons/resources/wood_small.png"),
    ("portrait-black", "session/icons/bkg/portrait_black.dds"),
    ("stance-violent", "session/icons/stances/violent.png"),
    ("stance-aggressive", "session/icons/stances/aggressive.png"),
    ("stance-passive", "session/icons/stances/passive.png"),
    (
        "stance-standground",
        "session/icons/stances/standground.png",
    ),
    (
        "formation-battle_line",
        "session/icons/formations/battle_line.png",
    ),
    ("formation-box", "session/icons/formations/box.png"),
    (
        "formation-column_closed",
        "session/icons/formations/column_closed.png",
    ),
    (
        "formation-column_open",
        "session/icons/formations/column_open.png",
    ),
    ("formation-flank", "session/icons/formations/flank.png"),
    (
        "formation-line_closed",
        "session/icons/formations/line_closed.png",
    ),
    (
        "formation-line_open",
        "session/icons/formations/line_open.png",
    ),
    ("formation-null", "session/icons/formations/null.png"),
    ("formation-phalanx", "session/icons/formations/phalanx.png"),
    ("formation-scatter", "session/icons/formations/scatter.png"),
    (
        "formation-skirmish",
        "session/icons/formations/skirmish.png",
    ),
    (
        "formation-syntagma",
        "session/icons/formations/syntagma.png",
    ),
    ("formation-testudo", "session/icons/formations/testudo.png"),
    ("formation-wedge", "session/icons/formations/wedge.png"),
    ("corners", "session/icons/corners.png"),
    ("upgrade", "session/icons/upgrade.png"),
    ("bell-raise", "session/icons/bell_level1.png"),
    ("bell-end", "session/icons/bell_level0.png"),
    ("lock-locked", "session/icons/lock_locked.png"),
    ("lock-unlocked", "session/icons/lock_unlocked.png"),
    ("pack", "session/icons/pack.png"),
    ("unpack", "session/icons/unpack.png"),
    ("cancel", "session/icons/cancel.png"),
    ("command-garrison-out", "session/icons/garrison-out.png"),
    ("command-delete", "session/icons/kill_small.png"),
    (
        "command-delete-disabled",
        "session/icons/kill_small_disabled.png",
    ),
    ("command-call-to-arms", "session/icons/call-to-arms.png"),
    ("command-garrison", "session/icons/garrison.png"),
    ("command-occupy-turret", "session/icons/occupy-turret.png"),
    ("command-leave-turret", "session/icons/leave-turret.png"),
    ("command-focus-rally", "session/icons/focus-rally.png"),
    ("command-back-to-work", "session/icons/back-to-work.png"),
    ("command-add-guard", "session/icons/add-guard.png"),
    ("command-remove-guard", "session/icons/remove-guard.png"),
    ("command-economics", "session/icons/economics.png"),
    ("command-patrol", "session/icons/patrol.png"),
    ("command-locked", "session/icons/locked_small.png"),
    ("command-unlocked", "session/icons/unlocked_small.png"),
    ("command-autoqueue-on", "session/icons/autoqueue-on.png"),
    ("command-autoqueue-off", "session/icons/autoqueue-off.png"),
    ("diplomacy", "session/icons/diplomacy.png"),
    ("diplomacy-on", "session/icons/diplomacy-on.png"),
    ("victory", "session/icons/victory.png"),
    ("defeat", "session/icons/defeat.png"),
    ("stone-button", "global/button/button_stone_unselected.png"),
    (
        "stone-button-over",
        "global/button/button_stone_selected.png",
    ),
    ("arrow-up", "global/button/arrowbox_up.dds"),
    ("arrow-up-over", "global/button/arrowbox_up_over.dds"),
    ("arrow-down", "global/button/arrowbox_dn.dds"),
    ("arrow-down-over", "global/button/arrowbox_dn_over.dds"),
    ("info", "global/icon/info.png"),
];
/// The `ModernDialog`, `ModernButtonRed` and `ModernGoldLine` sprites' textures
/// (`gui/common/modern/sprites.xml`),
/// which only the engine's own `mod` holds, under the same `UI` path.
const MOD_TEXTURES: [(&str, &str); 17] = [
    ("dialog-background", "global/modern/background.png"),
    ("gold-line", "global/modern/gold-separator.png"),
    ("dialog-shadow", "global/modern/shadow-low.png"),
    ("dialog-border", "global/modern/border.png"),
    ("dialog-deco-top", "global/modern/dialog-deco-top.png"),
    ("dialog-deco-bottom", "global/modern/dialog-deco-bottom.png"),
    (
        "dialog-titlebar-middle",
        "global/modern/titlebar-middle.png",
    ),
    ("dialog-titlebar-left", "global/modern/titlebar-left.png"),
    (
        "red-button-left-top",
        "global/modern/button/red-unselected-left-top.png",
    ),
    (
        "red-button-left-center",
        "global/modern/button/red-unselected-left-center.png",
    ),
    (
        "red-button-left-bottom",
        "global/modern/button/red-unselected-left-bottom.png",
    ),
    (
        "red-button-center-top",
        "global/modern/button/red-unselected-center-top.png",
    ),
    (
        "red-button-center-center",
        "global/modern/button/red-unselected-center-center.png",
    ),
    (
        "red-button-center-bottom",
        "global/modern/button/red-unselected-center-bottom.png",
    ),
    (
        "red-button-right-top",
        "global/modern/button/red-unselected-right-top.png",
    ),
    (
        "red-button-right-center",
        "global/modern/button/red-unselected-right-center.png",
    ),
    (
        "red-button-right-bottom",
        "global/modern/button/red-unselected-right-bottom.png",
    ),
];
/// The engine's `sans` (config/default.cfg, [fonts.sans]): (family, CSS
/// weight, file under fonts/). FreeSans is the engine's fallback for what
/// Biolinum lacks, which a browser does by the family list as well.
const FONTS: [(&str, u16, &str); 4] = [
    ("0ad sans", 400, "LinBiolinum_Rah.ttf"),
    ("0ad sans", 700, "LinBiolinum_RBah.ttf"),
    ("0ad sans fallback", 400, "FreeSans.ttf"),
    ("0ad sans fallback", 700, "FreeSansBold.ttf"),
];

struct Rgba {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mod_dir = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_MOD.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));
    let civs = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_CIVS.into()));
    match run(&mod_dir, &output, &civs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(mod_dir: &Path, output: &Path, civs_path: &Path) -> Result<(), String> {
    fs::create_dir_all(output).map_err(|error| format!("{}: {error}", output.display()))?;
    let ui = mod_dir.join(UI);
    let engine_mod = mod_dir
        .parent()
        .ok_or("the mod dir has no parent")?
        .join("mod");
    let engine_ui = engine_mod.join(UI);
    let mut properties = Vec::new();
    let mut written = 0usize;
    let textures = TEXTURES
        .iter()
        .map(|&(key, path)| (key, ui.join(path)))
        .chain(
            MOD_TEXTURES
                .iter()
                .map(|&(key, path)| (key, engine_ui.join(path))),
        );
    for (key, source) in textures {
        let path = source.to_string_lossy();
        let bytes = read(&source)?;
        let bytes = if path.ends_with(".dds") {
            encode_png(&decode_dds(&bytes).map_err(|error| format!("{path}: {error}"))?)?
        } else {
            bytes
        };
        let name = put(output, key, "png", &bytes, false)?;
        written += bytes.len();
        properties.push((key.to_string(), name));
    }
    let civs = civ_identities(mod_dir)?;
    let mut civ_names = String::from(
        "# Written by specula-tools session-gui: civ code, tab, Identity/GenericName of\n# simulation/templates/special/players/<civ>.xml, as loadCivFiles names a civ.\n",
    );
    for civ in &civs {
        civ_names.push_str(&format!("{}\t{}\n", civ.code, civ.name));
    }
    write(civs_path, civ_names.as_bytes())?;
    for Civ {
        code: civ, icon, ..
    } in civs
    {
        let path = format!("session/portraits/{icon}");
        let mut image =
            decode_png(&read(&ui.join(&path))?).map_err(|error| format!("{path}: {error}"))?;
        grayscale(&mut image);
        let bytes = encode_png(&image)?;
        let key = format!("emblem-{civ}");
        let name = put(output, &key, "png", &bytes, false)?;
        written += bytes.len();
        properties.push((key, name));
    }
    for (civ, path) in minimap_backgrounds(&ui)? {
        let image =
            decode_png(&read(&ui.join(&path))?).map_err(|error| format!("{path}: {error}"))?;
        let bytes = encode_webp(&resize(image, MINIMAP_BACKGROUND_SIDE)?)?;
        let key = format!("minimap-background-{civ}");
        let name = put(output, &key, "webp", &bytes, false)?;
        written += bytes.len();
        properties.push((key, name));
    }
    // The engine's own mod first, so a public cursor of the same name
    // wins, as the VFS mounts public over it.
    let mut cursors = BTreeMap::new();
    for dir in [engine_mod.join(CURSORS), mod_dir.join(CURSORS)] {
        for cursor in read_cursors(&dir)? {
            cursors.insert(cursor.name.clone(), cursor);
        }
    }
    let mut cursor_rules = Vec::new();
    for CursorFile {
        name,
        bytes,
        hotspot: [x, y],
    } in cursors.into_values()
    {
        let file = put(output, &format!("cursor-{name}"), "png", &bytes, false)?;
        written += bytes.len();
        cursor_rules.push((name, format!("url(\"/gui/{file}\") {x} {y}, auto")));
    }
    let fonts_dir = engine_mod.join("fonts");
    let mut faces = Vec::new();
    for (family, weight, file) in FONTS {
        let bytes = read(&fonts_dir.join(file))?;
        let stem = file
            .trim_end_matches(".ttf")
            .to_ascii_lowercase()
            .replace('_', "-");
        let name = put(output, &stem, "ttf", &bytes, true)?;
        written += bytes.len();
        faces.push((family, weight, name));
    }

    let mut css = String::from(
        "/* Written by specula-tools session-gui; textures of 0 A.D. by Wildfire\n   Games, CC-BY-SA 3.0, fonts under their own licences (gui.LICENSE). */\n",
    );
    for (family, weight, name) in &faces {
        css.push_str(&format!(
            "@font-face {{ font-family: \"{family}\"; font-weight: {weight}; font-display: swap;\n  src: url(\"/gui/{name}\") format(\"truetype\"); }}\n"
        ));
    }
    css.push_str(":root {\n");
    for (key, name) in &properties {
        css.push_str(&format!("  --gui-{key}: url(\"/gui/{name}\");\n"));
    }
    if let Some((_, value)) = cursor_rules.iter().find(|(name, _)| name == DEFAULT_CURSOR) {
        css.push_str(&format!("  --gui-cursor-default: {value};\n"));
    }
    css.push_str("}\n");
    for (name, value) in &cursor_rules {
        css.push_str(&format!(
            "[data-cursor=\"{name}\"] {{ cursor: {value}; }}\n"
        ));
    }
    let css_path = output.join(CSS);
    write(&css_path, css.as_bytes())?;
    write_brotli(&css_path, css.as_bytes())?;
    println!(
        "{} textures, {} cursors and {} fonts, {written} bytes, named in {}",
        properties.len(),
        cursor_rules.len(),
        faces.len(),
        css_path.display()
    );
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.starts_with(b"version https://git-lfs") {
        return Err(format!(
            "{}: a Git LFS pointer, run git lfs pull in ../0ad",
            path.display()
        ));
    }
    Ok(bytes)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_brotli(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut writer =
        brotli::CompressorWriter::new(Vec::new(), 4096, BROTLI_QUALITY, BROTLI_WINDOW_BITS);
    writer
        .write_all(bytes)
        .map_err(|error| format!("cannot compress: {error}"))?;
    let mut copy = path.as_os_str().to_owned();
    copy.push(".br");
    write(&PathBuf::from(copy), &writer.into_inner())
}

struct CursorFile {
    name: String,
    /// The PNG.
    bytes: Vec<u8>,
    hotspot: [u32; 2],
}

/// Each cursor in `dir` that has its hotspot file, as `CCursor::Load`
/// reads them.
fn read_cursors(dir: &Path) -> Result<Vec<CursorFile>, String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut cursors = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("{}: {error}", dir.display()))?
            .path();
        if path.extension().is_none_or(|extension| extension != "png") {
            continue;
        }
        let Ok(hotspot) = fs::read_to_string(path.with_extension("txt")) else {
            continue;
        };
        let hotspot: Vec<u32> = hotspot
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let [x, y] = hotspot[..] else {
            return Err(format!("{}: not an x and y hotspot", path.display()));
        };
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("{}: not UTF-8", path.display()))?
            .to_string();
        cursors.push(CursorFile {
            name,
            bytes: read(&path)?,
            hotspot: [x, y],
        });
    }
    Ok(cursors)
}

/// Writes `<key>-<hash>.<extension>`, with a brotli copy if asked, and
/// returns its name.
fn put(
    output: &Path,
    key: &str,
    extension: &str,
    bytes: &[u8],
    brotli: bool,
) -> Result<String, String> {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    let name = format!("{key}-{:016x}.{extension}", hasher.finish());
    let path = output.join(&name);
    write(&path, bytes)?;
    if brotli {
        write_brotli(&path, bytes)?;
    }
    Ok(name)
}

/// (civ code, Identity/Icon) of every player template but the abstract
/// ones, as `Templates.js` names a civ's `Emblem`.
struct Civ {
    code: String,
    /// Under `session/portraits/`.
    icon: String,
    name: String,
}

fn civ_identities(mod_dir: &Path) -> Result<Vec<Civ>, String> {
    let dir = mod_dir.join(PLAYERS);
    let entries = fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut emblems = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(civ) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".xml"))
        else {
            continue;
        };
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let document = roxmltree::Document::parse(&text)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let identity = document
            .descendants()
            .find(|node| node.has_tag_name("Identity"));
        let field = |name: &str| {
            identity
                .and_then(|identity| identity.children().find(|node| node.has_tag_name(name)))
                .and_then(|node| node.text())
                .map(|text| text.trim().to_string())
        };
        if let (Some(icon), Some(name)) = (field("Icon"), field("GenericName")) {
            emblems.push(Civ {
                code: civ.to_string(),
                icon,
                name,
            });
        }
    }
    emblems.sort_by(|a, b| a.code.cmp(&b.code));
    Ok(emblems)
}

/// (civ code, path under `UI`) of each `background_circle_<civ>.png`, as
/// `MiniMapPanel.setCivBackgroundTexture` names them.
fn minimap_backgrounds(ui: &Path) -> Result<Vec<(String, String)>, String> {
    let dir = ui.join("session/icons/bkg");
    let entries = fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut backgrounds = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(civ) = name
            .strip_prefix("background_circle_")
            .and_then(|rest| rest.strip_suffix(".png"))
        {
            backgrounds.push((civ.to_string(), format!("session/icons/bkg/{name}")));
        }
    }
    backgrounds.sort();
    Ok(backgrounds)
}

/// `canvas2d.fs` with a grayscale factor of 1, on the texture's values.
fn grayscale(image: &mut Rgba) {
    for pixel in image.pixels.chunks_exact_mut(4) {
        let grey =
            0.3 * f32::from(pixel[0]) + 0.59 * f32::from(pixel[1]) + 0.11 * f32::from(pixel[2]);
        let grey = grey.round().clamp(0.0, 255.0) as u8;
        pixel[..3].fill(grey);
    }
}

fn resize(image: Rgba, side: u32) -> Result<Rgba, String> {
    if image.width.max(image.height) <= side {
        return Ok(image);
    }
    let scale = |length: u32| (length * side / image.width.max(image.height)).max(1);
    let (width, height) = (scale(image.width), scale(image.height));
    let source = Image::from_vec_u8(image.width, image.height, image.pixels, PixelType::U8x4)
        .map_err(|error| error.to_string())?;
    let mut resized = Image::new(width, height, PixelType::U8x4);
    Resizer::new()
        .resize(&source, &mut resized, None)
        .map_err(|error| error.to_string())?;
    Ok(Rgba {
        width,
        height,
        pixels: resized.into_vec(),
    })
}

fn encode_webp(image: &Rgba) -> Result<Vec<u8>, String> {
    Ok(
        webp::Encoder::from_rgba(&image.pixels, image.width, image.height)
            .encode(WEBP_QUALITY)
            .to_vec(),
    )
}

fn encode_png(image: &Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(&image.pixels)
        .map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    Ok(out)
}

fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut pixels = vec![0; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| error.to_string())?;
    pixels.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        png::ColorType::Grayscale => pixels
            .iter()
            .flat_map(|&grey| [grey, grey, grey, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        png::ColorType::Indexed => return Err("palette left unexpanded".into()),
    };
    Ok(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// An uncompressed DDS with channel masks, the only kind this step reads.
fn decode_dds(bytes: &[u8]) -> Result<Rgba, &'static str> {
    const HEADER: usize = 128;
    const FOURCC_FLAG: u32 = 4;
    if bytes.len() < HEADER || &bytes[..4] != b"DDS " {
        return Err("not a DDS file");
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let height = u32_at(12);
    let width = u32_at(16);
    // The trade dialog's arrows are DXT1.
    if u32_at(80) & FOURCC_FLAG != 0 {
        let format = dds::block_format(bytes).map_err(|_| "unknown compressed DDS format")?;
        let pixels = bc::decode(format, width, height, &bytes[HEADER..])?;
        return Ok(Rgba {
            width,
            height,
            pixels,
        });
    }
    let bits = u32_at(88);
    if bits != 32 {
        return Err("only 32-bit uncompressed DDS is read");
    }
    let masks = [u32_at(92), u32_at(96), u32_at(100), u32_at(104)];
    let count = width as usize * height as usize;
    let data = bytes
        .get(HEADER..HEADER + count * 4)
        .ok_or("DDS truncated")?;
    let mut pixels = Vec::with_capacity(count * 4);
    for texel in data.chunks_exact(4) {
        let value = u32::from_le_bytes(texel.try_into().unwrap());
        for mask in masks {
            pixels.push(if mask == 0 {
                255
            } else {
                ((value & mask) >> mask.trailing_zeros()) as u8
            });
        }
    }
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}
