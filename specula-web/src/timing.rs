//! Frame times for the stats box, to tell a frame that waits on the
//! processor from one that waits on the graphics card: the CPU time of the
//! frame callback, the GPU time of its passes from timestamp queries, and
//! the time from submit until the GPU reports the work done.

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

/// The timed pass groups, in the order the frame encodes them.
pub const SHADOW: usize = 0;
pub const REFRACTION: usize = 1;
pub const WAVES: usize = 2;
pub const REFLECTION: usize = 3;
pub const MAIN: usize = 4;
pub const POST: usize = 5;
const GROUPS: usize = 6;
const QUERIES: u32 = 2 * GROUPS as u32;
const BYTES: u64 = QUERIES as u64 * wgpu::QUERY_SIZE as u64;
/// Readbacks in flight; a frame that finds them all mapping goes untimed
/// rather than waiting for the GPU.
const RING: usize = 3;

/// Milliseconds over the stats window.
#[derive(Clone, Copy, Default)]
pub struct Span {
    sum: f64,
    max: f64,
    count: u32,
}

impl Span {
    fn add(&mut self, ms: f64) {
        self.sum += ms;
        self.max = self.max.max(ms);
        self.count += 1;
    }

    pub fn average(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / f64::from(self.count))
    }

    pub fn max(&self) -> f64 {
        self.max
    }
}

#[derive(Clone, Copy, Default)]
pub struct Times {
    /// The whole frame callback, on frames that drew.
    pub frame: Span,
    /// Callbacks that drew nothing, the scene being unchanged.
    pub idle: u32,
    /// From the start of the draw to the submit: the scene's update and
    /// the command encoding.
    pub encode: Span,
    /// The submit and the present.
    pub submit: Span,
    /// The first timed pass's start to the last one's end.
    pub gpu: Span,
    /// Shadow, water (refraction, waves and reflection together), main,
    /// post-processing.
    pub groups: [Span; 4],
    /// From the submit until the GPU reports the work done.
    pub done: Span,
    /// Model and ground draw calls over all passes.
    pub draws: Span,
}

/// Every value a benchmark run records, one per frame, for its
/// percentiles; the `Times` fields of the same names.
#[derive(Default)]
pub struct Samples {
    pub frame: Vec<f64>,
    pub encode: Vec<f64>,
    pub submit: Vec<f64>,
    pub gpu: Vec<f64>,
    pub groups: [Vec<f64>; 4],
    pub done: Vec<f64>,
    pub draws: Vec<f64>,
}

thread_local! {
    // on_submitted_work_done wants a Send callback, which an Rc is not;
    // the page has one thread, so the callbacks share this instead.
    static TIMES: RefCell<Times> = RefCell::new(Times::default());
    static SAMPLES: RefCell<Option<Samples>> = const { RefCell::new(None) };
}

fn record(update: impl FnOnce(&mut Times)) {
    TIMES.with(|times| update(&mut times.borrow_mut()));
}

fn sample(update: impl FnOnce(&mut Samples)) {
    SAMPLES.with(|samples| {
        if let Some(samples) = &mut *samples.borrow_mut() {
            update(samples);
        }
    });
}

/// Starts keeping every sample, dropping any kept before.
pub fn start_samples() {
    SAMPLES.with(|samples| *samples.borrow_mut() = Some(Samples::default()));
}

/// The samples since `start_samples`, which stops keeping them.
pub fn take_samples() -> Samples {
    SAMPLES.with(|samples| samples.borrow_mut().take().unwrap_or_default())
}

/// The times since the last call.
pub fn take() -> Times {
    TIMES.with(|times| std::mem::take(&mut *times.borrow_mut()))
}

/// One frame callback's CPU time, and whether it drew.
pub fn frame(ms: f64, drew: bool) {
    record(|times| {
        if drew {
            times.frame.add(ms);
            sample(|samples| samples.frame.push(ms));
        } else {
            times.idle += 1;
        }
    });
}

/// The draw's CPU time up to the submit, the submit's, and its draw calls.
pub fn draw(encode_ms: f64, submit_ms: f64, draws: u32) {
    record(|times| {
        times.encode.add(encode_ms);
        times.submit.add(submit_ms);
        times.draws.add(f64::from(draws));
    });
    sample(|samples| {
        samples.encode.push(encode_ms);
        samples.submit.push(submit_ms);
        samples.draws.push(f64::from(draws));
    });
}

struct Readback {
    buffer: wgpu::Buffer,
    mapping: Rc<Cell<bool>>,
}

struct Queries {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    ring: Vec<Readback>,
    /// The readback this frame copies into, if one was free.
    slot: Option<usize>,
    /// A bit per group whose passes this frame wrote, since a group that
    /// did not run leaves an earlier frame's values in its queries.
    written: Cell<u32>,
    /// Nanoseconds per tick.
    period: f64,
}

pub struct Timer {
    /// `None` without the timestamp-query feature, as on WebGL.
    queries: Option<Queries>,
    /// Whether `on_submitted_work_done` follows the GPU: WebGL's fires
    /// only when the device is next polled.
    work_done: bool,
}

impl Timer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, webgpu: bool) -> Self {
        let queries = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| Queries {
                set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("frame timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: QUERIES,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timestamps"),
                    size: BYTES,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                ring: (0..RING)
                    .map(|_| Readback {
                        buffer: device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("frame timestamps readback"),
                            size: BYTES,
                            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                        mapping: Rc::new(Cell::new(false)),
                    })
                    .collect(),
                slot: None,
                written: Cell::new(0),
                period: f64::from(queue.get_timestamp_period()),
            });
        Timer {
            queries,
            work_done: webgpu,
        }
    }

    /// Whether GPU times are measured at all.
    pub fn measures_gpu(&self) -> bool {
        self.queries.is_some()
    }

    /// Before the frame's first pass.
    pub fn begin(&mut self) {
        if let Some(queries) = &mut self.queries {
            queries.slot = queries
                .ring
                .iter()
                .position(|readback| !readback.mapping.get());
            queries.written.set(0);
        }
    }

    /// The writes for a pass of `group`: its start on the group's `first`
    /// pass, its end on the `last`.
    pub fn writes(
        &self,
        group: usize,
        first: bool,
        last: bool,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let queries = self.queries.as_ref()?;
        queries.slot?;
        if !first && !last {
            return None;
        }
        if first {
            queries.written.set(queries.written.get() | 1 << group);
        }
        let index = 2 * group as u32;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &queries.set,
            beginning_of_pass_write_index: first.then_some(index),
            end_of_pass_write_index: last.then_some(index + 1),
        })
    }

    /// After the frame's last pass.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(queries) = &self.queries else {
            return;
        };
        let Some(slot) = queries.slot else {
            return;
        };
        if queries.written.get() == 0 {
            return;
        }
        encoder.resolve_query_set(&queries.set, 0..QUERIES, &queries.resolve, 0);
        encoder.copy_buffer_to_buffer(&queries.resolve, 0, &queries.ring[slot].buffer, 0, BYTES);
    }

    /// Right after the frame's submit.
    pub fn submitted(&self, queue: &wgpu::Queue) {
        if self.work_done {
            let at = now();
            queue.on_submitted_work_done(move || {
                let ms = now() - at;
                record(|times| times.done.add(ms));
                sample(|samples| samples.done.push(ms));
            });
        }
        let Some(queries) = &self.queries else {
            return;
        };
        let Some(slot) = queries.slot else {
            return;
        };
        let written = queries.written.get();
        if written == 0 {
            return;
        }
        let readback = &queries.ring[slot];
        readback.mapping.set(true);
        let buffer = readback.buffer.clone();
        let mapping = Rc::clone(&readback.mapping);
        let period = queries.period;
        readback
            .buffer
            .map_async(wgpu::MapMode::Read, .., move |result| {
                if result.is_ok() {
                    let stamps: Vec<u64> = match buffer.get_mapped_range(..) {
                        Ok(view) => view
                            .chunks_exact(8)
                            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
                            .collect(),
                        Err(_) => Vec::new(),
                    };
                    buffer.unmap();
                    if stamps.len() == QUERIES as usize {
                        add_stamps(&stamps, written, period);
                    }
                }
                mapping.set(false);
            });
    }
}

fn add_stamps(stamps: &[u64], written: u32, period: f64) {
    let ms = |ticks: u64| ticks as f64 * period / 1e6;
    let mut groups = [None; 4];
    let mut start = u64::MAX;
    let mut end = 0;
    for group in 0..GROUPS {
        if written & 1 << group == 0 {
            continue;
        }
        let (begin, finish) = (stamps[2 * group], stamps[2 * group + 1]);
        start = start.min(begin);
        end = end.max(finish);
        // The spec lets an end come before its start on some GPUs.
        let span = ms(finish.saturating_sub(begin));
        let shown = match group {
            SHADOW => 0,
            REFRACTION | WAVES | REFLECTION => 1,
            MAIN => 2,
            _ => 3,
        };
        *groups[shown].get_or_insert(0.0) += span;
    }
    let total = ms(end.saturating_sub(start));
    record(|times| {
        times.gpu.add(total);
        for (span, ms) in times.groups.iter_mut().zip(groups) {
            if let Some(ms) = ms {
                span.add(ms);
            }
        }
    });
    sample(|samples| {
        samples.gpu.push(total);
        for (series, ms) in samples.groups.iter_mut().zip(groups) {
            if let Some(ms) = ms {
                series.push(ms);
            }
        }
    });
}

fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}
