// 씬 particles 레이어 시뮬레이션 (docs/architecture.md#씬-렌더러).
// 인터페이스: 엔트리 cs_main(@workgroup_size 64), 아래 바인딩과 `Sim` 필드 순서(CPU가 이 순서로 쓴다), `Particle` 배치.
// 좌표는 설계 px(왼쪽 위 원점, y 아래로 +), 시간은 초. age >= life 이면 죽은 파티클.
struct Particle {
    pos: vec2<f32>,
    vel: vec2<f32>,
    age: f32,
    life: f32,
}

struct Sim {
    // 고리 버퍼 [start, start + emit + burst)가 이번 프레임에 새로 태어난다. 앞 emit개는 방출기, 뒤 burst개는 클릭 위치.
    start: u32,
    emit: u32,
    burst: u32,
    count: u32,
    shape: u32,          // 0 point, 1 box, 2 circle
    pointer_mode: u32,   // 0 none, 1 attract, 2 repel (포인터가 밖이면 0)
    seed: u32,
    _pad0: u32,
    emitter: vec4<f32>,  // 중심 xy, box 반폭 xy / circle 반지름 x
    burst_pos: vec2<f32>,
    dt: f32,
    time: f32,
    vel_min: vec2<f32>,
    vel_max: vec2<f32>,
    gravity: vec2<f32>,
    drag: f32,
    _pad1: f32,
    lifetime: vec2<f32>, // 최소, 최대
    turb_strength: f32,
    turb_scale: f32,
    pointer: vec2<f32>,
    pointer_radius: f32,
    pointer_strength: f32,
    turb_speed: f32,
    _pad2: f32,
    _pad3: f32,
    _pad4: f32,
}

@group(0) @binding(0) var<uniform> sim: Sim;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn rand(state: ptr<function, u32>) -> f32 {
    *state = pcg(*state);
    return f32(*state) / 4294967295.0;
}

fn hash3(p: vec3<i32>) -> f32 {
    let h = pcg(bitcast<u32>(p.x) ^ pcg(bitcast<u32>(p.y) ^ pcg(bitcast<u32>(p.z))));
    return f32(h) / 4294967295.0;
}

// 3D value noise, 0..1
fn noise(p: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(hash3(i), hash3(i + vec3<i32>(1, 0, 0)), u.x);
    let b = mix(hash3(i + vec3<i32>(0, 1, 0)), hash3(i + vec3<i32>(1, 1, 0)), u.x);
    let c = mix(hash3(i + vec3<i32>(0, 0, 1)), hash3(i + vec3<i32>(1, 0, 1)), u.x);
    let d = mix(hash3(i + vec3<i32>(0, 1, 1)), hash3(i + vec3<i32>(1, 1, 1)), u.x);
    return mix(mix(a, b, u.y), mix(c, d, u.y), u.z);
}

// 스칼라 노이즈의 2D curl: 발산이 없는 흐름이라 파티클이 한곳에 몰리지 않는다.
fn curl(p: vec2<f32>, t: f32) -> vec2<f32> {
    let e = 0.25;
    let dx = noise(vec3<f32>(p + vec2<f32>(e, 0.0), t)) - noise(vec3<f32>(p - vec2<f32>(e, 0.0), t));
    let dy = noise(vec3<f32>(p + vec2<f32>(0.0, e), t)) - noise(vec3<f32>(p - vec2<f32>(0.0, e), t));
    return vec2<f32>(dy, -dx) / (2.0 * e);
}

fn spawn(i: u32, k: u32) -> Particle {
    var s = pcg(i ^ pcg(sim.seed));
    var p: Particle;
    if k >= sim.emit {
        p.pos = sim.burst_pos;
    } else if sim.shape == 1u {
        p.pos = sim.emitter.xy + (vec2<f32>(rand(&s), rand(&s)) * 2.0 - 1.0) * sim.emitter.zw;
    } else if sim.shape == 2u {
        let r = sqrt(rand(&s)) * sim.emitter.z;
        let a = rand(&s) * 6.28318530718;
        p.pos = sim.emitter.xy + r * vec2<f32>(cos(a), sin(a));
    } else {
        p.pos = sim.emitter.xy;
    }
    p.vel = mix(sim.vel_min, sim.vel_max, vec2<f32>(rand(&s), rand(&s)));
    p.age = 0.0;
    p.life = mix(sim.lifetime.x, sim.lifetime.y, rand(&s));
    return p;
}

@compute @workgroup_size(64)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= sim.count {
        return;
    }
    let k = (i + sim.count - sim.start) % sim.count;
    if k < sim.emit + sim.burst {
        particles[i] = spawn(i, k);
        return;
    }
    var p = particles[i];
    if p.age >= p.life {
        return;
    }
    let dt = sim.dt;
    var acc = sim.gravity;
    if sim.pointer_mode != 0u {
        let d = sim.pointer - p.pos;
        let dist = length(d);
        if dist < sim.pointer_radius && dist > 0.001 {
            let dir = select(-1.0, 1.0, sim.pointer_mode == 1u);
            acc += d / dist * (sim.pointer_strength * (1.0 - dist / sim.pointer_radius) * dir);
        }
    }
    p.vel = (p.vel + acc * dt) * exp(-sim.drag * dt);
    var flow = vec2<f32>(0.0);
    if sim.turb_strength != 0.0 {
        flow = curl(p.pos * sim.turb_scale, sim.time * sim.turb_speed) * sim.turb_strength;
    }
    p.pos += (p.vel + flow) * dt;
    p.age += dt;
    particles[i] = p;
}
