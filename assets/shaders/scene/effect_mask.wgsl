// `type = "shader"` 레이어 효과의 마지막 단계: 셰이더 결과를 마스크로 효과 전 이미지와 섞는다
// (docs/wallpaper-format.md#레이어-효과-effects). 버텍스는 fullscreen.wgsl, 출력 = 이미지 크기.
// 인터페이스: 엔트리 fs_main, 아래 바인딩.
@group(0) @binding(0) var before: texture_2d<f32>;
@group(0) @binding(1) var after: texture_2d<f32>;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / vec2<f32>(textureDimensions(before));
    let m = textureSampleLevel(mask, samp, uv, 0.0).r;
    let at = vec2<i32>(p.xy);
    return mix(textureLoad(before, at, 0), textureLoad(after, at, 0), m);
}
