// `lit` 레이어의 조명 (docs/wallpaper-format.md#조명). scene/image.wgsl 뒤에 붙는다.
// n: 단위 법선 (x 오른쪽, y 아래, z 보는 사람 쪽), p: 픽셀 위치 (설계 px). 반환: 텍스처 색에 곱할 빛.
fn lighting(n: vec3<f32>, p: vec2<f32>) -> vec3<f32> {
    var c = lights.ambient.rgb;
    for (var i = 0u; i < arrayLength(&lights.items); i++) {
        let l = lights.items[i];
        if l.kind == 0.0 {
            let d = l.geom.xy - p;
            let k = clamp(1.0 - dot(d, d) / (l.geom.w * l.geom.w), 0.0, 1.0);
            let dir = normalize(vec3<f32>(d, l.geom.z));
            c += l.color * k * k * max(dot(n, dir), 0.0);
        } else if l.kind == 1.0 {
            c += l.color * max(dot(n, -normalize(l.geom.xyz)), 0.0);
        }
    }
    return c;
}
