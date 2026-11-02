// 톤매핑 없음: 1.0을 넘는 값은 잘린다.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}
