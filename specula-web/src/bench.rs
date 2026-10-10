//! The Benchmark button: with the match paused and the camera held, it
//! draws every frame under one quality setting after another and offers
//! the frame times as a commented JSON report, readable as it is and by a
//! JSONC parser.
//!
//! The runs are the Min, Balanced and Max presets and the current
//! settings, one setting turned on at a time from Min, one turned off at
//! a time from Max, and from Max the levels of the shadows, their cutoff
//! and the textures: every combination would be millions of runs, and the costs
//! mostly add up, which the two one-at-a-time sets also check.
//!
//! The settings are changed as the presets change them, through the
//! controls' own change handlers, so each is applied and saved as a
//! viewer's choice would be, and the original ones are put back the same
//! way at the end.

use std::cell::RefCell;
use std::fmt::Write;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::Document;
use web_sys::HtmlButtonElement;
use web_sys::HtmlElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlOptionElement;
use web_sys::HtmlSelectElement;

use crate::App;
use crate::assets::Progress;
use crate::element;
use crate::timing;
use crate::timing::Samples;
use crate::worker::COMMAND_LIVE;
use crate::worker::COMMAND_PAUSE;
use crate::worker::COMMAND_PLAY;

/// Where the camera looks from, which the benchmark holds still.
#[derive(Clone, Copy)]
pub struct CameraPose {
    pub target: [f32; 2],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub birds_eye: bool,
}

/// What the report says about the device and the view.
#[derive(Default)]
pub struct BenchInfo {
    pub adapter: String,
    pub backend: &'static str,
    pub gpu_timed: bool,
    pub canvas: [u32; 2],
    pub device_pixel_ratio: f32,
    pub entities: u32,
    pub in_view: u32,
    pub models: u32,
}

/// The quality pane's controls in document order, which puts a parent
/// before its dependents, so a parent's handler enables them before they
/// are set; and their names in the pane.
const CONTROLS: [(&str, &str); 23] = [
    ("textures", "Textures"),
    ("maps", "Material maps"),
    ("triplanar", "Triplanar cliffs"),
    ("wind", "Wind"),
    ("shadows", "Shadows"),
    ("shadowscutoff", "Cutoff distance"),
    ("water", "Water effects"),
    ("waterfancyeffects", "High-quality water effects"),
    ("waterreflection", "Water reflections"),
    ("waterrefraction", "Water refraction"),
    ("waterrealdepth", "Real water depth"),
    ("sky", "Sky"),
    ("fog", "Fog"),
    ("particles", "Particles"),
    ("projectiles", "Projectiles"),
    ("postproc", "Post-processing"),
    ("bloom", "Bloom"),
    ("dof", "Depth of field"),
    ("antialiasing", "Antialiasing"),
    ("sharpening", "Sharpening"),
    ("sharpness", "Sharpness factor"),
    ("renderscale", "Resolution scale"),
    ("upscale", "Upscale technique"),
];

/// The lists a list needs on to apply, as lib.rs disables it otherwise;
/// turning it on from Min turns these on too.
const PARENTS: [(&str, &[&str]); 9] = [
    ("waterfancyeffects", &["water"]),
    ("waterreflection", &["water"]),
    ("waterrefraction", &["water"]),
    ("waterrealdepth", &["water", "waterrefraction"]),
    ("bloom", &["postproc"]),
    ("dof", &["postproc"]),
    ("antialiasing", &["postproc"]),
    ("sharpening", &["postproc"]),
    ("renderscale", &["postproc"]),
];
/// Lists left out of the one-at-a-time runs: the upscale technique does
/// nothing at Max's scale of 1, so it is measured with the scales instead.
const UNTOGGLED: [&str; 1] = ["upscale"];
/// Resolution scales measured from Max with FSR, and the one also
/// measured with bilinear.
const SCALES: [&str; 3] = ["0.5", "0.75", "0.875"];
const BILINEAR_SCALE: &str = "0.75";

/// Shadow cutoffs measured from Max, in metres.
const CUTOFFS: [&str; 4] = ["100", "300", "700", "1500"];
/// Frames drawn after a change before measuring, past shader builds,
/// target allocations and the GPU times that arrive frames late.
const WARMUP_FRAMES: u32 = 30;
const MEASURED_FRAMES: u32 = 90;
/// The longest a run waits for its assets before it measures anyway.
const SETTLE_LIMIT_MS: f64 = 20_000.0;

struct Control {
    id: &'static str,
    name: &'static str,
    select: Option<HtmlSelectElement>,
    input: Option<HtmlInputElement>,
    /// The Min and Max presets' values, as index.html's applyPreset picks
    /// them.
    low: String,
    high: String,
}

impl Control {
    fn value(&self) -> String {
        match (&self.select, &self.input) {
            (Some(select), _) => select.value(),
            (_, Some(input)) => input.value(),
            _ => String::new(),
        }
    }

    fn disabled(&self) -> bool {
        match (&self.select, &self.input) {
            (Some(select), _) => select.disabled(),
            (_, Some(input)) => input.disabled(),
            _ => false,
        }
    }

    /// The Balanced preset's value, from the same data-balanced attribute
    /// index.html's applyPreset reads; a list's largest option where the
    /// device lacks the one named, as there.
    fn balanced(&self) -> Option<String> {
        let value = match (&self.select, &self.input) {
            (Some(select), _) => select.get_attribute("data-balanced"),
            (_, Some(input)) => input.get_attribute("data-balanced"),
            _ => None,
        }?;
        if self.is_slider() || self.options().iter().any(|(option, _)| *option == value) {
            Some(value)
        } else {
            Some(self.high.clone())
        }
    }

    fn is_slider(&self) -> bool {
        self.input.is_some()
    }

    /// The options a viewer can pick, as (value, text).
    fn options(&self) -> Vec<(String, String)> {
        let Some(select) = &self.select else {
            return Vec::new();
        };
        (0..select.length())
            .filter_map(|index| select.item(index))
            .filter_map(|item| item.dyn_into::<HtmlOptionElement>().ok())
            .filter(|option| !option.disabled())
            .map(|option| (option.value(), option.text()))
            .collect()
    }

    /// `value` as the pane shows it.
    fn show(&self, value: &str) -> String {
        if self.is_slider() {
            return value.to_string();
        }
        self.options()
            .into_iter()
            .find(|(option, _)| option == value)
            .map_or_else(|| value.to_string(), |(_, text)| text)
    }

    /// Sets it and runs its handlers, as a viewer's choice would.
    fn set(&self, value: &str) {
        let target: &web_sys::EventTarget = match (&self.select, &self.input) {
            (Some(select), _) => {
                select.set_value(value);
                select
            }
            (_, Some(input)) => {
                input.set_value(value);
                if let Ok(event) = web_sys::Event::new("input") {
                    let _ = input.dispatch_event(&event);
                }
                input
            }
            _ => return,
        };
        if let Ok(event) = web_sys::Event::new("change") {
            let _ = target.dispatch_event(&event);
        }
    }
}

struct Run {
    /// The comment over the runs of its set, on the set's first run.
    set: Option<&'static str>,
    name: String,
    base: &'static str,
    /// The controls it changes from `base`.
    changed: Vec<usize>,
    values: Vec<String>,
}

enum Phase {
    Apply,
    Settle {
        frames: u32,
        since: f64,
    },
    Measure {
        frames: u32,
        since: f64,
        settled: bool,
    },
}

struct Running {
    runs: Vec<Run>,
    index: usize,
    phase: Phase,
    original: Vec<String>,
    was_paused: bool,
    was_live: bool,
    camera: Option<CameraPose>,
    started: String,
    start_info: BenchInfo,
    results: Vec<Measured>,
}

struct Measured {
    run: usize,
    settled: bool,
    fps: f64,
    frames: u32,
    samples: Samples,
    info: BenchInfo,
    inactive: Vec<&'static str>,
}

pub struct Bench {
    button: HtmlButtonElement,
    status: HtmlElement,
    download: HtmlElement,
    controls: Vec<Control>,
    running: Option<Running>,
}

impl Bench {
    pub fn new(document: &Document) -> Result<Self, String> {
        let mut controls = Vec::new();
        for (id, name) in CONTROLS {
            let select = document
                .get_element_by_id(id)
                .and_then(|element| element.dyn_into::<HtmlSelectElement>().ok());
            let input = document
                .get_element_by_id(id)
                .and_then(|element| element.dyn_into::<HtmlInputElement>().ok());
            if select.is_none() && input.is_none() {
                return Err(format!("no control #{id}"));
            }
            controls.push(Control {
                id,
                name,
                select,
                input,
                low: String::new(),
                high: String::new(),
            });
        }
        Ok(Bench {
            button: element(document, "bench")?,
            status: element(document, "bench-status")?,
            download: element(document, "bench-download")?,
            controls,
            running: None,
        })
    }

    fn start(&mut self, app: &Rc<RefCell<App>>) {
        // Read now: the device decides which shadow sizes are offered.
        for control in &mut self.controls {
            if let Some(input) = &control.input {
                control.low = input.min();
                control.high = input.max();
            } else {
                // A list may name its cheapest and dearest, as index.html's
                // presets take them, where they are not its ends.
                let options = control.options();
                let named = |attribute| {
                    let value = control.select.as_ref()?.get_attribute(attribute)?;
                    options
                        .iter()
                        .any(|(option, _)| *option == value)
                        .then_some(value)
                };
                let low = named("data-min").or_else(|| options.first().map(|(v, _)| v.clone()));
                let high = named("data-max").or_else(|| options.last().map(|(v, _)| v.clone()));
                control.low = low.unwrap_or_default();
                control.high = high.unwrap_or_default();
            }
        }
        let original: Vec<String> = self.controls.iter().map(Control::value).collect();
        let runs = self.plan(&original);
        let (was_paused, was_live, camera, start_info) = {
            let app = app.borrow();
            (
                app.paused,
                app.live,
                app.renderer.camera(),
                app.renderer.bench_info(),
            )
        };
        if !was_paused {
            app.borrow().command(COMMAND_PAUSE, 0.0);
        }
        self.download.set_hidden(true);
        self.button.set_text_content(Some("Stop benchmark"));
        self.running = Some(Running {
            runs,
            index: 0,
            phase: Phase::Apply,
            original,
            was_paused,
            was_live,
            camera,
            started: String::from(js_sys::Date::new_0().to_iso_string()),
            start_info,
            results: Vec::new(),
        });
    }

    /// The runs, without repeating a set of values already measured.
    fn plan(&self, original: &[String]) -> Vec<Run> {
        let index = |id: &str| self.controls.iter().position(|control| control.id == id);
        let preset = |high: bool| -> Vec<String> {
            self.controls
                .iter()
                .zip(original)
                .map(|(control, original)| match control.id {
                    // As applyPreset: projectiles show what happens, not how
                    // well, and fog hides the map's far side.
                    "projectiles" => original.clone(),
                    "fog" => "0".into(),
                    _ if high => control.high.clone(),
                    _ => control.low.clone(),
                })
                .collect()
        };
        let min = preset(false);
        let max = preset(true);
        let balanced: Vec<String> = self
            .controls
            .iter()
            .zip(original)
            .map(|(control, original)| control.balanced().unwrap_or_else(|| original.clone()))
            .collect();
        let mut runs: Vec<Run> = Vec::new();
        let push = |runs: &mut Vec<Run>,
                    set: &mut Option<&'static str>,
                    name: String,
                    base: &'static str,
                    changed: Vec<usize>,
                    values: Vec<String>| {
            if runs.iter().any(|run| run.values == values) {
                return;
            }
            runs.push(Run {
                set: set.take(),
                name,
                base,
                changed,
                values,
            });
        };
        let describe = |values: &[String], changed: &[usize]| -> String {
            changed
                .iter()
                .map(|&i| {
                    let control = &self.controls[i];
                    format!("{}: {}", control.name, control.show(&values[i]))
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        let mut set = Some(
            "Baselines: the Min, Balanced and Max presets, and the settings before the benchmark",
        );
        push(
            &mut runs,
            &mut set,
            "Min".into(),
            "min",
            Vec::new(),
            min.clone(),
        );
        push(
            &mut runs,
            &mut set,
            "Balanced".into(),
            "balanced",
            Vec::new(),
            balanced,
        );
        push(
            &mut runs,
            &mut set,
            "Max".into(),
            "max",
            Vec::new(),
            max.clone(),
        );
        push(
            &mut runs,
            &mut set,
            "Current".into(),
            "current",
            Vec::new(),
            original.to_vec(),
        );

        let toggled = |base: &[String], i: usize| {
            let control = &self.controls[i];
            if base[i] == control.high {
                control.low.clone()
            } else {
                control.high.clone()
            }
        };
        let mut set = Some("From Min, one setting changed at a time, with the lists it needs");
        for (i, control) in self.controls.iter().enumerate() {
            if control.is_slider() || UNTOGGLED.contains(&control.id) {
                continue;
            }
            let mut values = min.clone();
            let mut changed = Vec::new();
            if let Some((_, parents)) = PARENTS.iter().find(|(id, _)| *id == control.id) {
                for parent in *parents {
                    if let Some(p) = index(parent) {
                        values[p] = self.controls[p].high.clone();
                        changed.push(p);
                    }
                }
            }
            values[i] = toggled(&min, i);
            changed.push(i);
            changed.sort_unstable();
            let name = format!("Min, {}", describe(&values, &changed));
            push(&mut runs, &mut set, name, "min", changed, values);
        }

        let mut set = Some("From Max, one setting changed at a time");
        for (i, control) in self.controls.iter().enumerate() {
            if control.is_slider() || UNTOGGLED.contains(&control.id) {
                continue;
            }
            let mut values = max.clone();
            values[i] = toggled(&max, i);
            let name = format!("Max, {}", describe(&values, &[i]));
            push(&mut runs, &mut set, name, "max", vec![i], values);
        }

        let mut set = Some("From Max, the levels of shadows, their cutoff and textures");
        for id in ["shadows", "textures"] {
            let Some(i) = index(id) else {
                continue;
            };
            for (value, _) in self.controls[i].options() {
                let mut values = max.clone();
                values[i] = value;
                let name = format!("Max, {}", describe(&values, &[i]));
                push(&mut runs, &mut set, name, "max", vec![i], values);
            }
        }
        if let Some(i) = index("shadowscutoff") {
            for cutoff in CUTOFFS {
                let mut values = max.clone();
                values[i] = cutoff.into();
                let name = format!("Max, {} m", describe(&values, &[i]));
                push(&mut runs, &mut set, name, "max", vec![i], values);
            }
        }

        let mut set = Some("From Max, resolution scales and upscale techniques");
        if let (Some(scale), Some(upscale)) = (index("renderscale"), index("upscale")) {
            let techniques = SCALES
                .iter()
                .map(|value| (*value, "fsr"))
                .chain([(BILINEAR_SCALE, "bilinear")]);
            for (value, technique) in techniques {
                let mut values = max.clone();
                values[scale] = value.into();
                values[upscale] = technique.into();
                let changed = vec![scale, upscale];
                let name = format!("Max, {}", describe(&values, &changed));
                push(&mut runs, &mut set, name, "max", changed, values);
            }
        }
        runs
    }

    fn apply(&self, values: &[String]) {
        for (control, value) in self.controls.iter().zip(values) {
            if control.value() != *value {
                control.set(value);
            }
        }
    }

    /// After each frame. Changes settings only here, with `app` not
    /// borrowed, as their handlers borrow it.
    pub fn step(&mut self, app: &Rc<RefCell<App>>, now: f64) {
        let Some(running) = &mut self.running else {
            return;
        };
        let mut finished = false;
        match &mut running.phase {
            Phase::Apply => {
                let values = running.runs[running.index].values.clone();
                running.phase = Phase::Settle {
                    frames: 0,
                    since: now,
                };
                self.apply(&values);
            }
            Phase::Settle { frames, since } => {
                *frames += 1;
                let loaded = matches!(app.borrow().renderer.progress(), Progress::Idle);
                let waited = now - *since > SETTLE_LIMIT_MS;
                if *frames >= WARMUP_FRAMES && (loaded || waited) {
                    timing::start_samples();
                    running.phase = Phase::Measure {
                        frames: 0,
                        since: now,
                        settled: loaded,
                    };
                }
            }
            Phase::Measure {
                frames,
                since,
                settled,
            } => {
                *frames += 1;
                if *frames >= MEASURED_FRAMES {
                    let samples = timing::take_samples();
                    let inactive = self
                        .controls
                        .iter()
                        .filter(|control| control.disabled())
                        .map(|control| control.id)
                        .collect();
                    running.results.push(Measured {
                        run: running.index,
                        settled: *settled,
                        fps: f64::from(*frames) * 1000.0 / (now - *since).max(1.0),
                        frames: *frames,
                        samples,
                        info: app.borrow().renderer.bench_info(),
                        inactive,
                    });
                    running.index += 1;
                    running.phase = Phase::Apply;
                    finished = running.index == running.runs.len();
                }
            }
        }
        if finished {
            self.finish(app);
            return;
        }
        let Some(running) = &self.running else {
            return;
        };
        let run = &running.runs[running.index];
        let phase = match running.phase {
            Phase::Apply | Phase::Settle { .. } => "warming up",
            Phase::Measure { .. } => "measuring",
        };
        self.status.set_text_content(Some(&format!(
            "Run {}/{}, {phase}: {}",
            running.index + 1,
            running.runs.len(),
            run.name
        )));
        let app = &mut *app.borrow_mut();
        // Drawn every frame, as a paused scene otherwise is not.
        app.dirty = true;
        if let Some(pose) = running.camera {
            app.renderer.set_camera(pose);
        }
    }

    /// Puts the settings, the camera and the playback back, and offers the
    /// report of the runs measured so far.
    fn finish(&mut self, app: &Rc<RefCell<App>>) {
        let Some(running) = self.running.take() else {
            return;
        };
        timing::take_samples();
        self.apply(&running.original);
        {
            let app = &mut *app.borrow_mut();
            if let Some(pose) = running.camera {
                app.renderer.set_camera(pose);
            }
            // A live viewer goes back to live, not to the paused turn.
            if running.was_live && !running.was_paused {
                app.command(COMMAND_LIVE, 0.0);
            } else if !running.was_paused {
                app.command(COMMAND_PLAY, 0.0);
            }
            app.dirty = true;
        }
        self.button.set_text_content(Some("Benchmark"));
        let complete = running.results.len() == running.runs.len();
        self.status.set_text_content(Some(&format!(
            "{} of {} runs measured{}",
            running.results.len(),
            running.runs.len(),
            if complete { "" } else { ", stopped" }
        )));
        if running.results.is_empty() {
            return;
        }
        let report = self.report(&running, complete);
        let href = format!(
            "data:application/jsonc;charset=utf-8,{}",
            js_sys::encode_uri_component(&report)
        );
        let stamp: String = running
            .started
            .chars()
            .take(19)
            .map(|c| if c == ':' { '-' } else { c })
            .collect();
        let _ = self.download.set_attribute("href", &href);
        let _ = self
            .download
            .set_attribute("download", &format!("specula-benchmark-{stamp}.jsonc"));
        self.download.set_hidden(false);
    }

    fn report(&self, running: &Running, complete: bool) -> String {
        let info = &running.start_info;
        let mut out = String::new();
        let line = |out: &mut String, text: &str| {
            out.push_str(text);
            out.push('\n');
        };
        line(&mut out, "// Specula graphics benchmark.");
        line(&mut out, "//");
        line(
            &mut out,
            "// Each run sets the quality settings, waits for their assets and",
        );
        line(
            &mut out,
            &format!(
                "// {WARMUP_FRAMES} frames, then measures {MEASURED_FRAMES} frames, with the match paused,"
            ),
        );
        line(&mut out, "// the camera held still and every frame drawn.");
        line(&mut out, "//");
        line(
            &mut out,
            "// Times are milliseconds per frame: p50 the median, p95 the 95th",
        );
        line(
            &mut out,
            "// percentile, max the worst frame, n the samples.",
        );
        line(&mut out, "//   cpu.frame   the whole frame callback");
        line(
            &mut out,
            "//   cpu.encode  from the start of the draw to the submit: placing",
        );
        line(
            &mut out,
            "//               the entities, building and uploading instances,",
        );
        line(&mut out, "//               recording the passes");
        line(&mut out, "//   cpu.submit  the submit and the present");
        line(
            &mut out,
            "//   gpu.total   the first timed pass's start to the last one's end,",
        );
        line(
            &mut out,
            "//               from timestamp queries; null without them (WebGL)",
        );
        line(
            &mut out,
            "//   gpu.shadow, gpu.water (refraction, waves, reflection),",
        );
        line(
            &mut out,
            "//   gpu.main, gpu.post  the passes; null where the pass did not run",
        );
        line(
            &mut out,
            "//   done        from the submit until the GPU reports the work done",
        );
        line(&mut out, "//               (WebGPU only)");
        line(
            &mut out,
            "//   draws       model and ground draw calls over all passes (sky,",
        );
        line(
            &mut out,
            "//               decals, impostors, particles and post-processing",
        );
        line(&mut out, "//               are not counted)");
        line(
            &mut out,
            "//   settled     false where the assets were still loading after",
        );
        line(
            &mut out,
            &format!(
                "//               {} s and the run measured anyway",
                SETTLE_LIMIT_MS / 1000.0
            ),
        );
        line(
            &mut out,
            "//   inactive    lists disabled by an off parent, whose value has",
        );
        line(&mut out, "//               no effect");
        line(
            &mut out,
            "// The FPS is capped by the display's refresh rate; compare the times.",
        );
        line(
            &mut out,
            "// Impostor bakes and far uploads submit work of their own, which the",
        );
        line(
            &mut out,
            "// GPU times leave out but the CPU frame time includes.",
        );
        if !complete {
            line(&mut out, "//");
            line(
                &mut out,
                "// Stopped before the end: only the runs measured are here.",
            );
        }
        line(&mut out, "//");
        line(&mut out, "// Summary (p50 ms):");
        let width = running
            .results
            .iter()
            .map(|result| running.runs[result.run].name.len())
            .max()
            .unwrap_or(0)
            .max(3);
        line(
            &mut out,
            &format!(
                "//   {:<width$}  {:>6}  {:>6}  {:>6}  {:>6}  {:>6}",
                "run", "fps", "cpu", "encode", "gpu", "draws"
            ),
        );
        for result in &running.results {
            let median = |series: &[f64]| {
                percentile(series, 0.5)
                    .map_or_else(|| "-".to_string(), |value| format!("{value:.1}"))
            };
            let draws = percentile(&result.samples.draws, 0.5)
                .map_or_else(|| "-".to_string(), |value| format!("{value:.0}"));
            line(
                &mut out,
                &format!(
                    "//   {:<width$}  {:>6.1}  {:>6}  {:>6}  {:>6}  {:>6}",
                    running.runs[result.run].name,
                    result.fps,
                    median(&result.samples.frame),
                    median(&result.samples.encode),
                    median(&result.samples.gpu),
                    draws,
                ),
            );
        }
        line(&mut out, "{");
        let _ = writeln!(out, "  \"started\": {},", quote(&running.started));
        let _ = writeln!(out, "  \"complete\": {complete},");
        let _ = writeln!(out, "  \"backend\": {},", quote(info.backend));
        let _ = writeln!(out, "  \"adapter\": {},", quote(&info.adapter));
        let _ = writeln!(out, "  \"gpuTimestamps\": {},", info.gpu_timed);
        let user_agent = web_sys::window()
            .and_then(|window| window.navigator().user_agent().ok())
            .unwrap_or_default();
        let _ = writeln!(out, "  \"userAgent\": {},", quote(&user_agent));
        let _ = writeln!(out, "  // The canvas in device pixels.");
        let _ = writeln!(
            out,
            "  \"canvas\": [{}, {}],",
            info.canvas[0], info.canvas[1]
        );
        let _ = writeln!(out, "  \"devicePixelRatio\": {},", info.device_pixel_ratio);
        let _ = writeln!(out, "  // Entities in the match, of any kind.");
        let _ = writeln!(out, "  \"entities\": {},", info.entities);
        if let Some(pose) = running.camera {
            let _ = writeln!(
                out,
                "  // The ground point looked at (x, z in metres), angles in radians."
            );
            let _ = writeln!(
                out,
                "  \"camera\": {{ \"target\": [{:.1}, {:.1}], \"yaw\": {:.3}, \"pitch\": {:.3}, \"distance\": {:.1}, \"birdsEye\": {} }},",
                pose.target[0], pose.target[1], pose.yaw, pose.pitch, pose.distance, pose.birds_eye
            );
        }
        let _ = writeln!(out, "  \"warmupFrames\": {WARMUP_FRAMES},");
        let _ = writeln!(out, "  \"measuredFrames\": {MEASURED_FRAMES},");
        let _ = writeln!(out, "  \"runs\": [");
        let last = running.results.len() - 1;
        for (n, result) in running.results.iter().enumerate() {
            let run = &running.runs[result.run];
            if let Some(set) = run.set {
                let _ = writeln!(out, "    // {set}.");
            }
            let _ = writeln!(
                out,
                "    // {}/{}: {}",
                result.run + 1,
                running.runs.len(),
                run.name
            );
            let _ = writeln!(out, "    {{");
            let _ = writeln!(out, "      \"name\": {},", quote(&run.name));
            let _ = writeln!(out, "      \"base\": {},", quote(run.base));
            let changed = run
                .changed
                .iter()
                .map(|&i| format!("{}: {}", quote(self.controls[i].id), quote(&run.values[i])))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "      \"changed\": {{ {changed} }},");
            let _ = writeln!(out, "      \"settled\": {},", result.settled);
            let _ = writeln!(out, "      \"fps\": {:.1},", result.fps);
            let _ = writeln!(out, "      \"frames\": {},", result.frames);
            let samples = &result.samples;
            let _ = writeln!(
                out,
                "      \"cpu\": {{ \"frame\": {}, \"encode\": {}, \"submit\": {} }},",
                series(&samples.frame),
                series(&samples.encode),
                series(&samples.submit)
            );
            let [shadow, water, main, post] = &samples.groups;
            let _ = writeln!(
                out,
                "      \"gpu\": {{ \"total\": {}, \"shadow\": {}, \"water\": {}, \"main\": {}, \"post\": {} }},",
                series(&samples.gpu),
                series(shadow),
                series(water),
                series(main),
                series(post)
            );
            let _ = writeln!(out, "      \"done\": {},", series(&samples.done));
            let _ = writeln!(out, "      \"draws\": {},", series(&samples.draws));
            let _ = writeln!(out, "      \"inView\": {},", result.info.in_view);
            let _ = writeln!(out, "      \"models\": {},", result.info.models);
            let settings = self
                .controls
                .iter()
                .zip(&run.values)
                .map(|(control, value)| format!("{}: {}", quote(control.id), quote(value)))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "      \"settings\": {{ {settings} }},");
            let inactive = result
                .inactive
                .iter()
                .map(|id| quote(id))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "      \"inactive\": [{inactive}]");
            let _ = writeln!(out, "    }}{}", if n == last { "" } else { "," });
        }
        let _ = writeln!(out, "  ]");
        let _ = writeln!(out, "}}");
        out
    }
}

/// The nearest-rank percentile.
fn percentile(series: &[f64], fraction: f64) -> Option<f64> {
    if series.is_empty() {
        return None;
    }
    let mut sorted = series.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((fraction * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

fn series(series: &[f64]) -> String {
    match (
        percentile(series, 0.5),
        percentile(series, 0.95),
        percentile(series, 1.0),
    ) {
        (Some(p50), Some(p95), Some(max)) => format!(
            "{{ \"p50\": {p50:.2}, \"p95\": {p95:.2}, \"max\": {max:.2}, \"n\": {} }}",
            series.len()
        ),
        _ => "null".into(),
    }
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Starts or stops the benchmark from its button.
pub fn wire(bench: &Rc<RefCell<Bench>>, app: &Rc<RefCell<App>>) {
    let target = Rc::clone(bench);
    let app = Rc::clone(app);
    let on_click = Closure::<dyn FnMut()>::new(move || {
        let bench = &mut *target.borrow_mut();
        if bench.running.is_some() {
            bench.finish(&app);
        } else {
            bench.start(&app);
        }
    });
    bench
        .borrow()
        .button
        .set_onclick(Some(on_click.as_ref().unchecked_ref()));
    // The button lives as long as the page, so its handler does too.
    on_click.forget();
}
