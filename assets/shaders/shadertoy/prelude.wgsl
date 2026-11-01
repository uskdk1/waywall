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
    // waywall:properties
}

@group(0) @binding(0) var<uniform> g: Globals;

