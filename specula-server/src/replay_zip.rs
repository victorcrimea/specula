//! A hosted match's replay as one zip download. Vanilla 0 A.D. lists a
//! replay as a folder under its replays dir, so the archive unpacks to a
//! single folder.

use std::io;
use std::io::Write;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use crate::usage::civil_from_days;

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL: u32 = 0x0605_4b50;
/// 2.0, the first version with deflate.
const VERSION: u16 = 20;
/// Made on Unix, so the external attributes below are a file mode.
const MADE_BY: u16 = 3 << 8 | VERSION;
/// The names are UTF-8.
const FLAG_UTF8: u16 = 1 << 11;
const METHOD_DEFLATE: u16 = 8;
/// A plain file, rw-r--r--.
const FILE_MODE: u32 = 0o100_644 << 16;
const DAY: u64 = 86_400;

/// The archive's folder name: the match's start date, then the start of its
/// UUID, so two matches of a day stay apart in the replays dir.
pub fn folder_name(started_ms: u64, uuid: &str) -> String {
    let (year, month, day) = civil_from_days(started_ms / 1000 / DAY);
    let short: String = uuid.chars().take(8).collect();
    format!("{year:04}-{month:02}-{day:02}_{short}")
}

/// `files` go into `folder`, dated `unix` seconds.
pub fn build(folder: &str, files: &[(&str, Vec<u8>)], unix: u64) -> io::Result<Vec<u8>> {
    let (time, date) = dos_time(unix);
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let path = format!("{folder}/{name}");
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(data)?;
        let packed = encoder.finish()?;
        let entry = Entry {
            crc: crc32fast::hash(data),
            packed: size(packed.len())?,
            size: size(data.len())?,
            name: path.as_bytes(),
            time,
            date,
        };
        let offset = size(out.len())?;
        put_u32(&mut out, LOCAL_HEADER);
        put_u16(&mut out, VERSION);
        entry.put_common(&mut out)?;
        out.extend_from_slice(entry.name);
        out.extend_from_slice(&packed);

        put_u32(&mut central, CENTRAL_HEADER);
        put_u16(&mut central, MADE_BY);
        put_u16(&mut central, VERSION);
        entry.put_common(&mut central)?;
        // Comment length, disk, internal attributes.
        put_u16(&mut central, 0);
        put_u16(&mut central, 0);
        put_u16(&mut central, 0);
        put_u32(&mut central, FILE_MODE);
        put_u32(&mut central, offset);
        central.extend_from_slice(entry.name);
    }
    let count = u16::try_from(files.len()).map_err(|_| too_large())?;
    let central_offset = size(out.len())?;
    let central_size = size(central.len())?;
    out.extend_from_slice(&central);
    put_u32(&mut out, END_OF_CENTRAL);
    // This disk, the central directory's disk.
    put_u16(&mut out, 0);
    put_u16(&mut out, 0);
    put_u16(&mut out, count);
    put_u16(&mut out, count);
    put_u32(&mut out, central_size);
    put_u32(&mut out, central_offset);
    // Comment length.
    put_u16(&mut out, 0);
    Ok(out)
}

struct Entry<'a> {
    crc: u32,
    packed: u32,
    size: u32,
    name: &'a [u8],
    time: u16,
    date: u16,
}

impl Entry<'_> {
    /// From the flags to the extra field's length, the same in the local
    /// and the central header.
    fn put_common(&self, out: &mut Vec<u8>) -> io::Result<()> {
        put_u16(out, FLAG_UTF8);
        put_u16(out, METHOD_DEFLATE);
        put_u16(out, self.time);
        put_u16(out, self.date);
        put_u32(out, self.crc);
        put_u32(out, self.packed);
        put_u32(out, self.size);
        put_u16(
            out,
            u16::try_from(self.name.len()).map_err(|_| too_large())?,
        );
        put_u16(out, 0);
        Ok(())
    }
}

/// MS-DOS time and date, UTC; the format cannot hold a time zone.
fn dos_time(unix: u64) -> (u16, u16) {
    let (year, month, day) = civil_from_days(unix / DAY);
    let seconds = unix % DAY;
    let time = (seconds / 3600) << 11 | (seconds % 3600 / 60) << 5 | (seconds % 60 / 2);
    // The format starts in 1980.
    let year = u64::try_from(year - 1980).unwrap_or(0);
    let date = year << 9 | (month as u64) << 5 | day as u64;
    (time as u16, date as u16)
}

/// No zip64: a replay is far below 4 GiB.
fn size(len: usize) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| too_large())
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "too large for a zip without zip64",
    )
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
