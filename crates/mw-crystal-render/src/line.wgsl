// Lines of constant screen width, such as unit-cell edges: each instance is
// a quad around the projected segment, depth-tested like the rest of the
// scene. MSAA smooths the edges. Prefixed with common.wgsl.

struct Instance {
    @location(0) start_width: vec4<f32>,
    @location(1) end_dash: vec4<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    // Physical pixels from the start along the line.
    @location(0) along: f32,
    @location(1) @interpolate(flat) dash: f32,
    @location(2) @interpolate(flat) color: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VsOut {
    // x runs from start to end, y across the line.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_index];
    let clip_start = camera.proj * camera.view * vec4<f32>(instance.start_width.xyz, 1.0);
    let clip_end = camera.proj * camera.view * vec4<f32>(instance.end_dash.xyz, 1.0);

    // Offsets are computed in pixels, then converted back to clip space.
    let half_size = camera.viewport.xy * 0.5;
    let screen = (clip_end.xy / clip_end.w - clip_start.xy / clip_start.w) * half_size;
    let screen_length = length(screen);
    var along = vec2<f32>(1.0, 0.0);
    if screen_length > 1e-4 {
        along = screen / screen_length;
    }
    let across = vec2<f32>(-along.y, along.x);
    let half_width = instance.start_width.w * camera.viewport.w * 0.5;
    // Square caps: extend half a width past each end, so the corners of the
    // cell close.
    let extend = (corner.x * 2.0 - 1.0) * half_width;
    let offset = along * extend + across * corner.y * half_width;
    let base = mix(clip_start, clip_end, corner.x);

    var out: VsOut;
    out.position = base + vec4<f32>(offset / half_size * base.w, 0.0, 0.0);
    out.along = corner.x * screen_length + extend;
    out.dash = instance.end_dash.w * camera.viewport.w;
    out.color = instance.color.rgb;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if in.dash > 0.0 && fract(in.along / (2.0 * in.dash)) >= 0.5 {
        discard;
    }
    return vec4<f32>(in.color, 1.0);
}
