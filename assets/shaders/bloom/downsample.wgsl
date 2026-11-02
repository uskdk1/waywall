// 블룸 축소 (docs/architecture.md#씬-렌더러). compute, 출력 픽셀마다 원본 2×2를 평균.
// 인터페이스: 엔트리 cs_main(@workgroup_size 8, 8), 아래 바인딩과 `Down` 필드 순서.
// 원본은 HDR 포맷 그대로일 수 있어(필터링 불가) textureLoad.
struct Down {
    threshold: f32,
    first: u32,          // 1이면 첫 단계: threshold를 넘는 부분만 남긴다
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> down: Down;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

fn tap(p: vec2<i32>) -> vec3<f32> {
    let dim = vec2<i32>(textureDimensions(src)) - 1;
    return textureLoad(src, clamp(p, vec2<i32>(0), dim), 0).rgb;
}

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= textureDimensions(dst)) {
        return;
    }
    let p = vec2<i32>(id.xy) * 2;
    var c = (tap(p) + tap(p + vec2<i32>(1, 0)) + tap(p + vec2<i32>(0, 1)) + tap(p + vec2<i32>(1, 1))) * 0.25;
    if down.first == 1u {
        // 밝기가 threshold를 넘는 만큼만 (색조는 유지).
        let b = max(max(c.r, c.g), c.b);
        c *= max(b - down.threshold, 0.0) / max(b, 1e-4);
    }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(c, 1.0));
}
