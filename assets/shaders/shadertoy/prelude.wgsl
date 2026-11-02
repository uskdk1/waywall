// waywall WGSL 프리앰블 (docs/wallpaper-format.md#wgsl-wgsl).
// 필드는 이름으로 값을 채운다. `// waywall:properties` 줄은 배경 속성(u_<이름>) 선언으로 바뀐다.
struct Globals {
    resolution: vec3<f32>,
    time: f32,
    time_delta: f32,
    frame: i32,
    frame_rate: f32,
    _pad0: f32,
    date: vec4<f32>,
    mouse: vec4<f32>,
    pointer: vec4<f32>,
    click: vec4<f32>,
    channel_resolution: array<vec3<f32>, 4>,
    // waywall:properties
}

@group(0) @binding(0) var<uniform> g: Globals;

// 텍스처 채널: binding 1–4 텍스처, 5–8 샘플러 (docs/wallpaper-format.md#텍스처-입력-ichannel0ichannel3).
@group(0) @binding(1) var channel0: texture_2d<f32>;
@group(0) @binding(2) var channel1: texture_2d<f32>;
@group(0) @binding(3) var channel2: texture_2d<f32>;
@group(0) @binding(4) var channel3: texture_2d<f32>;
@group(0) @binding(5) var channel0_sampler: sampler;
@group(0) @binding(6) var channel1_sampler: sampler;
@group(0) @binding(7) var channel2_sampler: sampler;
@group(0) @binding(8) var channel3_sampler: sampler;

