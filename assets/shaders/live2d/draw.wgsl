// Live2D drawable 그리기 (docs/architecture.md#씬-렌더러). 레이어 자기 텍스처(선형, premultiplied)에 그린다.
// 정점: 모델 단위 위치(y 위로 +), UV(왼쪽 아래 원점). 텍스처는 씬 텍스처처럼 premultiplied sRGB로 올라와 선형으로 읽힌다.

struct Layer {
    // clip = position × scale + offset
    scale: vec2<f32>,
    offset: vec2<f32>,
}

struct Drawable {
    // x = 불투명도, y = 마스크 있음(1), z = 마스크 반전(1)
    params: vec4<f32>,
    // 선형 RGB
    multiply: vec4<f32>,
    screen: vec4<f32>,
}

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex_sampler: sampler;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(0) var mask: texture_2d<f32>;
@group(3) @binding(0) var<uniform> drawable: Drawable;

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
    let c = textureSample(tex, tex_sampler, in.uv);
    // premultiplied 값에 곱하기 색, 화면 색(straight 기준 s + c − s·c를 알파를 곱한 꼴로).
    var rgb = c.rgb * drawable.multiply.rgb;
    rgb = rgb + drawable.screen.rgb * c.a - drawable.screen.rgb * rgb;
    var k = drawable.params.x;
    if drawable.params.y > 0.5 {
        let last = textureDimensions(mask) - vec2<u32>(1u);
        var m = textureLoad(mask, min(vec2<u32>(in.pos.xy), last), 0).r;
        if drawable.params.z > 0.5 {
            m = 1.0 - m;
        }
        k = k * m;
    }
    return vec4<f32>(rgb * k, c.a * k);
}
