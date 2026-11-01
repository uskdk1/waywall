
// Shadertoy와 같은 왼쪽 아래 원점으로 바꿔 mainImage를 부른다.
void main() {
    vec2 fragCoord = vec2(gl_FragCoord.x, iResolution.y - gl_FragCoord.y);
    mainImage(waywall_fragColor, fragCoord);
}
