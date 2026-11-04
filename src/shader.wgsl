// Textured quad. `view` = (scale_x, scale_y, offset_x, offset_y) in NDC.

struct View {
    scale_offset: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // Two triangles covering the unit quad, centered at the origin.
    var corners = array<vec2<f32>, 6>(
        vec2(-0.5, -0.5), vec2(0.5, -0.5), vec2(-0.5, 0.5),
        vec2(-0.5, 0.5), vec2(0.5, -0.5), vec2(0.5, 0.5),
    );
    let c = corners[i];
    var out: VsOut;
    out.pos = vec4(
        c.x * 2.0 * view.scale_offset.x + view.scale_offset.z,
        c.y * 2.0 * view.scale_offset.y + view.scale_offset.w,
        0.0, 1.0,
    );
    out.uv = vec2(c.x + 0.5, 0.5 - c.y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, in.uv);
    // Premultiplied alpha output for the transparent surface.
    return vec4(c.rgb * c.a, c.a);
}
