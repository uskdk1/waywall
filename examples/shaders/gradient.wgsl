// WGSL 예제: 시간에 따라 바뀌는 그라데이션 + 포인터 위치 표시.
@fragment
fn main_image(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag_coord.xy / g.resolution.xy;
    let p = vec2<f32>(frag_coord.x, g.resolution.y - frag_coord.y);
    let glow = exp(-distance(p, g.pointer.xy) * 0.02) * g.pointer.z;
    return vec4<f32>(uv, 0.5 + 0.5 * sin(g.time), 1.0) + glow;
}
