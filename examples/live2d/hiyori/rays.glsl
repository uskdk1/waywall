// 빛줄기: iChannel0(아래 장면 = 방)의 밝은 곳(창)을 왼쪽 위 창 쪽 광원으로 모아 더한다.
const vec2  SUN_POS   = vec2(0.16, 0.82);  // room.png의 창 (화면 비율, 왼쪽 아래 원점)
const int   SAMPLES   = 32;
const float DENSITY   = 0.9;
const float DECAY     = 0.96;
const float THRESHOLD = 0.8;               // 이 밝기(선형)보다 밝은 곳만 빛을 냄

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 stride = (uv - SUN_POS) * DENSITY / float(SAMPLES);
    float jitter = fract(sin(dot(fragCoord, vec2(12.9898, 78.233))) * 43758.5453);
    vec2 p = uv - stride * jitter;
    float weight = 1.0;
    vec3 acc = vec3(0.0);
    for (int i = 0; i < SAMPLES; i++) {
        p -= stride;
        vec3 c = texture(iChannel0, p).rgb;
        float lum = dot(c, vec3(0.2126, 0.7152, 0.0722));
        acc += c * smoothstep(THRESHOLD, 1.0, lum) * weight;
        weight *= DECAY;
    }
    fragColor = vec4(acc / float(SAMPLES) * u_rays * u_sun.rgb, 0.0);
}
