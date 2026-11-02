// 텍스처 채널 예제 (docs/wallpaper-format.md#텍스처-입력-ichannel0ichannel3).
// iChannel0 그림이 커서 주변에서 물결처럼 일렁인다. 그림의 위쪽(빨강·삼각형)이 화면 위에 보이면 방향이 맞다.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 d = fragCoord - iPointer.xy;
    float r = length(d);
    float wave = sin(r * 0.08 - iTime * 4.0) * exp(-r * 0.004) * iPointer.z;
    uv += d / max(r, 1.0) * wave * 0.012;
    fragColor = texture(iChannel0, uv);
}
