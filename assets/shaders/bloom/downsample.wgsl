// 블룸 축소 (docs/architecture.md#씬-렌더러). 버텍스는 fullscreen.wgsl, 출력 = 원본의 절반 크기.
// 인터페이스: 엔트리 fs_main, 아래 바인딩과 `Down` 필드 순서. 원본은 HDR 포맷 그대로일 수 있어(필터링 불가) textureLoad.
struct Down {
    threshold: f32,
    first: u32,          // 1이면 첫 단계: threshold를 넘는 부분만 남긴다
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> down: Down;
@group(0) @binding(1) var src: texture_2d<f32>;

fn tap(p: vec2<i32>) -> vec3<f32> {
    let dim = vec2<i32>(textureDimensions(src)) - 1;
    return textureLoad(src, clamp(p, vec2<i32>(0), dim), 0).rgb;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy) * 2;
    var c = (tap(p) + tap(p + vec2<i32>(1, 0)) + tap(p + vec2<i32>(0, 1)) + tap(p + vec2<i32>(1, 1))) * 0.25;
    if down.first == 1u {
        // 밝기가 threshold를 넘는 만큼만 (색조는 유지).
        let b = max(max(c.r, c.g), c.b);
        c *= max(b - down.threshold, 0.0) / max(b, 1e-4);
    }
    return vec4<f32>(c, 1.0);
}
