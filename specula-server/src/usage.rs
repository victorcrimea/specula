//! How the broadcast is used, for the operator's `/stats`. Behind Cloudflare
//! the server never sees a viewer's address, so a visitor is a random id in
//! the `sv` cookie, set on the first page load; nothing else about the
//! visitor is kept.
//!
//! Every event is a line of `events.log`, tab separated:
//! `<unix seconds> <kind> <visitor or -> <fields...>`:
//!
//! - `page <page> <referrer host or ->`: a page load.
//! - `live <seconds>`: a closed `/ws` stream, the line written at its end.
//! - `replay <match id>`: a stored match opened.
//! - `beat <seconds> <mode> <renderer> <fps> <cpu ms> <gpu ms> <settings>`:
//!   the client's heartbeat while it is shown, `-` for what it did not
//!   measure.
//! - `fail <reason>`: the client's renderer could not start.
//!
//! The report reads the whole log each time: it survives restarts with no
//! state of its own, and an early broadcast's log is small.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::hash::BuildHasher;
use std::hash::RandomState;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;
use std::time::SystemTime;

use rocket::Request;
use rocket::http::Cookie;
use rocket::http::CookieJar;
use rocket::http::SameSite;
use rocket::request::FromRequest;
use rocket::request::Outcome;

const LOG: &str = "events.log";
const COOKIE: &str = "sv";
/// About as long as a browser keeps a cookie anyway.
const COOKIE_DAYS: i64 = 400;
const DAY: u64 = 86_400;
const WEEK: u64 = 7 * DAY;
/// Shorter streams are a page that failed or was left at once.
const VIEWER_MIN_LIVE_SECS: u64 = 10;
/// Twice the client's beat interval.
const ACTIVE_SECS: u64 = 120;
/// A beat longer than its interval allows is not believed past this.
const MAX_BEAT_SECS: u64 = 600;
const DAILY_ROWS: u64 = 14;
const MAX_FIELD: usize = 4096;

pub struct Usage {
    path: PathBuf,
    file: Mutex<File>,
    /// None: `/stats` is not served.
    key: Option<String>,
    live: AtomicUsize,
    /// A stored match is counted once per visitor: the client polls the
    /// open match's info while it plays.
    replays: Mutex<HashSet<(String, String)>>,
}

impl Usage {
    pub fn open(dir: PathBuf, key: Option<String>) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let path = dir.join(LOG);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Usage {
            path,
            file: Mutex::new(file),
            key,
            live: AtomicUsize::new(0),
            replays: Mutex::new(HashSet::new()),
        })
    }

    pub fn summary(&self) -> String {
        format!(
            "usage logged to {}, /stats {}",
            self.path.display(),
            if self.key.is_some() {
                "served with --stats-key"
            } else {
                "off (no --stats-key)"
            }
        )
    }

    fn log(&self, kind: &str, visitor: &str, fields: &[&str]) {
        let mut line = format!("{}\t{kind}\t{}", unix_now(), clean(visitor));
        for field in fields {
            line.push('\t');
            line.push_str(&clean(field));
        }
        line.push('\n');
        // Usage is a side matter: a full disk must not take viewers down.
        if let Err(error) = self.file.lock().unwrap().write_all(line.as_bytes()) {
            eprintln!("cannot log usage: {error}");
        }
    }

    pub fn page(&self, cookies: &CookieJar<'_>, page: &str, referrer: &Referrer) {
        let visitor = visitor(cookies).unwrap_or_else(|| {
            let id = new_id();
            cookies.add(
                Cookie::build((COOKIE, id.clone()))
                    .max_age(rocket::time::Duration::days(COOKIE_DAYS))
                    .same_site(SameSite::Lax)
                    .http_only(true),
            );
            id
        });
        self.log(
            "page",
            &visitor,
            &[page, referrer.0.as_deref().unwrap_or("-")],
        );
    }

    /// Held for as long as a `/ws` stream is open.
    pub fn watch(self: &Arc<Self>, cookies: &CookieJar<'_>) -> Watch {
        self.live.fetch_add(1, Ordering::Relaxed);
        Watch {
            usage: Arc::clone(self),
            visitor: visitor(cookies).unwrap_or_else(|| "-".to_string()),
            start: Instant::now(),
        }
    }

    pub fn replay(&self, cookies: &CookieJar<'_>, match_id: &str) {
        let visitor = visitor(cookies).unwrap_or_else(|| "-".to_string());
        if self
            .replays
            .lock()
            .unwrap()
            .insert((visitor.clone(), match_id.to_string()))
        {
            self.log("replay", &visitor, &[match_id]);
        }
    }

    /// The body of a `/beat` beacon, as `usage.rs` in the client writes it.
    pub fn beat(&self, cookies: &CookieJar<'_>, body: &str) {
        let visitor = visitor(cookies).unwrap_or_else(|| "-".to_string());
        let fields: Vec<&str> = body.trim_end().split('\t').collect();
        match fields.as_slice() {
            ["beat", seconds, mode, renderer, fps, cpu, gpu, settings] => {
                let Ok(seconds) = seconds.parse::<u64>() else {
                    return;
                };
                let seconds = seconds.min(MAX_BEAT_SECS).to_string();
                self.log(
                    "beat",
                    &visitor,
                    &[&seconds, mode, renderer, fps, cpu, gpu, settings],
                );
            }
            ["fail", reason] => self.log("fail", &visitor, &[reason]),
            _ => {}
        }
    }

    pub fn authorized(&self, key: Option<&str>) -> bool {
        self.key.is_some() && self.key.as_deref() == key
    }

    pub fn report(&self) -> String {
        let mut text = String::new();
        // Read under the lock, so a line being written is not read half.
        let read = {
            let _file = self.file.lock().unwrap();
            File::open(&self.path).and_then(|mut file| file.read_to_string(&mut text))
        };
        if let Err(error) = read {
            return format!("cannot read {}: {error}\n", self.path.display());
        }
        report(&text, unix_now(), self.live.load(Ordering::Relaxed))
    }
}

pub struct Watch {
    usage: Arc<Usage>,
    visitor: String,
    start: Instant,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.usage.live.fetch_sub(1, Ordering::Relaxed);
        let seconds = self.start.elapsed().as_secs().to_string();
        self.usage.log("live", &self.visitor, &[&seconds]);
    }
}

/// The host of the page a visitor came from, if another site.
pub struct Referrer(Option<String>);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Referrer {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, ()> {
        let host = |url: &str| {
            let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            host.strip_prefix("www.")
                .unwrap_or(host)
                .to_ascii_lowercase()
        };
        let own = request.headers().get_one("Host").map(host);
        let referrer = request
            .headers()
            .get_one("Referer")
            .map(host)
            .filter(|referrer| !referrer.is_empty() && Some(referrer) != own.as_ref());
        Outcome::Success(Referrer(referrer))
    }
}

/// The visitor id, which also binds a player's seat (`play.rs`).
pub fn visitor(cookies: &CookieJar<'_>) -> Option<String> {
    let value = cookies.get(COOKIE)?.value();
    (value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_string())
}

fn new_id() -> String {
    let mut bytes = [0; 8];
    let random = File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if random.is_err() {
        // Still unique enough to tell visitors apart.
        let state = RandomState::new();
        bytes = state.hash_one(SystemTime::now()).to_le_bytes();
    }
    format!("{:016x}", u64::from_le_bytes(bytes))
}

fn clean(field: &str) -> String {
    field
        .chars()
        .take(MAX_FIELD)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

struct Event<'a> {
    at: u64,
    kind: &'a str,
    /// None for a visitor without the cookie.
    visitor: Option<&'a str>,
    fields: Vec<&'a str>,
}

fn parse(text: &str) -> Vec<Event<'_>> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let at = parts.next()?.parse().ok()?;
            let kind = parts.next()?;
            let visitor = parts.next()?;
            Some(Event {
                at,
                kind,
                visitor: (visitor != "-").then_some(visitor),
                fields: parts.collect(),
            })
        })
        .collect()
}

fn number(field: Option<&&str>) -> Option<f64> {
    field?.parse().ok()
}

/// The seconds an event adds to the time watched, and whether it makes its
/// visitor a viewer.
fn watched(event: &Event) -> Option<(u64, bool)> {
    let seconds = number(event.fields.first())? as u64;
    match event.kind {
        "live" => Some((seconds, seconds >= VIEWER_MIN_LIVE_SECS)),
        "beat" => Some((seconds, true)),
        _ => None,
    }
}

#[derive(Default)]
struct Period {
    page_loads: u64,
    page_visitors: HashSet<String>,
    viewers: HashSet<String>,
    new_viewers: u64,
    live_secs: u64,
    beat_secs: u64,
    replays: u64,
    fails: u64,
    /// Stream start and end, for the peak.
    streams: Vec<(u64, u64)>,
}

impl Period {
    fn add(&mut self, event: &Event, first_seen: &HashMap<&str, u64>, from: u64) {
        let visitor = event.visitor;
        match event.kind {
            "page" => {
                self.page_loads += 1;
                if let Some(visitor) = visitor {
                    self.page_visitors.insert(visitor.to_string());
                }
            }
            "replay" => self.replays += 1,
            "fail" => self.fails += 1,
            _ => {}
        }
        if let Some((seconds, viewer)) = watched(event) {
            if event.kind == "live" {
                self.live_secs += seconds;
                self.streams
                    .push((event.at.saturating_sub(seconds), event.at));
            } else {
                self.beat_secs += seconds;
            }
            if viewer
                && let Some(visitor) = visitor
                && self.viewers.insert(visitor.to_string())
                && first_seen.get(visitor).is_some_and(|first| *first >= from)
            {
                self.new_viewers += 1;
            }
        }
    }

    /// The most streams open at once, and when.
    fn peak(&self) -> (usize, u64) {
        let mut edges: Vec<(u64, i32)> = Vec::new();
        for (start, end) in &self.streams {
            edges.push((*start, 1));
            edges.push((*end, -1));
        }
        // Ends before starts at the same second: a reconnect is one viewer.
        edges.sort_unstable_by_key(|(at, step)| (*at, *step));
        let (mut open, mut peak, mut peak_at) = (0, 0, 0);
        for (at, step) in edges {
            open += step;
            if open > peak {
                peak = open;
                peak_at = at;
            }
        }
        (peak as usize, peak_at)
    }
}

fn report(text: &str, now: u64, live_now: usize) -> String {
    let events = parse(text);
    let mut first_seen: HashMap<&str, u64> = HashMap::new();
    for event in &events {
        if let Some((_, true)) = watched(event)
            && let Some(visitor) = event.visitor
        {
            first_seen.entry(visitor).or_insert(event.at);
        }
    }
    let today_from = now - now % DAY;
    let week_from = now.saturating_sub(WEEK);
    let mut today = Period::default();
    let mut week = Period::default();
    let mut total = Period::default();
    let first_day = (now / DAY).saturating_sub(DAILY_ROWS - 1);
    let mut days: BTreeMap<u64, Period> = BTreeMap::new();
    for event in &events {
        total.add(event, &first_seen, 0);
        if event.at >= week_from {
            week.add(event, &first_seen, week_from);
        }
        if event.at >= today_from {
            today.add(event, &first_seen, today_from);
        }
        let day = event.at / DAY;
        if day >= first_day {
            days.entry(day)
                .or_default()
                .add(event, &first_seen, day * DAY);
        }
    }

    let mut out = String::new();
    let active: HashSet<&str> = events
        .iter()
        .filter(|event| event.kind == "beat" && event.at + ACTIVE_SECS >= now)
        .filter_map(|event| event.visitor)
        .collect();
    let _ = writeln!(out, "now {} UTC", format_time(now));
    let _ = writeln!(
        out,
        "watching now: {live_now} live streams open, {} visitors active in the last {} s",
        active.len(),
        ACTIVE_SECS
    );
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "{:<28}{:>10}{:>10}{:>10}",
        "", "today", "7 days", "total"
    );
    let periods = [&today, &week, &total];
    let mut row = |label: &str, value: &dyn Fn(&Period) -> String| {
        let _ = write!(out, "{label:<28}");
        for period in periods {
            let _ = write!(out, "{:>10}", value(period));
        }
        let _ = writeln!(out);
    };
    row("page loads", &|period| period.page_loads.to_string());
    row("visitors (page loaded)", &|period| {
        period.page_visitors.len().to_string()
    });
    row("viewers (watched)", &|period| {
        period.viewers.len().to_string()
    });
    row("new viewers", &|period| period.new_viewers.to_string());
    row("minutes watched (shown)", &|period| {
        minutes(period.beat_secs).to_string()
    });
    row("minutes of live streams", &|period| {
        minutes(period.live_secs).to_string()
    });
    row("stored matches opened", &|period| {
        period.replays.to_string()
    });
    row("renderer failed", &|period| period.fails.to_string());
    row("peak live streams", &|period| period.peak().0.to_string());
    let _ = writeln!(
        out,
        "  (minutes shown: the page visible; live streams: open, also in background tabs)"
    );
    let (peak, peak_at) = week.peak();
    if peak > 0 {
        let _ = writeln!(out, "7-day peak of {peak} at {} UTC", format_time(peak_at));
    }

    let recent: Vec<&Event> = events
        .iter()
        .filter(|event| event.at >= week_from)
        .collect();
    funnel(&mut out, &recent);
    modes(&mut out, &recent);
    referrers(&mut out, &recent);
    renderers(&mut out, &recent);
    frame_times(&mut out, &recent);
    settings(&mut out, &recent);

    let _ = writeln!(out, "\nby day (UTC)");
    let _ = writeln!(
        out,
        "{:<12}{:>9}{:>9}{:>9}{:>9}{:>9}{:>7}",
        "", "visitors", "viewers", "new", "min seen", "min live", "peak"
    );
    for day in (first_day..=now / DAY).rev() {
        let empty = Period::default();
        let period = days.get(&day).unwrap_or(&empty);
        let _ = writeln!(
            out,
            "{:<12}{:>9}{:>9}{:>9}{:>9}{:>9}{:>7}",
            format_date(day),
            period.page_visitors.len(),
            period.viewers.len(),
            period.new_viewers,
            minutes(period.beat_secs),
            minutes(period.live_secs),
            period.peak().0
        );
    }
    out
}

fn heading(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n{title}, last 7 days");
}

/// How far visitors got: loaded the page, saw the match, stayed.
fn funnel(out: &mut String, events: &[&Event]) {
    let mut loaded = HashSet::new();
    let mut seconds: HashMap<&str, u64> = HashMap::new();
    for event in events {
        let Some(visitor) = event.visitor else {
            continue;
        };
        if event.kind == "page" {
            loaded.insert(visitor);
        }
        if let Some((watched, viewer)) = watched(event)
            && viewer
        {
            *seconds.entry(visitor).or_default() += watched;
        }
    }
    heading(out, "funnel, by visitor");
    let _ = writeln!(out, "  loaded a page        {}", loaded.len());
    let _ = writeln!(out, "  watched              {}", seconds.len());
    for minutes in [1, 5, 15, 60] {
        let count = seconds
            .values()
            .filter(|seconds| **seconds >= minutes * 60)
            .count();
        let _ = writeln!(out, "  stayed {minutes:>2}+ min        {count}");
    }
}

fn modes(out: &mut String, events: &[&Event]) {
    let mut by_mode: BTreeMap<&str, u64> = BTreeMap::new();
    for event in events.iter().filter(|event| event.kind == "beat") {
        if let (Some(seconds), Some(mode)) = (number(event.fields.first()), event.fields.get(1)) {
            *by_mode.entry(mode).or_default() += seconds as u64;
        }
    }
    heading(out, "minutes shown by mode");
    for (mode, seconds) in by_mode {
        let _ = writeln!(out, "  {mode:<20} {}", minutes(seconds));
    }
}

fn referrers(out: &mut String, events: &[&Event]) {
    let mut by_host: HashMap<&str, HashSet<&str>> = HashMap::new();
    for event in events.iter().filter(|event| event.kind == "page") {
        let host = match event.fields.get(1) {
            Some(&"-") | None => "(none)",
            Some(host) => host,
        };
        by_host
            .entry(host)
            .or_default()
            .insert(event.visitor.unwrap_or("-"));
    }
    let mut hosts: Vec<(&str, usize)> = by_host
        .into_iter()
        .map(|(host, visitors)| (host, visitors.len()))
        .collect();
    hosts.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    heading(out, "referrers, by visitor");
    for (host, count) in hosts {
        let _ = writeln!(out, "  {host:<32} {count}");
    }
}

/// Each viewer's last renderer, or the failure if none ever started.
fn renderers(out: &mut String, events: &[&Event]) {
    let mut last: HashMap<&str, &str> = HashMap::new();
    for event in events {
        let Some(visitor) = event.visitor else {
            continue;
        };
        match event.kind {
            "beat" => {
                if let Some(renderer) = event.fields.get(2).filter(|renderer| **renderer != "-") {
                    last.insert(visitor, renderer);
                }
            }
            "fail" => {
                last.entry(visitor).or_insert("failed");
            }
            _ => {}
        }
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for renderer in last.values() {
        *counts.entry(renderer).or_default() += 1;
    }
    heading(out, "renderer, by visitor");
    for (renderer, count) in counts {
        let _ = writeln!(out, "  {renderer:<20} {count}");
    }
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for event in events.iter().filter(|event| event.kind == "fail") {
        *reasons
            .entry(event.fields.first().unwrap_or(&""))
            .or_default() += 1;
    }
    for (reason, count) in reasons {
        let _ = writeln!(out, "  failed x{count}: {reason}");
    }
}

/// Over viewers, each one's median of its beats, so a long session does
/// not outweigh many short ones.
fn frame_times(out: &mut String, events: &[&Event]) {
    // Renderer, then visitor, then fps, CPU and GPU samples.
    type Samples = [Vec<f64>; 3];
    let mut by_renderer: BTreeMap<&str, HashMap<&str, Samples>> = BTreeMap::new();
    for event in events.iter().filter(|event| event.kind == "beat") {
        let (Some(visitor), Some(renderer)) = (event.visitor, event.fields.get(2)) else {
            continue;
        };
        let samples = by_renderer
            .entry(renderer)
            .or_default()
            .entry(visitor)
            .or_default();
        for (index, field) in [3, 4, 5].into_iter().enumerate() {
            if let Some(value) = number(event.fields.get(field)) {
                samples[index].push(value);
            }
        }
    }
    heading(out, "frame times, by visitor median (p10 / p50 / p90)");
    for (renderer, visitors) in by_renderer {
        if renderer == "-" {
            continue;
        }
        let mut medians: Samples = Default::default();
        for samples in visitors.values() {
            for (index, values) in samples.iter().enumerate() {
                if let Some(median) = percentile(values.clone(), 0.5) {
                    medians[index].push(median);
                }
            }
        }
        let _ = writeln!(out, "  {renderer}, {} visitors", visitors.len());
        for (label, values) in ["fps", "CPU ms", "GPU ms"].into_iter().zip(medians) {
            let spread = [0.1, 0.5, 0.9]
                .map(|p| percentile(values.clone(), p).map_or("-".into(), |v| format!("{v:.1}")));
            let _ = writeln!(
                out,
                "    {label:<8} {:>7} {:>7} {:>7}   ({} visitors)",
                spread[0],
                spread[1],
                spread[2],
                values.len()
            );
        }
    }
}

fn percentile(mut values: Vec<f64>, p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * p).round() as usize;
    Some(values[index])
}

/// Each viewer's last settings, as counts per value.
fn settings(out: &mut String, events: &[&Event]) {
    let mut last: HashMap<&str, &str> = HashMap::new();
    for event in events.iter().filter(|event| event.kind == "beat") {
        if let (Some(visitor), Some(settings)) = (event.visitor, event.fields.get(6))
            && !settings.is_empty()
        {
            last.insert(visitor, settings);
        }
    }
    let mut counts: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for settings in last.values() {
        for pair in settings.split(' ') {
            if let Some((id, value)) = pair.split_once('=') {
                *counts.entry(id).or_default().entry(value).or_default() += 1;
            }
        }
    }
    heading(out, "settings, by visitor");
    for (id, values) in counts {
        let mut values: Vec<(&str, usize)> = values.into_iter().collect();
        values.sort_unstable_by_key(|(_, count)| Reverse(*count));
        let list: Vec<String> = values
            .iter()
            .map(|(value, count)| format!("{} {count}", if value.is_empty() { "-" } else { value }))
            .collect();
        let _ = writeln!(out, "  {id:<24} {}", list.join(", "));
    }
}

fn minutes(seconds: u64) -> u64 {
    (seconds + 30) / 60
}

fn format_time(unix: u64) -> String {
    let seconds = unix % DAY;
    format!(
        "{} {:02}:{:02}",
        format_date(unix / DAY),
        seconds / 3600,
        seconds % 3600 / 60
    )
}

/// Days since 1970-01-01 as a date, after Howard Hinnant's
/// `civil_from_days`.
fn format_date(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
