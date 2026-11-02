#version 450
// waywall Shadertoy 프리앰블 (docs/wallpaper-format.md#셰이더-배경).
// 이 블록의 멤버는 이름으로 값을 채운다. 모르는 이름(패딩 등)은 0으로 남는다.
// `// waywall:properties` 줄은 배경 속성(u_<이름>) 선언으로 바뀐다.
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
    vec3  iChannelResolution[4];
    vec2  waywall_origin;   // 그리는 영역의 왼쪽 위 (렌더 타깃 px). 배경은 0
    // waywall:properties
};

// 텍스처 채널: binding 1–4 텍스처, 5–8 샘플러 (docs/wallpaper-format.md#텍스처-입력-ichannel0ichannel3).
layout(set = 0, binding = 1) uniform texture2D waywall_channel0;
layout(set = 0, binding = 2) uniform texture2D waywall_channel1;
layout(set = 0, binding = 3) uniform texture2D waywall_channel2;
layout(set = 0, binding = 4) uniform texture2D waywall_channel3;
layout(set = 0, binding = 5) uniform sampler waywall_sampler0;
layout(set = 0, binding = 6) uniform sampler waywall_sampler1;
layout(set = 0, binding = 7) uniform sampler waywall_sampler2;
layout(set = 0, binding = 8) uniform sampler waywall_sampler3;
#define iChannel0 sampler2D(waywall_channel0, waywall_sampler0)
#define iChannel1 sampler2D(waywall_channel1, waywall_sampler1)
#define iChannel2 sampler2D(waywall_channel2, waywall_sampler2)
#define iChannel3 sampler2D(waywall_channel3, waywall_sampler3)

layout(location = 0) out vec4 waywall_fragColor;

