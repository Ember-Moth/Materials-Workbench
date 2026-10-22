use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal_render::{
    CameraMatrices, FrameImage, OrbitCamera, RenderError, RenderRequest, Renderer, SphereInstance,
    srgb_to_linear,
};

/// Frames that may be rendering or reading back at the same time.
const MAX_IN_FLIGHT: usize = 2;
const DRAG_RADIANS_PER_PIXEL: f32 = 0.008;
const ZOOM_PER_SCROLL_PIXEL: f32 = 0.004;

/// The viewport's resolved size, captured during prepaint.
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    size: Size<Pixels>,
    scale_factor: f32,
}

impl Geometry {
    fn physical(&self) -> (u32, u32) {
        let to_px = |v: Pixels| (f32::from(v) * self.scale_factor).round().max(0.0) as u32;
        (to_px(self.size.width), to_px(self.size.height))
    }

    fn aspect(&self) -> f32 {
        f32::from(self.size.width) / f32::from(self.size.height).max(1.0)
    }
}

/// Owns the renderer, the camera and the frame pipeline. Render requests are
/// coalesced; at most [`MAX_IN_FLIGHT`] frames are read back concurrently on
/// the background executor, and each finished frame replaces the painted
/// image.
pub struct Viewport {
    renderer: Result<Renderer, SharedString>,
    camera: OrbitCamera,
    /// Subtracted from scene positions so the camera orbits the scene center.
    scene_center: Vec3,
    scene_radius: f32,
    /// Whether the camera has been framed on a scene yet.
    framed: bool,
    geometry: Option<Geometry>,
    clear_color: Option<Hsla>,
    in_flight: usize,
    needs_render: bool,
    image: Option<Arc<RenderImage>>,
    /// Images replaced since the last paint; paint removes them from GPUI's
    /// sprite atlas so it does not grow by one image per frame.
    retired: Rc<RefCell<Vec<Arc<RenderImage>>>>,
    drag_origin: Option<Point<Pixels>>,
}

impl Viewport {
    pub fn new() -> Self {
        let renderer = Renderer::new().map_err(|e| {
            log::error!("viewport renderer unavailable: {e}");
            SharedString::from(e.to_string())
        });
        Self {
            renderer,
            camera: OrbitCamera::framing(1.0),
            scene_center: Vec3::ZERO,
            scene_radius: 1.0,
            framed: false,
            geometry: None,
            clear_color: None,
            in_flight: 0,
            needs_render: false,
            image: None,
            retired: Rc::default(),
            drag_origin: None,
        }
    }

    /// Replaces the drawn spheres (positions in Å). The first scene is framed;
    /// later ones keep the current angle and zoom.
    pub fn set_scene(&mut self, spheres: &[SphereInstance], cx: &mut Context<Self>) {
        let center = if spheres.is_empty() {
            Vec3::ZERO
        } else {
            spheres
                .iter()
                .map(|s| Vec3::from_slice(&s.center_radius[..3]))
                .sum::<Vec3>()
                / spheres.len() as f32
        };
        let centered: Vec<SphereInstance> = spheres
            .iter()
            .map(|s| {
                let p = Vec3::from_slice(&s.center_radius[..3]) - center;
                SphereInstance {
                    center_radius: [p.x, p.y, p.z, s.center_radius[3]],
                    color: s.color,
                }
            })
            .collect();
        self.scene_center = center;
        if let Ok(renderer) = &mut self.renderer {
            self.scene_radius = renderer.set_spheres(&centered);
        }
        if self.framed {
            self.camera.set_scene_radius(self.scene_radius);
            self.request_frame(cx);
        } else {
            self.framed = true;
            self.reset_camera(cx);
        }
    }

    /// Restores the default angle and zoom.
    pub fn reset_camera(&mut self, cx: &mut Context<Self>) {
        self.camera = OrbitCamera::framing(self.scene_radius);
        self.request_frame(cx);
    }

    /// Why the viewport cannot draw, if it cannot.
    pub fn renderer_error(&self) -> Option<&SharedString> {
        self.renderer.as_ref().err()
    }

    /// Where a scene point (Å) appears, relative to the viewport's top-left
    /// corner, under the current camera. `None` before the first layout.
    pub fn project(&self, position: [f64; 3]) -> Option<Point<Pixels>> {
        let geometry = self.geometry?;
        let CameraMatrices { view, proj } = self.camera.matrices(geometry.aspect());
        let world = Vec3::new(position[0] as f32, position[1] as f32, position[2] as f32)
            - self.scene_center;
        let clip = proj * view * world.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        Some(point(
            geometry.size.width * ((ndc.x + 1.0) * 0.5),
            geometry.size.height * ((1.0 - ndc.y) * 0.5),
        ))
    }

    fn prepaint(
        &mut self,
        size: Size<Pixels>,
        scale_factor: f32,
        clear: Hsla,
        cx: &mut Context<Self>,
    ) {
        let geometry = Geometry { size, scale_factor };
        let changed = self.geometry != Some(geometry) || self.clear_color != Some(clear);
        self.geometry = Some(geometry);
        self.clear_color = Some(clear);
        if changed {
            self.request_frame(cx);
        }
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
        self.camera.zoom((delta * ZOOM_PER_SCROLL_PIXEL).exp());
        self.request_frame(cx);
    }

    fn request_frame(&mut self, cx: &mut Context<Self>) {
        self.needs_render = true;
        self.pump(cx);
        // Projected overlays follow the camera even before the frame lands.
        cx.notify();
    }

    /// Submits a frame if one is wanted and the pipeline has room.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if !self.needs_render || self.in_flight >= MAX_IN_FLIGHT {
            return;
        }
        let (Some(geometry), Some(clear)) = (self.geometry, self.clear_color) else {
            return;
        };
        let (width, height) = geometry.physical();
        if width == 0 || height == 0 {
            return;
        }
        let clear = Rgba::from(clear);
        let request = RenderRequest {
            width,
            height,
            camera: self.camera.matrices(geometry.aspect()),
            clear_color: [
                srgb_to_linear(clear.r) as f64,
                srgb_to_linear(clear.g) as f64,
                srgb_to_linear(clear.b) as f64,
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
                cx.spawn(async move |this, cx| {
                    let frame = cx.background_spawn(async move { pending.wait() }).await;
                    this.update(cx, |viewport, cx| viewport.finish_frame(frame, cx))
                        .ok();
                })
                .detach();
            }
            Err(RenderError::EmptyViewport) => {}
            Err(e) => {
                log::error!("viewport frame failed: {e}");
                self.renderer = Err(e.to_string().into());
                cx.notify();
            }
        }
    }

    fn finish_frame(&mut self, frame: Result<FrameImage, RenderError>, cx: &mut Context<Self>) {
        self.in_flight -= 1;
        match frame {
            Ok(frame) => {
                if let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                {
                    let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                    if let Some(old) = self.image.replace(image) {
                        self.retired.borrow_mut().push(old);
                    }
                }
            }
            Err(e) => log::error!("viewport frame failed: {e}"),
        }
        self.pump(cx);
        cx.notify();
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for Viewport {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let image = self.image.clone();
        let retired = self.retired.clone();
        let theme = cx.theme();

        let surface = canvas(
            {
                let view = view.clone();
                move |bounds, window, cx| {
                    let scale_factor = window.scale_factor();
                    let clear = cx.theme().muted;
                    view.update(cx, |viewport, cx| {
                        viewport.prepaint(bounds.size, scale_factor, clear, cx)
                    });
                }
            },
            move |bounds, _, window, _| {
                for old in retired.borrow_mut().drain(..) {
                    window.drop_image(old).ok();
                }
                if let Some(image) = image {
                    window
                        .paint_image(bounds, bounds, Corners::default(), image, 0, false)
                        .ok();
                }
                // Drags keep rotating when the pointer leaves the viewport.
                window.on_mouse_event({
                    let view = view.clone();
                    move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            view.update(cx, |viewport, cx| viewport.drag_to(event.position, cx));
                        }
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        view.update(cx, |viewport, _| viewport.end_drag());
                    }
                });
            },
        )
        .size_full();

        div()
            .id("viewport")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(theme.muted)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| this.begin_drag(event.position)),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                this.zoom_by_scroll(event, window, cx)
            }))
            .child(surface)
            .when_some(self.renderer_error().cloned(), |this, error| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(
                            rust_i18n::t!("viewport.unavailable", reason = error).to_string(),
                        )),
                )
            })
    }
}
