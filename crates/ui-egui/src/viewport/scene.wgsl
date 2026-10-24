struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_scene(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.clip = u.view_proj * vec4<f32>(v.pos, 1.0);
    o.normal = v.normal;
    o.color = v.color;
    return o;
}

@fragment
fn fs_scene(i: VertexOut) -> @location(0) vec4<f32> {
    let len = length(i.normal);
    if (len < 0.5) {
        return i.color;
    }
    let n = i.normal / len;
    // Two-sided headlight plus a soft sky term from +Z.
    let key = abs(dot(n, u.light_dir.xyz));
    let sky = 0.5 + 0.5 * n.z;
    let shade = 0.28 + 0.62 * key + 0.12 * sky;
    return vec4<f32>(i.color.rgb * shade, i.color.a);
}

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) t: f32,
};

@vertex
fn vs_background(@builtin(vertex_index) index: u32) -> BgOut {
    // One triangle covering the screen.
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    var o: BgOut;
    o.clip = vec4<f32>(x, y, 0.0, 1.0);
    o.t = y * 0.5 + 0.5;
    return o;
}

@fragment
fn fs_background(i: BgOut) -> @location(0) vec4<f32> {
    return mix(u.bg_bottom, u.bg_top, clamp(i.t, 0.0, 1.0));
}
