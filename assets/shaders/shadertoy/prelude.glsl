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
    // waywall:properties
};

layout(location = 0) out vec4 waywall_fragColor;

