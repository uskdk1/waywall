// 커서 주변이 빛나고, 클릭하면 파문이 퍼지는 예제.
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float d = distance(fragCoord, iPointer.xy);
    vec3 col = 0.5 + 0.5 * cos(iTime + uv.xyx + vec3(0, 2, 4));
    col *= 0.6;
    col += 0.5 * exp(-d * 0.01) * iPointer.z;
    float t = iTime - iClick.z;
    float ring = abs(distance(fragCoord, iClick.xy) - t * 600.0);
    col += 0.8 * exp(-t * 2.0) * exp(-ring * 0.05) * step(0.5, iClick.w); // w=0: 아직 클릭 없음
    fragColor = vec4(col, 1.0);
}
