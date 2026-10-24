// Translucent polyhedra: one instance per triangle, flat-shaded in the
// color of its polyhedron and blended over what is behind it. Drawn after
// the opaque scene, in the back-to-front order the renderer sorts them in,
// depth-tested but not writing depth. Prefixed with common.wgsl.

struct Instance {
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
    @location(2) c: vec4<f32>,
    // Linear RGB and opacity.
    @location(3) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) normal_view: vec3<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VsOut {
    var corners = array<vec3<f32>, 3>(instance.a.xyz, instance.b.xyz, instance.c.xyz);
    let normal = cross(instance.b.xyz - instance.a.xyz, instance.c.xyz - instance.a.xyz);
    let normal_view = (camera.view * vec4<f32>(normal, 0.0)).xyz;

    var out: VsOut;
    out.position = camera.proj * camera.view * vec4<f32>(corners[vertex_index], 1.0);
    out.normal_view = normal_view / max(length(normal_view), 1e-12);
    out.color = instance.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Lit from the side that faces the camera: the inside of the far faces
    // shows through the near ones, and flat polygons have two sides.
    var normal = in.normal_view;
    if normal.z < 0.0 {
        normal = -normal;
    }
    return vec4<f32>(shade(in.color.rgb, normal), in.color.a);
}
