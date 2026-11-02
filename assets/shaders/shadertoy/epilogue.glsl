
// Shadertoy와 같은 왼쪽 아래 원점, 그리는 영역 기준으로 바꿔 mainImage를 부른다.
void main() {
    vec2 p = gl_FragCoord.xy - waywall_origin;
    vec2 fragCoord = vec2(p.x, iResolution.y - p.y);
    mainImage(waywall_fragColor, fragCoord);
}
