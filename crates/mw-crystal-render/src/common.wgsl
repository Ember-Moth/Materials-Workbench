// Declarations shared by the scene shaders; prepended to each of them.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // Render target size in physical pixels (xy), the world-space size of one
    // pixel (z), and physical pixels per logical pixel (w), which scales line
    // and ring widths.
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Pick IDs: kind in the top two bits, instance index below; 0 is nothing.
// Must match `PickId` in renderer.rs.
const SPHERE_ID: u32 = 0x40000000u;
const BOND_ID: u32 = 0x80000000u;

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

struct IdOut {
    @location(0) id: u32,
    @builtin(frag_depth) depth: f32,
};

fn depth_of(position_view: vec3<f32>) -> f32 {
    let clip = camera.proj * vec4<f32>(position_view, 1.0);
    return clip.z / clip.w;
}

// Soft diffuse with a small specular highlight.
fn shade(color: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let light = normalize(vec3<f32>(0.4, 0.6, 1.0));
    let diffuse = max(dot(normal, light), 0.0);
    let half_vector = normalize(light + vec3<f32>(0.0, 0.0, 1.0));
    let specular = pow(max(dot(normal, half_vector), 0.0), 48.0) * 0.35;
    return color * (0.22 + 0.78 * diffuse) + vec3<f32>(specular);
}
