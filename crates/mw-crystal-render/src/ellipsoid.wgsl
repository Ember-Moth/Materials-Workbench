// Orthographic impostor ellipsoids, such as atoms' displacement ellipsoids:
// the points center + M·u with |u| ≤ 1, where the columns of M are the
// semi-axes. Each instance is a screen-aligned quad around the projected
// ellipse; the fragment shader solves the ray in the unit sphere's space
// u = M⁻¹·(r − center), writes the true depth and shades with the normal
// M⁻ᵀ·u. Edge coverage comes from alpha-to-coverage with MSAA, as for
// spheres. Optionally darkens the three principal ellipses (u_i = 0), as
// ORTEP drawings do. Prefixed with common.wgsl.

struct Instance {
    @location(0) center_radius: vec4<f32>,
    // Semi-axes in Å (xyz); the first's w is 1 to draw the principal
    // ellipses.
    @location(1) axis_a: vec4<f32>,
    @location(2) axis_b: vec4<f32>,
    @location(3) axis_c: vec4<f32>,
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    // Offset from the center on the view plane, in Å.
    @location(0) offset: vec2<f32>,
    @location(1) @interpolate(flat) center_view: vec3<f32>,
    // Rows of M⁻¹ in view space.
    @location(2) @interpolate(flat) inverse_0: vec3<f32>,
    @location(3) @interpolate(flat) inverse_1: vec3<f32>,
    @location(4) @interpolate(flat) inverse_2: vec3<f32>,
    @location(5) @interpolate(flat) color: vec4<f32>,
    @location(6) @interpolate(flat) id: u32,
    @location(7) @interpolate(flat) outlined: u32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
    instance: Instance,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let rotation = mat3x3<f32>(camera.view[0].xyz, camera.view[1].xyz, camera.view[2].xyz);
    let m = rotation * mat3x3<f32>(instance.axis_a.xyz, instance.axis_b.xyz, instance.axis_c.xyz);
    // The projected ellipse spans √(Σ m_xi²) along x and likewise along y;
    // plus room for the partial-coverage band half a pixel outside it.
    let half = vec2<f32>(
        length(vec3<f32>(m[0].x, m[1].x, m[2].x)),
        length(vec3<f32>(m[0].y, m[1].y, m[2].y)),
    ) + vec2<f32>(0.75 * camera.viewport.z);
    let offset = corners[vertex_index] * half;
    let center_view = (camera.view * vec4<f32>(instance.center_radius.xyz, 1.0)).xyz;

    // M⁻¹ = adj(M) / det(M): its rows are the cross products of M's columns.
    let det = dot(m[0], cross(m[1], m[2]));
    let scale = 1.0 / select(det, 1e-30, abs(det) < 1e-30);

    var out: VsOut;
    out.position = camera.proj * vec4<f32>(center_view + vec3<f32>(offset, 0.0), 1.0);
    out.offset = offset;
    out.center_view = center_view;
    out.inverse_0 = cross(m[1], m[2]) * scale;
    out.inverse_1 = cross(m[2], m[0]) * scale;
    out.inverse_2 = cross(m[0], m[1]) * scale;
    out.color = instance.color;
    out.id = ELLIPSOID_ID | instance_index;
    out.outlined = select(0u, 1u, instance.axis_a.w > 0.5);
    return out;
}

fn to_unit(in: VsOut, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(in.inverse_0, v), dot(in.inverse_1, v), dot(in.inverse_2, v));
}

struct Hit {
    // The point in the unit sphere's space, and its view-space offset from
    // the center along z.
    u: vec3<f32>,
    z: f32,
    // Distance from the center of the projected ellipse, 1 on its outline.
    rho: f32,
};

// The ray through the fragment toward the camera: the point o + t·d of the
// unit sphere with the largest t, which is nearest the camera.
fn hit(in: VsOut) -> Hit {
    let o = to_unit(in, vec3<f32>(in.offset, 0.0));
    let d = to_unit(in, vec3<f32>(0.0, 0.0, 1.0));
    let a = dot(d, d);
    let b = dot(o, d);
    let c = dot(o, o);
    // The ray's closest approach to the center of the unit sphere.
    let rho2 = max(c - b * b / a, 0.0);
    let t = (-b + sqrt(max(b * b - a * (c - 1.0), 0.0))) / a;
    var h: Hit;
    h.u = o + d * t;
    h.z = t;
    h.rho = sqrt(rho2);
    return h;
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let h = hit(in);
    // Derivatives before any branch: they need uniform control flow.
    let edge = max(fwidth(h.rho), 1e-4);
    let du = max(fwidth(h.u), vec3<f32>(1e-6));
    let coverage = clamp((1.0 - h.rho) / edge + 0.5, 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }
    // The gradient of |M⁻¹·r|² is M⁻ᵀ·u.
    let normal = normalize(in.inverse_0 * h.u.x + in.inverse_1 * h.u.y + in.inverse_2 * h.u.z);
    var color = shade(in.color.rgb, normal);
    if in.outlined == 1u {
        // A line about one logical pixel wide where a coordinate of u is 0.
        let half_width = 0.6 * camera.viewport.w;
        let distance = abs(h.u) / du;
        let ink = clamp(half_width - min(distance.x, min(distance.y, distance.z)) + 0.5, 0.0, 1.0);
        color = mix(color, color * 0.25, ink);
    }

    var out: FsOut;
    out.color = vec4<f32>(color, coverage);
    out.depth = depth_of(in.center_view + vec3<f32>(in.offset, h.z));
    return out;
}

@fragment
fn fs_id(in: VsOut) -> IdOut {
    let h = hit(in);
    if h.rho > 1.0 {
        discard;
    }
    var out: IdOut;
    out.id = in.id;
    out.depth = depth_of(in.center_view + vec3<f32>(in.offset, h.z));
    return out;
}
