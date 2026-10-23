use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use gpui_kit::component::{ActiveTheme, Theme};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal_render::{
    CameraMatrices, CameraPose, FrameImage, HighlightRing, OrbitCamera, Pick, PickRequest,
    RenderError, RenderRequest, Renderer, Scene, linear_to_srgb, srgb_to_linear,
};

/// Frames that may be rendering or reading back at the same time.
const MAX_IN_FLIGHT: usize = 2;
/// Resolution while the camera moves, relative to full; GPUI stretches the
/// smaller image over the viewport (`viewport-rendering.md` §5.3).
const INTERACTIVE_SCALE: f32 = 0.5;
/// How long the camera must rest before a full-resolution frame replaces
/// the reduced one.
const SETTLE_DELAY: Duration = Duration::from_millis(150);
const DRAG_RADIANS_PER_PIXEL: f32 = 0.008;
const ZOOM_PER_SCROLL_PIXEL: f32 = 0.004;
/// A press that moves less than this is a click, not a drag.
const CLICK_SLOP: Pixels = px(4.);
/// Ring widths in logical pixels; the selection ring is the stronger mark.
const SELECTION_RING_WIDTH: f32 = 2.5;
const HOVER_RING_WIDTH: f32 = 2.0;

/// The viewport's resolved bounds, captured during prepaint.
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    bounds: Bounds<Pixels>,
    scale_factor: f32,
}

impl Geometry {
    /// The size in physical pixels at a fraction of full resolution.
    fn physical(&self, scale: f32) -> (u32, u32) {
        let size = self.bounds.size;
        let scale = self.scale_factor * scale;
        let to_px = |v: Pixels| (f32::from(v) * scale).round().max(0.0) as u32;
        (to_px(size.width), to_px(size.height))
    }

    fn aspect(&self) -> f32 {
        f32::from(self.bounds.size.width) / f32::from(self.bounds.size.height).max(1.0)
    }
}

/// Theme colors the frame and the hover ring depend on.
#[derive(Clone, Copy, PartialEq)]
struct Colors {
    clear: Hsla,
    selection: Hsla,
}

impl Colors {
    fn new(theme: &Theme) -> Self {
        Self {
            clear: theme.muted,
            selection: theme.selection,
        }
    }

    /// Halfway between the selection color and the background, in linear
    /// light: a lighter mark than the selection ring.
    fn hover(&self) -> Hsla {
        let (selection, clear) = (linear(self.selection), linear(self.clear));
        let [r, g, b] = [0, 1, 2].map(|i| linear_to_srgb((selection[i] + clear[i]) / 2.0));
        Rgba { r, g, b, a: 1.0 }.into()
    }
}

fn linear(color: Hsla) -> [f32; 3] {
    let rgba = Rgba::from(color);
    [rgba.r, rgba.g, rgba.b].map(srgb_to_linear)
}

/// The camera and scene placement one frame was rendered with. Overlays
/// are placed with the frame on screen, so they stay on the image while
/// newer frames are still being rendered.
#[derive(Clone, Copy)]
struct FrameView {
    camera: CameraMatrices,
    /// The scene center, which the frame's positions are relative to.
    center: Vec3,
}

impl FrameView {
    /// Where a scene point (Å) appears in a viewport of the given size.
    fn project(&self, position: Vec3, size: Size<Pixels>) -> Point<Pixels> {
        let CameraMatrices { view, proj } = self.camera;
        let clip = proj * view * (position - self.center).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        point(
            size.width * ((ndc.x + 1.0) * 0.5),
            size.height * ((1.0 - ndc.y) * 0.5),
        )
    }

    /// Logical pixels per Å in a viewport of the given height; the
    /// projection is orthographic.
    fn pixels_per_angstrom(&self, height: Pixels) -> f32 {
        self.camera.proj.y_axis.y * f32::from(height) / 2.0
    }
}

/// A left-button press that may become a click or a drag.
struct Press {
    origin: Point<Pixels>,
    last: Point<Pixels>,
    dragging: bool,
    modifiers: Modifiers,
}

/// Why a pick was requested.
#[derive(Clone, Copy)]
enum PickPurpose {
    Hover,
    Click(Modifiers),
}

/// What the viewport reports to its host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewportEvent {
    /// A press and release without dragging; `pick` is what was under the
    /// pointer, indexing the host's scene.
    Clicked {
        pick: Option<Pick>,
        modifiers: Modifiers,
    },
    /// The object under the pointer changed.
    Hovered(Option<Pick>),
}

/// Owns the renderer, the camera and the frame pipeline. Changes made in
/// one update (scene, rings, camera) go into a single frame, submitted at
/// the end of the update; at most [`MAX_IN_FLIGHT`] frames are read back
/// concurrently on the background executor, and each finished frame
/// replaces the painted image. While the camera moves, frames render at
/// [`INTERACTIVE_SCALE`].
///
/// Picking goes through the renderer's ID texture, also off the UI thread:
/// one pick at a time, hover picks coalesced to the latest pointer position.
/// The host chooses which spheres show the selection ring, which is part of
/// the 3D image; the hovered sphere's lighter ring is painted over the
/// image, so hovering never renders a frame.
pub struct Viewport {
    renderer: Result<Renderer, SharedString>,
    camera: OrbitCamera,
    /// Subtracted from scene positions so the camera orbits the scene center.
    scene_center: Vec3,
    scene_radius: f32,
    /// The scene as drawn, centered; rings are built from its spheres.
    scene: Scene,
    /// Whether the camera has been framed on a scene yet.
    framed: bool,
    geometry: Option<Geometry>,
    colors: Option<Colors>,
    in_flight: usize,
    /// Whether something changed since the last frame was submitted.
    needs_render: bool,
    /// Whether a submission is scheduled for the end of this update.
    submit_scheduled: bool,
    /// Whether the camera is moving, so frames render at reduced size.
    interactive: bool,
    /// Ends `interactive` once the camera has rested; replaced, and so
    /// restarted, by every move.
    settle: Task<()>,
    /// Frames submitted so far; each frame's number is its index.
    submitted: u64,
    /// The number of the frame `image` holds, and how it was rendered.
    shown: Option<(u64, FrameView)>,
    image: Option<Arc<RenderImage>>,
    /// Images replaced since the last paint; paint removes them from GPUI's
    /// sprite atlas so it does not grow by one image per frame.
    retired: Rc<RefCell<Vec<Arc<RenderImage>>>>,
    press: Option<Press>,
    /// The pointer relative to the viewport, while it is over the 3D view.
    pointer: Option<Point<Pixels>>,
    hovered: Option<Pick>,
    selected: Vec<usize>,
    pick_in_flight: bool,
    /// Clicks waiting for the pick in flight to finish, in order.
    queued_clicks: VecDeque<(Point<Pixels>, Modifiers)>,
    /// Whether what lies under the pointer may have changed.
    hover_stale: bool,
}

impl EventEmitter<ViewportEvent> for Viewport {}

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
            scene: Scene::default(),
            framed: false,
            geometry: None,
            colors: None,
            in_flight: 0,
            needs_render: false,
            submit_scheduled: false,
            interactive: false,
            settle: Task::ready(()),
            submitted: 0,
            shown: None,
            image: None,
            retired: Rc::default(),
            press: None,
            pointer: None,
            hovered: None,
            selected: Vec::new(),
            pick_in_flight: false,
            queued_clicks: VecDeque::new(),
            hover_stale: false,
        }
    }

    /// Replaces what is drawn (positions in Å) and clears the selection
    /// rings. The first scene is framed; later ones keep the current angle
    /// and zoom.
    pub fn set_scene(&mut self, mut scene: Scene, cx: &mut Context<Self>) {
        let points = scene
            .spheres
            .iter()
            .map(|s| &s.center_radius[..3])
            .chain(scene.lines.iter().map(|l| &l.start_width[..3]))
            .chain(scene.lines.iter().map(|l| &l.end_dash[..3]))
            .map(Vec3::from_slice);
        let (min, max) = points.fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), p| (min.min(p), max.max(p)),
        );
        let center = if min.x <= max.x {
            (min + max) / 2.0
        } else {
            Vec3::ZERO
        };
        scene.translate(-center);
        self.scene_center = center;
        if let Ok(renderer) = &mut self.renderer {
            self.scene_radius = renderer.set_scene(&scene);
        }
        self.scene = scene;
        self.selected.clear();
        self.hovered = None;
        self.hover_stale = true;
        if self.framed {
            self.camera.set_scene_radius(self.scene_radius);
            self.request_frame(cx);
        } else {
            self.framed = true;
            self.reset_camera(cx);
        }
        // The hover ring and the pointer cursor are gone.
        cx.notify();
    }

    /// Spheres of the current scene that show the selection ring.
    pub fn selection(&self) -> &[usize] {
        &self.selected
    }

    /// Whether the image on screen is the last frame submitted, no frame is
    /// pending and the camera has settled to full resolution, so the image
    /// shows the current scene, camera and rings.
    pub fn is_current(&self) -> bool {
        !self.needs_render
            && !self.interactive
            && self.in_flight == 0
            && self.shown.map(|(number, _)| number) == self.submitted.checked_sub(1)
    }

    /// Frames submitted so far, for tests and diagnostics.
    pub fn frames_submitted(&self) -> u64 {
        self.submitted
    }

    /// Spheres of the current scene that show the selection ring.
    pub fn set_selection(&mut self, spheres: Vec<usize>, cx: &mut Context<Self>) {
        if self.selected != spheres {
            self.selected = spheres;
            self.upload_rings();
            self.request_frame(cx);
        }
    }

    /// What is under the pointer, as of the last hover pick.
    pub fn hovered(&self) -> Option<Pick> {
        self.hovered
    }

    /// The camera's angle and zoom, to save the view.
    pub fn camera_pose(&self) -> CameraPose {
        self.camera.pose()
    }

    /// Returns to a saved angle and zoom. Set after the first scene, so that
    /// the zoom is limited to that scene's size.
    pub fn set_camera_pose(&mut self, pose: CameraPose, cx: &mut Context<Self>) {
        self.framed = true;
        self.camera.set_pose(pose);
        self.request_frame(cx);
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

    /// The viewport's bounds in the window, as of the last layout.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.geometry.map(|g| g.bounds)
    }

    /// Where a scene point (Å) appears in the image on screen, relative to
    /// the viewport's top-left corner. That image was rendered with an
    /// earlier camera while the camera moves, so overlays placed here stay
    /// on it. `None` before the first layout.
    pub fn project(&self, position: [f64; 3]) -> Option<Point<Pixels>> {
        let geometry = self.geometry?;
        let position = Vec3::from_array(position.map(|v| v as f32));
        Some(
            self.view_on_screen(geometry)
                .project(position, geometry.bounds.size),
        )
    }

    /// How the image on screen was rendered; the current camera before the
    /// first frame lands.
    fn view_on_screen(&self, geometry: Geometry) -> FrameView {
        self.shown.map_or_else(
            || FrameView {
                camera: self.camera.matrices(geometry.aspect()),
                center: self.scene_center,
            },
            |(_, view)| view,
        )
    }

    /// The ring around the hovered sphere, relative to the viewport, and
    /// its width. Painted over the image rather than rendered in 3D: the
    /// hovered sphere was picked, so it is visible. A selected sphere keeps
    /// its selection ring instead.
    fn hover_ring(&self) -> Option<(Bounds<Pixels>, Pixels)> {
        let Some(Pick::Sphere(index)) = self.hovered else {
            return None;
        };
        if self.selected.contains(&index) {
            return None;
        }
        let [x, y, z, radius] = self.scene.spheres.get(index)?.center_radius;
        let geometry = self.geometry?;
        let view = self.view_on_screen(geometry);
        let viewport = geometry.bounds.size;
        let center = view.project(Vec3::new(x, y, z) + self.scene_center, viewport);
        let outer = px(radius * view.pixels_per_angstrom(viewport.height) + HOVER_RING_WIDTH);
        // Overlaps the sphere's soft edge by half a physical pixel, as the
        // selection ring does, so no background shows between them.
        let width = px(HOVER_RING_WIDTH + 0.5 / geometry.scale_factor);
        Some((
            Bounds::centered_at(center, size(outer * 2., outer * 2.)),
            width,
        ))
    }

    fn prepaint(
        &mut self,
        bounds: Bounds<Pixels>,
        scale_factor: f32,
        colors: Colors,
        cx: &mut Context<Self>,
    ) {
        let geometry = Geometry {
            bounds,
            scale_factor,
        };
        let resized = self.geometry.map(|g| (g.bounds.size, g.scale_factor))
            != Some((bounds.size, scale_factor));
        let recolored = self.colors != Some(colors);
        self.geometry = Some(geometry);
        self.colors = Some(colors);
        if recolored {
            self.upload_rings();
        }
        if recolored || resized {
            self.needs_render = true;
        }
        // The last chance before paint, and the first once there is a size.
        self.pump(cx);
    }

    /// Uploads the selection rings to the renderer.
    fn upload_rings(&mut self) {
        let Some(colors) = self.colors else {
            return;
        };
        let [r, g, b] = linear(colors.selection);
        let rings: Vec<HighlightRing> = self
            .selected
            .iter()
            .filter_map(|&index| self.scene.spheres.get(index))
            .map(|sphere| HighlightRing {
                center_radius: sphere.center_radius,
                color_width: [r, g, b, SELECTION_RING_WIDTH],
            })
            .collect();
        if let Ok(renderer) = &mut self.renderer {
            renderer.set_highlights(&rings);
        }
    }

    fn set_hovered(&mut self, pick: Option<Pick>, cx: &mut Context<Self>) {
        if self.hovered != pick {
            self.hovered = pick;
            cx.emit(ViewportEvent::Hovered(pick));
            // Repaints the ring and the cursor; the 3D image stays.
            cx.notify();
        }
    }

    // MARK: Pointer

    fn press(&mut self, event: &MouseDownEvent) {
        self.press = Some(Press {
            origin: event.position,
            last: event.position,
            dragging: false,
            modifiers: event.modifiers,
        });
    }

    /// Any pointer move in the window: rotates while dragging.
    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(press) = &mut self.press else {
            return;
        };
        if !press.dragging {
            let moved = position - press.origin;
            if moved.x.abs().max(moved.y.abs()) < CLICK_SLOP {
                return;
            }
            press.dragging = true;
            self.pointer = None;
            self.set_hovered(None, cx);
        }
        let Some(press) = &mut self.press else {
            return;
        };
        let delta = position - std::mem::replace(&mut press.last, position);
        self.camera.rotate(
            -f32::from(delta.x) * DRAG_RADIANS_PER_PIXEL,
            f32::from(delta.y) * DRAG_RADIANS_PER_PIXEL,
        );
        self.camera_moved(cx);
    }

    /// A left-button release anywhere in the window.
    fn release(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        if !press.dragging
            && let Some(geometry) = self.geometry
        {
            self.queued_clicks
                .push_back((position - geometry.bounds.origin, press.modifiers));
        }
        self.hover_stale = true;
        self.pump_picks(cx);
    }

    /// Pointer moves over the 3D view itself, not over overlays on it.
    fn hover_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.press.as_ref().is_some_and(|p| p.dragging) {
            return;
        }
        let Some(geometry) = self.geometry else {
            return;
        };
        self.pointer = Some(position - geometry.bounds.origin);
        self.hover_stale = true;
        self.pump_picks(cx);
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        self.pointer = None;
        self.set_hovered(None, cx);
    }

    fn zoom_by_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        self.camera.zoom((delta * ZOOM_PER_SCROLL_PIXEL).exp());
        self.hover_stale = true;
        self.camera_moved(cx);
        self.pump_picks(cx);
    }

    /// The pointer moved the camera: frames render at the interactive scale
    /// until the camera has rested for [`SETTLE_DELAY`], then once more at
    /// full resolution.
    fn camera_moved(&mut self, cx: &mut Context<Self>) {
        self.interactive = true;
        self.settle = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE_DELAY).await;
            this.update(cx, |viewport, cx| {
                viewport.interactive = false;
                viewport.request_frame(cx);
            })
            .ok();
        });
        self.request_frame(cx);
    }

    /// Submits the next pick if none is in flight: a queued click first,
    /// then a hover pick if the pointer is over the view.
    fn pump_picks(&mut self, cx: &mut Context<Self>) {
        if self.pick_in_flight {
            return;
        }
        let (purpose, position) =
            if let Some((position, modifiers)) = self.queued_clicks.pop_front() {
                (PickPurpose::Click(modifiers), position)
            } else if self.hover_stale
                && self.press.is_none()
                && let Some(pointer) = self.pointer
            {
                self.hover_stale = false;
                (PickPurpose::Hover, pointer)
            } else {
                return;
            };
        let Some(geometry) = self.geometry else {
            return;
        };
        // Picks always use the full resolution, whatever the image on screen.
        let (width, height) = geometry.physical(1.0);
        let to_px = |v: Pixels, limit: u32| {
            ((f32::from(v) * geometry.scale_factor).max(0.0) as u32).min(limit.saturating_sub(1))
        };
        let request = PickRequest {
            width,
            height,
            camera: self.camera.matrices(geometry.aspect()),
            x: to_px(position.x, width),
            y: to_px(position.y, height),
        };
        let Ok(renderer) = &mut self.renderer else {
            return;
        };
        match renderer.pick(&request) {
            Ok(pending) => {
                self.pick_in_flight = true;
                cx.spawn(async move |this, cx| {
                    let pick = cx.background_spawn(async move { pending.wait() }).await;
                    this.update(cx, |viewport, cx| viewport.finish_pick(purpose, pick, cx))
                        .ok();
                })
                .detach();
            }
            Err(e) => {
                if !matches!(e, RenderError::EmptyViewport) {
                    log::error!("viewport pick failed: {e}");
                }
                if let PickPurpose::Click(modifiers) = purpose {
                    cx.emit(ViewportEvent::Clicked {
                        pick: None,
                        modifiers,
                    });
                }
            }
        }
    }

    fn finish_pick(
        &mut self,
        purpose: PickPurpose,
        pick: Result<Option<Pick>, RenderError>,
        cx: &mut Context<Self>,
    ) {
        self.pick_in_flight = false;
        let pick = pick.unwrap_or_else(|e| {
            log::error!("viewport pick failed: {e}");
            None
        });
        match purpose {
            PickPurpose::Click(modifiers) => cx.emit(ViewportEvent::Clicked { pick, modifiers }),
            // The pointer may have left or started a drag meanwhile.
            PickPurpose::Hover if self.pointer.is_some() && self.press.is_none() => {
                self.set_hovered(pick, cx)
            }
            PickPurpose::Hover => {}
        }
        self.pump_picks(cx);
    }

    // MARK: Frames

    /// Asks for a frame with the current scene, rings and camera. Everything
    /// requested during one update goes into one frame, submitted when the
    /// update ends.
    fn request_frame(&mut self, cx: &mut Context<Self>) {
        self.needs_render = true;
        if !std::mem::replace(&mut self.submit_scheduled, true) {
            let this = cx.weak_entity();
            cx.defer(move |cx| {
                this.update(cx, |viewport, cx| {
                    viewport.submit_scheduled = false;
                    viewport.pump(cx);
                })
                .ok();
            });
        }
    }

    /// Submits a frame if one is wanted and the pipeline has room.
    fn pump(&mut self, cx: &mut Context<Self>) {
        if !self.needs_render || self.in_flight >= MAX_IN_FLIGHT {
            return;
        }
        let (Some(geometry), Some(colors)) = (self.geometry, self.colors) else {
            return;
        };
        let scale = if self.interactive {
            INTERACTIVE_SCALE
        } else {
            1.0
        };
        let (width, height) = geometry.physical(scale);
        if width == 0 || height == 0 {
            return;
        }
        let view = FrameView {
            camera: self.camera.matrices(geometry.aspect()),
            center: self.scene_center,
        };
        let [r, g, b] = linear(colors.clear).map(f64::from);
        let request = RenderRequest {
            width,
            height,
            camera: view.camera,
            clear_color: [r, g, b, 1.0],
            scale_factor: geometry.scale_factor * scale,
        };
        let Ok(renderer) = &mut self.renderer else {
            return;
        };
        match renderer.render(&request) {
            Ok(pending) => {
                self.needs_render = false;
                self.in_flight += 1;
                let number = self.submitted;
                self.submitted += 1;
                cx.spawn(async move |this, cx| {
                    let frame = cx
                        .background_spawn(async move { pending.wait_latest() })
                        .await;
                    this.update(cx, |viewport, cx| {
                        viewport.finish_frame(number, view, frame, cx)
                    })
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

    fn finish_frame(
        &mut self,
        number: u64,
        view: FrameView,
        frame: Result<Option<FrameImage>, RenderError>,
        cx: &mut Context<Self>,
    ) {
        self.in_flight -= 1;
        match frame {
            // A newer frame was submitted before this one finished; its
            // pixels were not even copied.
            Ok(None) => {}
            // Frames reach the UI thread in any order. One older than the
            // image on screen is out of date: showing it would undo the
            // newer frame, e.g. drop the rings an edit brought back.
            Ok(Some(_)) if self.shown.is_some_and(|(shown, _)| number < shown) => {}
            Ok(Some(frame)) => {
                if let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                {
                    let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                    self.shown = Some((number, view));
                    if let Some(old) = self.image.replace(image) {
                        self.retired.borrow_mut().push(old);
                    }
                    // Repaints, and moves the host's overlays with the image.
                    cx.notify();
                }
            }
            Err(e) => log::error!("viewport frame failed: {e}"),
        }
        self.pump(cx);
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
        let hover_ring = self.hover_ring();
        let hover_color = Colors::new(theme).hover();

        let surface = canvas(
            {
                let view = view.clone();
                move |bounds, window, cx| {
                    let scale_factor = window.scale_factor();
                    let colors = Colors::new(cx.theme());
                    view.update(cx, |viewport, cx| {
                        viewport.prepaint(bounds, scale_factor, colors, cx)
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
                if let Some((ring, width)) = hover_ring {
                    let ring = Bounds::new(ring.origin + bounds.origin, ring.size);
                    window.paint_quad(quad(
                        ring,
                        ring.size.width / 2.,
                        transparent_black(),
                        width,
                        hover_color,
                        BorderStyle::Solid,
                    ));
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
                        view.update(cx, |viewport, cx| viewport.release(event.position, cx));
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
            .when(matches!(self.hovered, Some(Pick::Sphere(_))), |this| {
                this.cursor_pointer()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| this.press(event)),
            )
            .on_mouse_move(
                cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.hover_at(event.position, cx)
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.leave(cx);
                }
            }))
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
