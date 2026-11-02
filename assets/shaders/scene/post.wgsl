// 씬 후처리: (HDR + 블룸) → 노출 → tonemap() → vignette() → 화면 (docs/architecture.md#씬-렌더러).
// 버텍스는 fullscreen.wgsl. 뒤에 tonemap/<이름>.wgsl(`fn tonemap(c: vec3<f32>) -> vec3<f32>`)과
// vignette.wgsl(`fn vignette(uv: vec2<f32>, strength: f32) -> f32`)이 붙는다.
// 출력은 선형 값이고, sRGB 인코딩은 swapchain 뷰가 한다.
struct Post {
    exposure: f32,
    bloom: f32,      // 블룸 세기 (0이면 블룸 텍스처는 1×1 검정)
    vignette: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var bloom: texture_2d<f32>;
@group(0) @binding(3) var bloom_sampler: sampler;

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / vec2<f32>(textureDimensions(hdr));
    var c = textureLoad(hdr, vec2<i32>(p.xy), 0).rgb;
    c += textureSample(bloom, bloom_sampler, uv).rgb * post.bloom;
    c *= post.exposure;
    return vec4<f32>(tonemap(max(c, vec3<f32>(0.0))) * vignette(uv, post.vignette), 1.0);
}
