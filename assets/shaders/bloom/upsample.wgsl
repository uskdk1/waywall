// 블룸 확대 (docs/architecture.md#씬-렌더러). 버텍스는 fullscreen.wgsl, 한 단계 작은 텍스처를 3×3 텐트 필터로 읽어
// 이 단계에 더한다(additive blend). 인터페이스: 엔트리 fs_main, 아래 바인딩.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dst = vec2<f32>(textureDimensions(src)) * 2.0;
    let uv = pos.xy / dst;
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    var c = textureSample(src, samp, uv).rgb * 4.0;
    c += (textureSample(src, samp, uv + vec2<f32>(t.x, 0.0)).rgb
        + textureSample(src, samp, uv - vec2<f32>(t.x, 0.0)).rgb
        + textureSample(src, samp, uv + vec2<f32>(0.0, t.y)).rgb
        + textureSample(src, samp, uv - vec2<f32>(0.0, t.y)).rgb) * 2.0;
    c += textureSample(src, samp, uv + t).rgb
        + textureSample(src, samp, uv - t).rgb
        + textureSample(src, samp, uv + vec2<f32>(t.x, -t.y)).rgb
        + textureSample(src, samp, uv + vec2<f32>(-t.x, t.y)).rgb;
    return vec4<f32>(c / 16.0, 1.0);
}
