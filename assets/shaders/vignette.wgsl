// 비네트 (docs/wallpaper-format.md#후처리-post). scene/post.wgsl 뒤에 붙는다.
// uv: 화면 0–1, strength: [post] vignette (0–1). 반환: 색에 곱할 값.
fn vignette(uv: vec2<f32>, strength: f32) -> f32 {
    let d = length(uv - 0.5) * 1.41421356;
    return 1.0 - strength * smoothstep(0.3, 1.0, d);
}
