// 씬 후처리: HDR 버퍼 → 노출 → tonemap() → 화면 (docs/architecture.md#씬-렌더러).
// 버텍스는 fullscreen.wgsl. 뒤에 tonemap/<이름>.wgsl이 붙어 `fn tonemap(c: vec3<f32>) -> vec3<f32>`를 정의한다.
// 출력은 선형 값이고, sRGB 인코딩은 swapchain 뷰가 한다.
struct Post {
    exposure: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var hdr: texture_2d<f32>;

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(hdr, vec2<i32>(p.xy), 0).rgb * post.exposure;
    return vec4<f32>(tonemap(max(c, vec3<f32>(0.0))), 1.0);
}
