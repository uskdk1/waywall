// 빛줄기(god rays): iChannel0(아래 장면)의 밝은 픽셀을 광원 쪽으로 모아 더한다.
// 광원 = 커서가 배경 위에 있으면 커서, 아니면 SUN_POS(화면 비율, 왼쪽 아래 원점).
const vec2  SUN_POS   = vec2(0.646, 0.435);
const int   SAMPLES   = 48;
const float DENSITY   = 0.9;   // 광원까지 거리 중 훑는 비율
const float DECAY     = 0.965; // 광원에서 멀어질수록 약해지는 정도
const float THRESHOLD = 0.6;   // 이 밝기(선형)보다 밝은 곳만 빛을 냄

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 light = iPointer.z > 0.5 ? iPointer.xy / iResolution.xy : SUN_POS;
    vec2 stride = (uv - light) * DENSITY / float(SAMPLES);
    vec2 p = uv;
    float weight = 1.0;
    vec3 acc = vec3(0.0);
    for (int i = 0; i < SAMPLES; i++) {
        p -= stride;
        vec3 c = texture(iChannel0, p).rgb;
        float lum = dot(c, vec3(0.2126, 0.7152, 0.0722));
        acc += c * smoothstep(THRESHOLD, 1.0, lum) * weight;
        weight *= DECAY;
    }
    // 선형 HDR, add 블렌드라 알파는 쓰지 않음.
    fragColor = vec4(acc / float(SAMPLES) * u_strength * u_sun.rgb, 0.0);
}
