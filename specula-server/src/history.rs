//! Every match's history on disk, kept under a size limit by deleting the
//! oldest matches. One directory per match, named by its UUID
//! (`/g/<uuid>`):
//!
//! - `id`: the stream's match id as 16 hex digits.
//! - `seg/<n>`, `detail/<n>`: the closed segments, as served.
//! - `terrain-<id>` and `terrain-<id>.br`: the terrain resource and its
//!   brotli copy, by terrain id as 16 hex digits.
//! - `data/<hash>`: the kind 18 and 19 JSON, by hash as 16 hex digits.
//! - `summary/<turn>.br`: every statistics capture shown to `/`, the brotli
//!   of its envelope, by turn as 8 hex digits. A later capture does not
//!   hold an earlier one's "now" sample, so each is kept.
//! - `replay/commands.txt`, `replay/metadata.json`: a hosted match's replay
//!   as the engine wrote it, which vanilla 0 A.D. plays.
//!
//! Each file is written under a `.tmp` name and renamed, so a crash leaves
//! no half file. Match ids are nanosecond timestamps, so the smallest is the
//! oldest. A directory named by 16 hex digits is from before UUIDs and is
//! given a new UUID at open.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use crate::registry::MatchId;

const TMP_SUFFIX: &str = ".tmp";
const TERRAIN_PREFIX: &str = "terrain-";
const BROTLI_SUFFIX: &str = ".br";
const SEGMENTS_DIR: &str = "seg";
const DETAIL_DIR: &str = "detail";
const DATA_DIR: &str = "data";
const SUMMARY_DIR: &str = "summary";
const REPLAY_DIR: &str = "replay";
/// Vanilla lists a replay folder only with this file in it.
pub const REPLAY_COMMANDS: &str = "commands.txt";
pub const REPLAY_METADATA: &str = "metadata.json";
const ID_FILE: &str = "id";
const META_FILE: &str = "meta.json";
/// A stored match without a `meta.json` is dated and sized from its files:
/// 150 turns of 200 ms per segment.
const SEGMENT_SECS: u32 = 30;

struct Stored {
    /// Names the directory.
    uuid: MatchId,
    bytes: u64,
    /// Visual segments; the detail stream is cut on the same turns.
    segments: usize,
    terrains: Vec<u64>,
    data: Vec<u64>,
    /// The turns of the stored captures, sorted.
    summaries: Vec<u32>,
    replay: bool,
}

impl Stored {
    fn new(uuid: MatchId) -> Self {
        Stored {
            uuid,
            bytes: 0,
            segments: 0,
            terrains: Vec::new(),
            data: Vec::new(),
            summaries: Vec::new(),
            replay: false,
        }
    }
}

/// What the lobby shows of a stored match (M40); written at the start and
/// again at the end, so a match cut short keeps the first.
pub struct Meta {
    pub map_name: Option<String>,
    pub host: String,
    /// Name, civ code and 0xRRGGBB colour, gaia left out.
    pub players: Vec<(String, String, u32)>,
    pub duration_secs: u32,
}

impl Meta {
    fn to_bytes(&self) -> Vec<u8> {
        serde_json::json!({
            "map": self.map_name,
            "host": self.host,
            "players": self.players.iter()
                .map(|(name, civ, color)| serde_json::json!([name, civ, color]))
                .collect::<Vec<_>>(),
            "duration": self.duration_secs,
        })
        .to_string()
        .into_bytes()
    }

    fn from_bytes(bytes: &[u8]) -> Option<Meta> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let players = value
            .get("players")?
            .as_array()?
            .iter()
            .filter_map(|entry| {
                Some((
                    entry.get(0)?.as_str()?.to_string(),
                    entry.get(1)?.as_str()?.to_string(),
                    u32::try_from(entry.get(2)?.as_u64()?).ok()?,
                ))
            })
            .collect();
        Some(Meta {
            map_name: value.get("map")?.as_str().map(str::to_string),
            host: value.get("host")?.as_str()?.to_string(),
            players,
            duration_secs: u32::try_from(value.get("duration")?.as_u64()?).ok()?,
        })
    }
}

/// A finished stored match as the lobby lists it.
pub struct StoredInfo {
    pub uuid: MatchId,
    pub started_ms: u64,
    pub duration_secs: u32,
    /// None for a match stored before M40.
    pub meta: Option<Meta>,
    pub replay: bool,
}

pub struct History {
    dir: PathBuf,
    limit: u64,
    total: u64,
    matches: BTreeMap<u64, Stored>,
    /// The stream id of each stored match, for `/g/<uuid>`.
    by_uuid: HashMap<MatchId, u64>,
    /// By id and hash, the matches holding them: the same JSON and terrain
    /// recur across matches.
    terrains: HashMap<u64, Vec<u64>>,
    data: HashMap<u64, Vec<u64>>,
    /// Matches still being written, by any hub: one hub's write must not
    /// evict another's running match.
    live: HashSet<u64>,
}

impl History {
    pub fn open(dir: PathBuf, limit: u64) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let mut history = History {
            dir,
            limit,
            total: 0,
            matches: BTreeMap::new(),
            by_uuid: HashMap::new(),
            terrains: HashMap::new(),
            data: HashMap::new(),
            live: HashSet::new(),
        };
        // Listed before any migration renames a directory: a directory
        // renamed while it is read may be listed again under its new name.
        let entries = fs::read_dir(&history.dir)?.collect::<io::Result<Vec<_>>>()?;
        for entry in entries {
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let path = entry.path();
            let (uuid, match_id, dir) = if let Some(uuid) = MatchId::parse(name) {
                let Some(match_id) = read_id(&path) else {
                    eprintln!("history: {} has no id file, skipped", path.display());
                    continue;
                };
                if history.matches.contains_key(&match_id) {
                    eprintln!("history: {} is a duplicate id, skipped", path.display());
                    continue;
                }
                (uuid, match_id, path)
            } else if let Some(match_id) = parse_hex(name) {
                let uuid = MatchId::new_v4();
                let new = history.dir.join(uuid.to_string());
                // One unmovable directory must not keep the server down.
                if let Err(error) = migrate(&path, &new, match_id) {
                    eprintln!("history: cannot migrate {}: {error}", path.display());
                    continue;
                }
                println!("history: match {match_id:016x} is now {uuid}");
                (uuid, match_id, new)
            } else {
                continue;
            };
            let stored = scan_match(&dir, uuid)?;
            history.add(match_id, stored);
        }
        Ok(history)
    }

    pub fn set_live(&mut self, match_id: u64, live: bool) {
        if live {
            self.live.insert(match_id);
        } else {
            self.live.remove(&match_id);
        }
    }

    /// For the startup log.
    pub fn summary(&self) -> String {
        format!(
            "history in {}: {} matches, {} of {} bytes",
            self.dir.display(),
            self.matches.len(),
            self.total,
            self.limit
        )
    }

    fn add(&mut self, match_id: u64, stored: Stored) {
        self.total += stored.bytes;
        for id in &stored.terrains {
            self.terrains.entry(*id).or_default().push(match_id);
        }
        for hash in &stored.data {
            self.data.entry(*hash).or_default().push(match_id);
        }
        self.by_uuid.insert(stored.uuid, match_id);
        self.matches.insert(match_id, stored);
    }

    fn match_dir(&self, match_id: u64) -> Option<PathBuf> {
        self.matches
            .get(&match_id)
            .map(|stored| self.dir.join(stored.uuid.to_string()))
    }

    /// Names a new match's directory; every other write needs it first.
    /// `protect` as for the other writes.
    pub fn start_match(&mut self, match_id: u64, uuid: MatchId, protect: &[u64]) -> io::Result<()> {
        if self.matches.contains_key(&match_id) {
            return Ok(());
        }
        // Inserted before the write, so a failed id write still lets the
        // segments be stored (the directory is then skipped at the next open).
        self.matches.insert(match_id, Stored::new(uuid));
        self.by_uuid.insert(uuid, match_id);
        let path = self.dir.join(uuid.to_string()).join(ID_FILE);
        self.write(
            match_id,
            &path,
            format!("{match_id:016x}").as_bytes(),
            protect,
        )
    }

    /// Replaces the match's lobby record; a failure only costs the lobby
    /// its details.
    pub fn write_meta(&mut self, match_id: u64, meta: &Meta, protect: &[u64]) -> io::Result<()> {
        let Some(stored) = self.matches.get(&match_id) else {
            return Ok(());
        };
        let path = self.dir.join(stored.uuid.to_string()).join(META_FILE);
        self.write(match_id, &path, &meta.to_bytes(), protect)
    }

    /// The `limit` newest finished matches, and whether older ones remain.
    pub fn list(&self, limit: usize) -> (Vec<StoredInfo>, bool) {
        let mut finished = self
            .matches
            .iter()
            .rev()
            .filter(|(id, _)| !self.live.contains(id));
        let listed = finished
            .by_ref()
            .take(limit)
            .map(|(match_id, stored)| {
                let path = self.dir.join(stored.uuid.to_string()).join(META_FILE);
                let meta = fs::read(path)
                    .ok()
                    .and_then(|bytes| Meta::from_bytes(&bytes));
                StoredInfo {
                    uuid: stored.uuid,
                    // Match ids are nanoseconds since the epoch.
                    started_ms: match_id / 1_000_000,
                    duration_secs: meta.as_ref().map_or_else(
                        || stored.segments as u32 * SEGMENT_SECS,
                        |meta| meta.duration_secs,
                    ),
                    meta,
                    replay: stored.replay,
                }
            })
            .collect();
        (listed, finished.next().is_some())
    }

    pub fn match_of(&self, uuid: MatchId) -> Option<u64> {
        self.by_uuid.get(&uuid).copied()
    }

    pub fn uuid_of(&self, match_id: u64) -> Option<MatchId> {
        self.matches.get(&match_id).map(|stored| stored.uuid)
    }

    /// `protect`: matches that must stay, besides `match_id` itself.
    pub fn write_segment(
        &mut self,
        match_id: u64,
        detail: bool,
        index: usize,
        bytes: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        let dir = if detail { DETAIL_DIR } else { SEGMENTS_DIR };
        let path = self
            .match_dir(match_id)
            .ok_or_else(not_stored)?
            .join(dir)
            .join(index.to_string());
        self.write(match_id, &path, bytes, protect)?;
        if !detail && let Some(stored) = self.matches.get_mut(&match_id) {
            stored.segments = stored.segments.max(index + 1);
        }
        Ok(())
    }

    pub fn write_terrain(
        &mut self,
        match_id: u64,
        id: u64,
        bytes: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        let path = self
            .match_dir(match_id)
            .ok_or_else(not_stored)?
            .join(format!("{TERRAIN_PREFIX}{id:016x}"));
        self.write(match_id, &path, bytes, protect)?;
        if let Some(stored) = self.matches.get_mut(&match_id)
            && !stored.terrains.contains(&id)
        {
            stored.terrains.push(id);
            self.terrains.entry(id).or_default().push(match_id);
        }
        Ok(())
    }

    /// The terrain's brotli copy, made long after the raw file.
    pub fn write_terrain_brotli(
        &mut self,
        match_id: u64,
        id: u64,
        bytes: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        // The match may have been deleted while the copy was made.
        let Some(dir) = self.match_dir(match_id) else {
            return Ok(());
        };
        let path = dir.join(format!("{TERRAIN_PREFIX}{id:016x}{BROTLI_SUFFIX}"));
        self.write(match_id, &path, bytes, protect)
    }

    pub fn write_data(
        &mut self,
        match_id: u64,
        hash: u64,
        bytes: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        if self
            .matches
            .get(&match_id)
            .is_some_and(|stored| stored.data.contains(&hash))
        {
            return Ok(());
        }
        let path = self
            .match_dir(match_id)
            .ok_or_else(not_stored)?
            .join(DATA_DIR)
            .join(format!("{hash:016x}"));
        self.write(match_id, &path, bytes, protect)?;
        if let Some(stored) = self.matches.get_mut(&match_id) {
            stored.data.push(hash);
            self.data.entry(hash).or_default().push(match_id);
        }
        Ok(())
    }

    pub fn write_summary(
        &mut self,
        match_id: u64,
        turn: u32,
        brotli: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        let path = self
            .match_dir(match_id)
            .ok_or_else(not_stored)?
            .join(SUMMARY_DIR)
            .join(format!("{turn:08x}{BROTLI_SUFFIX}"));
        self.write(match_id, &path, brotli, protect)?;
        if let Some(stored) = self.matches.get_mut(&match_id)
            && let Err(index) = stored.summaries.binary_search(&turn)
        {
            stored.summaries.insert(index, turn);
        }
        Ok(())
    }

    /// One file of the engine's replay, `REPLAY_COMMANDS` or
    /// `REPLAY_METADATA`.
    pub fn write_replay(
        &mut self,
        match_id: u64,
        name: &str,
        bytes: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        let path = self
            .match_dir(match_id)
            .ok_or_else(not_stored)?
            .join(REPLAY_DIR)
            .join(name);
        self.write(match_id, &path, bytes, protect)?;
        if name == REPLAY_COMMANDS
            && let Some(stored) = self.matches.get_mut(&match_id)
        {
            stored.replay = true;
        }
        Ok(())
    }

    /// The replay's files by name, `REPLAY_COMMANDS` first; None without
    /// one.
    pub fn replay_files(&self, match_id: u64) -> Option<Vec<(&'static str, PathBuf)>> {
        if !self.matches.get(&match_id)?.replay {
            return None;
        }
        let dir = self.match_dir(match_id)?.join(REPLAY_DIR);
        let mut files = vec![(REPLAY_COMMANDS, dir.join(REPLAY_COMMANDS))];
        let metadata = dir.join(REPLAY_METADATA);
        // The engine writes it only when asked before its stop.
        if metadata.is_file() {
            files.push((REPLAY_METADATA, metadata));
        }
        Some(files)
    }

    /// The file of the capture at or before `turn`; the last one for `None`.
    pub fn summary_at(&self, match_id: u64, turn: Option<u32>) -> Option<PathBuf> {
        let stored = self.matches.get(&match_id)?;
        let turn = match turn {
            Some(turn) => *stored
                .summaries
                .iter()
                .rev()
                .find(|known| **known <= turn)?,
            None => *stored.summaries.last()?,
        };
        Some(
            self.match_dir(match_id)?
                .join(SUMMARY_DIR)
                .join(format!("{turn:08x}{BROTLI_SUFFIX}")),
        )
    }

    fn write(
        &mut self,
        match_id: u64,
        path: &Path,
        bytes: &[u8],
        protect: &[u64],
    ) -> io::Result<()> {
        let incoming = bytes.len() as u64;
        if self.total + incoming > self.limit {
            self.evict(incoming, match_id, protect);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(TMP_SUFFIX);
        // A rewrite replaces the file, so its old size no longer counts.
        let replaced = fs::metadata(path).map_or(0, |metadata| metadata.len());
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)?;
        if let Some(stored) = self.matches.get_mut(&match_id) {
            stored.bytes = stored.bytes + incoming - replaced;
        }
        self.total = self.total + incoming - replaced;
        Ok(())
    }

    /// Deletes the oldest matches until `incoming` more bytes leave the
    /// store under 90% of the limit, so that deletions come in batches
    /// rather than on every write.
    fn evict(&mut self, incoming: u64, writing: u64, protect: &[u64]) {
        let target = self.limit / 10 * 9;
        let oldest: Vec<u64> = self
            .matches
            .keys()
            .copied()
            .filter(|id| *id != writing && !protect.contains(id) && !self.live.contains(id))
            .collect();
        for match_id in oldest {
            if self.total + incoming <= target {
                return;
            }
            self.remove(match_id);
        }
        if self.total + incoming > self.limit {
            eprintln!(
                "history over its limit: {} bytes in matches being shown or written, limit {}",
                self.total + incoming,
                self.limit
            );
        }
    }

    fn remove(&mut self, match_id: u64) {
        let Some(stored) = self.matches.remove(&match_id) else {
            return;
        };
        let path = self.dir.join(stored.uuid.to_string());
        self.by_uuid.remove(&stored.uuid);
        if let Err(error) = fs::remove_dir_all(&path) {
            eprintln!("cannot delete {}: {error}", path.display());
        }
        self.total -= stored.bytes;
        unindex(&mut self.terrains, &stored.terrains, match_id);
        unindex(&mut self.data, &stored.data, match_id);
        println!(
            "history: deleted match {match_id:016x} ({} bytes), {} bytes kept",
            stored.bytes, self.total
        );
    }

    pub fn segment_count(&self, match_id: u64) -> Option<usize> {
        self.matches
            .get(&match_id)
            .map(|stored| stored.segments)
            .filter(|segments| *segments > 0)
    }

    pub fn segment_path(&self, match_id: u64, detail: bool, index: usize) -> Option<PathBuf> {
        if index >= self.segment_count(match_id)? {
            return None;
        }
        let dir = if detail { DETAIL_DIR } else { SEGMENTS_DIR };
        Some(self.match_dir(match_id)?.join(dir).join(index.to_string()))
    }

    /// The raw file and the brotli copy's path, which may not exist yet.
    pub fn terrain_paths(&self, id: u64) -> Option<(PathBuf, PathBuf)> {
        let match_id = *self.terrains.get(&id)?.last()?;
        let raw = self
            .match_dir(match_id)?
            .join(format!("{TERRAIN_PREFIX}{id:016x}"));
        let mut brotli = raw.as_os_str().to_owned();
        brotli.push(BROTLI_SUFFIX);
        Some((raw, brotli.into()))
    }

    pub fn data_path(&self, hash: u64) -> Option<PathBuf> {
        let match_id = *self.data.get(&hash)?.last()?;
        Some(
            self.match_dir(match_id)?
                .join(DATA_DIR)
                .join(format!("{hash:016x}")),
        )
    }
}

fn unindex(index: &mut HashMap<u64, Vec<u64>>, ids: &[u64], match_id: u64) {
    for id in ids {
        if let Some(matches) = index.get_mut(id) {
            matches.retain(|kept| *kept != match_id);
            if matches.is_empty() {
                index.remove(id);
            }
        }
    }
}

fn not_stored() -> io::Error {
    io::Error::new(ErrorKind::NotFound, "match not started in the history")
}

fn read_id(dir: &Path) -> Option<u64> {
    parse_hex(fs::read_to_string(dir.join(ID_FILE)).ok()?.trim())
}

/// The id file goes in first, so a crash between the two steps leaves a
/// legacy directory that migrates again at the next start.
fn migrate(old: &Path, new: &Path, match_id: u64) -> io::Result<()> {
    let path = old.join(ID_FILE);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(TMP_SUFFIX);
    fs::write(&tmp, format!("{match_id:016x}"))?;
    fs::rename(&tmp, &path)?;
    fs::rename(old, new)
}

fn parse_hex(name: &str) -> Option<u64> {
    (name.len() == 16)
        .then(|| u64::from_str_radix(name, 16).ok())
        .flatten()
}

fn parse_turn(name: &str) -> Option<u32> {
    (name.len() == 8)
        .then(|| u32::from_str_radix(name, 16).ok())
        .flatten()
}

/// Sizes and names one match directory; a `.tmp` file a crash left behind
/// is deleted.
fn scan_match(dir: &Path, uuid: MatchId) -> io::Result<Stored> {
    let mut stored = Stored::new(uuid);
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            for file in fs::read_dir(&path)? {
                let file = file?;
                let file_name = file.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name.ends_with(TMP_SUFFIX) {
                    let _ = fs::remove_file(file.path());
                    continue;
                }
                stored.bytes += file.metadata()?.len();
                match &*name {
                    SEGMENTS_DIR => {
                        if let Ok(index) = file_name.parse::<usize>() {
                            stored.segments = stored.segments.max(index + 1);
                        }
                    }
                    DATA_DIR => stored.data.extend(parse_hex(&file_name)),
                    SUMMARY_DIR => {
                        if let Some(turn) =
                            file_name.strip_suffix(BROTLI_SUFFIX).and_then(parse_turn)
                        {
                            stored.summaries.push(turn);
                        }
                    }
                    REPLAY_DIR if file_name == REPLAY_COMMANDS => stored.replay = true,
                    _ => {}
                }
            }
            continue;
        }
        if name.ends_with(TMP_SUFFIX) {
            let _ = fs::remove_file(&path);
            continue;
        }
        stored.bytes += entry.metadata()?.len();
        if let Some(id) = name.strip_prefix(TERRAIN_PREFIX).and_then(parse_hex) {
            stored.terrains.push(id);
        }
    }
    stored.summaries.sort_unstable();
    Ok(stored)
}

/// `1G`, `500M`, `64K` (binary) or plain bytes.
pub fn parse_size(text: &str) -> Option<u64> {
    let (digits, shift) = match text.chars().last()?.to_ascii_uppercase() {
        'K' => (&text[..text.len() - 1], 10),
        'M' => (&text[..text.len() - 1], 20),
        'G' => (&text[..text.len() - 1], 30),
        'T' => (&text[..text.len() - 1], 40),
        _ => (text, 0),
    };
    digits.parse::<u64>().ok()?.checked_mul(1 << shift)
}
