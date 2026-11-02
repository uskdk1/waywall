// Reinhard: c / (1 + c)
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    return c / (vec3<f32>(1.0) + c);
}
