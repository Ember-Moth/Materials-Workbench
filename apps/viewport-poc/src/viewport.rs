use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme, Selectable, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::*;
use mw_crystal_render::{
    FrameImage, OrbitCamera, RenderError, RenderRequest, Renderer, srgb_to_linear,
    synthetic_lattice,
};

use crate::stats::{FrameStats, PaintProbe};

const ATOM_COUNTS: [(usize, &str); 3] = [(1_000, "1 千"), (10_000, "1 万"), (100_000, "10 万")];
const RENDER_SCALES: [(u32, &str); 2] = [(100, "100%"), (50, "50%")];
/// Frames that may be rendering or reading back at the same time.
const MAX_IN_FLIGHT: usize = 2;
const DRAG_RADIANS_PER_PIXEL: f32 = 0.008;
const AUTO_ROTATE_RADIANS_PER_SECOND: f32 = 0.8;
const BENCH_WARMUP: Duration = Duration::from_secs(1);
const BENCH_MEASURE: Duration = Duration::from_secs(5);

/// Scripted measurement run, enabled with `VIEWPORT_POC_BENCH`: rotates
/// through each load, prints one line per load and quits.
///
/// `VIEWPORT_POC_BENCH=1` runs every load; a list such as
/// `100000:50,1000:100` (atoms:render scale percent) runs those in order.
struct Bench {
    steps: Vec<(usize, u32)>,
    ix: usize,
    step_started: Option<Instant>,
    measure_started: Option<Instant>,
    /// Whether the window lost focus at any point while measuring.
    was_inactive: bool,
}

impl Bench {
    fn from_env() -> Option<Self> {
        let value = std::env::var("VIEWPORT_POC_BENCH").ok()?;
        let custom: Vec<(usize, u32)> = value
            .split(',')
            .filter_map(|step| {
                let (atoms, scale) = step.trim().split_once(':')?;
                Some((atoms.parse().ok()?, scale.parse().ok()?))
            })
            .collect();
        let steps = if custom.is_empty() {
            ATOM_COUNTS
                .iter()
                .flat_map(|(count, _)| RENDER_SCALES.iter().map(|(scale, _)| (*count, *scale)))
                .collect()
        } else {
            custom
        };
        Some(Self {
            steps,
            ix: 0,
            step_started: None,
            measure_started: None,
            was_inactive: false,
        })
    }
}

/// The viewport's resolved size, captured during prepaint.
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    size: Size<Pixels>,
    scale_factor: f32,
}

impl Geometry {
    fn physical(&self, render_scale_percent: u32) -> (u32, u32) {
        let scale = self.scale_factor * render_scale_percent as f32 / 100.0;
        let to_px = |v: Pixels| (f32::from(v) * scale).round().max(0.0) as u32;
        (to_px(self.size.width), to_px(self.size.height))
    }
}

/// Owns the renderer, the camera and the frame pipeline: render requests are
/// coalesced, at most [`MAX_IN_FLIGHT`] frames are read back concurrently on
/// the background executor, and each finished frame replaces the painted
/// image.
pub struct ViewportView {
    renderer: Result<Renderer, SharedString>,
    camera: OrbitCamera,
    atom_count: usize,
    render_scale_percent: u32,
    auto_rotate: bool,
    last_auto_rotate: Option<Instant>,
    geometry: Option<Geometry>,
    in_flight: usize,
    needs_render: bool,
    /// When the oldest unrendered change was requested, for latency.
    requested_at: Option<Instant>,
    image: Option<Arc<RenderImage>>,
    last_frame_size: Option<(u32, u32)>,
    probe: Rc<PaintProbe>,
    stats: FrameStats,
    drag_origin: Option<Point<Pixels>>,
    gpui_gpu: Option<SharedString>,
    /// Frames read back since measurements were reset, painted or not.
    frames_finished: u64,
    bench: Option<Bench>,
}

impl ViewportView {
    pub fn new(window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let atom_count = ATOM_COUNTS[1].0;
        let mut camera = OrbitCamera::framing(1.0);
        let renderer = Renderer::new()
            .map(|mut renderer| {
                let radius = renderer.set_spheres(&synthetic_lattice(atom_count));
                camera = OrbitCamera::framing(radius);
                renderer
            })
            .map_err(|e| SharedString::from(e.to_string()));
        let bench = Bench::from_env();
        Self {
            renderer,
            camera,
            atom_count,
            render_scale_percent: 100,
            auto_rotate: bench.is_some(),
            last_auto_rotate: None,
            geometry: None,
            in_flight: 0,
            needs_render: false,
            requested_at: None,
            image: None,
            last_frame_size: None,
            probe: Rc::default(),
            stats: FrameStats::default(),
            drag_origin: None,
            gpui_gpu: window.gpu_specs().map(|s| s.device_name.into()),
            frames_finished: 0,
            bench,
        }
    }

    fn set_geometry(&mut self, size: Size<Pixels>, scale_factor: f32, cx: &mut Context<Self>) {
        let geometry = Geometry { size, scale_factor };
        if self.geometry != Some(geometry) {
            self.geometry = Some(geometry);
            self.request_frame(cx);
        }
    }

    fn set_atom_count(&mut self, count: usize, cx: &mut Context<Self>) {
        if count == self.atom_count {
            return;
        }
        self.atom_count = count;
        if let Ok(renderer) = &mut self.renderer {
            let radius = renderer.set_spheres(&synthetic_lattice(count));
            self.camera = OrbitCamera::framing(radius);
        }
        self.reset_measurements();
        self.request_frame(cx);
    }

    fn set_render_scale(&mut self, percent: u32, cx: &mut Context<Self>) {
        if percent != self.render_scale_percent {
            self.render_scale_percent = percent;
            self.reset_measurements();
            self.request_frame(cx);
        }
    }

    fn toggle_auto_rotate(&mut self, cx: &mut Context<Self>) {
        self.auto_rotate = !self.auto_rotate;
        self.last_auto_rotate = None;
        self.reset_measurements();
        cx.notify();
    }

    fn reset_view(&mut self, cx: &mut Context<Self>) {
        if let Ok(renderer) = &mut self.renderer {
            let radius = renderer.set_spheres(&synthetic_lattice(self.atom_count));
            self.camera = OrbitCamera::framing(radius);
        }
        self.request_frame(cx);
    }

    fn reset_measurements(&mut self) {
        self.stats.clear();
        self.probe.clear();
        self.frames_finished = 0;
    }

    /// Advances the scripted run; called once per displayed frame.
    fn drive_bench(&mut self, window_active: bool, cx: &mut Context<Self>) {
        let Some(bench) = &mut self.bench else {
            return;
        };
        if bench.measure_started.is_some() && !window_active {
            bench.was_inactive = true;
        }
        let now = Instant::now();
        let Some(&(atoms, scale)) = bench.steps.get(bench.ix) else {
            cx.quit();
            return;
        };
        let step_started = *bench.step_started.get_or_insert(now);
        if now.duration_since(step_started) < BENCH_WARMUP {
            if step_started == now {
                self.set_atom_count(atoms, cx);
                self.set_render_scale(scale, cx);
            }
            return;
        }
        let Some(bench) = &mut self.bench else {
            return;
        };
        let Some(measure_started) = bench.measure_started else {
            bench.measure_started = Some(now);
            self.reset_measurements();
            return;
        };
        let measured = now.duration_since(measure_started);
        if measured < BENCH_MEASURE {
            return;
        }
        let seconds = measured.as_secs_f64();
        let was_inactive = bench.was_inactive;
        let ms = |v: Option<f64>| v.map_or(f64::NAN, |v| v);
        let (w, h) = self.last_frame_size.unwrap_or_default();
        println!(
            "bench atoms={atoms} scale={scale}% render={w}x{h} frame_mb={:.1} \
             presented_fps={:.1} finished_fps={:.1} encode_ms={:.2} gpu_wait_ms={:.2} \
             copy_ms={:.2} paint_ms={:.2} request_to_image_ms={:.2} window_lost_focus={was_inactive}",
            self.stats.last_bytes().unwrap_or(0) as f64 / 1e6,
            self.probe.presented_total() as f64 / seconds,
            self.frames_finished as f64 / seconds,
            ms(self.stats.mean_encode_ms()),
            ms(self.stats.mean_gpu_wait_ms()),
            ms(self.stats.mean_copy_ms()),
            ms(self.probe.mean_paint_ms()),
            ms(self.stats.mean_latency_ms()),
        );
        if let Some(bench) = &mut self.bench {
            bench.ix += 1;
            bench.step_started = None;
            bench.measure_started = None;
            bench.was_inactive = false;
        }
    }

    /// Called once per displayed frame while auto-rotation is on, so the
    /// benchmark is paced by the display rather than by how fast frames come
    /// back.
    fn advance_auto_rotation(&mut self, window_active: bool, cx: &mut Context<Self>) {
        if !self.auto_rotate {
            return;
        }
        let now = Instant::now();
        let elapsed = self
            .last_auto_rotate
            .replace(now)
            .map_or(Duration::ZERO, |t| now.duration_since(t));
        self.camera
            .rotate(elapsed.as_secs_f32() * AUTO_ROTATE_RADIANS_PER_SECOND, 0.0);
        self.request_frame(cx);
        self.drive_bench(window_active, cx);
    }

    fn begin_drag(&mut self, position: Point<Pixels>) {
        self.drag_origin = Some(position);
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(origin) = self.drag_origin.replace(position) else {
            return;
        };
        let dx = f32::from(position.x - origin.x);
        let dy = f32::from(position.y - origin.y);
        self.camera
            .rotate(-dx * DRAG_RADIANS_PER_PIXEL, dy * DRAG_RADIANS_PER_PIXEL);
        self.request_frame(cx);
    }

    fn end_drag(&mut self) {
        self.drag_origin = None;
    }

    fn zoom_by_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        self.camera.zoom((delta * 0.004).exp());
        self.request_frame(cx);
    }

    fn request_frame(&mut self, cx: &mut Context<Self>) {
        self.needs_render = true;
        self.requested_at.get_or_insert_with(Instant::now);
        self.pump(cx);
    }

    /// Submits a frame if one is wanted and the pipeline has room.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if !self.needs_render || self.in_flight >= MAX_IN_FLIGHT {
            return;
        }
        let Some(geometry) = self.geometry else {
            return;
        };
        let (width, height) = geometry.physical(self.render_scale_percent);
        if width == 0 || height == 0 {
            return;
        }
        let background = Rgba::from(cx.theme().muted);
        let request = RenderRequest {
            width,
            height,
            camera: self.camera.matrices(width as f32 / height as f32),
            clear_color: [
                srgb_to_linear(background.r) as f64,
                srgb_to_linear(background.g) as f64,
                srgb_to_linear(background.b) as f64,
                1.0,
            ],
        };
        let Ok(renderer) = &mut self.renderer else {
            return;
        };
        match renderer.render(&request) {
            Ok(pending) => {
                self.needs_render = false;
                self.in_flight += 1;
                let requested_at = self.requested_at.take();
                cx.spawn(async move |this, cx| {
                    let frame = cx.background_spawn(async move { pending.wait() }).await;
                    this.update(cx, |view, cx| view.finish_frame(frame, requested_at, cx))
                        .ok();
                })
                .detach();
            }
            Err(RenderError::EmptyViewport) => {}
            Err(e) => {
                self.renderer = Err(e.to_string().into());
                cx.notify();
            }
        }
    }

    fn finish_frame(
        &mut self,
        frame: Result<FrameImage, RenderError>,
        requested_at: Option<Instant>,
        cx: &mut Context<Self>,
    ) {
        self.in_flight -= 1;
        match frame {
            Ok(frame) => {
                let bytes = frame.bgra.len();
                let size = (frame.width, frame.height);
                if let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                {
                    let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                    if let Some(old) = self.image.replace(image) {
                        self.probe.retired.borrow_mut().push(old);
                    }
                    self.last_frame_size = Some(size);
                }
                self.stats
                    .record(frame.timings, requested_at.map(|t| t.elapsed()), bytes);
                self.frames_finished += 1;
            }
            Err(e) => log::error!("viewport frame failed: {e}"),
        }
        self.pump(cx);
        cx.notify();
    }

    fn render_viewport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let image = self.image.clone();
        let probe = self.probe.clone();
        let auto_rotate = self.auto_rotate;
        let theme = cx.theme();

        let surface = canvas(
            {
                let view = view.clone();
                move |bounds, window, cx| {
                    let scale_factor = window.scale_factor();
                    view.update(cx, |view, cx| {
                        view.set_geometry(bounds.size, scale_factor, cx)
                    });
                }
            },
            move |bounds, _, window, _| {
                for old in probe.retired.borrow_mut().drain(..) {
                    window.drop_image(old).ok();
                }
                if let Some(image) = image {
                    let started = Instant::now();
                    let id = image.id;
                    window
                        .paint_image(bounds, bounds, Corners::default(), image, 0, false)
                        .ok();
                    probe.note_paint(id, started.elapsed());
                }
                // Drags keep rotating when the pointer leaves the viewport.
                window.on_mouse_event({
                    let view = view.clone();
                    move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            view.update(cx, |view, cx| view.drag_to(event.position, cx));
                        }
                    }
                });
                window.on_mouse_event({
                    let view = view.clone();
                    move |event: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                            view.update(cx, |view, _| view.end_drag());
                        }
                    }
                });
                if auto_rotate && !probe.frame_scheduled.replace(true) {
                    window.on_next_frame(move |window, cx| {
                        probe.frame_scheduled.set(false);
                        let active = window.is_window_active();
                        view.update(cx, |view, cx| view.advance_auto_rotation(active, cx));
                    });
                }
            },
        )
        .size_full();

        let info = match (&self.renderer, self.last_frame_size) {
            (Err(e), _) => format!("渲染器不可用：{e}"),
            (Ok(_), Some((w, h))) => {
                format!("{} 个原子 · {w} × {h} px · 正交投影", self.atom_count)
            }
            (Ok(_), None) => "正在渲染…".to_string(),
        };

        div()
            .id("viewport")
            .relative()
            .flex_1()
            .min_w_0()
            .bg(theme.muted)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| this.begin_drag(event.position)),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.zoom_by_scroll(event, window, cx)
            }))
            .child(surface)
            .child(
                div()
                    .absolute()
                    .top_3()
                    .left_3()
                    .px_2()
                    .py_0p5()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(info),
            )
            .child(
                h_flex()
                    .absolute()
                    .bottom_4()
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(self.render_toolbar(cx)),
            )
    }

    /// Floating toolbar over the viewport; also checks that GPUI overlays
    /// (tooltip, popover) draw above the read-back image.
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_1()
            .p_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_md()
            .child(
                Button::new("model-ball-stick")
                    .ghost()
                    .small()
                    .selected(true)
                    .label("球棍"),
            )
            .child(
                Button::new("model-space-fill")
                    .ghost()
                    .small()
                    .label("空间填充"),
            )
            .child(
                Button::new("model-polyhedra")
                    .ghost()
                    .small()
                    .label("多面体"),
            )
            .child(div().w_px().h_5().mx_1().bg(theme.border))
            .child(
                Popover::new("display-range")
                    .anchor(Anchor::BottomCenter)
                    .trigger(
                        Button::new("display-range-trigger")
                            .ghost()
                            .small()
                            .label("显示范围…"),
                    )
                    .content(|_, _, cx| {
                        v_flex().w_64().gap_1().text_sm().child("显示范围").child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("PoC 占位：用来确认弹出层能盖在视口上。"),
                        )
                    }),
            )
            .child(
                Button::new("reset-view")
                    .ghost()
                    .small()
                    .label("重置视图")
                    .tooltip("恢复默认角度和缩放")
                    .on_click(cx.listener(|this, _, _, cx| this.reset_view(cx))),
            )
    }

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let ms = |v: Option<f64>| v.map_or("—".to_string(), |v| format!("{v:.2} ms"));
        let physical = self
            .geometry
            .map(|g| g.physical(100))
            .map_or("—".to_string(), |(w, h)| format!("{w} × {h}"));
        let rendered = self
            .last_frame_size
            .map_or("—".to_string(), |(w, h)| format!("{w} × {h}"));
        let bytes = self.stats.last_bytes();
        let fps = self.probe.presented_per_second();
        let frame_mb = bytes.map_or("—".to_string(), |b| format!("{:.1} MB", b as f64 / 1e6));
        let bandwidth = bytes.map_or("—".to_string(), |b| {
            format!("{:.0} MB/s", b as f64 * fps as f64 / 1e6)
        });

        let section = |title: &'static str| {
            div()
                .pt_2()
                .text_xs()
                .font_medium()
                .text_color(theme.muted_foreground)
                .child(title)
        };
        let row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .gap_3()
                .text_sm()
                .child(div().text_color(theme.muted_foreground).child(label))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_right()
                        .child(value),
                )
        };

        v_flex()
            .w_80()
            .flex_none()
            .gap_2()
            .p_4()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(div().text_base().font_semibold().child("视口 PoC"))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("wgpu 离屏渲染 → 回读 → GPUI paint_image"),
            )
            .child(section("负载"))
            .child(h_flex().gap_1().children(ATOM_COUNTS.map(|(count, label)| {
                Button::new(("atoms", count))
                    .small()
                    .selected(self.atom_count == count)
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_atom_count(count, cx)))
            })))
            .child(
                h_flex()
                    .gap_1()
                    .children(RENDER_SCALES.map(|(percent, label)| {
                        Button::new(("render-scale", percent as usize))
                            .small()
                            .selected(self.render_scale_percent == percent)
                            .label(label)
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.set_render_scale(percent, cx)
                                }),
                            )
                    })),
            )
            .child(
                Button::new("auto-rotate")
                    .small()
                    .selected(self.auto_rotate)
                    .label(if self.auto_rotate {
                        "停止自动旋转"
                    } else {
                        "自动旋转"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_auto_rotate(cx))),
            )
            .child(section("测量（最近 60 帧平均）"))
            .child(row("视口物理像素", physical))
            .child(row("渲染尺寸", rendered))
            .child(row("每帧数据", frame_mb))
            .child(row("显示帧率", format!("{fps} fps")))
            .child(row("回读带宽", bandwidth))
            .child(row("编码提交", ms(self.stats.mean_encode_ms())))
            .child(row("GPU + 回读等待", ms(self.stats.mean_gpu_wait_ms())))
            .child(row("内存拷贝", ms(self.stats.mean_copy_ms())))
            .child(row("GPUI 绘制（含上传）", ms(self.probe.mean_paint_ms())))
            .child(row("请求 → 出图", ms(self.stats.mean_latency_ms())))
            .child(section("设备"))
            .child(row(
                "渲染器",
                self.renderer
                    .as_ref()
                    .map_or("不可用".to_string(), |r| r.adapter_name().to_string()),
            ))
            .child(row(
                "GPUI",
                self.gpui_gpu
                    .as_ref()
                    .map_or("—".to_string(), |name| name.to_string()),
            ))
    }
}

impl Render for ViewportView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_stretch()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_viewport(cx))
            .child(self.render_panel(cx))
    }
}
