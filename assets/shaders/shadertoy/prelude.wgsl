// waywall WGSL 프리앰블 (docs/wallpaper-format.md#wgsl-wgsl).
// 필드는 이름으로 값을 채운다. `// waywall:properties` 줄은 배경 속성(u_<이름>) 선언으로,
// `// waywall:channel_resolution` 줄은 채널 수만큼의 channel_resolution 배열로,
// `// waywall:channels` 줄은 채널 텍스처·샘플러(channel<N>, channel<N>_sampler) 선언으로 바뀐다.
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
    // waywall:channel_resolution
    origin: vec2<f32>,      // 그리는 영역의 왼쪽 위 (렌더 타깃 px). 영역 안 좌표 = frag_coord.xy - origin
    // waywall:properties
}

@group(0) @binding(0) var<uniform> g: Globals;

// 텍스처 채널 channel0..N-1 (docs/wallpaper-format.md#텍스처-입력-ichanneln).
// binding 1..N 텍스처, N+1..2N 샘플러. N = max(4, 쓴 가장 큰 번호 + 1).
// waywall:channels

