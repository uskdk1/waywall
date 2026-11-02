// 레이어 효과 sway: 바람에 흔들리듯 이미지를 노이즈로 민다 (docs/wallpaper-format.md#레이어-효과-effects).
// 미는 거리 = strength × 마스크라서 마스크가 검은 곳(뿌리 쪽)은 고정된다.
struct Effect {
    time: f32,
    size: vec2<f32>,        // 레이어의 화면 크기 (설계 px)
    resolution: vec2<f32>,  // 이미지 크기 (px)
    strength: f32,          // 설계 px
    speed: f32,
    scale: f32,             // 노이즈 주파수 (1/설계 px)
    direction: vec2<f32>,
}

@group(0) @binding(0) var<uniform> fx: Effect;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;
@group(0) @binding(3) var mask: texture_2d<f32>;

fn hash(p: vec3<f32>) -> f32 {
    let q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    let r = q + dot(q, q.yzx + 33.33);
    return fract((r.x + r.y) * r.z);
}

// 3D value noise, −1–1
fn noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(hash(i), hash(i + vec3<f32>(1.0, 0.0, 0.0)), u.x);
    let b = mix(hash(i + vec3<f32>(0.0, 1.0, 0.0)), hash(i + vec3<f32>(1.0, 1.0, 0.0)), u.x);
    let c = mix(hash(i + vec3<f32>(0.0, 0.0, 1.0)), hash(i + vec3<f32>(1.0, 0.0, 1.0)), u.x);
    let d = mix(hash(i + vec3<f32>(0.0, 1.0, 1.0)), hash(i + vec3<f32>(1.0, 1.0, 1.0)), u.x);
    return mix(mix(a, b, u.y), mix(c, d, u.y), u.z) * 2.0 - 1.0;
}

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / fx.resolution;
    let m = textureSampleLevel(mask, src_sampler, uv, 0.0).r;
    let design = uv * fx.size;
    let n = noise(vec3<f32>(design * fx.scale, fx.time * fx.speed));
    let dir = normalize(fx.direction);
    let push = dir * n * fx.strength * m;
    return textureSampleLevel(src, src_sampler, uv - push / fx.size, 0.0);
}
