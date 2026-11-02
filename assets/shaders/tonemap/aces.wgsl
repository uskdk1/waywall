// ACES 필름 곡선 근사 (Krzysztof Narkowicz, 2015).
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let d = 2.43;
    let e = 0.59;
    let f = 0.14;
    return clamp((c * (a * c + b)) / (c * (d * c + e) + f), vec3<f32>(0.0), vec3<f32>(1.0));
}
