// 씬 particles 레이어 그리기: 파티클마다 화면을 향한 사각형 하나 (docs/architecture.md#씬-렌더러).
// 인터페이스: 엔트리 vs_main / fs_texture(텍스처 무늬) / fs_sprite(절차적 무늬), 아래 바인딩과 `Draw` 필드 순서,
// `Particle` 배치(simulate.wgsl과 같음). 뒤에 스프라이트 파일이 붙어 `fn sprite(uv: vec2<f32>) -> vec4<f32>`를 정의한다.
// 출력은 선형 HDR(premultiplied).
struct Particle {
    pos: vec2<f32>,
    vel: vec2<f32>,
    age: f32,
    life: f32,
}

struct Draw {
    // clip = pos * to_clip.xy + to_clip.zw  (패럴랙스 이동 포함)
    to_clip: vec4<f32>,
    size: vec2<f32>,         // 지름 시작, 끝 (설계 px)
    _pad: vec2<f32>,
    color_start: vec4<f32>,  // 선형 premultiplied × emissive × opacity
    color_end: vec4<f32>,
}

@group(0) @binding(0) var<uniform> draw: Draw;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

// 삼각형 스트립 정점 4개: (0,0) (1,0) (0,1) (1,1)
@vertex
fn vs_main(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> VsOut {
    var out: VsOut;
    let p = particles[i];
    if p.age >= p.life {
        out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let t = p.age / p.life;
    let uv = vec2<f32>(f32(v & 1u), f32(v >> 1u));
    let size = mix(draw.size.x, draw.size.y, t);
    let corner = p.pos + (uv - 0.5) * size;
    out.pos = vec4<f32>(corner * draw.to_clip.xy + draw.to_clip.zw, 0.0, 1.0);
    out.uv = uv;
    out.color = mix(draw.color_start, draw.color_end, t);
    return out;
}

@fragment
fn fs_texture(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * in.color;
}

@fragment
fn fs_sprite(in: VsOut) -> @location(0) vec4<f32> {
    return sprite(in.uv) * in.color;
}
