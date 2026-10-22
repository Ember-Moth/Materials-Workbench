// Orthographic impostor spheres: each instance is a camera-facing quad, and
// the fragment shader solves the sphere surface, writes its true depth, and
// shades it. Edge coverage comes from alpha-to-coverage with MSAA.
//
// The same quads draw highlight rings just outside a sphere's silhouette,
// and write instance IDs for picking. Prefixed with common.wgsl.

struct Instance {
    @location(0) center_radius: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    // Position on the quad in sphere radii.
    @location(0) local: vec2<f32>,
    @location(1) center_view: vec3<f32>,
    @location(2) radius: f32,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) id: u32,
};

fn quad(vertex_index: u32, instance: Instance, extent: f32, id: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let local = corners[vertex_index] * extent;
    let radius = instance.center_radius.w;
    let center_view = (camera.view * vec4<f32>(instance.center_radius.xyz, 1.0)).xyz;
    let position_view = center_view + vec3<f32>(local * radius, 0.0);

    var out: VsOut;
    out.position = camera.proj * vec4<f32>(position_view, 1.0);
    out.local = local;
    out.center_view = center_view;
    out.radius = radius;
    out.color = instance.color;
    out.id = id;
    return out;
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
    instance: Instance,
) -> VsOut {
    // Room for the partial-coverage band, which ends half a pixel outside
    // the silhouette. No more: for atoms a few pixels wide, padding is most
    // of the quad.
    let extent = 1.0 + 0.75 * camera.viewport.z / instance.center_radius.w;
    return quad(vertex_index, instance, extent, SPHERE_ID | instance_index);
}

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

    var out: FsOut;
    out.color = vec4<f32>(shade(in.color.rgb, normal), coverage);
    out.depth = depth_of(in.center_view + normal * in.radius);
    return out;
}

@fragment
fn fs_id(in: VsOut) -> IdOut {
    let d2 = dot(in.local, in.local);
    if d2 > 1.0 {
        discard;
    }
    var out: IdOut;
    out.id = in.id;
    out.depth = depth_of(in.center_view + vec3<f32>(in.local, sqrt(1.0 - d2)) * in.radius);
    return out;
}

// Highlight rings. The instance color's w is the ring width in logical
// pixels.

@vertex
fn vs_ring(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VsOut {
    let pixels = instance.color.w * camera.viewport.w + 0.75;
    let extent = 1.0 + pixels * camera.viewport.z / instance.center_radius.w;
    return quad(vertex_index, instance, extent, 0u);
}

@fragment
fn fs_ring(in: VsOut) -> FsOut {
    let d = length(in.local);
    let pixel = camera.viewport.z / in.radius;
    // Overlap the silhouette by half a pixel so no background shows between
    // the sphere's soft edge and the ring.
    let inner = 1.0 - 0.5 * pixel;
    let outer = 1.0 + in.color.w * camera.viewport.w * pixel;
    let edge = max(fwidth(d), 1e-4);
    let coverage = min(
        clamp((outer - d) / edge + 0.5, 0.0, 1.0),
        clamp((d - inner) / edge + 0.5, 0.0, 1.0),
    );
    if coverage <= 0.0 {
        discard;
    }
    var out: FsOut;
    out.color = vec4<f32>(in.color.rgb, coverage);
    // At the sphere's nearest point, so bonds leaving the sphere pass behind
    // the ring while nearer atoms still cover it.
    out.depth = depth_of(in.center_view + vec3<f32>(0.0, 0.0, in.radius));
    return out;
}
