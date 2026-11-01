# 배경 형식

## 타입

| 타입 | 설명 | 렌더 경로 |
|---|---|---|
| `image` | 정적 이미지, GIF | wl_shm + viewporter (GIF는 GStreamer) |
| `video` | 동영상 | GStreamer → dmabuf zero-copy |
| `shader` | Shadertoy 스타일 단일 셰이더 | wgpu |
| `scene` | 레이어 + 파티클 + 조명 + 후처리 | wgpu |
| `web` | HTML/JS 페이지 | 웹 헬퍼 (WebKitGTK) |

## 단일 파일

경로가 파일이면 확장자로 타입을 판별합니다.

| 타입 | 확장자 |
|---|---|
| `image` | `png` `jpg` `jpeg` `webp` `bmp` `gif` `avif` |
| `video` | `mp4` `webm` `mkv` `mov` |
| `shader` | `glsl` `frag` (Shadertoy GLSL), `wgsl` |
| `web` | `html` `htm` |

`http://` / `https://` URL은 `web` 타입입니다. `scene`은 디렉터리형으로만 만듭니다.

## 디렉터리형 배경 (`project.toml`)

```
forest/
├─ project.toml
├─ scene.toml
├─ textures/…
└─ preview.png
```

```toml
[wallpaper]
name    = "Firefly Forest"      # 표시 이름 (필수)
type    = "scene"               # image | video | shader | scene | web (필수)
entry   = "scene.toml"          # 디렉터리 기준 상대 경로 (필수)
author  = "miku"                # 선택
preview = "preview.png"         # 선택, GUI 매니저용 썸네일

[options]                       # 타입별 옵션, 모두 선택
fps         = 60
interactive = true

[properties.density]            # 사용자 조절 속성
type    = "float"               # float | int | bool | color
default = 1.0
min     = 0.0
max     = 4.0

[properties.tint]
type    = "color"
default = "#88aaff"
```

속성 값은 `config.toml`의 출력별 설정에서 덮어쓸 수 있습니다([config.md](config.md)).
전달 방식: shader → `u_<이름>` uniform, scene → `"$<이름>"` 참조, web → `window.waywall.properties`.

### 옵션

여기 적은 옵션은 배경 작성자의 **권장값**입니다. 사용자가 `config.toml`(`[types.<타입>]`, `[default]`,
`[output.*]`)에 같은 키를 적으면 사용자 값이 이깁니다. 작성자도 생략한 옵션은 내장 `assets/defaults.toml` 값을 씁니다.
코드에 고정된 기본값은 없습니다([config.md](config.md#계층과-우선순위)).

| 옵션 | 적용 타입 | 설명 |
|---|---|---|
| `interactive` | 전부 | `false`면 입력 영역을 비워 마우스 이벤트를 통과시킴 |
| `fps` | shader, scene | 최대 fps. `0`이면 frame 콜백마다(모니터 주사율). `render.max_fps`가 0이 아니면 그 값이 상한 |
| `render_scale` | shader, scene | 내부 렌더 해상도 배율 (예: `0.5`로 무거운 효과의 비용 1/4). 전역 `render.render_scale`과 곱해짐 |
| `fit` | image, video, scene | `cover`(채우고 자름) · `contain`(레터박스) · `fill`(늘림) |
| `background` | image, video, shader, web | `contain` 시 여백 색 / 셰이더 첫 프레임 전에 보이는 색 / 웹 페이지의 투명한 부분 색 |
| `loop` | video | 반복 재생 |
| `audio` | video, web | `false`면 항상 음소거 |
| `speed` | video | 재생 속도 |
| `hwdec` | video | `false`면 소프트웨어 디코딩 강제 |

## 셰이더 배경

### Shadertoy GLSL (`.glsl`, `.frag`)
[Shadertoy](https://www.shadertoy.com)의 단일 패스(Image 탭) 셰이더와 호환됩니다.
`mainImage`만 작성하면 waywall이 프리앰블과 `main()`을 붙여 `#version 450` Vulkan GLSL로 만든 뒤
naga로 SPIR-V로 컴파일합니다.

```glsl
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    float d = distance(fragCoord, iPointer.xy);
    vec3 col = 0.5 + 0.5 * cos(iTime + uv.xyx + vec3(0, 2, 4));
    col += 0.4 * exp(-d * 0.01) * iPointer.z;               // 커서 주변 빛
    col += 0.6 * exp(-(iTime - iClick.z) * 3.0) * exp(-distance(fragCoord, iClick.xy) * 0.02) * step(0.5, iClick.w); // 클릭 파문 (w=0이면 클릭 전)
    fragColor = vec4(col, 1.0);
}
```

실제로 컴파일되는 소스는 `assets/shaders/shadertoy/prelude.glsl` + 사용자 셰이더 + `assets/shaders/shadertoy/epilogue.glsl`입니다.
프리앰블은 인스턴스 이름 없는 uniform 블록(`layout(set=0, binding=0) uniform Globals {…};`)을 선언하므로
셰이더에서는 Shadertoy처럼 그냥 `iTime`으로 씁니다. 에필로그의 `main()`이 `gl_FragCoord`를 왼쪽 아래 원점으로
바꿔 `mainImage`를 부릅니다. 두 파일은 사용자 assets 디렉터리에 같은 이름으로 두어 교체할 수 있습니다.

- 출력 색은 Shadertoy처럼 **변환 없이** 화면에 씁니다(sRGB가 아닌 swapchain 포맷). Shadertoy에서 보던 색 그대로입니다.
- 셰이더 안의 줄 번호는 사용자 파일 기준으로 오류에 표시됩니다.
- 아직 지원하지 않는 것: `iChannel0..3`(텍스처·버퍼 입력), 멀티패스, 사운드 셰이더. 쓰면 컴파일 오류가 됩니다.
- naga GLSL 프론트엔드가 처리하지 못하는 구문이 있으면 오류가 `waywall list`와 `set`의 응답에 표시됩니다.
  shaderc(glslang) 대체 컴파일러는 로드맵입니다.

### WGSL (`.wgsl`)
wgpu 네이티브 셰이더 언어입니다. 다음 엔트리를 작성합니다.

```wgsl
@fragment
fn main_image(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = frag_coord.xy / g.resolution.xy;
    return vec4<f32>(uv, 0.5 + 0.5 * sin(g.time), 1.0);
}
```

`g`는 waywall이 앞에 붙이는 `assets/shaders/shadertoy/prelude.wgsl`의 `var<uniform> g: Globals;` 이며 필드는 아래 표의 snake_case
(`resolution`, `time`, `time_delta`, `frame`, `date`, `mouse`, `pointer`, `click`, 그리고 속성 `u_<이름>`)입니다.
`frag_coord`는 WGSL 그대로 **왼쪽 위 원점**이고, `g.mouse`/`g.pointer`/`g.click`의 좌표는 GLSL과 같은 왼쪽 아래 원점입니다.
화면을 덮는 삼각형을 그리는 버텍스 셰이더는 `assets/shaders/fullscreen.wgsl`입니다.

### 제공 uniform

| GLSL 이름 | 타입 | 설명 |
|---|---|---|
| `iResolution` | `vec3` | 렌더 크기(물리 픽셀 × 유효 `render_scale`), z=1.0. 아래 좌표는 모두 이 픽셀 단위 |
| `iTime` | `float` | 시작 후 경과 초 (pause 중에는 멈춤) |
| `iTimeDelta` | `float` | 직전 프레임과의 간격(초) |
| `iFrame` | `int` | 프레임 번호 |
| `iFrameRate` | `float` | 직전 간격 기준 순간 fps (`1 / iTimeDelta`, 첫 프레임은 0) |
| `iDate` | `vec4` | 연, 월(0-11), 일, 자정 이후 초 |
| `iMouse` | `vec4` | **Shadertoy 규칙**(왼쪽 버튼만): xy=버튼 누른 동안의 위치, zw=누른 위치(누르는 중이면 z>0, 떼면 z<0. w는 누른 첫 프레임에만 양수) |
| `iPointer` | `vec4` | **waywall 확장**: xy=현재 포인터 위치(호버 포함), z=포인터가 배경 위에 있으면 1, w=눌린 버튼 비트마스크(1=좌, 2=우, 4=중) |
| `iClick` | `vec4` | **waywall 확장**: xy=마지막 클릭 위치, z=마지막 클릭 시각(`iTime` 기준), w=버튼(1=좌, 2=우, 4=중; 클릭 전에는 0) |

좌표계는 Shadertoy와 같이 **왼쪽 아래가 원점**입니다. `properties`는 `u_<이름>`으로 추가됩니다
(`float`→`float`, `int`→`int`, `bool`→`int`, `color`→`vec4`). 색은 0–1 범위의 RGBA이며 변환(선형화, premultiply)하지 않습니다.

`interactive = true`(셰이더 기본값)이면 배경 위에서 마우스를 움직일 때마다 데몬이 이벤트를 받습니다(정지 중에도).
입력에 반응하지 않는 셰이더는 `interactive = false`로 두면 그 비용도 없습니다.

## 씬 배경

빛, 파티클, 패럴랙스, 블룸을 선언적으로 조합하는 형식입니다. `project.toml`의 `type = "scene"`,
`entry`가 가리키는 `scene.toml`로 정의합니다.

### 렌더 파이프라인

```
 레이어(순서대로)            조명                  후처리
 image / shader / particles ─▶ 노멀맵 조명 ─▶ HDR 버퍼 ─▶ bloom ─▶ 톤매핑/노출 ─▶ 비네트 ─▶ 화면
       ▲                                (Rgba16Float)
 particles는 compute 셰이더로 GPU에서 시뮬레이션
```

- 내부는 HDR(`render.hdr_format`)로 그려서 밝기 1.0을 넘는 빛이 블룸으로 번지게 합니다.
- 파이프라인의 각 단계(fit, 조명, 블룸, 톤매핑, 비네트, 파티클)는 `assets/shaders/`의 WGSL 파일이며,
  사용자 assets 디렉터리에 같은 이름의 파일을 두어 교체할 수 있습니다([config.md](config.md#assets)).
- 씬 값의 우선순위: 사용자 `[types.scene]`/출력 설정 > `scene.toml` > 내장 `defaults.toml`.
  사용자가 `[types.scene.post] bloom`을 적으면 모든 씬의 블룸이 그 값을 씁니다.
- 좌표계는 `[scene]`의 설계 해상도 기준 픽셀, **왼쪽 위 원점**. 출력 크기에는 `fit`으로 맞춥니다.
- `"$이름"` 문자열은 `project.toml` properties 값으로 치환됩니다(숫자·색 필드에 사용 가능).

### 예시

```toml
[scene]
width  = 1920              # 설계 해상도
height = 1080
fit    = "cover"
clear  = "#05060a"

# ── 레이어: 아래에서 위 순서 ──────────────────────────────
[[layer]]
type       = "image"
id         = "sky"
texture    = "textures/sky.png"
position   = [960, 540]    # 중심 좌표
scale      = 1.05
parallax   = 0.01          # 포인터 위치에 따라 움직이는 비율 (깊이감)

[[layer]]
type       = "image"
id         = "trees"
texture    = "textures/trees.png"
normal_map = "textures/trees_n.png"   # 있으면 조명에 반응
position   = [960, 540]
scale      = 1.1
parallax   = 0.03
lit        = true

[[layer]]
type       = "particles"
id         = "fireflies"
max        = 4000
texture    = "textures/glow.png"
blend      = "add"         # normal | add | multiply | screen
emitter    = { shape = "box", position = [960, 700], size = [1920, 500], rate = "$density" }
lifetime   = [3.0, 7.0]    # [최소, 최대] 초
velocity   = { min = [-15, -25], max = [15, 5] }
gravity    = [0, -2]
turbulence = { strength = 20, scale = 0.004 }   # 노이즈 기반 흔들림
size       = { start = 6, end = 2 }
color      = { start = "#fff2a8", end = "#ffb03000" }   # RGBA, 끝에서 투명
emissive   = 3.0           # HDR 밝기 배율 → 블룸
pointer    = { mode = "attract", radius = 200, strength = 300 }   # none | attract | repel

[[layer]]
type       = "shader"
id         = "fog"
source     = "fog.glsl"    # Shadertoy 형식, 이 레이어 영역에 그림
blend      = "screen"
opacity    = 0.6

# ── 조명 ───────────────────────────────────────────────────
[[light]]
type      = "ambient"
color     = "#1a2030"

[[light]]
type      = "point"
position  = "pointer"      # 포인터를 따라감. 또는 [x, y]
color     = "#ffd29a"
intensity = 2.5
radius    = 450
height    = 120            # 노멀맵 조명 계산에 쓰는 z 높이
flicker   = 0.1            # 밝기 흔들림 정도

# ── 후처리 ─────────────────────────────────────────────────
[post]
exposure = 1.0
tonemap  = "agx"           # assets/shaders/tonemap/<이름>.wgsl
bloom    = { threshold = 1.0, intensity = 0.7, radius = 5 }
vignette = 0.35
```

### 레이어 공통 필드

생략하면 `defaults.toml` 값을 쓰고, 사용자 `[types.scene]`에 같은 필드가 있으면 그 값이 우선합니다.

| 필드 | 설명 |
|---|---|
| `id` | 고유 이름 (로그/오류 표시용) |
| `blend` | `normal` · `add` · `multiply` · `screen` |
| `opacity` | 불투명도 |
| `parallax` | 포인터가 화면 중심에서 벗어난 만큼 반대로 이동하는 비율 |
| `visible` | 표시 여부. `"$속성"`으로 토글 가능 |

### 레이어 타입

**image** — `texture`, `position`, `scale`(숫자 또는 `[x, y]`), `rotation`(도), `normal_map`, `lit`

**shader** — `source`(GLSL/WGSL), `rect`(`[x, y, w, h]`, 기본 전체). 셰이더 배경과 같은 uniform 사용

**particles**

| 필드 | 설명 |
|---|---|
| `max` | 최대 파티클 수 (GPU 버퍼 크기). 수십만 개까지 실용적 |
| `texture` | 스프라이트 이미지 |
| `sprite` | 절차적 스프라이트 셰이더(WGSL) 경로. `texture`와 둘 다 없으면 `defaults.toml`의 `sprite` 값 사용 (사용자 `[types.scene.particles] sprite`가 있으면 그것이 우선) |
| `emitter` | `shape`(`point` · `box` · `circle`), `position`, `size`/`radius`, `rate`(초당), `burst`(클릭 시 방출 개수) |
| `lifetime` | `[최소, 최대]` 초 |
| `velocity` | `{min, max}` 초기 속도 범위 (px/s) |
| `gravity` | 가속도 (px/s²) |
| `drag` | 감속 계수 |
| `turbulence` | `{strength, scale}` curl noise |
| `size`, `color` | `{start, end}` 수명에 따른 보간 |
| `emissive` | HDR 밝기 배율 |
| `pointer` | `{mode, radius, strength}` 포인터 상호작용 |

### 조명 타입

| 타입 | 필드 |
|---|---|
| `ambient` | `color` |
| `point` | `position`(`[x,y]` 또는 `"pointer"`), `color`, `intensity`, `radius`, `height`, `flicker` |
| `directional` | `direction`(`[x, y, z]`), `color`, `intensity` |

조명은 `lit = true`인 image 레이어(노멀맵이 있으면 입체감 포함)에 적용됩니다. 파티클은 `emissive`로
스스로 빛나며 블룸으로 번집니다.

### 상호작용 요약
- 커서 이동: `parallax`, `light.position = "pointer"`, `particles.pointer`
- 클릭: `emitter.burst`, 셰이더 레이어의 `iClick`

### 성능 팁
- 무거운 블룸/셰이더 레이어는 `render_scale = 0.75` 같은 값으로 비용을 크게 줄일 수 있습니다.
- `fps = 30`으로도 대부분의 배경은 충분히 부드럽습니다.
- 텍스처는 출력 해상도 이상으로 크게 만들 필요가 없습니다.

## 웹 배경

일반 HTML 페이지를 웹 헬퍼(WebKitGTK)가 렌더링합니다. 마우스 이동·클릭·휠은 WebKit이 직접 받으므로
평범한 DOM 이벤트(`mousemove`, `click`, …)로 상호작용을 구현합니다.

### 동작 규칙
- 뷰포트 = 모니터 논리 크기, `devicePixelRatio` = 모니터 스케일
- 배경은 불투명. 페이지가 투명한 부분은 `background` 옵션 색으로 채움
- 키보드 입력은 받지 않음
- 로컬 파일은 `file://`로 로드되며 같은 디렉터리 안의 리소스 접근 허용
- 오디오(`<audio>`, `<video>`, WebAudio) 재생 가능, waywall 볼륨/음소거 반영

### `window.waywall` API

페이지 로드 전에 주입됩니다. 없어도 페이지는 동작해야 합니다.

```js
window.waywall = {
  version: "0.1.0",
  output: "DP-1",
  properties: { density: 1.0, tint: "#88aaff" },
  paused: false,
  volume: 0.5,        // 0.0 ~ 1.0
  muted: false,
};

window.addEventListener("waywall-pause",      () => { /* 애니메이션 정지 */ });
window.addEventListener("waywall-resume",     () => { /* 재개 */ });
window.addEventListener("waywall-properties", (e) => { /* e.detail = 바뀐 속성 */ });
window.addEventListener("waywall-volume",     (e) => { /* e.detail = {volume, muted} */ });
```

pause 시 waywall은 페이지의 `<audio>`/`<video>`를 일시정지하고 `waywall-pause`를 보냅니다.
`requestAnimationFrame`은 자연히 멈추지만 `setInterval` 같은 작업은 페이지가 이벤트를 받아 직접 멈춰야 합니다.

## 동영상 배경

GStreamer로 재생합니다. VA-API 하드웨어 디코딩 결과를 복사 없이 화면에 보내고, 기본으로 무한 반복합니다.
지원 코덱은 GPU 하드웨어 디코더에 따라 다르며(H.264/HEVC/VP9/AV1), 없으면 소프트웨어 디코딩으로 폴백합니다.

## 이미지 배경

정적 이미지는 한 번 그리고 나면 아무 비용이 없습니다. 크기 맞춤은 컴포지터가 처리합니다.
GIF는 영상처럼 재생되며 pause 시 멈춥니다.
