// 스테인드글라스 빛줄기: iChannel0(하늘)의 밝은 곳에서 빛을 모으고, iChannel1–5(창틀 막대 3 + 색유리 2)가
// 광원 앞을 덮은 곳만큼 가리거나 색을 입힌다 (docs/wallpaper-format.md#빛-가리기-빛줄기와-캐릭터레이어).
// 채널 6개 — Shadertoy의 4개를 넘는 채널 예제 (docs/wallpaper-format.md#4개를-넘는-채널).
const vec2  SUN_POS   = vec2(0.5, 0.852);  // sky.png의 해 (화면 비율, 왼쪽 아래 원점)
const int   SAMPLES   = 32;
const float DENSITY   = 0.95;  // 광원까지 거리 중 훑는 비율
const float DECAY     = 0.955; // 광원에서 멀어질수록 약해지는 정도
const float THRESHOLD = 0.6;   // 이 밝기(선형)보다 밝은 곳만 빛을 냄

// 가리는 레이어 하나를 지난 빛: 알파만큼 막고, 색유리는 자기 색(premultiplied를 되돌린 값)을 곱한다.
vec3 through(vec4 c) {
    return mix(vec3(1.0), c.rgb / max(c.a, 1e-4), c.a);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 stride = (uv - SUN_POS) * DENSITY / float(SAMPLES);
    vec2 p = uv;
    float weight = 1.0;
    vec3 acc = vec3(0.0);
    for (int i = 0; i < SAMPLES; i++) {
        p -= stride;
        vec3 c = texture(iChannel0, p).rgb;
        float lum = dot(c, vec3(0.2126, 0.7152, 0.0722));
        vec3 t = vec3((1.0 - texture(iChannel1, p).a) * (1.0 - texture(iChannel2, p).a)
                      * (1.0 - texture(iChannel3, p).a));
        t *= through(texture(iChannel4, p)) * through(texture(iChannel5, p));
        acc += c * smoothstep(THRESHOLD, 1.0, lum) * t * weight;
        weight *= DECAY;
    }
    // 선형 HDR, add 블렌드라 알파는 쓰지 않음.
    fragColor = vec4(acc / float(SAMPLES) * u_strength * u_sun.rgb, 0.0);
}
