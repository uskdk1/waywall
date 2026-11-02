// 무늬를 정하지 않은 파티클의 기본 스프라이트 (docs/wallpaper-format.md#스프라이트).
// uv: 사각형 왼쪽 위 (0, 0) ~ 오른쪽 아래 (1, 1). 반환: 선형 premultiplied RGBA (파티클 색과 곱해짐).
fn sprite(uv: vec2<f32>) -> vec4<f32> {
    let d = length(uv * 2.0 - 1.0);
    return vec4<f32>(pow(1.0 - smoothstep(0.0, 1.0, d), 2.0));
}
