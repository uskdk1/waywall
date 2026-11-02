// 씬 image 레이어: 텍스처를 입힌 사각형 하나 (docs/architecture.md#씬-렌더러).
// 인터페이스: 엔트리 vs_main/fs_main, 아래 바인딩과 `Layer` 필드 순서(CPU가 이 순서로 쓴다).
// 텍스처는 sRGB + premultiplied 알파, 출력은 선형 HDR(premultiplied).
struct Layer {
    // clip = origin + uv.x * axis_u + uv.y * axis_v  (uv: 텍스처 왼쪽 위가 (0, 0))
    axis_u: vec2<f32>,
    axis_v: vec2<f32>,
    origin: vec2<f32>,
    opacity: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// 삼각형 스트립 정점 4개: (0,0) (1,0) (0,1) (1,1)
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: VsOut;
    out.pos = vec4<f32>(layer.origin + uv.x * layer.axis_u + uv.y * layer.axis_v, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * layer.opacity;
}
