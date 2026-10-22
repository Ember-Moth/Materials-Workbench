use std::fmt;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::camera::CameraMatrices;
use crate::scene::{SphereInstance, bounding_radius};

/// sRGB so that the bytes read back match what GPUI expects from a decoded
/// image: sRGB-encoded BGRA.
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLE_COUNT: u32 = 4;
/// Readback buffers kept for reuse; a few frames can be in flight at once.
const READBACK_POOL_LIMIT: usize = 4;

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
}

impl PendingFrame {
    pub fn wait(self) -> Result<FrameImage, RenderError> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission),
                timeout: None,
            })
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        // The map callback runs inside `poll` once the copy has completed.
        self.mapped
            .recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;
        let gpu_wait = self.submitted_at.elapsed();

        let copy_started = Instant::now();
        let row = self.width as usize * 4;
        let padded = self.padded_row as usize;
        let mut bgra = Vec::with_capacity(row * self.height as usize);
        {
            let mapped = self.buffer.slice(..).get_mapped_range();
            for y in 0..self.height as usize {
                let start = y * padded;
                bgra.extend_from_slice(&mapped[start..start + row]);
            }
        }
        self.buffer.unmap();
        let copy = copy_started.elapsed();

        if let Ok(mut pool) = self.pool.lock()
            && pool.len() < READBACK_POOL_LIMIT
        {
            pool.push(self.buffer);
        }

        Ok(FrameImage {
            width: self.width,
            height: self.height,
            bgra,
            timings: FrameTimings {
                encode: self.encode,
                gpu_wait,
                copy,
            },
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
}

struct Targets {
    width: u32,
    height: u32,
    msaa_view: wgpu::TextureView,
    resolve: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

struct SphereBuffer {
    buffer: wgpu::Buffer,
    count: u32,
}

/// Renders impostor spheres offscreen and reads the pixels back.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    spheres: Option<SphereBuffer>,
    targets: Option<Targets>,
    readback_pool: Arc<Mutex<Vec<wgpu::Buffer>>>,
}

impl Renderer {
    /// Creates a renderer on the high-performance adapter. Blocks while the
    /// device is requested.
    pub fn new() -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("mw-crystal-render"),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Device(e.to_string()))?;

        let shader = device.create_shader_module(wgpu::include_wgsl!("sphere.wgsl"));
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
            label: Some("spheres"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spheres"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SphereInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                mask: !0,
                alpha_to_coverage_enabled: true,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    // Alpha only drives coverage; keep the cleared opaque alpha.
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            pipeline,
            camera_buffer,
            camera_bind_group,
            spheres: None,
            targets: None,
            readback_pool: Arc::default(),
        })
    }

    /// Adapter name and backend, for diagnostics.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Replaces the scene. Returns the scene's bounding radius in Å.
    pub fn set_spheres(&mut self, spheres: &[SphereInstance]) -> f32 {
        self.spheres = (!spheres.is_empty()).then(|| SphereBuffer {
            buffer: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("spheres"),
                    contents: bytemuck::cast_slice(spheres),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
            count: spheres.len() as u32,
        });
        bounding_radius(spheres)
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
        let Some(targets) = &self.targets else {
            return Err(RenderError::EmptyViewport);
        };

        let uniform = CameraUniform {
            view: request.camera.view.to_cols_array_2d(),
            proj: request.camera.proj.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

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
                label: Some("spheres"),
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
            if let Some(spheres) = &self.spheres {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_vertex_buffer(0, spheres.buffer.slice(..));
                pass.draw(0..6, 0..spheres.count);
            }
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

        let (sender, mapped) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });

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
        })
    }

    fn ensure_targets(&mut self, width: u32, height: u32) {
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.width == width && t.height == height)
        {
            return;
        }
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = |label, sample_count, format, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = texture(
            "viewport msaa",
            SAMPLE_COUNT,
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let resolve = texture(
            "viewport resolve",
            1,
            COLOR_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = texture(
            "viewport depth",
            SAMPLE_COUNT,
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        self.targets = Some(Targets {
            width,
            height,
            msaa_view: msaa.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            depth_view: depth.create_view(&Default::default()),
            resolve,
        });
        // Buffers sized for the old viewport are useless now.
        if let Ok(mut pool) = self.readback_pool.lock() {
            pool.clear();
        }
    }
}

fn padded_bytes_per_row(width: u32) -> u32 {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (width * 4).div_ceil(align) * align
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
    fn srgb_conversion_hits_endpoints() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    }

    /// Renders one sphere and checks that it lands in the middle of the frame
    /// and not in the corner. Needs a GPU; skipped when none is available.
    #[test]
    fn renders_sphere_at_center() {
        let mut renderer = match Renderer::new() {
            Ok(renderer) => renderer,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let sphere = SphereInstance {
            center_radius: [0.0, 0.0, 0.0, 1.0],
            color: [1.0, 0.0, 0.0, 1.0],
        };
        let radius = renderer.set_spheres(&[sphere]);
        let camera = OrbitCamera::framing(radius).matrices(1.0);
        let frame = renderer
            .render(&RenderRequest {
                width: 65,
                height: 65,
                camera,
                clear_color: [0.0, 0.0, 0.0, 1.0],
            })
            .and_then(PendingFrame::wait)
            .expect("frame");

        assert_eq!(frame.bgra.len(), 65 * 65 * 4);
        let pixel = |x: usize, y: usize| {
            let i = (y * 65 + x) * 4;
            &frame.bgra[i..i + 4]
        };
        let center = pixel(32, 32);
        assert!(center[2] > 100 && center[0] < 60, "center BGRA {center:?}");
        assert_eq!(pixel(0, 0), &[0, 0, 0, 255]);
    }
}
