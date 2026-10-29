// Analytic, antialiased strokes. One instanced draw for the complete document.
struct View { size: vec2<f32>, padding: vec2<f32> }
@group(0) @binding(0) var<uniform> view: View;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) point: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) data: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
}

@vertex fn vs_main(
    @builtin(vertex_index) index: u32,
    @location(0) bounds: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) data: vec4<f32>,
    @location(3) params: vec4<f32>,
) -> VertexOut {
    let corners = array<vec2<f32>, 6>(
        vec2(0., 0.), vec2(1., 0.), vec2(0., 1.),
        vec2(0., 1.), vec2(1., 0.), vec2(1., 1.),
    );
    let p = bounds.xy + corners[index] * bounds.zw;
    var out: VertexOut;
    out.clip = vec4(p.x / view.size.x * 2. - 1., 1. - p.y / view.size.y * 2., 0., 1.);
    out.point = p;
    out.color = color;
    out.data = data;
    out.params = params;
    return out;
}

@fragment fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var distance: f32;
    if in.params.x < 0.5 {
        let a = in.data.xy;
        let ab = in.data.zw - a;
        let t = clamp(dot(in.point - a, ab) / max(dot(ab, ab), 0.0001), 0., 1.);
        distance = length(in.point - a - t * ab) - in.params.y;
    } else {
        let q = in.point - in.data.xy;
        let r = in.data.zw;
        let k0 = length(q / r);
        let k1 = length(q / (r * r));
        var d = min(r.x, r.y);
        if k1 > 0.00001 { d = abs(k0 * (k0 - 1.) / k1); }
        distance = d - in.params.y;
    }
    let aa = max(fwidth(distance), 0.35);
    let alpha = (1. - smoothstep(-aa * 0.5, aa * 0.5, distance)) * in.color.a;
    // Premultiplied alpha is required for a transparent Wayland surface.
    return vec4(in.color.rgb * alpha, alpha);
}
