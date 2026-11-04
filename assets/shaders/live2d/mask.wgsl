// Live2D 클리핑 마스크: 마스크 drawable들의 텍스처 알파를 합친다(R 채널, 1 − Π(1 − a)).
// 위치 변환은 draw.wgsl과 같다.

struct Layer {
    scale: vec2<f32>,
    offset: vec2<f32>,
}

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex_sampler: sampler;
@group(1) @binding(0) var tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.pos = vec4<f32>(position * layer.scale + layer.offset, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(tex, tex_sampler, in.uv).a;
    return vec4<f32>(a, a, a, a);
}
