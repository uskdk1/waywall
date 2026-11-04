#version 450
// waywall Shadertoy 프리앰블 (docs/wallpaper-format.md#셰이더-배경).
// 이 블록의 멤버는 이름으로 값을 채운다. 모르는 이름(패딩 등)은 0으로 남는다.
// `// waywall:properties` 줄은 배경 속성(u_<이름>) 선언으로, `// waywall:channel_resolution` 줄은 채널 수만큼의
// iChannelResolution 배열로, `// waywall:channels` 줄은 채널 텍스처·샘플러 선언으로 바뀐다.
layout(set = 0, binding = 0) uniform Globals {
    vec3  iResolution;
    float iTime;
    float iTimeDelta;
    int   iFrame;
    float iFrameRate;
    float waywall_pad0;
    vec4  iDate;
    vec4  iMouse;
    vec4  iPointer;
    vec4  iClick;
    // waywall:channel_resolution
    vec2  waywall_origin;   // 그리는 영역의 왼쪽 위 (렌더 타깃 px). 배경은 0
    // waywall:properties
};

// 텍스처 채널 iChannel0..N-1 (docs/wallpaper-format.md#텍스처-입력-ichanneln).
// binding 1..N 텍스처, N+1..2N 샘플러. N = max(4, 쓴 가장 큰 번호 + 1).
// waywall:channels

layout(location = 0) out vec4 waywall_fragColor;

