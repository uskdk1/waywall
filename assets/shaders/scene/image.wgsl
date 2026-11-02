// 씬 image 레이어: 텍스처를 입힌 사각형 하나 (docs/architecture.md#씬-렌더러).
// 인터페이스: 엔트리 vs_main/fs_main, 아래 바인딩과 `Layer`·`Lights` 필드 순서(CPU가 이 순서로 쓴다).
// 텍스처는 sRGB + premultiplied 알파, 출력은 선형 HDR(premultiplied).
// 뒤에 scene/lighting.wgsl이 붙어 `fn lighting(n: vec3<f32>, p: vec2<f32>) -> vec3<f32>`를 정의한다.
struct Layer {
    // clip = origin + uv.x * axis_u + uv.y * axis_v  (uv: 텍스처 왼쪽 위가 (0, 0))
    axis_u: vec2<f32>,
    axis_v: vec2<f32>,
    origin: vec2<f32>,
    opacity: f32,
    lit: f32,              // 1이면 조명 적용
    // 같은 사각형의 설계 px 좌표 (조명 위치 계산)
    design_origin: vec2<f32>,
    design_u: vec2<f32>,
    design_v: vec2<f32>,
    rotation: vec2<f32>,   // (cos, sin), 시계 방향 — 노멀맵을 함께 돌린다
    clip: f32,             // 1이면 clip 텍스처의 같은 픽셀 알파를 곱한다
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

// 광원 (docs/wallpaper-format.md#조명). 색은 선형 × intensity × 흔들림.
struct Light {
    color: vec3<f32>,
    kind: f32,             // 0 point, 1 directional, 그 밖은 무시
    geom: vec4<f32>,       // point: x, y, height, radius / directional: 방향 xyz
}

struct Lights {
    ambient: vec4<f32>,
    items: array<Light>,
}

@group(0) @binding(0) var<uniform> layer: Layer;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var normal_map: texture_2d<f32>;
@group(0) @binding(4) var<storage, read> lights: Lights;
@group(0) @binding(5) var clip_mask: texture_2d<f32>;   // clip 대상 레이어를 단독으로 그린 결과 (장면 크기)

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// 삼각형 스트립 정점 4개: (0,0) (1,0) (0,1) (1,1)
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: VsOut;
    out.pos = vec4<f32>(layer.origin + uv.x * layer.axis_u + uv.y * layer.axis_v, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(tex, samp, in.uv) * layer.opacity;
    if layer.clip > 0.5 {
        c *= textureLoad(clip_mask, vec2<i32>(in.pos.xy), 0).a;
    }
    if layer.lit < 0.5 {
        return c;
    }
    // OpenGL 규약(G = 위) → y가 아래로 +인 설계 좌표, 레이어 회전만큼 돌림.
    let t = textureSample(normal_map, samp, in.uv).xyz * 2.0 - 1.0;
    let r = layer.rotation;
    let local = vec2<f32>(t.x, -t.y);
    let n = normalize(vec3<f32>(local.x * r.x - local.y * r.y, local.x * r.y + local.y * r.x, t.z));
    let p = layer.design_origin + in.uv.x * layer.design_u + in.uv.y * layer.design_v;
    return vec4<f32>(c.rgb * lighting(n, p), c.a);
}
