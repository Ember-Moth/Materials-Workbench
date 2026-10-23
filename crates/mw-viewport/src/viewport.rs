use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal_render::{
    CameraMatrices, CameraPose, FrameImage, HighlightRing, OrbitCamera, Pick, PickRequest,
    RenderError, RenderRequest, Renderer, Scene, srgb_to_linear,
};

/// Frames that may be rendering or reading back at the same time.
const MAX_IN_FLIGHT: usize = 2;
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
    fn physical(&self) -> (u32, u32) {
        let size = self.bounds.size;
        let to_px = |v: Pixels| (f32::from(v) * self.scale_factor).round().max(0.0) as u32;
        (to_px(size.width), to_px(size.height))
    }

    fn aspect(&self) -> f32 {
        f32::from(self.bounds.size.width) / f32::from(self.bounds.size.height).max(1.0)
    }
}

/// Theme colors the frame depends on, captured during prepaint.
#[derive(Clone, Copy, PartialEq)]
struct Colors {
    clear: Hsla,
    selection: Hsla,
}

fn linear(color: Hsla) -> [f32; 3] {
    let rgba = Rgba::from(color);
    [rgba.r, rgba.g, rgba.b].map(srgb_to_linear)
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

/// Owns the renderer, the camera and the frame pipeline. Render requests are
/// coalesced; at most [`MAX_IN_FLIGHT`] frames are read back concurrently on
/// the background executor, and each finished frame replaces the painted
/// image.
///
/// Picking goes through the renderer's ID texture, also off the UI thread:
/// one pick at a time, hover picks coalesced to the latest pointer position.
/// Hovered spheres get a light ring; the host chooses which spheres show the
/// selection ring.
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
    needs_render: bool,
    /// Frames submitted so far; each frame's number is its index.
    submitted: u64,
    /// The number of the frame `image` holds.
    shown: Option<u64>,
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
    }

    /// Spheres of the current scene that show the selection ring.
    pub fn selection(&self) -> &[usize] {
        &self.selected
    }

    /// Whether the image on screen is the last frame submitted and no frame
    /// is pending, so it shows the current scene, camera and highlights.
    pub fn is_current(&self) -> bool {
        !self.needs_render && self.in_flight == 0 && self.shown == self.submitted.checked_sub(1)
    }

    /// Spheres of the current scene that show the selection ring.
    pub fn set_selection(&mut self, spheres: Vec<usize>, cx: &mut Context<Self>) {
        if self.selected != spheres {
            self.selected = spheres;
            self.update_highlights(cx);
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

    /// Where a scene point (Å) appears, relative to the viewport's top-left
    /// corner, under the current camera. `None` before the first layout.
    pub fn project(&self, position: [f64; 3]) -> Option<Point<Pixels>> {
        let geometry = self.geometry?;
        let CameraMatrices { view, proj } = self.camera.matrices(geometry.aspect());
        let world = Vec3::new(position[0] as f32, position[1] as f32, position[2] as f32)
            - self.scene_center;
        let clip = proj * view * world.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        let size = geometry.bounds.size;
        Some(point(
            size.width * ((ndc.x + 1.0) * 0.5),
            size.height * ((1.0 - ndc.y) * 0.5),
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
            self.update_highlights(cx);
        } else if resized {
            self.request_frame(cx);
        }
    }

    /// Rebuilds the rings from the selection and the hovered sphere.
    fn update_highlights(&mut self, cx: &mut Context<Self>) {
        let Some(colors) = self.colors else {
            return;
        };
        let selection = linear(colors.selection);
        let clear = linear(colors.clear);
        let hover: [f32; 3] = [0, 1, 2].map(|i| (selection[i] + clear[i]) / 2.0);
        let ring = |index: usize, color: [f32; 3], width: f32| {
            self.scene.spheres.get(index).map(|sphere| HighlightRing {
                center_radius: sphere.center_radius,
                color_width: [color[0], color[1], color[2], width],
            })
        };
        let hovered = match self.hovered {
            Some(Pick::Sphere(index)) if !self.selected.contains(&index) => Some(index),
            _ => None,
        };
        let rings: Vec<HighlightRing> = hovered
            .and_then(|index| ring(index, hover, HOVER_RING_WIDTH))
            .into_iter()
            .chain(
                self.selected
                    .iter()
                    .filter_map(|&index| ring(index, selection, SELECTION_RING_WIDTH)),
            )
            .collect();
        if let Ok(renderer) = &mut self.renderer {
            renderer.set_highlights(&rings);
        }
        self.request_frame(cx);
    }

    fn set_hovered(&mut self, pick: Option<Pick>, cx: &mut Context<Self>) {
        if self.hovered != pick {
            self.hovered = pick;
            self.update_highlights(cx);
            cx.emit(ViewportEvent::Hovered(pick));
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
        self.request_frame(cx);
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
        self.request_frame(cx);
        self.pump_picks(cx);
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
        let (width, height) = geometry.physical();
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
        let (Some(geometry), Some(colors)) = (self.geometry, self.colors) else {
            return;
        };
        let (width, height) = geometry.physical();
        if width == 0 || height == 0 {
            return;
        }
        let [r, g, b] = linear(colors.clear).map(f64::from);
        let request = RenderRequest {
            width,
            height,
            camera: self.camera.matrices(geometry.aspect()),
            clear_color: [r, g, b, 1.0],
            scale_factor: geometry.scale_factor,
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
                    let frame = cx.background_spawn(async move { pending.wait() }).await;
                    this.update(cx, |viewport, cx| viewport.finish_frame(number, frame, cx))
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
        frame: Result<FrameImage, RenderError>,
        cx: &mut Context<Self>,
    ) {
        self.in_flight -= 1;
        match frame {
            // Frames finish on background threads in any order. One older
            // than the image on screen is out of date: showing it would undo
            // the newer frame, e.g. drop the rings an edit's second frame
            // brought back.
            Ok(_) if self.shown.is_some_and(|shown| number < shown) => {}
            Ok(frame) => {
                if let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                {
                    let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                    self.shown = Some(number);
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
                    let theme = cx.theme();
                    let colors = Colors {
                        clear: theme.muted,
                        selection: theme.selection,
                    };
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
