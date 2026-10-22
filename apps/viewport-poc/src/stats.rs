use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{ImageId, RenderImage};
use mw_crystal_render::FrameTimings;

/// Frames averaged for the timing readouts.
const WINDOW: usize = 60;

/// Rolling timings of recently finished frames.
#[derive(Default)]
pub struct FrameStats {
    samples: VecDeque<Sample>,
}

struct Sample {
    timings: FrameTimings,
    latency: Option<Duration>,
    bytes: usize,
}

impl FrameStats {
    pub fn record(&mut self, timings: FrameTimings, latency: Option<Duration>, bytes: usize) {
        if self.samples.len() == WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            timings,
            latency,
            bytes,
        });
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn mean_encode_ms(&self) -> Option<f64> {
        self.mean_ms(|s| Some(s.timings.encode))
    }

    pub fn mean_gpu_wait_ms(&self) -> Option<f64> {
        self.mean_ms(|s| Some(s.timings.gpu_wait))
    }

    pub fn mean_copy_ms(&self) -> Option<f64> {
        self.mean_ms(|s| Some(s.timings.copy))
    }

    pub fn mean_latency_ms(&self) -> Option<f64> {
        self.mean_ms(|s| s.latency)
    }

    pub fn last_bytes(&self) -> Option<usize> {
        self.samples.back().map(|s| s.bytes)
    }

    fn mean_ms(&self, pick: impl Fn(&Sample) -> Option<Duration>) -> Option<f64> {
        let values: Vec<f64> = self
            .samples
            .iter()
            .filter_map(&pick)
            .map(|d| d.as_secs_f64() * 1e3)
            .collect();
        (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
    }
}

/// Shared between the view and its paint callback, which runs after render
/// and must not mutate the view itself.
#[derive(Default)]
pub struct PaintProbe {
    /// Images replaced since the last paint; paint removes them from GPUI's
    /// sprite atlas so it does not grow by one image per frame.
    pub retired: RefCell<Vec<Arc<RenderImage>>>,
    /// Whether an auto-rotation step is already registered for the next
    /// display frame; several paints can happen before it fires.
    pub frame_scheduled: Cell<bool>,
    last_image: Cell<Option<ImageId>>,
    presented: RefCell<VecDeque<Instant>>,
    presented_total: Cell<u64>,
    paint_samples: RefCell<VecDeque<Duration>>,
}

impl PaintProbe {
    /// Records one paint of `image` that took `elapsed`, counting it as
    /// presented only when it is a new image.
    pub fn note_paint(&self, image: ImageId, elapsed: Duration) {
        if self.last_image.replace(Some(image)) == Some(image) {
            return;
        }
        self.presented_total.set(self.presented_total.get() + 1);
        let now = Instant::now();
        let mut presented = self.presented.borrow_mut();
        presented.push_back(now);
        while presented
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
        {
            presented.pop_front();
        }
        let mut samples = self.paint_samples.borrow_mut();
        if samples.len() == WINDOW {
            samples.pop_front();
        }
        samples.push_back(elapsed);
    }

    /// New images painted during the last second.
    pub fn presented_per_second(&self) -> usize {
        let presented = self.presented.borrow();
        let now = Instant::now();
        presented
            .iter()
            .filter(|t| now.duration_since(**t) <= Duration::from_secs(1))
            .count()
    }

    /// Mean time spent in `paint_image` for new images, which includes the
    /// upload into GPUI's sprite atlas.
    pub fn mean_paint_ms(&self) -> Option<f64> {
        let samples = self.paint_samples.borrow();
        (!samples.is_empty()).then(|| {
            samples.iter().map(|d| d.as_secs_f64() * 1e3).sum::<f64>() / samples.len() as f64
        })
    }

    /// New images painted since the last [`PaintProbe::clear`].
    pub fn presented_total(&self) -> u64 {
        self.presented_total.get()
    }

    pub fn clear(&self) {
        self.presented.borrow_mut().clear();
        self.paint_samples.borrow_mut().clear();
        self.presented_total.set(0);
    }
}
