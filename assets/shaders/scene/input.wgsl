// shader 레이어 채널 입력: 장면(또는 레이어 하나만 그린 결과)에서 레이어의 rect 영역을 잘라 낸다
// (docs/architecture.md#씬-렌더러). 버텍스는 fullscreen.wgsl, 출력 크기 = rect의 렌더 크기.
// 인터페이스: 엔트리 fs_main, 아래 바인딩과 `Crop` 필드 순서. 원본은 HDR 포맷 그대로(필터링 불가일 수 있어 textureLoad).
struct Crop {
    origin: vec2<i32>,   // 원본에서 rect의 왼쪽 위 (px)
    flip: u32,           // 1이면 상하 반전 (GLSL 채널은 uv (0, 0)이 왼쪽 아래)
    height: u32,         // 출력 높이 (px)
}

@group(0) @binding(0) var<uniform> crop: Crop;
@group(0) @binding(1) var src: texture_2d<f32>;

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    var y = i32(p.y);
    if crop.flip == 1u {
        y = i32(crop.height) - 1 - y;
    }
    let at = vec2<i32>(i32(p.x), y) + crop.origin;
    let dim = vec2<i32>(textureDimensions(src));
    if any(at < vec2<i32>(0)) || any(at >= dim) {
        return vec4<f32>(0.0);
    }
    return textureLoad(src, at, 0);
}
