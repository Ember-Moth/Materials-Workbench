// Orthographic impostor cylinders for bonds. Each instance is a quad around
// the cylinder's projection; the fragment shader intersects the view ray
// with the cylinder, writes the true depth and shades each half in the color
// of the atom at its end. The ends are open: they lie inside the atoms.
// Prefixed with common.wgsl.

struct Instance {
    @location(0) start_radius: vec4<f32>,
    @location(1) end: vec4<f32>,
    @location(2) start_color: vec4<f32>,
    @location(3) end_color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    // View-space xy of the fragment.
    @location(0) plane: vec2<f32>,
    // Signed screen-plane distance from the projected axis, in world units.
    @location(1) across: f32,
    @location(2) @interpolate(flat) start: vec3<f32>,
    @location(3) @interpolate(flat) end: vec3<f32>,
    @location(4) @interpolate(flat) radius: f32,
    @location(5) @interpolate(flat) start_color: vec3<f32>,
    @location(6) @interpolate(flat) end_color: vec3<f32>,
    @location(7) @interpolate(flat) id: u32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
    instance: Instance,
) -> VsOut {
    // x runs from start to end, y across the axis.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_index];
    let radius = instance.start_radius.w;
    let start = (camera.view * vec4<f32>(instance.start_radius.xyz, 1.0)).xyz;
    let end = (camera.view * vec4<f32>(instance.end.xyz, 1.0)).xyz;

    // An orthographic cylinder projects to a strip of half-width `radius`
    // around the projected axis; the tilted ends reach at most `radius`
    // beyond the projected end points. Pad for the coverage band.
    let axis = end.xy - start.xy;
    let projected = length(axis);
    var along_axis = vec2<f32>(1.0, 0.0);
    if projected > 1e-6 {
        along_axis = axis / projected;
    }
    let normal = vec2<f32>(-along_axis.y, along_axis.x);
    let pad = radius + 0.75 * camera.viewport.z;
    let along = mix(-pad, projected + pad, corner.x);
    let across = corner.y * pad;
    let plane = start.xy + along_axis * along + normal * across;
    let z = max(start.z, end.z) + radius;

    var out: VsOut;
    out.position = camera.proj * vec4<f32>(plane, z, 1.0);
    out.plane = plane;
    out.across = across;
    out.start = start;
    out.end = end;
    out.radius = radius;
    out.start_color = instance.start_color.rgb;
    out.end_color = instance.end_color.rgb;
    out.id = BOND_ID | instance_index;
    return out;
}

struct Hit {
    // View-space surface point.
    position: vec3<f32>,
    normal: vec3<f32>,
    // 0 at the start, 1 at the end.
    along: f32,
    found: bool,
};

// The nearest surface point on the view ray through the fragment. Outside
// the silhouette (the coverage band) the ray is moved onto the silhouette.
fn intersect(in: VsOut) -> Hit {
    var hit: Hit;
    hit.found = false;
    let axis = in.end - in.start;
    let axis_length = length(axis);
    let w = axis / max(axis_length, 1e-6);
    // The view ray is (plane, z) for all z; solve
    // |q|² − (q·w)² = r² with q = e + (0, 0, z) relative to the start.
    let e = vec3<f32>(in.plane, 0.0) - in.start;
    let ew = dot(e, w);
    let a = 1.0 - w.z * w.z;
    // Seen end-on, the cylinder hides inside its atoms.
    if a < 1e-5 {
        return hit;
    }
    let b = e.z - ew * w.z;
    let c = dot(e, e) - ew * ew - in.radius * in.radius;
    let z = (-b + sqrt(max(b * b - a * c, 0.0))) / a;
    let q = e + vec3<f32>(0.0, 0.0, z);
    let t = dot(q, w);
    if t < 0.0 || t > axis_length {
        return hit;
    }
    hit.position = in.start + q;
    hit.normal = normalize(q - t * w);
    hit.along = t / axis_length;
    hit.found = true;
    return hit;
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let edge = max(fwidth(in.across), 1e-6);
    let coverage = clamp((in.radius - abs(in.across)) / edge + 0.5, 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }
    let hit = intersect(in);
    if !hit.found {
        discard;
    }
    var color = in.end_color;
    if hit.along < 0.5 {
        color = in.start_color;
    }
    var out: FsOut;
    out.color = vec4<f32>(shade(color, hit.normal), coverage);
    out.depth = depth_of(hit.position);
    return out;
}

@fragment
fn fs_id(in: VsOut) -> IdOut {
    if abs(in.across) > in.radius {
        discard;
    }
    let hit = intersect(in);
    if !hit.found {
        discard;
    }
    var out: IdOut;
    out.id = in.id;
    out.depth = depth_of(hit.position);
    return out;
}
