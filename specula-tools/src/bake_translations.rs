//! The game's translations as JSON, one file per language, for the web
//! client to look an engine string up in.
//!
//! Usage: bake-translations [release data dir] [output dir]
//! Defaults: target/translations and specula-web/lang.
//!
//! The engine's repository holds no .po files (they live on Transifex), but
//! its release tarball ships them inside `public.zip` and `mod.zip`. The
//! tarball is fetched with curl into the release data dir when that has no
//! zips yet; `unzip` then reads the .po files out of them.
//!
//! Each `<code>-<hash>.json` is `{"plural": expr, "messages": {key: str}}`
//! where key is `msgctxt \u{4} msgid` (just the msgid with no context, as
//! gettext's own .mo files do) and str is the msgstr, or an array of the
//! msgstr forms for a plural message, indexed by the plural expression.
//! `langs.txt` names them: `code<TAB>native name<TAB>file`, in the order
//! the language menu shows them. Untranslated and fuzzy messages are left
//! out, so a lookup that misses falls back to the English msgid.
//!
//! Specula's own strings are in `specula-web/i18n/<code>.json` (English
//! text -> translation, with `en.json` naming each one's context). They are
//! merged over the engine's messages, so a string the engine has too takes
//! the engine's wording unless Specula's file names it. A translation that
//! lacks or adds a placeholder or tag of its English is an error.
//!
//! Old files are left in the output directory, so a page that still names
//! them keeps working until the next deploy clears it.

use std::collections::BTreeMap;
use std::fs;
use std::hash::DefaultHasher;
use std::hash::Hasher;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;
use std::process::Stdio;

use polib::po_file;
use serde_json::Value;

const DEFAULT_RELEASE: &str = "target/translations";
const DEFAULT_OUTPUT: &str = "specula-web/lang";
const OWN_STRINGS: &str = "specula-web/i18n";
const TARBALL: &str = "https://releases.wildfiregames.com/0ad-0.28.0-unix-data.tar.gz";
const PREFIX: &str = "0ad-0.28.0/";
const ZIPS: [&str; 2] = [
    "binaries/data/mods/public/public.zip",
    "binaries/data/mods/mod/mod.zip",
];
/// The domains the in-game text comes from: names and descriptions of what
/// a match holds, and the GUI strings the session panels and their
/// tooltips use. Lobby, manual, tutorials and maps are not shown.
const DOMAINS: [&str; 10] = [
    "public-civilizations",
    "public-gui-ingame",
    "public-gui-other",
    "public-simulation-auras",
    "public-simulation-other",
    "public-simulation-technologies",
    "public-templates-buildings",
    "public-templates-other",
    "public-templates-units",
    "public-gui-gamesetup",
];
const BROTLI_QUALITY: u32 = 11;
const BROTLI_WINDOW_BITS: u32 = 22;
const CONTEXT_SEPARATOR: char = '\u{4}';

/// (engine locale, native name), in menu order. The engine's own list is
/// the .po files' names; the native names are not in the checkout, since
/// the engine asks ICU for them.
const LANGUAGES: [(&str, &str); 27] = [
    ("ca", "Català"),
    ("cs", "Čeština"),
    ("de", "Deutsch"),
    ("el", "Ελληνικά"),
    ("en_GB", "English (United Kingdom)"),
    ("es", "Español"),
    ("eu", "Euskara"),
    ("fi", "Suomi"),
    ("fr", "Français"),
    ("gl", "Galego"),
    ("hu", "Magyar"),
    ("id", "Bahasa Indonesia"),
    ("it", "Italiano"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("pt_BR", "Português (Brasil)"),
    ("pt_PT", "Português (Portugal)"),
    ("ru", "Русский"),
    ("sk", "Slovenčina"),
    ("sv", "Svenska"),
    ("tr", "Türkçe"),
    ("uk", "Українська"),
    ("vi", "Tiếng Việt"),
    ("zh", "中文 (简体)"),
    ("zh_TW", "中文 (繁體)"),
];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let release = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_RELEASE.into()));
    let output = PathBuf::from(args.next().unwrap_or_else(|| DEFAULT_OUTPUT.into()));
    match run(&release, &output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bake-translations: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(release: &Path, output: &Path) -> Result<(), String> {
    fetch(release)?;
    fs::create_dir_all(output).map_err(|error| format!("{}: {error}", output.display()))?;
    let mut listing = String::new();
    for (code, name) in LANGUAGES {
        let mut messages = BTreeMap::new();
        let mut plural = String::new();
        for zip in ZIPS {
            for domain in DOMAINS.iter().copied().chain(["mod-selector"]) {
                let entry = format!("l10n/{code}.{domain}.po");
                let Some(text) = read_entry(&release.join(zip), &entry)? else {
                    continue;
                };
                read_catalog(&entry, &text, &mut plural, &mut messages)?;
            }
        }
        read_own(code, &mut messages)?;
        if messages.is_empty() {
            return Err(format!("no translations for {code}"));
        }
        let mut object = serde_json::Map::new();
        object.insert("plural".into(), Value::String(plural));
        object.insert(
            "messages".into(),
            Value::Object(messages.into_iter().collect()),
        );
        let bytes = serde_json::to_vec(&Value::Object(object))
            .map_err(|error| format!("{code}: {error}"))?;
        let file = put(output, code, &bytes)?;
        listing.push_str(&format!("{code}\t{name}\t{file}\n"));
        println!("{code}: {} bytes", bytes.len());
    }
    write(&output.join("langs.txt"), listing.as_bytes())
}

/// Downloads the tarball's two zips unless `release` already holds them.
fn fetch(release: &Path) -> Result<(), String> {
    if ZIPS.iter().all(|zip| release.join(zip).is_file()) {
        return Ok(());
    }
    fs::create_dir_all(release).map_err(|error| format!("{}: {error}", release.display()))?;
    let mut curl = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location", TARBALL])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl: {error}"))?;
    let stream = curl.stdout.take().ok_or("curl: no output")?;
    let mut tar = Command::new("tar");
    tar.arg("-xz")
        .arg("--strip-components=1")
        .arg("-C")
        .arg(release);
    for zip in ZIPS {
        tar.arg(format!("{PREFIX}{zip}"));
    }
    let status = tar
        .stdin(stream)
        .status()
        .map_err(|error| format!("tar: {error}"))?;
    let fetched = curl.wait().map_err(|error| format!("curl: {error}"))?;
    if !status.success() || !fetched.success() {
        return Err(format!("cannot fetch {TARBALL}"));
    }
    Ok(())
}

/// The text of `entry` in `zip`, or `None` if the zip has no such file.
fn read_entry(zip: &Path, entry: &str) -> Result<Option<String>, String> {
    let out = Command::new("unzip")
        .arg("-p")
        .arg(zip)
        .arg(entry)
        .output()
        .map_err(|error| format!("unzip: {error}"))?;
    // unzip exits 11 when the entry is not in the archive.
    if out.status.code() == Some(11) {
        return Ok(None);
    }
    if !out.status.success() {
        return Err(format!("{}: cannot read {entry}", zip.display()));
    }
    String::from_utf8(out.stdout)
        .map(Some)
        .map_err(|error| format!("{entry}: {error}"))
}

fn read_catalog(
    entry: &str,
    text: &str,
    plural: &mut String,
    messages: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    let catalog =
        po_file::parse_from_reader(text.as_bytes()).map_err(|error| format!("{entry}: {error}"))?;
    let expr = &catalog.metadata.plural_rules.expr;
    if plural.is_empty() {
        plural.clone_from(expr);
    }
    for message in catalog.messages() {
        if !message.is_translated() || message.is_fuzzy() {
            continue;
        }
        let key = match message.msgctxt() {
            Some(context) if !context.is_empty() => {
                format!("{context}{CONTEXT_SEPARATOR}{}", message.msgid())
            }
            _ => message.msgid().to_string(),
        };
        let value = if message.is_plural() {
            let forms = message
                .msgstr_plural()
                .map_err(|error| format!("{entry}: {error}"))?;
            Value::Array(forms.iter().cloned().map(Value::String).collect())
        } else {
            Value::String(
                message
                    .msgstr()
                    .map_err(|error| format!("{entry}: {error}"))?
                    .to_string(),
            )
        };
        messages.insert(key, value);
    }
    Ok(())
}

/// Merges Specula's strings for `code` over `messages`, if it has any.
fn read_own(code: &str, messages: &mut BTreeMap<String, Value>) -> Result<(), String> {
    let path = Path::new(OWN_STRINGS).join(format!("{code}.json"));
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    let own: BTreeMap<String, String> =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    for (english, translation) in own {
        if translation.trim().is_empty() || markers(&english) != markers(&translation) {
            return Err(format!(
                "{}: bad translation of {english:?}",
                path.display()
            ));
        }
        messages.insert(english, Value::String(translation));
    }
    Ok(())
}

/// The `%(name)s` placeholders, `{size}` and `<b>` tags of `text`, sorted,
/// which a translation must keep.
fn markers(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['%', '{', '<']) {
        rest = &rest[start..];
        let close = match rest.as_bytes()[0] {
            b'%' => rest
                .find(")s")
                .map(|at| at + 2)
                .filter(|_| rest[1..].starts_with('(')),
            b'{' => rest.find('}').map(|at| at + 1),
            _ => rest.find('>').map(|at| at + 1),
        };
        match close {
            Some(end) => {
                found.push(rest[..end].to_string());
                rest = &rest[end..];
            }
            None => rest = &rest[1..],
        }
    }
    found.sort();
    found
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

/// Writes `<code>-<hash>.json` with a brotli copy and returns its name.
fn put(output: &Path, code: &str, bytes: &[u8]) -> Result<String, String> {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    let name = format!("{code}-{:016x}.json", hasher.finish());
    let path = output.join(&name);
    write(&path, bytes)?;
    write_brotli(&path, bytes)?;
    Ok(name)
}
