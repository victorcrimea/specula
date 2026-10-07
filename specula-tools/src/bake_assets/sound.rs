//! The sound groups under audio/ as `CSoundGroup::LoadSoundGroup` reads
//! them, the session music of gui/common/music.js and the civ files, and
//! the transcoding of their Ogg Vorbis files to Ogg Opus for the browser.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use lewton::inside_ogg::OggStreamReader;
use ogg::writing::PacketWriteEndInfo;
use ogg::writing::PacketWriter;
use opus::Application;
use opus::Bitrate;
use opus::Channels;
use opus::Encoder;
use roxmltree::Document;
use rubato::Fft;
use rubato::FixedSync;
use rubato::Resampler;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use specula_assets::files::MusicKind;
use specula_assets::files::SOUND_DISTANCELESS;
use specula_assets::files::SOUND_OMNIPRESENT;
use specula_assets::files::SOUND_OWNER_ONLY;
use specula_assets::files::SOUND_RANDOM_GAIN;
use specula_assets::files::SOUND_RANDOM_PITCH;

const AUDIO_DIR: &str = "audio";
const CIV_DIR: &str = "simulation/data/civs";
/// music.js' RELATIVE_MUSIC_PATH.
const MUSIC_DIR: &str = "audio/music";
/// gui/session/Ambient.js; the map does not choose it yet.
pub const AMBIENT: &str = "audio/ambient/dayscape/day_temperate_gen_03.ogg";
/// music.js' DEFAULT_*_TRACKS, which a civ without its own tracks of a
/// kind falls back to.
pub const DEFAULT_TRACKS: [(MusicKind, &str); 7] = [
    (MusicKind::Peace, "Tale_of_Warriors.ogg"),
    (MusicKind::Peace, "Tavern_in_the_Mist.ogg"),
    (MusicKind::Peace, "The_Road_Ahead.ogg"),
    (MusicKind::Battle, "Taiko_1.ogg"),
    (MusicKind::Battle, "Taiko_2.ogg"),
    (MusicKind::Victory, "You_are_Victorious!.ogg"),
    (MusicKind::Defeat, "Dried_Tears.ogg"),
];
/// Opus codes at 48 kHz only; every other rate is resampled.
const OPUS_RATE: u32 = 48_000;
/// 20 ms, Opus' usual frame.
const FRAME: usize = 960;
const RESAMPLE_CHUNK: usize = 1024;
/// RFC 7845 allows any serial; a fixed one keeps the bytes, and so the
/// content-named file, the same from bake to bake.
const SERIAL: u32 = 1;
const MAX_PACKET: usize = 4000;

/// `CSoundGroup` after `LoadSoundGroup`.
pub struct Group {
    pub flags: u8,
    pub gain: f32,
    pub pitch: f32,
    pub gain_lower: f32,
    pub gain_upper: f32,
    pub pitch_lower: f32,
    pub pitch_upper: f32,
    /// Paths under the mod, in the group's order.
    pub files: Vec<String>,
}

impl Default for Group {
    /// `CSoundGroup::SetDefaultValues`.
    fn default() -> Self {
        Group {
            flags: 0,
            gain: 0.7,
            pitch: 1.0,
            gain_lower: 0.8,
            gain_upper: 1.0,
            pitch_lower: 0.9,
            pitch_upper: 1.1,
            files: Vec::new(),
        }
    }
}

/// Every group under audio/, by its path relative to audio/ (as the
/// simulation names them), and the files that failed to load.
pub fn load_groups(mod_dir: &Path) -> (Vec<(String, Group)>, Vec<String>) {
    let mut paths = Vec::new();
    list(&mod_dir.join(AUDIO_DIR), "", "xml", &mut paths);
    paths.sort();
    let mut groups = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let file = mod_dir.join(AUDIO_DIR).join(&path);
        match fs::read_to_string(&file)
            .map_err(|error| error.to_string())
            .and_then(|text| parse_group(&text))
        {
            Ok(group) => groups.push((path, group)),
            Err(error) => errors.push(format!("{AUDIO_DIR}/{path}: {error}")),
        }
    }
    (groups, errors)
}

/// Xeromyces lower-cases element names, so they match in any case.
fn parse_group(text: &str) -> Result<Group, String> {
    let document = Document::parse(text).map_err(|error| error.to_string())?;
    let root = document.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("soundgroup") {
        return Err(format!("root element {}", root.tag_name().name()));
    }
    let mut group = Group::default();
    let mut directory = String::new();
    let mut names = Vec::new();
    for child in root.children().filter(|node| node.is_element()) {
        let value = child.text().unwrap_or_default().trim();
        let flag = |bit: u8| if to_int(value) == 1 { bit } else { 0 };
        match child.tag_name().name().to_ascii_lowercase().as_str() {
            // `SetGain` caps it; the random range is not capped.
            "gain" => group.gain = to_float(value).min(1.0),
            "omnipresent" => group.flags |= flag(SOUND_OMNIPRESENT),
            "distanceless" => group.flags |= flag(SOUND_DISTANCELESS),
            "randgain" => group.flags |= flag(SOUND_RANDOM_GAIN),
            "randpitch" => group.flags |= flag(SOUND_RANDOM_PITCH),
            "heardby" => {
                if value.to_ascii_lowercase().starts_with("owner") {
                    group.flags |= SOUND_OWNER_ONLY;
                }
            }
            "pitch" => group.pitch = to_float(value),
            "gainupper" => group.gain_upper = to_float(value),
            "gainlower" => group.gain_lower = to_float(value),
            "pitchupper" => group.pitch_upper = to_float(value),
            "pitchlower" => group.pitch_lower = to_float(value),
            "sound" => names.push(value.to_string()),
            "path" => directory = value.to_string(),
            _ => {}
        }
    }
    group.files = names
        .iter()
        .map(|name| format!("{}/{name}", directory.trim_end_matches('/')))
        .collect();
    Ok(group)
}

/// A civ's tracks: kind and path under the mod.
pub type Tracks = Vec<(MusicKind, String)>;

/// Each civ's tracks, by civ code, as `session.js` hands them to the music
/// player; the defaults fill in only the kinds a civ has none of.
pub fn load_music(mod_dir: &Path) -> (Vec<(String, Tracks)>, Vec<String>) {
    let mut civs = Vec::new();
    list(&mod_dir.join(CIV_DIR), "", "json", &mut civs);
    civs.sort();
    let mut music = Vec::new();
    let mut errors = Vec::new();
    for civ in civs {
        let path = mod_dir.join(CIV_DIR).join(&civ);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let code = civ.trim_end_matches(".json").to_string();
        match civ_tracks(&text) {
            Ok(tracks) => music.push((
                code,
                tracks
                    .into_iter()
                    .map(|(kind, file)| (kind, format!("{MUSIC_DIR}/{file}")))
                    .collect(),
            )),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    (music, errors)
}

/// The "Music" array of a civ file: objects of "File" and "Type" strings.
/// Its entries hold no nested objects, so a scan of string literals per
/// object is enough, without a JSON parser.
fn civ_tracks(text: &str) -> Result<Vec<(MusicKind, String)>, String> {
    let Some(start) = text.find("\"Music\"") else {
        return Ok(Vec::new());
    };
    let rest = &text[start..];
    let open = rest.find('[').ok_or("Music is not an array")?;
    let close = rest[open..].find(']').ok_or("Music array not closed")? + open;
    let mut tracks = Vec::new();
    for object in rest[open + 1..close].split('}') {
        let strings = string_literals(object);
        let field = |key: &str| {
            strings
                .windows(2)
                .find(|pair| pair[0] == key)
                .map(|pair| pair[1].clone())
        };
        let (Some(file), Some(kind)) = (field("File"), field("Type")) else {
            continue;
        };
        let kind = match kind.as_str() {
            "peace" => MusicKind::Peace,
            "battle" => MusicKind::Battle,
            "victory" => MusicKind::Victory,
            "defeat" => MusicKind::Defeat,
            // music.js plays only these in a session.
            _ => continue,
        };
        tracks.push((kind, file));
    }
    Ok(tracks)
}

/// The string literals in order, with `\"` and `\\` unescaped.
fn string_literals(text: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut literal = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => literal.extend(chars.next()),
                _ => literal.push(c),
            }
        }
        strings.push(literal);
    }
    strings
}

/// An Ogg Vorbis file as Ogg Opus at `bitrate` bits per second for each
/// channel.
pub fn transcode(vorbis: &[u8], bitrate_per_channel: i32) -> Result<Vec<u8>, String> {
    let mut reader =
        OggStreamReader::new(Cursor::new(vorbis)).map_err(|error| error.to_string())?;
    let channels = usize::from(reader.ident_hdr.audio_channels);
    let rate = reader.ident_hdr.audio_sample_rate;
    let opus_channels = match channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        _ => return Err(format!("{channels} channels")),
    };
    let mut samples: Vec<f32> = Vec::new();
    while let Some(packet) = reader
        .read_dec_packet_itl()
        .map_err(|error| error.to_string())?
    {
        samples.extend(packet.iter().map(|&sample| f32::from(sample) / 32768.0));
    }
    if rate != OPUS_RATE {
        let frames = samples.len() / channels;
        let mut resampler = Fft::<f32>::new(
            rate as usize,
            OPUS_RATE as usize,
            RESAMPLE_CHUNK,
            channels,
            FixedSync::Both,
        )
        .map_err(|error| error.to_string())?;
        let input =
            InterleavedSlice::new(&samples, channels, frames).map_err(|error| error.to_string())?;
        samples = resampler
            .process_all(&input, frames, None)
            .map_err(|error| error.to_string())?
            .take_data();
    }

    let mut encoder = Encoder::new(OPUS_RATE, opus_channels, Application::Audio)
        .map_err(|error| error.to_string())?;
    encoder
        .set_bitrate(Bitrate::Bits(bitrate_per_channel * channels as i32))
        .map_err(|error| error.to_string())?;
    let pre_skip = encoder.get_lookahead().map_err(|error| error.to_string())? as usize;
    let frames = samples.len() / channels;
    // The encoder's lookahead delays the output, so the input is padded by
    // as much at the end for its tail to come out; the end granule
    // position trims the padding off again.
    let total = (frames + pre_skip).div_ceil(FRAME) * FRAME;
    samples.resize(total * channels, 0.0);

    let mut out = Vec::new();
    let mut writer = PacketWriter::new(&mut out);
    let write = |writer: &mut PacketWriter<&mut Vec<u8>>, packet: Vec<u8>, end, granule| {
        writer
            .write_packet(packet, SERIAL, end, granule)
            .map_err(|error| error.to_string())
    };
    write(
        &mut writer,
        opus_head(channels as u8, pre_skip as u16, rate),
        PacketWriteEndInfo::EndPage,
        0,
    )?;
    write(&mut writer, opus_tags(), PacketWriteEndInfo::EndPage, 0)?;
    let end_granule = (pre_skip + frames) as u64;
    let mut buffer = [0u8; MAX_PACKET];
    for (index, chunk) in samples.chunks(FRAME * channels).enumerate() {
        let length = encoder
            .encode_float(chunk, &mut buffer)
            .map_err(|error| error.to_string())?;
        let last = (index + 1) * FRAME == total;
        let granule = if last {
            end_granule
        } else {
            ((index + 1) * FRAME) as u64
        };
        let end = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        write(&mut writer, buffer[..length].to_vec(), end, granule)?;
    }
    drop(writer);
    Ok(out)
}

/// RFC 7845 5.1, channel mapping family 0.
fn opus_head(channels: u8, pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(channels);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&input_rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    head
}

/// RFC 7845 5.2, with no comments.
fn opus_tags() -> Vec<u8> {
    const VENDOR: &[u8] = b"specula";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    tags.extend_from_slice(VENDOR);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

/// Files with `extension` under `dir`, relative to it.
fn list(dir: &Path, prefix: &str, extension: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        if entry.path().is_dir() {
            list(&entry.path(), &format!("{path}/"), extension, out);
        } else if Path::new(&name)
            .extension()
            .is_some_and(|found| found == extension)
        {
            out.push(path);
        }
    }
}

/// `CStr::ToInt`: the leading integer, 0 if there is none.
fn to_int(value: &str) -> i32 {
    let end = value
        .char_indices()
        .find(|&(at, c)| !(c.is_ascii_digit() || (at == 0 && (c == '-' || c == '+'))))
        .map_or(value.len(), |(at, _)| at);
    value[..end].parse().unwrap_or(0)
}

/// `CStr::ToFloat`: the leading number, 0 if there is none.
fn to_float(value: &str) -> f32 {
    (1..=value.len())
        .rev()
        .filter(|end| value.is_char_boundary(*end))
        .find_map(|end| value[..end].parse().ok())
        .unwrap_or(0.0)
}
