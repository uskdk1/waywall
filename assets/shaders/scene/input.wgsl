// shader 레이어 채널 입력: 장면(또는 레이어 하나만 그린 결과)에서 레이어의 rect 영역을 잘라 낸다
// (docs/architecture.md#씬-렌더러). 버텍스는 fullscreen.wgsl, 출력 크기 = rect의 렌더 크기 × render_scale.
// 인터페이스: 엔트리 fs_main, 아래 바인딩과 `Crop` 필드 순서. 원본은 HDR 포맷 그대로(필터링 불가일 수 있어 textureLoad).
struct Crop {
    origin: vec2<i32>,   // 원본에서 rect의 왼쪽 위 (px)
    flip: u32,           // 1이면 상하 반전 (GLSL 채널은 uv (0, 0)이 왼쪽 아래)
    height: u32,         // 출력 높이 (px)
    step: f32,           // 출력 1px당 원본 px (1 / render_scale)
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> crop: Crop;
@group(0) @binding(1) var src: texture_2d<f32>;

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    var y = i32(p.y);
    if crop.flip == 1u {
        y = i32(crop.height) - 1 - y;
    }
    // 출력 픽셀 중심에 해당하는 원본 픽셀 (가장 가까운 것).
    let at = vec2<i32>(floor((vec2<f32>(f32(i32(p.x)), f32(y)) + 0.5) * crop.step)) + crop.origin;
    let dim = vec2<i32>(textureDimensions(src));
    if any(at < vec2<i32>(0)) || any(at >= dim) {
        return vec4<f32>(0.0);
    }
    return textureLoad(src, at, 0);
}
