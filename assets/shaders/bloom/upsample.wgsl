// 블룸 확대 (docs/architecture.md#씬-렌더러). compute, dst = 같은 크기의 축소 결과 + 한 단계 작은 결과(3×3 텐트 필터).
// 인터페이스: 엔트리 cs_main(@workgroup_size 8, 8), 아래 바인딩.
@group(0) @binding(0) var same: texture_2d<f32>;     // 이 단계의 축소 결과 (dst와 같은 크기)
@group(0) @binding(1) var small: texture_2d<f32>;    // 한 단계 작은 확대(가장 작은 단계는 축소) 결과
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

fn s(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(small, samp, uv, 0.0).rgb;
}

@compute @workgroup_size(8, 8)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if any(id.xy >= size) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(size);
    let t = 1.0 / vec2<f32>(textureDimensions(small));
    var c = s(uv) * 4.0;
    c += (s(uv + vec2<f32>(t.x, 0.0)) + s(uv - vec2<f32>(t.x, 0.0))
        + s(uv + vec2<f32>(0.0, t.y)) + s(uv - vec2<f32>(0.0, t.y))) * 2.0;
    c += s(uv + t) + s(uv - t) + s(uv + vec2<f32>(t.x, -t.y)) + s(uv + vec2<f32>(-t.x, t.y));
    let base = textureLoad(same, vec2<i32>(id.xy), 0).rgb;
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(base + c / 16.0, 1.0));
}
