use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use glam::{Mat4, Vec3};

use crate::camera::CameraMatrices;
use crate::scene::{BondInstance, HighlightRing, LineInstance, Scene, SphereInstance};

/// sRGB so that the bytes read back match what GPUI expects from a decoded
/// image: sRGB-encoded BGRA.
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Pick IDs (`docs/design/viewport-rendering.md` §5.4). WebGPU does not
/// guarantee multisampling for this format, and integer formats cannot be
/// resolved, so IDs are drawn in a separate single-sample pass.
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
const SAMPLE_COUNT: u32 = 4;
/// Readback buffers kept for reuse; a few frames can be in flight at once.
const READBACK_POOL_LIMIT: usize = 4;
/// Sizes whose render targets are kept: a viewport alternates between its
/// full size and the reduced size it renders at while the camera moves.
const TARGET_SIZES_KEPT: usize = 2;
/// Pixels around the pointer searched for the nearest object, so that thin
/// bonds and the soft edges of atoms are easy to hit.
const PICK_RADIUS: u32 = 4;

// Must match the constants in common.wgsl.
const SPHERE_ID: u32 = 1 << 30;
const BOND_ID: u32 = 2 << 30;
const ID_INDEX_MASK: u32 = (1 << 30) - 1;

const SPHERE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
const BOND_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
const LINE_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
const TRIANGLE_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

/// One face of a polyhedron as uploaded: its corners in Å (xyz) and its
/// polyhedron's linear RGB and opacity.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct TriangleInstance {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    color: [f32; 4],
}

#[derive(Debug)]
pub enum RenderError {
    NoAdapter(String),
    Device(String),
    Readback(String),
    EmptyViewport,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(reason) => write!(f, "no GPU adapter: {reason}"),
            Self::Device(reason) => write!(f, "GPU device request failed: {reason}"),
            Self::Readback(reason) => write!(f, "frame readback failed: {reason}"),
            Self::EmptyViewport => write!(f, "viewport has no area"),
        }
    }
}

impl std::error::Error for RenderError {}

/// One frame to render, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct RenderRequest {
    pub width: u32,
    pub height: u32,
    pub camera: CameraMatrices,
    /// Linear RGBA.
    pub clear_color: [f64; 4],
    /// Physical pixels per logical pixel. Line and ring widths are given in
    /// logical pixels.
    pub scale_factor: f32,
}

/// What is under one pixel, for a frame of the given size and camera.
#[derive(Clone, Copy, Debug)]
pub struct PickRequest {
    pub width: u32,
    pub height: u32,
    pub camera: CameraMatrices,
    /// Physical pixels from the top-left corner.
    pub x: u32,
    pub y: u32,
}

/// An object found under the pointer: the index of a sphere or bond in the
/// current [`Scene`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    Sphere(usize),
    Bond(usize),
}

impl Pick {
    fn decode(id: u32) -> Option<Self> {
        let index = (id & ID_INDEX_MASK) as usize;
        match id & !ID_INDEX_MASK {
            SPHERE_ID => Some(Self::Sphere(index)),
            BOND_ID => Some(Self::Bond(index)),
            _ => None,
        }
    }
}

/// Where the time of one frame went, measured on the CPU.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimings {
    /// Encoding and submitting the GPU work.
    pub encode: Duration,
    /// From submission until the pixels were mapped for reading.
    pub gpu_wait: Duration,
    /// Copying the mapped rows into a tightly packed buffer.
    pub copy: Duration,
}

/// A finished frame.
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    /// Tightly packed, opaque, sRGB-encoded BGRA rows.
    pub bgra: Vec<u8>,
    pub timings: FrameTimings,
}

/// Waits for a submission and the mapping of its readback buffer.
fn wait_for_map(
    device: &wgpu::Device,
    submission: wgpu::SubmissionIndex,
    mapped: &mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
) -> Result<(), RenderError> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|e| RenderError::Readback(e.to_string()))?;
    // The map callback runs inside `poll` once the copy has completed.
    mapped
        .recv()
        .map_err(|e| RenderError::Readback(e.to_string()))?
        .map_err(|e| RenderError::Readback(e.to_string()))
}

fn map_for_reading(buffer: &wgpu::Buffer) -> mpsc::Receiver<Result<(), wgpu::BufferAsyncError>> {
    let (sender, mapped) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    mapped
}

/// A submitted frame whose pixels are not yet on the CPU.
///
/// [`PendingFrame::wait`] blocks until the GPU has finished, so call it off
/// the UI thread.
pub struct PendingFrame {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    mapped: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    width: u32,
    height: u32,
    padded_row: u32,
    encode: Duration,
    submitted_at: Instant,
    pool: Arc<Mutex<Vec<wgpu::Buffer>>>,
    /// This frame's place in submission order, and how many frames the
    /// renderer has submitted so far.
    number: u64,
    submitted: Arc<AtomicU64>,
}

impl PendingFrame {
    /// Waits for the frame and copies its pixels.
    pub fn wait(self) -> Result<FrameImage, RenderError> {
        self.finish(false)
            .map(|frame| frame.expect("the copy is only skipped when asked"))
    }

    /// Like [`PendingFrame::wait`], but returns `None` without copying the
    /// pixels if a newer frame was submitted meanwhile. Frames finish in
    /// submission order, so the newer one would replace this one right away.
    pub fn wait_latest(self) -> Result<Option<FrameImage>, RenderError> {
        self.finish(true)
    }

    fn finish(self, skip_superseded: bool) -> Result<Option<FrameImage>, RenderError> {
        wait_for_map(&self.device, self.submission, &self.mapped)?;
        let gpu_wait = self.submitted_at.elapsed();
        let superseded = self.submitted.load(Ordering::Acquire) > self.number + 1;
        if skip_superseded && superseded {
            self.buffer.unmap();
            return_readback_buffer(&self.pool, self.buffer);
            return Ok(None);
        }

        let copy_started = Instant::now();
        let row = self.width as usize * 4;
        let padded = self.padded_row as usize;
        let len = row * self.height as usize;
        let mut bgra = Vec::with_capacity(len);
        {
            let mapped = self.buffer.slice(..).get_mapped_range();
            if padded == row {
                // Widths that are multiples of 64 pixels need no padding.
                bgra.extend_from_slice(&mapped[..len]);
            } else {
                for y in 0..self.height as usize {
                    let start = y * padded;
                    bgra.extend_from_slice(&mapped[start..start + row]);
                }
            }
        }
        self.buffer.unmap();
        let copy = copy_started.elapsed();
        return_readback_buffer(&self.pool, self.buffer);

        Ok(Some(FrameImage {
            width: self.width,
            height: self.height,
            bgra,
            timings: FrameTimings {
                encode: self.encode,
                gpu_wait,
                copy,
            },
        }))
    }
}

/// A submitted pick whose IDs are not yet on the CPU. Like
/// [`PendingFrame::wait`], [`PendingPick::wait`] blocks.
pub struct PendingPick {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    mapped: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    /// Size of the region read back.
    width: u32,
    height: u32,
    /// The pointer, relative to the region.
    target: (u32, u32),
}

impl PendingPick {
    /// The object nearest to the pointer within [`PICK_RADIUS`] pixels.
    pub fn wait(self) -> Result<Option<Pick>, RenderError> {
        wait_for_map(&self.device, self.submission, &self.mapped)?;
        let mut nearest: Option<(u32, Pick)> = None;
        {
            let mapped = self.buffer.slice(..).get_mapped_range();
            let ids: &[u32] = bytemuck::cast_slice(&mapped);
            let row = padded_bytes_per_row(self.width) / 4;
            for y in 0..self.height {
                for x in 0..self.width {
                    let Some(pick) = Pick::decode(ids[(y * row + x) as usize]) else {
                        continue;
                    };
                    let (dx, dy) = (x.abs_diff(self.target.0), y.abs_diff(self.target.1));
                    let distance = dx * dx + dy * dy;
                    if nearest.is_none_or(|(d, _)| distance < d) {
                        nearest = Some((distance, pick));
                    }
                }
            }
        }
        self.buffer.unmap();
        Ok(nearest.map(|(_, pick)| pick))
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    /// Width and height in physical pixels, world-space size of one pixel,
    /// physical pixels per logical pixel.
    viewport: [f32; 4],
}

impl CameraUniform {
    fn new(camera: &CameraMatrices, width: u32, height: u32, scale_factor: f32) -> Self {
        // For the orthographic projection, proj[1][1] is 1 / half-height.
        let world_per_pixel = 2.0 / (camera.proj.y_axis.y * height as f32);
        Self {
            view: camera.view.to_cols_array_2d(),
            proj: camera.proj.to_cols_array_2d(),
            viewport: [width as f32, height as f32, world_per_pixel, scale_factor],
        }
    }
}

struct Targets {
    width: u32,
    height: u32,
    msaa_view: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

/// The single-sample ID pass, kept until the camera, the scene or the size
/// changes so that hovering over a still image costs only a tiny readback.
struct IdTargets {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    /// The camera and scene generation the texture holds, if any.
    drawn: Option<(CameraMatrices, u64)>,
}

struct Instances {
    buffer: wgpu::Buffer,
    count: u32,
}

impl Instances {
    fn upload<T: Pod>(device: &wgpu::Device, label: &str, items: &[T]) -> Option<Self> {
        (!items.is_empty()).then(|| Self {
            buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(items),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            count: items.len() as u32,
        })
    }

    fn draw(this: &Option<Self>, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        if let Some(instances) = this {
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, instances.buffer.slice(..));
            pass.draw(0..6, 0..instances.count);
        }
    }
}

/// The scene's polyhedra, kept on the CPU so that their faces can be put in
/// back-to-front order for each view (`docs/design/viewport-rendering.md`
/// §5.6): polyhedra by the depth of their centroids, and within each, the
/// faces turned away from the camera before those turned toward it, which
/// is exact for a convex polyhedron.
struct Polyhedra {
    /// Each polyhedron's centroid and its faces in `faces`.
    groups: Vec<([f32; 3], std::ops::Range<usize>)>,
    /// Every face, polyhedron by polyhedron, and its normal.
    faces: Vec<TriangleInstance>,
    normals: Vec<[f32; 3]>,
    /// The faces in the order last uploaded; kept to spare an allocation
    /// per frame while the camera turns.
    ordered: Vec<TriangleInstance>,
    buffer: wgpu::Buffer,
    /// The view the buffer's order is for.
    ordered_for: Option<Mat4>,
}

impl Polyhedra {
    fn new(device: &wgpu::Device, scene: &Scene) -> Option<Self> {
        let mut groups = Vec::with_capacity(scene.polyhedra.len());
        let mut faces = Vec::new();
        let mut normals = Vec::new();
        for polyhedron in &scene.polyhedra {
            let corners: Vec<Vec3> = polyhedron
                .vertices
                .iter()
                .map(|v| Vec3::from_array(*v))
                .collect();
            if corners.is_empty() {
                continue;
            }
            let centroid = corners.iter().sum::<Vec3>() / corners.len() as f32;
            let start = faces.len();
            for triangle in &polyhedron.triangles {
                let corner = |i: u32| corners.get(i as usize).copied();
                let (Some(a), Some(b), Some(c)) = (
                    corner(triangle[0]),
                    corner(triangle[1]),
                    corner(triangle[2]),
                ) else {
                    continue;
                };
                let point = |p: Vec3| [p.x, p.y, p.z, 0.0];
                faces.push(TriangleInstance {
                    a: point(a),
                    b: point(b),
                    c: point(c),
                    color: polyhedron.color,
                });
                normals.push((b - a).cross(c - a).to_array());
            }
            groups.push((centroid.to_array(), start..faces.len()));
        }
        (!faces.is_empty()).then(|| Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("polyhedra"),
                size: (faces.len() * std::mem::size_of::<TriangleInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ordered: Vec::with_capacity(faces.len()),
            groups,
            faces,
            normals,
            ordered_for: None,
        })
    }

    /// Puts the faces in back-to-front order for `view`, unless they are
    /// in it already. Runs on the UI thread for every frame of a turning
    /// camera, so it keeps to plain loops (viewport design §8.6).
    fn order(&mut self, queue: &wgpu::Queue, view: Mat4) {
        if self.ordered_for == Some(view) {
            return;
        }
        // Depth along the view direction: the camera looks down −z, so the
        // farthest comes first in increasing z.
        let row = view.row(2);
        let toward = [row.x, row.y, row.z];
        let dot = |v: &[f32; 3]| toward[0] * v[0] + toward[1] * v[1] + toward[2] * v[2];
        let mut depths: Vec<(f32, usize)> = self
            .groups
            .iter()
            .enumerate()
            .map(|(i, (centroid, _))| (dot(centroid), i))
            .collect();
        depths.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        self.ordered.clear();
        for &(_, group) in &depths {
            let range = self.groups[group].1.clone();
            for facing in [false, true] {
                for i in range.clone() {
                    if (dot(&self.normals[i]) >= 0.0) == facing {
                        self.ordered.push(self.faces[i]);
                    }
                }
            }
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.ordered));
        self.ordered_for = Some(view);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        pass.draw(0..3, 0..self.faces.len() as u32);
    }
}

struct Pipelines {
    spheres: wgpu::RenderPipeline,
    bonds: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    polyhedra: wgpu::RenderPipeline,
    rings: wgpu::RenderPipeline,
    sphere_ids: wgpu::RenderPipeline,
    bond_ids: wgpu::RenderPipeline,
}

/// How one pipeline differs from the others.
struct PipelineSpec<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    vertex: &'a str,
    fragment: &'a str,
    stride: usize,
    attributes: &'a [wgpu::VertexAttribute],
    /// The ID pass: single-sample R32Uint instead of MSAA color.
    ids: bool,
    /// Rings are drawn over the scene without hiding anything behind them.
    depth_write: bool,
    /// Blended over what is behind, rather than covering the samples its
    /// alpha says; polyhedra.
    blend: bool,
}

impl Pipelines {
    fn new(device: &wgpu::Device, layout: &wgpu::PipelineLayout) -> Self {
        let shader = |label, body: &str| {
            let source = format!("{}\n{body}", include_str!("common.wgsl"));
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let sphere = shader("sphere.wgsl", include_str!("sphere.wgsl"));
        let bond = shader("bond.wgsl", include_str!("bond.wgsl"));
        let line = shader("line.wgsl", include_str!("line.wgsl"));
        let polyhedron = shader("polyhedron.wgsl", include_str!("polyhedron.wgsl"));
        let sphere_size = std::mem::size_of::<SphereInstance>();
        let pipeline = |spec: PipelineSpec| {
            let (format, samples, write_mask) = if spec.ids {
                (ID_FORMAT, 1, wgpu::ColorWrites::ALL)
            } else {
                // Alpha only drives coverage; keep the cleared opaque alpha.
                (COLOR_FORMAT, SAMPLE_COUNT, wgpu::ColorWrites::COLOR)
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(spec.label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: spec.module,
                    entry_point: Some(spec.vertex),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: spec.stride as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: spec.attributes,
                    }],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(spec.depth_write),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: samples > 1 && !spec.blend,
                },
                fragment: Some(wgpu::FragmentState {
                    module: spec.module,
                    entry_point: Some(spec.fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: spec.blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let spheres = |label, fragment, ids| PipelineSpec {
            label,
            module: &sphere,
            vertex: "vs_main",
            fragment,
            stride: sphere_size,
            attributes: &SPHERE_ATTRIBUTES,
            ids,
            depth_write: true,
            blend: false,
        };
        let bonds = |label, fragment, ids| PipelineSpec {
            label,
            module: &bond,
            vertex: "vs_main",
            fragment,
            stride: std::mem::size_of::<BondInstance>(),
            attributes: &BOND_ATTRIBUTES,
            ids,
            depth_write: true,
            blend: false,
        };
        Self {
            spheres: pipeline(spheres("spheres", "fs_main", false)),
            sphere_ids: pipeline(spheres("sphere ids", "fs_id", true)),
            bonds: pipeline(bonds("bonds", "fs_main", false)),
            bond_ids: pipeline(bonds("bond ids", "fs_id", true)),
            lines: pipeline(PipelineSpec {
                label: "lines",
                module: &line,
                vertex: "vs_main",
                fragment: "fs_main",
                stride: std::mem::size_of::<LineInstance>(),
                attributes: &LINE_ATTRIBUTES,
                ids: false,
                depth_write: true,
                blend: false,
            }),
            polyhedra: pipeline(PipelineSpec {
                label: "polyhedra",
                module: &polyhedron,
                vertex: "vs_main",
                fragment: "fs_main",
                stride: std::mem::size_of::<TriangleInstance>(),
                attributes: &TRIANGLE_ATTRIBUTES,
                ids: false,
                depth_write: false,
                blend: true,
            }),
            rings: pipeline(PipelineSpec {
                label: "highlight rings",
                vertex: "vs_ring",
                fragment: "fs_ring",
                stride: std::mem::size_of::<HighlightRing>(),
                depth_write: false,
                ..spheres("", "", false)
            }),
        }
    }
}

/// Renders impostor spheres and bonds offscreen, reads the pixels back, and
/// answers which object lies under a pixel.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    pipelines: Pipelines,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    spheres: Option<Instances>,
    bonds: Option<Instances>,
    lines: Option<Instances>,
    polyhedra: Option<Polyhedra>,
    rings: Option<Instances>,
    /// Counts scene changes, so a stale ID texture is redrawn.
    scene_generation: u64,
    /// Render targets by size, the most recently used last.
    targets: Vec<Targets>,
    id_targets: Option<IdTargets>,
    readback_pool: Arc<Mutex<Vec<wgpu::Buffer>>>,
    /// Frames submitted so far, shared with the pending frames so they can
    /// tell whether a newer one exists.
    frames_submitted: Arc<AtomicU64>,
}

impl Renderer {
    /// Creates a renderer on the high-performance adapter. Blocks while the
    /// device is requested.
    ///
    /// wgpu's environment variables narrow the choice: `WGPU_BACKEND`
    /// (`vulkan`, `dx12`, `metal`, `gl`), `WGPU_ADAPTER_NAME` (part of the
    /// adapter's name, any case), `WGPU_POWER_PREF` and the instance
    /// flags. CI pins the software adapters with them
    /// (`docs/development.md` §2.1).
    pub fn new() -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, RenderError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = match std::env::var("WGPU_ADAPTER_NAME") {
            Ok(name) if !name.is_empty() => {
                let wanted = name.to_lowercase();
                let adapters = instance.enumerate_adapters(wgpu::Backends::all()).await;
                let names: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();
                adapters
                    .into_iter()
                    .find(|a| a.get_info().name.to_lowercase().contains(&wanted))
                    .ok_or_else(|| {
                        RenderError::NoAdapter(format!(
                            "none is named like WGPU_ADAPTER_NAME={name}; there are {names:?}"
                        ))
                    })?
            }
            _ => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    force_fallback_adapter: false,
                    compatible_surface: None,
                })
                .await
                .map_err(|e| RenderError::NoAdapter(e.to_string()))?,
        };
        let info = adapter.get_info();
        log::info!(
            "renderer adapter: {} ({:?}, {:?})",
            info.name,
            info.backend,
            info.device_type
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("mw-crystal-render"),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Device(e.to_string()))?;

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });
        let pipelines = Pipelines::new(&device, &pipeline_layout);

        Ok(Self {
            device,
            queue,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            pipelines,
            camera_buffer,
            camera_bind_group,
            spheres: None,
            bonds: None,
            lines: None,
            polyhedra: None,
            rings: None,
            scene_generation: 0,
            targets: Vec::new(),
            id_targets: None,
            readback_pool: Arc::default(),
            frames_submitted: Arc::default(),
        })
    }

    /// Adapter name and backend, for diagnostics.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Replaces the scene and clears the highlights. Returns the scene's
    /// bounding radius in Å.
    pub fn set_scene(&mut self, scene: &Scene) -> f32 {
        self.spheres = Instances::upload(&self.device, "spheres", &scene.spheres);
        self.bonds = Instances::upload(&self.device, "bonds", &scene.bonds);
        self.lines = Instances::upload(&self.device, "lines", &scene.lines);
        self.polyhedra = Polyhedra::new(&self.device, scene);
        self.rings = None;
        self.scene_generation += 1;
        scene.bounding_radius()
    }

    /// Replaces the highlight rings. They are not pickable, so the ID
    /// texture stays valid.
    pub fn set_highlights(&mut self, rings: &[HighlightRing]) {
        self.rings = Instances::upload(&self.device, "highlight rings", rings);
    }

    fn write_camera(&self, camera: &CameraMatrices, width: u32, height: u32, scale_factor: f32) {
        let uniform = CameraUniform::new(camera, width, height, scale_factor);
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// Encodes and submits one frame. The returned frame must be waited on to
    /// get the pixels.
    pub fn render(&mut self, request: &RenderRequest) -> Result<PendingFrame, RenderError> {
        let (width, height) = (request.width, request.height);
        if width == 0 || height == 0 {
            return Err(RenderError::EmptyViewport);
        }
        let started = Instant::now();
        self.ensure_targets(width, height);
        let Some(targets) = self.targets.last() else {
            return Err(RenderError::EmptyViewport);
        };
        self.write_camera(&request.camera, width, height, request.scale_factor);
        if let Some(polyhedra) = &mut self.polyhedra {
            polyhedra.order(&self.queue, request.camera.view);
        }

        let padded_row = padded_bytes_per_row(width);
        let buffer = take_readback_buffer(
            &self.device,
            &self.readback_pool,
            padded_row as u64 * height as u64,
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport frame"),
            });
        {
            let [r, g, b, a] = request.clear_color;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&targets.resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            let pipelines = &self.pipelines;
            Instances::draw(&self.spheres, &mut pass, &pipelines.spheres);
            Instances::draw(&self.bonds, &mut pass, &pipelines.bonds);
            Instances::draw(&self.lines, &mut pass, &pipelines.lines);
            // Translucent, over everything opaque; rings stay on top.
            if let Some(polyhedra) = &self.polyhedra {
                polyhedra.draw(&mut pass, &pipelines.polyhedra);
            }
            Instances::draw(&self.rings, &mut pass, &pipelines.rings);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.resolve,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.queue.submit([encoder.finish()]);
        let mapped = map_for_reading(&buffer);
        let number = self.frames_submitted.fetch_add(1, Ordering::AcqRel);

        Ok(PendingFrame {
            device: self.device.clone(),
            buffer,
            submission,
            mapped,
            width,
            height,
            padded_row,
            encode: started.elapsed(),
            submitted_at: Instant::now(),
            pool: self.readback_pool.clone(),
            number,
            submitted: self.frames_submitted.clone(),
        })
    }

    /// Reads back the IDs around one pixel, drawing the ID pass first if
    /// the camera, size or scene changed since it was last drawn.
    pub fn pick(&mut self, request: &PickRequest) -> Result<PendingPick, RenderError> {
        let (width, height) = (request.width, request.height);
        if width == 0 || height == 0 || request.x >= width || request.y >= height {
            return Err(RenderError::EmptyViewport);
        }
        self.ensure_id_targets(width, height);
        let drawn = (request.camera, self.scene_generation);
        let stale = self
            .id_targets
            .as_ref()
            .is_some_and(|t| t.drawn != Some(drawn));
        if stale {
            // Widths do not matter: lines, rings and polyhedra are not
            // pickable.
            self.write_camera(&request.camera, width, height, 1.0);
        }
        let Some(targets) = &mut self.id_targets else {
            return Err(RenderError::EmptyViewport);
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport pick"),
            });
        if stale {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            Instances::draw(&self.spheres, &mut pass, &self.pipelines.sphere_ids);
            Instances::draw(&self.bonds, &mut pass, &self.pipelines.bond_ids);
            targets.drawn = Some(drawn);
        }

        let x0 = request.x.saturating_sub(PICK_RADIUS);
        let y0 = request.y.saturating_sub(PICK_RADIUS);
        let region_width = (request.x + PICK_RADIUS + 1).min(width) - x0;
        let region_height = (request.y + PICK_RADIUS + 1).min(height) - y0;
        let padded_row = padded_bytes_per_row(region_width);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick readback"),
            size: padded_row as u64 * region_height as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(region_height),
                },
            },
            wgpu::Extent3d {
                width: region_width,
                height: region_height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.queue.submit([encoder.finish()]);
        let mapped = map_for_reading(&buffer);

        Ok(PendingPick {
            device: self.device.clone(),
            buffer,
            submission,
            mapped,
            width: region_width,
            height: region_height,
            target: (request.x - x0, request.y - y0),
        })
    }

    fn texture(
        &self,
        label: &str,
        (width, height): (u32, u32),
        sample_count: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    /// Makes the targets for this size the last of `targets`, creating them
    /// if needed and dropping the least recently used size.
    fn ensure_targets(&mut self, width: u32, height: u32) {
        if let Some(ix) = self
            .targets
            .iter()
            .position(|t| t.width == width && t.height == height)
        {
            let targets = self.targets.remove(ix);
            self.targets.push(targets);
            return;
        }
        let size = (width, height);
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let msaa = self.texture(
            "viewport msaa",
            size,
            SAMPLE_COUNT,
            COLOR_FORMAT,
            attachment,
        );
        let resolve = self.texture(
            "viewport resolve",
            size,
            1,
            COLOR_FORMAT,
            attachment | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = self.texture(
            "viewport depth",
            size,
            SAMPLE_COUNT,
            DEPTH_FORMAT,
            attachment,
        );
        if self.targets.len() == TARGET_SIZES_KEPT {
            self.targets.remove(0);
        }
        self.targets.push(Targets {
            width,
            height,
            msaa_view: msaa.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            resolve,
        });
    }

    fn ensure_id_targets(&mut self, width: u32, height: u32) {
        if self
            .id_targets
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height)
        {
            return;
        }
        let size = (width, height);
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let texture = self.texture(
            "viewport ids",
            size,
            1,
            ID_FORMAT,
            attachment | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = self.texture("viewport id depth", size, 1, DEPTH_FORMAT, attachment);
        self.id_targets = Some(IdTargets {
            width,
            height,
            view: texture.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            texture,
            drawn: None,
        });
    }
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (width * 4).div_ceil(align) * align
}

/// Keeps a readback buffer for reuse, making room by dropping the oldest,
/// which may be sized for a viewport that has since changed size.
fn return_readback_buffer(pool: &Mutex<Vec<wgpu::Buffer>>, buffer: wgpu::Buffer) {
    if let Ok(mut pool) = pool.lock() {
        if pool.len() == READBACK_POOL_LIMIT {
            pool.remove(0);
        }
        pool.push(buffer);
    }
}

fn take_readback_buffer(
    device: &wgpu::Device,
    pool: &Mutex<Vec<wgpu::Buffer>>,
    size: u64,
) -> wgpu::Buffer {
    if let Ok(mut pool) = pool.lock()
        && let Some(ix) = pool.iter().position(|b| b.size() == size)
    {
        return pool.swap_remove(ix);
    }
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("viewport readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Converts one sRGB-encoded channel in 0..=1 to linear.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts one linear channel in 0..=1 to sRGB encoding.
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::OrbitCamera;

    #[test]
    fn rows_are_padded_to_copy_alignment() {
        assert_eq!(padded_bytes_per_row(1), 256);
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
    }

    #[test]
    fn srgb_conversion_hits_endpoints_and_round_trips() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        for c in [0.0, 0.02, 0.2, 0.5, 0.9, 1.0] {
            assert!((linear_to_srgb(srgb_to_linear(c)) - c).abs() < 1e-5, "{c}");
        }
    }

    #[test]
    fn pick_ids_round_trip() {
        assert_eq!(Pick::decode(0), None);
        assert_eq!(Pick::decode(SPHERE_ID | 7), Some(Pick::Sphere(7)));
        assert_eq!(Pick::decode(BOND_ID), Some(Pick::Bond(0)));
        assert_eq!(Pick::decode(3 << 30), None);
    }

    const SIZE: u32 = 65;

    /// A renderer, or `None` on machines without a GPU adapter: the test
    /// is skipped and says why. Where `MW_REQUIRE_GPU` is set, as in CI
    /// with its software adapters, a missing adapter fails the test.
    fn renderer() -> Option<Renderer> {
        match Renderer::new() {
            Ok(renderer) => Some(renderer),
            Err(e) if std::env::var_os("MW_REQUIRE_GPU").is_some_and(|v| v != "0") => {
                panic!("MW_REQUIRE_GPU is set, but there is no renderer: {e}")
            }
            Err(e) => {
                eprintln!("skipped, there is no renderer: {e} (MW_REQUIRE_GPU=1 fails instead)");
                None
            }
        }
    }

    fn submit(renderer: &mut Renderer, size: u32, camera: CameraMatrices) -> PendingFrame {
        renderer
            .render(&RenderRequest {
                width: size,
                height: size,
                camera,
                clear_color: [0.0, 0.0, 0.0, 1.0],
                scale_factor: 1.0,
            })
            .expect("frame")
    }

    fn frame(renderer: &mut Renderer, camera: CameraMatrices) -> FrameImage {
        submit(renderer, SIZE, camera).wait().expect("frame")
    }

    fn pick(renderer: &mut Renderer, camera: CameraMatrices, x: u32, y: u32) -> Option<Pick> {
        renderer
            .pick(&PickRequest {
                width: SIZE,
                height: SIZE,
                camera,
                x,
                y,
            })
            .and_then(PendingPick::wait)
            .expect("pick")
    }

    fn bgra(frame: &FrameImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        frame.bgra[i..i + 4].try_into().unwrap()
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    /// Renders one sphere and checks that it lands in the middle of the frame
    /// and not in the corner, and that picking finds it there.
    #[test]
    fn renders_and_picks_a_sphere_at_center() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let radius = renderer.set_scene(&Scene {
            spheres: vec![SphereInstance {
                center_radius: [0.0, 0.0, 0.0, 1.0],
                color: RED,
            }],
            ..Scene::default()
        });
        let camera = OrbitCamera::framing(radius).matrices(1.0);
        let frame = frame(&mut renderer, camera);

        assert_eq!(frame.bgra.len(), (SIZE * SIZE * 4) as usize);
        let center = bgra(&frame, 32, 32);
        assert!(center[2] > 100 && center[0] < 60, "center BGRA {center:?}");
        assert_eq!(bgra(&frame, 0, 0), [0, 0, 0, 255]);

        assert_eq!(pick(&mut renderer, camera, 32, 32), Some(Pick::Sphere(0)));
        assert_eq!(pick(&mut renderer, camera, 0, 0), None);
    }

    /// A bond seen side-on: red on the start half, blue on the end half,
    /// pickable along its length. The camera looks down −z.
    #[test]
    fn bonds_are_split_in_their_atoms_colors() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        renderer.set_scene(&Scene {
            bonds: vec![BondInstance {
                start_radius: [-2.0, 0.0, 0.0, 0.5],
                end: [2.0, 0.0, 0.0, 0.0],
                start_color: RED,
                end_color: BLUE,
            }],
            ..Scene::default()
        });
        let camera = CameraMatrices {
            view: glam::Mat4::look_at_rh(glam::Vec3::Z * 10.0, glam::Vec3::ZERO, glam::Vec3::Y),
            proj: glam::Mat4::orthographic_rh(-2.5, 2.5, -2.5, 2.5, 0.1, 20.0),
        };
        let frame = frame(&mut renderer, camera);
        let left = bgra(&frame, 16, 32);
        let right = bgra(&frame, 48, 32);
        assert!(left[2] > 100 && left[0] < 60, "left BGRA {left:?}");
        assert!(right[0] > 100 && right[2] < 60, "right BGRA {right:?}");
        // Above and below the cylinder: background.
        assert_eq!(bgra(&frame, 32, 20), [0, 0, 0, 255]);

        assert_eq!(pick(&mut renderer, camera, 20, 32), Some(Pick::Bond(0)));
        assert_eq!(pick(&mut renderer, camera, 32, 5), None);
    }

    /// The nearer of two overlapping objects wins, in the image and the pick.
    #[test]
    fn nearer_spheres_hide_and_win_picks() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let near = SphereInstance {
            center_radius: [0.0, 0.0, 1.0, 1.0],
            color: BLUE,
        };
        let far = SphereInstance {
            center_radius: [0.5, 0.0, -1.0, 1.5],
            color: RED,
        };
        renderer.set_scene(&Scene {
            spheres: vec![far, near],
            ..Scene::default()
        });
        let camera = CameraMatrices {
            view: glam::Mat4::look_at_rh(glam::Vec3::Z * 10.0, glam::Vec3::ZERO, glam::Vec3::Y),
            proj: glam::Mat4::orthographic_rh(-2.5, 2.5, -2.5, 2.5, 0.1, 20.0),
        };
        let center = bgra(&frame(&mut renderer, camera), 32, 32);
        assert!(center[0] > 100 && center[2] < 60, "center BGRA {center:?}");
        assert_eq!(pick(&mut renderer, camera, 32, 32), Some(Pick::Sphere(1)));
        // Right of the near sphere only the far one remains.
        assert_eq!(pick(&mut renderer, camera, 60, 32), Some(Pick::Sphere(0)));

        // A new scene invalidates the cached IDs.
        renderer.set_scene(&Scene {
            spheres: vec![near],
            ..Scene::default()
        });
        assert_eq!(pick(&mut renderer, camera, 60, 32), None);
    }

    /// Looking down −z from z = 10, five units across.
    fn from_front() -> CameraMatrices {
        CameraMatrices {
            view: glam::Mat4::look_at_rh(glam::Vec3::Z * 10.0, glam::Vec3::ZERO, glam::Vec3::Y),
            proj: glam::Mat4::orthographic_rh(-2.5, 2.5, -2.5, 2.5, 0.1, 20.0),
        }
    }

    /// A square facing the z axis at height `z`, from −`half` to `half`.
    fn square(z: f32, half: f32, color: [f32; 4]) -> crate::Polyhedron {
        crate::Polyhedron {
            vertices: vec![
                [-half, -half, z],
                [half, -half, z],
                [half, half, z],
                [-half, half, z],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            color,
        }
    }

    /// A translucent face over the background lets it show through; over a
    /// sphere behind it, both colors mix; a sphere in front hides it. It is
    /// not picked: the sphere behind is, and nothing where it covers only
    /// the background.
    #[test]
    fn polyhedra_are_seen_through_and_clicked_through() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let behind = SphereInstance {
            center_radius: [0.0, 0.0, -1.0, 0.8],
            color: BLUE,
        };
        let in_front = SphereInstance {
            center_radius: [1.5, 0.0, 2.0, 0.5],
            color: [0.0, 1.0, 0.0, 1.0],
        };
        renderer.set_scene(&Scene {
            spheres: vec![behind, in_front],
            polyhedra: vec![square(1.0, 2.0, [1.0, 0.0, 0.0, 0.5])],
            ..Scene::default()
        });
        let camera = from_front();
        let frame = frame(&mut renderer, camera);

        // Over the background: red, but not as red as the face itself; the
        // highlight adds as much green as blue.
        let over_background = bgra(&frame, 12, 32);
        let [b, g, r, _] = over_background;
        assert!(
            (60..235).contains(&r) && r > b + 80 && b == g,
            "BGRA {over_background:?}"
        );
        // The frame stays opaque.
        assert_eq!(over_background[3], 255);
        // Over the blue sphere: blue through red.
        let over_sphere = bgra(&frame, 32, 32);
        assert!(
            over_sphere[0] > 60 && over_sphere[2] > 60,
            "BGRA {over_sphere:?}"
        );
        // The green sphere is in front of the face: no red on it, as much
        // red as blue from its highlight.
        let green = bgra(&frame, 52, 32);
        assert!(green[1] > 100 && green[2] == green[0], "BGRA {green:?}");
        // Beyond the face, the background.
        assert_eq!(bgra(&frame, 1, 1), [0, 0, 0, 255]);

        assert_eq!(pick(&mut renderer, camera, 32, 32), Some(Pick::Sphere(0)));
        assert_eq!(pick(&mut renderer, camera, 12, 32), None);
    }

    /// Faces are blended from the farthest to the nearest whatever their
    /// order in the scene, and again when the camera turns round.
    #[test]
    fn polyhedra_are_drawn_back_to_front() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        renderer.set_scene(&Scene {
            polyhedra: vec![
                square(1.0, 2.0, [1.0, 0.0, 0.0, 0.5]),
                square(-1.0, 2.0, [0.0, 0.0, 1.0, 0.5]),
            ],
            ..Scene::default()
        });
        // The near face counts twice as much as the far one.
        let front = bgra(&frame(&mut renderer, from_front()), 32, 32);
        assert!(front[2] > front[0] + 30, "from the front BGRA {front:?}");

        let behind = CameraMatrices {
            view: glam::Mat4::look_at_rh(glam::Vec3::Z * -10.0, glam::Vec3::ZERO, glam::Vec3::Y),
            ..from_front()
        };
        let back = bgra(&frame(&mut renderer, behind), 32, 32);
        assert!(back[0] > back[2] + 30, "from behind BGRA {back:?}");
        // Back to the front: the order follows the camera.
        let again = bgra(&frame(&mut renderer, from_front()), 32, 32);
        assert_eq!(again, front);
    }

    /// Viewport design §8.6: what translucent polyhedra cost per frame while
    /// the camera turns, each frame sorting them again. 2331 octahedra and
    /// their edges among 7897 spheres, as 10 × 10 × 10 rutile cells in the
    /// polyhedra model, against the spheres alone. A measurement, not a
    /// check:
    ///
    /// cargo test -p mw-crystal-render --release frame_timings -- --ignored --nocapture
    #[test]
    #[ignore = "a measurement that prints frame timings"]
    fn polyhedra_frame_timings() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let spheres = crate::synthetic_lattice(7897);
        let polyhedra: Vec<crate::Polyhedron> = crate::synthetic_polyhedra(&spheres, 3)
            .into_iter()
            .take(2331)
            .collect();
        let mut lines = Vec::new();
        for polyhedron in &polyhedra {
            let mut edges = std::collections::BTreeSet::new();
            for t in &polyhedron.triangles {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    edges.insert((a.min(b), a.max(b)));
                }
            }
            for (a, b) in edges {
                let [x, y, z] = polyhedron.vertices[a as usize];
                let [ex, ey, ez] = polyhedron.vertices[b as usize];
                lines.push(LineInstance {
                    start_width: [x, y, z, 1.0],
                    end_dash: [ex, ey, ez, 0.0],
                    color: [0.2, 0.2, 0.2, 1.0],
                });
            }
        }
        let loads = [
            ("spheres", Vec::new(), Vec::new()),
            ("polyhedra", polyhedra, lines),
        ];
        for (name, polyhedra, lines) in loads {
            let count = polyhedra.len();
            let radius = renderer.set_scene(&Scene {
                spheres: spheres.clone(),
                lines,
                polyhedra,
                ..Scene::default()
            });
            for (width, height) in [(896, 800), (1792, 1600)] {
                let mut camera = OrbitCamera::framing(radius);
                let (mut encode, mut wait, frames) = (Duration::ZERO, Duration::ZERO, 30);
                for _ in 0..frames {
                    camera.rotate(0.03, 0.01);
                    let request = RenderRequest {
                        width,
                        height,
                        camera: camera.matrices(width as f32 / height as f32),
                        clear_color: [0.9, 0.9, 0.9, 1.0],
                        scale_factor: 2.0,
                    };
                    let frame = renderer.render(&request).unwrap().wait().unwrap();
                    encode += frame.timings.encode;
                    wait += frame.timings.gpu_wait;
                }
                eprintln!(
                    "{name} ({count} polyhedra) {width}x{height}: encode and submit {:.2} ms, \
                     GPU and readback {:.2} ms per frame on {}",
                    encode.as_secs_f64() * 1e3 / f64::from(frames),
                    wait.as_secs_f64() * 1e3 / f64::from(frames),
                    renderer.adapter_name()
                );
            }
        }
    }

    /// A frame overtaken by a newer submission is not copied; the newest
    /// always is. Sizes may alternate without losing either's targets.
    #[test]
    fn superseded_frames_skip_the_copy() {
        let Some(mut renderer) = renderer() else {
            return;
        };
        let radius = renderer.set_scene(&Scene {
            spheres: vec![SphereInstance {
                center_radius: [0.0, 0.0, 0.0, 1.0],
                color: RED,
            }],
            ..Scene::default()
        });
        let camera = OrbitCamera::framing(radius).matrices(1.0);
        let older = submit(&mut renderer, SIZE, camera);
        let newer = submit(&mut renderer, SIZE / 2, camera);
        assert!(older.wait_latest().expect("older").is_none());
        let newer = newer
            .wait_latest()
            .expect("newer")
            .expect("the newest frame");
        assert_eq!(newer.width, SIZE / 2);

        let full = submit(&mut renderer, SIZE, camera).wait_latest();
        let full = full.expect("full").expect("the newest frame");
        assert_eq!(full.bgra.len(), (SIZE * SIZE * 4) as usize);
        assert_eq!(renderer.targets.len(), TARGET_SIZES_KEPT);
        // A plain wait copies even a superseded frame.
        let older = submit(&mut renderer, SIZE, camera);
        let _newer = submit(&mut renderer, SIZE, camera);
        assert_eq!(older.wait().expect("older").width, SIZE);
    }
}
