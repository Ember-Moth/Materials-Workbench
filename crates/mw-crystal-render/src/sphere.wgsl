// Orthographic impostor spheres: each instance is a camera-facing quad, and
// the fragment shader solves the sphere surface, writes its true depth, and
// shades it. Edge coverage comes from alpha-to-coverage with MSAA.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct Instance {
    @location(0) center_radius: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) center_view: vec3<f32>,
    @location(2) radius: f32,
    @location(3) color: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let radius = instance.center_radius.w;
    let center_view = (camera.view * vec4<f32>(instance.center_radius.xyz, 1.0)).xyz;
    // Slightly larger than the sphere so the silhouette's partial-coverage
    // band is not cut off by the quad edge.
    let quad = corner * 1.05;
    let position_view = center_view + vec3<f32>(quad * radius, 0.0);

    var out: VsOut;
    out.position = camera.proj * vec4<f32>(position_view, 1.0);
    out.local = quad;
    out.center_view = center_view;
    out.radius = radius;
    out.color = instance.color.rgb;
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let d2 = dot(in.local, in.local);
    let d = sqrt(d2);
    // Coverage falls off over about one pixel at the silhouette.
    let edge = max(fwidth(d), 1e-4);
    let coverage = clamp((1.0 - d) / edge + 0.5, 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }

    let z = sqrt(max(1.0 - min(d2, 1.0), 0.0));
    let normal = vec3<f32>(in.local / max(d, 1.0), z);
    let surface_view = in.center_view + normal * in.radius;
    let clip = camera.proj * vec4<f32>(surface_view, 1.0);

    let light = normalize(vec3<f32>(0.4, 0.6, 1.0));
    let diffuse = max(dot(normal, light), 0.0);
    let half_vector = normalize(light + vec3<f32>(0.0, 0.0, 1.0));
    let specular = pow(max(dot(normal, half_vector), 0.0), 48.0) * 0.35;
    let shaded = in.color * (0.22 + 0.78 * diffuse) + vec3<f32>(specular);

    var out: FsOut;
    out.color = vec4<f32>(shaded, coverage);
    out.depth = clip.z / clip.w;
    return out;
}
