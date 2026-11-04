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

`type = "shader"`인 디렉터리형 배경은 `[inputs]`로 텍스처 채널을 넣을 수 있습니다([텍스처 입력](#텍스처-입력-ichannel0ichannel3)).

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
- 아직 지원하지 않는 것: 멀티패스(Buffer A–D), 큐브맵·볼륨·키보드·오디오 채널. 쓰면 컴파일 오류가 됩니다. 텍스처 채널은 [아래](#텍스처-입력-ichannel0ichannel3).
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
`frag_coord`는 WGSL 그대로 **왼쪽 위 원점**이고 렌더 타깃 기준입니다. 그리는 영역 안의 좌표는 `frag_coord.xy - g.origin`입니다
(셰이더 배경에서는 `g.origin`이 0이고, 씬 shader 레이어에서는 레이어 영역의 왼쪽 위). 배경과 레이어 양쪽에서 쓸 셰이더는 항상 빼 두면 됩니다.
`g.mouse`/`g.pointer`/`g.click`의 좌표는 GLSL과 같은 왼쪽 아래 원점이며 그리는 영역 기준입니다.
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


### 텍스처 입력 (`iChannel0`–`iChannel3`)

디렉터리형 셰이더 배경은 `project.toml`의 `[inputs]`로 채널에 이미지를 넣습니다(배경 디렉터리 기준 경로).
단일 파일 셰이더와 지정하지 않은 채널은 1×1 투명 텍스처이고 해상도는 0입니다.

```toml
[inputs]
iChannel0 = "textures/noise.png"
iChannel1 = { source = "textures/rock.jpg", filter = "nearest", wrap = "clamp" }
```

| 키 | 설명 |
|---|---|
| `source` | 이미지 경로. 값을 문자열 하나로 쓰면 이것만 지정한 것 |
| `filter` | `linear` · `nearest` |
| `wrap` | 0–1 밖을 읽을 때: `repeat` · `clamp` · `mirror` |

생략한 `filter`·`wrap`은 `[types.shader.channel]`(사용자 값 > `defaults.toml`)을 쓰며, 사용자가 그 섹션에 적은 값은 배경의 값보다 우선합니다.

- **GLSL:** Shadertoy처럼 `texture(iChannel0, uv)`, `texelFetch`, `textureLod`, `iChannelResolution[n]`(`vec3`, 픽셀 크기)을 씁니다.
  이미지는 Shadertoy 기본값처럼 **상하를 뒤집어** 올리므로 `uv = (0, 0)`이 이미지의 왼쪽 아래입니다.
- **WGSL:** `channel0`–`channel3`(`texture_2d<f32>`)과 `channel0_sampler`–`channel3_sampler`,
  `g.channel_resolution[n]`을 씁니다. `frag_coord`처럼 `uv = (0, 0)`이 이미지의 왼쪽 위입니다.
- 셰이더 배경의 출력이 변환 없이 화면에 쓰이는 것처럼, 채널 값도 Shadertoy와 같이 **변환 없이** 읽힙니다
  (sRGB 값 그대로, 알파를 곱하지 않음).

#### 채널 개수 제한 해제 (설계)

> **설계 단계.** 지금은 `iChannel0`–`iChannel3`만 받고 `iChannel4` 이상은 거절됩니다. 계획은
> [roadmap.md](roadmap.md#m8b--셰이더-채널-개수-제한-해제-설계). 구현하면 이 절의 제목·앵커도 `iChannelN`으로 바꿉니다.

4개 제한은 Shadertoy를 따라 한 것일 뿐 GPU 한계가 아닙니다. waywall 배경은 waywall에서 돌리므로 Shadertoy 사이트로 되돌려
올리는 호환은 목표가 아니고(`iPointer`·`iClick`·`u_<속성>`도 이미 waywall 확장), **Shadertoy 셰이더를 가져다 쓰는 쪽만** 지킵니다.

- `[inputs]`(셰이더 배경), shader 레이어 `inputs`, `shader` 효과 `inputs`에 `iChannel<N>`을 원하는 만큼 씁니다(N은 0부터의 정수).
- 셰이더가 받는 채널 수 = `max(4, 쓴 가장 큰 N + 1)`. `iChannel0`–`iChannel3`은 늘 선언되므로 입력을 지정하지 않은 Shadertoy 셰이더도
  지금과 똑같이 컴파일됩니다. 중간에 비운 채널(예: 0과 7만 지정)은 1×1 투명 텍스처, 해상도 0입니다.
- 상한은 코드에 숫자로 두지 않고 GPU 어댑터 한도(`max_sampled_textures_per_shader_stage`, `max_samplers_per_shader_stage`)에서 정합니다.
  넘으면 그 한도 값과 함께 거절됩니다.
- GLSL은 `iChannel<N>`·`iChannelResolution[N]`, WGSL은 `channel<N>`·`channel<N>_sampler`·`g.channel_resolution[N]` — 이름 규칙은 0–3과 같습니다.
- 바인딩 규칙: 0 = uniform, 1–N = 채널 텍스처, N+1–2N = 채널 샘플러(지금의 1–4 / 5–8을 일반화).
- 채널 선언은 waywall이 채널 수만큼 만들어 프리앰블의 표시 줄(`// @waywall:channels`) 자리에 넣고, `iChannelResolution` 배열 크기도
  그 수로 정합니다. 사용자 assets의 프리앰블 교체 파일도 이 표시 줄을 두면 됩니다.

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
  값 전체가 `"$이름"`인 문자열만 치환하고, 정의되지 않은 속성 이름은 오류입니다. `bool` 속성은 `visible` 같은 bool 필드에 씁니다.
- 움직일 것이 없으면 잠듭니다. 패럴랙스가 멈추고 시간에 따라 변하는 레이어(파티클, shader 레이어 등)가 없으면
  frame 콜백을 더 요청하지 않아 정적 이미지처럼 비용이 0이 되고, 포인터가 움직이면 다시 그립니다.

### 색과 알파

- 텍스처와 색 값(`clear`, 파티클 `color` 등)은 **sRGB**로 해석해 선형 값으로 바꾼 뒤 HDR 버퍼에서 계산합니다.
- 텍스처는 읽을 때 알파를 곱해 둡니다(premultiplied). 투명한 가장자리가 확대·회전에서 어둡게 번지지 않습니다.
- 톤매핑 결과는 sRGB로 인코딩해 화면에 씁니다. 셰이더 배경(출력값 그대로)과 달리, 씬의 이미지 레이어는
  원본 그림과 같은 색으로 보입니다(노출 1.0, `tonemap = "none"` 기준).
- 블렌드 모드(premultiplied 기준, `s` = 레이어, `d` = 아래 장면, `a` = 레이어 알파 × `opacity`):

| `blend` | 결과 |
|---|---|
| `normal` | `s + d × (1 − a)` |
| `add` | `s + d` |
| `multiply` | `s × d + d × (1 − a)` |
| `screen` | `s + d × (1 − s)` |

### `[scene]`

| 키 | 설명 |
|---|---|
| `width`, `height` | 설계 해상도(px). 필수. 레이어 좌표의 기준 |
| `fit` | 설계 영역을 출력에 맞추는 방식: `cover`(잘라서 채움) · `contain`(전부 보이게, 남는 곳은 `clear` 색) · `fill`(늘림) |
| `clear` | 레이어 아래 바탕색. 첫 프레임 전에도 이 색이 보임 |

`fit`과 `clear`는 배경 옵션이라 사용자 `[types.scene]`·출력 설정이 이깁니다. `contain`이면 설계 영역 밖(레터박스)에는
레이어를 그리지 않습니다. `fps`·`render_scale`·`interactive`는 `project.toml`의 `[options]`에 둡니다.

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

생략하면 `defaults.toml`의 `[types.scene.layer]` 값을 쓰고, 사용자 `[types.scene.layer]`에 같은 필드가 있으면
그 값이 모든 레이어에 우선합니다(예: `parallax = 0.0`으로 모든 씬의 패럴랙스 끄기). 타입별 필드는
`[types.scene.<레이어 타입>]`(예: `[types.scene.image]`)이 같은 역할을 합니다. 모르는 필드는 오류입니다.

| 필드 | 설명 |
|---|---|
| `id` | 고유 이름 (로그/오류 표시용, 다른 레이어가 참조할 때). 생략하면 `layer<번호>`(0부터) |
| `blend` | `normal` · `add` · `multiply` · `screen` ([색과 알파](#색과-알파)) |
| `opacity` | 불투명도 (0–1) |
| `parallax` | 포인터가 화면 중심에서 벗어난 만큼 반대로 이동하는 비율 ([패럴랙스](#패럴랙스)) |
| `visible` | 표시 여부. `"$속성"`으로 토글 가능 |
| `clip` | 다른 레이어 `id`. 그 레이어의 모양 안에만 그림 (image·shader, [clip](#clip)). 예: 눈동자를 흰자 안에만 |
| `follow` | 포인터 쪽으로 제한된 거리만큼 움직임 (image·shader, [포인터 따라가기](#포인터-따라가기-follow)) |
| `effects` | 레이어에 차례로 적용할 효과 목록 (image, [레이어 효과](#레이어-효과-effects)) |

### 레이어 타입

**image**

| 필드 | 설명 |
|---|---|
| `texture` | 이미지 파일 (배경 디렉터리 기준). 필수. `png` `jpg` `webp` `bmp` |
| `position` | 이미지 중심의 좌표 `[x, y]`. 생략하면 설계 영역 중앙 |
| `scale` | 숫자 또는 `[x, y]`. 화면 크기 = 텍스처 픽셀 크기 × `scale` (설계 px) |
| `rotation` | 중심 기준 회전(도, 시계 방향) |
| `lit` | `true`면 `[[light]]`의 영향을 받음 ([조명](#조명)) |
| `normal_map` | 노멀맵 이미지 (배경 디렉터리 기준). `lit`일 때 입체감 ([조명](#조명)) |

`scale`·`rotation`의 내장값은 `[types.scene.image]`에 있습니다.

**shader**

셰이더 배경과 같은 형식(Shadertoy GLSL 또는 WGSL)의 셰이더를 사각형 영역에 그립니다. 같은 uniform(`iTime`, `iPointer`, `u_<속성>` 등)을 씁니다.

| 필드 | 설명 |
|---|---|
| `source` | 셰이더 파일 (배경 디렉터리 기준). 필수. 확장자로 언어 판별(`.wgsl`이면 WGSL) |
| `rect` | 그릴 영역 `[x, y, 폭, 높이]` (설계 px, 왼쪽 위 기준). 생략하면 설계 영역 전체 |
| `inputs` | 채널 입력 (아래). 셰이더 배경의 `[inputs]`와 같은 형식에 씬 전용 값이 더해짐 |
| `render_scale` | 이 레이어를 그리는 해상도 배율 (0보다 큼). 무거운 효과를 `0.5`로 그리면 비용이 약 1/4. 그린 결과를 `rect` 크기로 늘려 합성 |

```toml
inputs = { iChannel0 = "below", iChannel1 = "textures/light_mask.png", iChannel2 = "trees" }
```

| 값 | 의미 |
|---|---|
| `"below"` | 이 레이어 아래에 지금까지 그려진 장면(HDR, 후처리 전) |
| 레이어 `id` | 이 레이어보다 **아래**에 있는 그 레이어 하나만 투명 바탕에 그린 결과. `visible = false`인 레이어도 입력으로 쓸 수 있음(화면에는 안 그림) |
| 경로 | 이미지 파일 (배경 디렉터리 기준) |

- 셰이더는 `rect`를 렌더 해상도로 옮긴 크기의 자기 HDR 텍스처에 그립니다(`iResolution` = 그 크기, GLSL `fragCoord`는 `rect` 기준,
  WGSL은 `frag_coord.xy - g.origin`).
  그 결과를 image 레이어처럼 `blend`·`opacity`·`parallax`로 장면에 합성합니다.
- 출력은 **선형, premultiplied** RGBA로 해석합니다. 1보다 큰 값은 블룸으로 번집니다. 화면에 그대로 쓰이는 셰이더 배경과 다릅니다.
- `"below"`와 레이어 입력은 장면에서 **이 레이어의 `rect` 영역만** 잘라 낸 것이라 `uv = fragCoord / iResolution.xy`로 읽으면
  바로 아래 픽셀입니다. 값은 선형 premultiplied이며 1을 넘을 수 있습니다. 이미지 입력은 image 레이어와 같이 선형화·premultiply한 값입니다.
- 방향 규칙은 셰이더 배경과 같습니다: GLSL은 모든 채널의 `uv = (0, 0)`이 왼쪽 아래, WGSL은 왼쪽 위.
- `iPointer`·`iMouse`·`iClick`의 좌표는 `rect` 기준 렌더 px입니다.
- `render_scale`이 1이 아니면 `iResolution`·`fragCoord`·포인터 좌표와 `"below"`·레이어 입력이 모두 그 해상도 기준입니다
  (`rect`의 렌더 크기 × `render_scale`). 입력은 그 해상도로 줄여서(가장 가까운 픽셀) 넘깁니다.
- 생략한 채널 `filter`·`wrap`은 `[types.scene.shader] channel`에서 옵니다.
- shader 레이어가 있으면 씬은 시간에 따라 변하므로 잠들지 않습니다.

**particles**

필드를 생략하면 `defaults.toml`의 `[types.scene.particles]` 값을 씁니다(사용자의 같은 섹션이 있으면 그 값이 모든 파티클 레이어에 우선).
좌표·크기·속도는 설계 px, 시간은 초입니다.

| 필드 | 설명 |
|---|---|
| `max` | 최대 파티클 수(GPU 버퍼 크기). 수십만 개까지 실용적. 모자라면 가장 오래된 파티클부터 새 파티클로 바뀜 |
| `texture` | 스프라이트 이미지 (배경 디렉터리 기준) |
| `sprite` | 절차적 스프라이트 셰이더(WGSL, 배경 디렉터리 기준). 아래 [스프라이트](#스프라이트) |
| `emitter` | 방출기 (아래 표) |
| `lifetime` | 수명. 숫자 또는 `[최소, 최대]` (파티클마다 무작위) |
| `velocity` | `{ min = [x, y], max = [x, y] }` 초기 속도 범위 (px/s, 성분마다 무작위) |
| `gravity` | `[x, y]` 가속도 (px/s², y가 아래로 +) |
| `drag` | 감속 계수(1/s). 속도가 초당 `e^(−drag)`배로 줄어듦 |
| `turbulence` | `{ strength, scale, speed }` curl noise 흐름. `strength`는 흐름 속도(px/s), `scale`은 노이즈 주파수(1/px), `speed`는 흐름이 바뀌는 빠르기 |
| `size` | 지름(px). 숫자 또는 `{ start, end }` (수명에 따라 보간) |
| `color` | 색(RGBA). 값 하나 또는 `{ start, end }`. 끝 색의 알파를 0으로 하면 사라지며 꺼짐 |
| `emissive` | 색에 곱하는 HDR 밝기 배율. 1보다 크면 블룸으로 번짐 |
| `pointer` | `{ mode, radius, strength }` 포인터 상호작용. `mode`: `none` · `attract`(끌어당김) · `repel`(밀어냄). `radius` 안에서 `strength`(px/s²)의 힘, 가장자리로 갈수록 약해짐 |
| `max_step` | 한 프레임 시뮬레이션의 최대 시간 간격(초). 오래 멈췄다 깨어나도 파티클이 튀거나 한꺼번에 나오지 않게 함 |

`emitter`:

| 필드 | 설명 |
|---|---|
| `shape` | `point` · `box` · `circle` |
| `position` | 중심 `[x, y]`. 생략하면 설계 영역 중앙 |
| `size` | `box`의 `[폭, 높이]`. 생략하면 설계 영역 전체 |
| `radius` | `circle`의 반지름. `circle`이면 필수 (원 안에서 고르게) |
| `rate` | 초당 방출 수. `0`이면 클릭 때만 방출 |
| `burst` | 왼쪽 클릭마다 **클릭한 위치**에서 방출하는 수 (`shape`와 관계없이 한 점). `0`이면 클릭에 반응하지 않음 |

#### 스프라이트

파티클 하나는 화면을 향한 사각형이며 `size`가 그 한 변입니다. 무늬는 다음 순서로 정합니다.

1. 레이어의 `texture` — 이미지 색 × 파티클 색
2. 레이어의 `sprite` — 셰이더가 돌려준 값 × 파티클 색
3. 둘 다 없으면 `[types.scene.particles] sprite`(사용자 값 > `defaults.toml`, **assets 기준 경로**. 내장은 `sprites/soft-circle.wgsl`)

`sprite`는 레이어가 정한 무늬를 바꾸지 않습니다. 사용자의 `[types.scene.particles] sprite`는 무늬를 정하지 않은 레이어의 기본값일 뿐입니다.
스프라이트 셰이더는 함수 하나만 정의합니다. `uv`는 사각형의 왼쪽 위가 `(0, 0)`, 오른쪽 아래가 `(1, 1)`입니다.

```wgsl
// 반환값: 선형, premultiplied RGBA. 파티클 색(× emissive × opacity)과 곱해짐
fn sprite(uv: vec2<f32>) -> vec4<f32> {
    let d = length(uv * 2.0 - 1.0);
    return vec4<f32>(pow(1.0 - smoothstep(0.0, 1.0, d), 2.0));
}
```

#### 동작

- 시뮬레이션은 compute 셰이더로 GPU에서 하므로 파티클 수와 관계없이 CPU 비용은 프레임마다 거의 같습니다.
- 파티클 버퍼는 고리 버퍼입니다. 새 파티클은 다음 자리부터 차례로 들어가고, `max`가 `rate × 최대 수명`보다 작으면
  수명이 남은 파티클이 먼저 바뀝니다.
- `rate = 0`이고 살아 있는 파티클이 없으면 그 레이어는 잠듭니다(burst 전용 레이어는 클릭 후 수명이 다하면 다시 비용 0).
  `rate > 0`인 레이어가 있으면 씬은 계속 그립니다.
- 포인터 상호작용과 burst 위치는 레이어의 패럴랙스 이동을 뺀 좌표로 계산해, 화면에서 커서가 있는 곳에 맞습니다.
- 클릭(burst)과 `pointer`는 `interactive = true`일 때만 동작합니다.

### 조명

`[[light]]`는 `lit = true`인 image 레이어에만 적용됩니다. 파티클·shader 레이어·`lit`이 아닌 이미지는 조명과 관계없이
자기 색 그대로입니다(파티클은 `emissive`로 스스로 빛나며 블룸으로 번짐). `lit = true`인 레이어가 있는데 `[[light]]`가 없으면 오류입니다.

| 타입 | 필드 |
|---|---|
| `ambient` | `color`, `intensity` |
| `point` | `position`(`[x, y]` 또는 `"pointer"`), `color`, `intensity`, `radius`, `height`, `flicker`, `flicker_speed` |
| `directional` | `direction`(`[x, y, z]`), `color`, `intensity` |

생략한 필드는 `[types.scene.light.<타입>]`(사용자 값 > `defaults.toml`)을 쓰며, 사용자가 그 섹션에 적은 값은 씬의 값보다 우선합니다.

`lit` 레이어의 색(선형, premultiplied)은 다음과 같이 계산합니다. 알파는 바뀌지 않습니다.

```
색 = 텍스처 색 × ( Σ ambient + Σ point + Σ directional )
ambient     = color × intensity
point       = color × intensity × 밝기흔들림 × 감쇠 × max(N·L, 0)
              L = normalize([x, y, height] − [픽셀 x, 픽셀 y, 0]),  감쇠 = clamp(1 − (d / radius)², 0, 1)²,  d = 평면 거리
directional = color × intensity × max(N·L, 0),   L = −normalize(direction)
```

- 좌표는 설계 px이고 x는 오른쪽, y는 **아래**, z는 화면에서 보는 사람 쪽이 +입니다. 그래서 `direction = [1, 1, -1]`은
  왼쪽 위 앞쪽에서 오른쪽 아래 안쪽으로 비추는 빛입니다. `height`는 점광원이 화면 위로 떠 있는 높이입니다.
- `N`은 노멀맵(`normal_map`)이 있으면 그 값, 없으면 화면을 똑바로 보는 `[0, 0, 1]`입니다. 노멀맵은 **OpenGL 규약**
  (R = 오른쪽, G = 위, B = 앞)의 탄젠트 공간이며 레이어 `rotation`만큼 함께 돌아갑니다. 노멀맵은 선형 값으로 읽고, 텍스처와 같은 uv로 대응합니다
  (크기는 달라도 됨).
- `position = "pointer"`: 포인터가 배경 위에 있으면 그 위치(설계 px)를 따라가고, 나가면 마지막 위치에 머뭅니다.
- `flicker`(0–1): 밝기를 `1 + flicker × 노이즈`배로 흔듭니다(노이즈는 −1–1의 매끄러운 값, 초당 약 `flicker_speed`번 바뀜).
  0보다 크면 씬이 계속 그려집니다.
- 조명 계산 함수는 `assets/shaders/scene/lighting.wgsl`이며 교체할 수 있습니다([config.md](config.md#assets)).

### 후처리 (`[post]`)

```
HDR 장면 ─▶ + 블룸 × intensity ─▶ × exposure ─▶ tonemap ─▶ × 비네트 ─▶ 화면(sRGB)
```

| 키 | 설명 |
|---|---|
| `exposure` | 톤매핑 전에 곱하는 밝기 |
| `tonemap` | `assets/shaders/tonemap/<이름>.wgsl` (`none` · `reinhard` · `aces` · `agx`, 새 파일 = 새 이름) |
| `bloom` | `{ threshold, intensity, radius }`. 밝기가 `threshold`를 넘는 부분(넘는 만큼)만 번짐. `radius`는 축소 단계 수(클수록 넓게, 한 단계마다 해상도 절반). `intensity = 0`이면 블룸 패스를 만들지 않아 비용 없음 |
| `vignette` | 0–1. 가장자리를 어둡게 하는 정도. 모양은 `assets/shaders/vignette.wgsl` |

`[post]`의 값은 배경 옵션이라 사용자 `[types.scene.post]`가 이깁니다.

### 패럴랙스

포인터가 배경 위에 있으면 레이어를 포인터 반대쪽으로 옮겨 깊이감을 냅니다. `parallax`가 클수록 가까운 레이어처럼 많이 움직입니다.

```
이동량 = −(포인터 − 설계 영역 중심) × parallax      (설계 px)
```

- 포인터가 배경 밖으로 나가면 이동량의 목표가 0(제자리)이 됩니다.
- 이동은 목표를 향해 지수적으로 따라갑니다. 시간 상수는 `[types.scene] pointer_smoothing`(초, `0`이면 즉시)이며
  목표와의 차이가 `[types.scene] settle_px`(출력 픽셀)보다 작아지면 멈추고 잠듭니다.
- 레이어가 움직여도 빈 곳이 보이지 않도록 `scale`을 1보다 조금 크게 두세요
  (가장자리에서 이동량은 `설계 폭/2 × parallax`).

### 구현 상태

이 문서의 씬 기능은 [캐릭터 레이어](#캐릭터-레이어-live2d--vrm--spine-설계)(설계 단계)를 빼고 모두 동작합니다. 레이어 타입이 지원하지 않는 필드(예: particles의 `clip`)를 쓰면
배경이 거절되고 `set` 응답과 `list`에 이유가 표시됩니다.

### 레이어 효과 (`effects`)

image 레이어의 이미지를 그리기 전에 바꾸는 셰이더입니다. 목록 순서대로 적용하며(앞 효과의 결과가 다음 효과의 입력), 시간에 따라
변하므로 효과가 있는 씬은 잠들지 않습니다.

```toml
effects = [
  { type = "sway", mask = "textures/hair_mask.png", strength = 6, speed = 0.8, scale = 0.01 },
]
```

| 필드 | 설명 |
|---|---|
| `type` | 효과 이름. `assets/shaders/effects/<type>.wgsl` 파일이거나 `shader`(직접 쓴 셰이더) |
| `mask` | 흑백 이미지 (배경 디렉터리 기준, R 채널, 흰색 = 효과 100%). 레이어 이미지와 같은 uv로 대응. 생략하면 전체에 적용 |
| 그 밖 | 효과마다 다름 (아래) |

내장 효과:

| 타입 | 필드 | 설명 |
|---|---|---|
| `sway` | `strength`(px), `speed`, `scale`(1/px), `direction`(`[x, y]`) | 매끄러운 노이즈로 이미지를 `direction` 쪽으로 밀어 바람에 흔들리게 함. 미는 거리 = `strength × 마스크`라 마스크가 검은 곳(뿌리 쪽)은 고정 |
| `shader` | `source`, `inputs` | 직접 쓴 효과 셰이더(Shadertoy GLSL 또는 WGSL). `iChannel0` = 효과를 적용할 이미지, `inputs`로 `iChannel1`–`iChannel3`에 이미지(경로)를 넣음. 생략한 채널 `filter`·`wrap`은 shader 레이어처럼 `[types.scene.shader] channel`(기본 `wrap = "clamp"`)에서 옴. 결과를 마스크로 원본과 섞음 |

생략한 필드는 `[types.scene.effects.<type>]`(사용자 값 > `defaults.toml`)을 쓰며, 사용자가 그 섹션에 적은 값은 씬의 값보다 우선합니다.

**효과 파일 만들기.** `assets/shaders/effects/<이름>.wgsl`(사용자 assets 디렉터리)에 파일을 두면 `type = "<이름>"`으로 쓸 수 있습니다.
화면을 덮는 삼각형(`fullscreen.wgsl`)으로 이미지 크기의 출력에 그리며, 아래를 선언합니다.

```wgsl
struct Effect {
    time: f32,             // waywall이 채움: 배경 시계(초)
    size: vec2<f32>,       // waywall이 채움: 레이어의 화면 크기 (설계 px)
    resolution: vec2<f32>, // waywall이 채움: 이미지 크기 (px)
    strength: f32,         // 그 밖의 멤버는 효과 표의 같은 이름 필드 (숫자 또는 숫자 배열)
}
@group(0) @binding(0) var<uniform> fx: Effect;
@group(0) @binding(1) var src: texture_2d<f32>;      // 입력 이미지 (선형, premultiplied)
@group(0) @binding(2) var src_sampler: sampler;
@group(0) @binding(3) var mask: texture_2d<f32>;     // 마스크 (없으면 1×1 흰색)

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { … }
```

효과 표의 필드와 구조체 멤버는 이름으로 짝지어지며, 멤버가 없는 필드나 값이 없는 멤버는 오류입니다. 내장 `sway.wgsl`이 예입니다.

### `clip`

image·shader 레이어에서 다른 레이어 `id`를 적으면, 그 레이어를 단독으로 그린 결과의 **알파를 곱해** 그 모양 안에만 보이게 합니다
(가장자리가 반투명하면 부드럽게 잘림). 예: 눈동자를 흰자 안에만.

- 대상은 이 레이어보다 **아래**의 image·particles 레이어입니다. `visible = false`인 레이어도 됩니다(화면에는 안 그림).
- 장면 버퍼에 알파가 있어야 하므로 `render.hdr_format = "rg11b10ufloat"`에서는 오류입니다.
- 대상 레이어의 패럴랙스·`follow`도 반영된 현재 모양으로 자릅니다.

### 포인터 따라가기 (`follow`)

image·shader 레이어를 기준점에서 포인터 방향으로 최대 `max_offset`만큼 옮깁니다. 캐릭터의 눈동자처럼 "쳐다보는" 움직임용입니다.
`parallax`(반대 방향, 깊이감)와 달리 포인터를 **향해** 움직이고, 거리에 상한이 있습니다. 둘을 같이 쓰면 더해집니다.

```toml
follow = { anchor = [812, 403], max_offset = [6, 4], range = 600, smoothing = 0.12, on_leave = "center" }
```

| 필드 | 설명 |
|---|---|
| `anchor` | 기준점(눈 중심) 좌표. 생략하면 레이어 중심(image는 `position`, shader는 `rect` 중심). 레이어의 패럴랙스 이동을 따라감 |
| `max_offset` | 최대 이동량 `[x, y]` (px). 필수 |
| `range` | 포인터가 기준점에서 이 거리(px) 이상 떨어지면 최대 이동 |
| `smoothing` | 따라가는 데 걸리는 시간 상수(초). `0`이면 즉시 |
| `on_leave` | 포인터가 배경 밖으로 나가면 `center`(제자리로) · `stay`(마지막 위치 유지) |

이동량 = `normalize(포인터 − anchor) × min(1, 거리 / range) × max_offset`. 생략한 필드는 `[types.scene.follow]` 값을 씁니다.
목표에 닿으면(`[types.scene] settle_px` 미만) 멈추고 잠듭니다.

### 예: 햇빛 + 흔들리는 머리카락 + 따라보는 눈

실제로 동작하는 예제: [examples/scenes/character/](../examples/scenes/character/).

원본 그림을 레이어로 나눈 PNG가 필요합니다(배경, 캐릭터, 흰자, 눈동자, 머리카락 마스크).

```toml
[[layer]]
type = "image"
id = "photo"
texture = "textures/character.png"
effects = [{ type = "sway", mask = "textures/hair_mask.png", strength = 5 }]

[[layer]]
type = "image"
id = "eye_white"
texture = "textures/eye_white.png"

[[layer]]
type = "image"
id = "pupils"
texture = "textures/pupils.png"
clip = "eye_white"
follow = { anchor = [812, 403], max_offset = [6, 4] }

[[layer]]
type = "shader"
id = "sunlight"
source = "sunlight.glsl"          # iChannel0의 밝은 곳에서 빛줄기를 뿜는 셰이더
inputs = { iChannel0 = "below" }
blend = "screen"
```

### 캐릭터 레이어 (live2d · vrm · spine, 설계)

> **설계 단계, 아직 구현되지 않음.** 쓰면 이유와 함께 거절됩니다. 계획은 [roadmap.md](roadmap.md#m9--live2d-레이어-설계)의 M9(Live2D)·M10(VRM)·M11(Spine).
> 필드 이름과 구성은 구현하며 바뀔 수 있습니다.

리깅된 캐릭터 모델을 레이어로 띄웁니다. `live2d`는 일러스트가 그대로 움직이는 2D 모델(`.moc3`), `vrm`은 3D 툰 모델(`.vrm`),
`spine`은 본·메시 변형 기반 2D 스켈레탈 애니메이션(Spine `.skel`/`.json` + `.atlas`)입니다.

**공통 규칙**

- 모델은 `rect` 크기(× `render_scale`)의 자기 HDR 텍스처에 그린 뒤 텍스처 경유 shader 레이어처럼 장면에 합성합니다.
  그래서 공통 필드 `blend`·`opacity`·`parallax`·`visible`과 `clip`이 그대로 적용되고, 블룸·톤매핑을 함께 받으며,
  레이어 순서로 파티클을 모델 앞이나 뒤에 둡니다. 다른 shader 레이어의 입력(레이어 `id`)으로도 쓸 수 있습니다.
- 출력은 shader 레이어와 같이 선형, premultiplied RGBA입니다. 모델 텍스처는 image 레이어처럼 sRGB로 읽어 선형화합니다.
- **시선(`look_at`):** 포인터 쪽을 봅니다. 계산은 [`follow`](#포인터-따라가기-follow)와 같은 모델입니다
  (`anchor`·`range`·`smoothing`·`on_leave`, 방향 = `normalize(포인터 − anchor) × min(1, 거리 / range)`). 이 값을 모델의
  시선·머리 회전으로 옮기는 방법만 타입마다 다릅니다. `follow`는 이 레이어 타입에 쓰지 않습니다(레이어 자체를 옮기려면 `parallax`).
- **클릭(`on_click`):** 왼쪽 클릭한 곳이 모델의 어느 부위인지 판정해 그 부위의 모션을 한 번 재생하고 대기 모션으로 돌아갑니다.
  `interactive = true`일 때만 동작합니다. 형식은 세 타입 공통입니다.
  - 문자열 하나(`on_click = "Tap"`): 모델 **어디를 눌러도** 그 모션. 부위 정보가 없는 모델도 이것만으로 됩니다.
  - 표(`on_click = { 부위 = "모션", … }`): 키는 타입별 부위 이름(live2d HitArea·drawable, vrm 본, spine 바운딩 박스).
    특별한 키 `"*"`는 다른 키에 걸리지 않은 모델 위 클릭입니다.
  - 판정 순서: 부위 키 → `"*"`. 부위가 겹치면 화면에서 위에 그려진 쪽이 이깁니다. 모델 밖(투명한 곳) 클릭은 무시하고 아래 레이어로 넘어갑니다
    (파티클 `burst` 등은 원래대로 동작).
  - "모델 위"는 지금 보이는 메시(불투명도가 0보다 큰 drawable·삼각형) 안인지로 판정합니다. GPU 읽기 없이 CPU에서 계산합니다.
- 대기 모션·눈 깜빡임·숨쉬기·물리로 늘 움직이므로 이 레이어가 있는 씬은 잠들지 않습니다. 비용은 사용자가 `fps`·`render_scale`로 정합니다(기본 상한 없음).
- 생략한 필드는 `[types.scene.live2d]` · `[types.scene.vrm]` · `[types.scene.spine]`(사용자 값 > `defaults.toml`)에서 옵니다.

| 필드 (공통) | 설명 |
|---|---|
| `model` | 모델 파일 (배경 디렉터리 기준). 필수 |
| `rect` | 그릴 영역 `[x, y, 폭, 높이]` (설계 px). 생략하면 설계 영역 전체 |
| `render_scale` | 이 레이어를 그리는 해상도 배율 (shader 레이어와 같음) |
| `look_at` | `{ anchor, range, smoothing, on_leave, … }` 위 공통 규칙 + 타입별 필드 |
| `on_click` | 모션 하나(모델 어디든) 또는 부위 이름 → 모션 표. 위 공통 규칙 |
| `blink` | 자동 눈 깜빡임 `{ enabled, interval }` (`interval` = `[최소, 최대]` 초) |

#### live2d

Cubism 모델(`.model3.json` + `.moc3` + 텍스처·모션·물리 파일)을 그립니다.

```toml
[[layer]]
type     = "live2d"
id       = "miku"
model    = "model/miku.model3.json"
rect     = [460, 0, 1000, 1080]
position = [500, 560]          # rect 안에서 모델 캔버스 중심
scale    = 1.0
idle     = "Idle"              # model3.json Motions의 그룹 이름 (그룹 안에서 무작위)
look_at  = { anchor = [960, 300], range = 700, params = { ParamAngleX = 30, ParamAngleY = 30, ParamEyeBallX = 1, ParamEyeBallY = 1, ParamBodyAngleX = 10 } }
on_click = { Head = "TapHead", "*" = "Tap" }        # HitArea 이름 → 모션 그룹, 나머지 모델 위는 "Tap"
parameters = { ParamCheek = "$blush" }              # 고정 파라미터 값 (속성으로 조절)
```

HitArea가 없는 모델(예: Live2D 공식 미쿠 샘플은 `"HitAreas": []`)은 모션 하나만 적거나 drawable을 직접 지정합니다.

```toml
on_click = "Tap"                                              # 모델 어디든
on_click = { "drawable:ArtMesh54" = "TapHead", "*" = "Tap" }  # 특정 drawable만 따로
```

| 필드 | 설명 |
|---|---|
| `model` | `.model3.json`. 거기 적힌 `.moc3`·텍스처·`physics3.json`·`pose3.json`·표정·모션 파일을 읽음 |
| `position`, `scale` | `rect` 안에서 모델 캔버스의 중심과 배율 |
| `idle` | 대기 모션 그룹 이름. 그룹 안의 모션을 무작위로 이어 재생 |
| `look_at.params` | 파라미터 이름 → 최대값. 시선 방향의 x(오른쪽 +)·y(위 +) 성분에 최대값을 곱해 넣음. `…X` 이름은 x, `…Y` 이름은 y 성분 |
| `on_click` | 모션 그룹 하나, 또는 표. 표의 키는 `model3.json` HitArea 이름(그 HitArea의 drawable 메시 안), `drawable:<ID>`(그 drawable 메시 안), `"*"`. 모르는 키는 그 모델의 HitArea 이름 목록과 함께 거절 |
| `physics` | `physics3.json` 적용 여부(bool) |
| `breath` | 숨쉬기(파라미터 사인파) `{ enabled, params }` |
| `parameters` | 파라미터 이름 → 고정값. 모션보다 나중에 적용. `"$속성"` 사용 가능 |
| `expression` | 처음부터 켜 둘 표정 이름(`model3.json` Expressions). 생략하면 없음 |

런타임은 waywall에 들어 있는 PurismCore(Cubism Core 호환 오픈소스 재구현)이며, 공식 Cubism Core를 쓰려면
config [`[live2d] core`](config.md#live2d-설계-m9)에 그 라이브러리 경로를 적습니다(M9b).

#### vrm

VRM 1.0 모델(glTF 2.0 + `VRMC_*` 확장)을 레이어 안의 3D 카메라로 그립니다. 깊이 버퍼는 레이어 안에서만 씁니다(씬 전체는 2D 합성 그대로).

```toml
[[layer]]
type      = "vrm"
id        = "miku"
model     = "model/miku.vrm"
rect      = [360, 0, 1200, 1080]
camera    = { target = "head", distance = 1.6, height = -0.15, fov = 25, yaw = 0, pitch = 0 }
animation = "anim/idle.vrma"   # 반복 재생할 대기 모션
look_at   = { range = 700, head = 0.4 }
on_click  = { head = "anim/nod.vrma", chest = "anim/wave.vrma" }
expressions = { happy = "$smile" }
spring_bone = { wind = [0.3, 0.0, 0.0], wind_speed = 0.5 }
```

| 필드 | 설명 |
|---|---|
| `model` | `.vrm` 파일 |
| `camera` | `target`(휴머노이드 본 이름, 바라볼 점) · `distance`(m) · `height`(m, target 기준 세로 오프셋) · `fov`(도, 세로) · `yaw`·`pitch`(도) |
| `animation` | 대기 모션 `.vrma`. 생략하면 기본 자세 + 숨쉬기 |
| `look_at` | 공통 필드 + `head`(머리가 시선을 따라 도는 비율 0–1, 나머지는 눈). 각도 한계는 모델의 `lookAt` 범위 |
| `on_click` | `.vrma` 하나, 또는 휴머노이드 본 이름(·`"*"`) → `.vrma`. 클릭 지점에서 모델로 광선을 쏴 맞은 삼각형의 가장 큰 스킨 가중치 본(또는 그 조상 중 키에 있는 본) |
| `expressions` | 표정 프리셋·커스텀 이름 → 가중치 0–1(고정). `"$속성"` 사용 가능 |
| `spring_bone` | `{ enabled, wind, wind_speed }`. `wind`는 모델 공간 방향 × 세기, `wind_speed`는 바람이 바뀌는 빠르기 |
| `light` | MToon 조명. `"scene"`이면 씬의 `directional`·`ambient` 광원을 씀(없으면 아래 값). 또는 `{ direction, color, intensity, ambient }` |
| `outline` | MToon 외곽선 그리기 여부(bool) |

MToon 셰이더는 `assets/shaders/vrm/` 파일이며 사용자 assets 디렉터리에서 교체할 수 있습니다.

#### spine

Spine 스켈레톤(`.skel` 바이너리 또는 `.json` + `.atlas` + 텍스처)을 그립니다. 대상은 Spine **4.2** 형식이며, 다른 버전 에디터로 내보낸
파일은 이유와 함께 거절됩니다(Spine 런타임은 에디터 버전마다 형식이 다름).

```toml
[[layer]]
type     = "spine"
id       = "miku"
model    = "spine/miku.skel"
atlas    = "spine/miku.atlas"    # 생략하면 model과 같은 이름의 .atlas
rect     = [460, 0, 1000, 1080]
position = [500, 1040]           # rect 안에서 스켈레톤 원점(보통 발밑)
scale    = 0.8
skin     = "default"
idle     = "idle"                # 트랙 0에서 반복
tracks   = { 1 = "hair_wind" }   # 더 위 트랙에서 함께 반복할 애니메이션
look_at  = { anchor = [960, 300], range = 700, bone = "look_target", max_offset = [60, 40] }
on_click = { head_box = "tap_head", body_box = "tap_body" }   # 바운딩 박스 이름 → 애니메이션
blink    = { animation = "blink" }
```

| 필드 | 설명 |
|---|---|
| `model` | `.skel` 또는 `.json` 스켈레톤 데이터 |
| `atlas` | 텍스처 아틀라스. 아틀라스의 `pma`(premultiplied) 표시를 읽어 색을 맞춤 |
| `position`, `scale` | `rect` 안에서 스켈레톤 원점의 위치와 배율 |
| `skin` | 스킨 이름 |
| `idle` | 대기 애니메이션 이름(트랙 0, 반복). 목록이면 그 안에서 무작위로 이어 재생 |
| `tracks` | 트랙 번호 → 함께 반복할 애니메이션. 대기 위에 겹쳐 섞임(숨쉬기, 바람 등) |
| `mix` | 애니메이션 전환 시 섞는 시간(초) |
| `look_at` | 공통 필드 + `bone`(포인터 쪽으로 옮길 본, 보통 IK 제약의 타깃) · `max_offset`(스켈레톤 px). Live2D처럼 표준 시선 파라미터가 없어 본을 직접 지정 |
| `on_click` | 애니메이션 하나, 또는 바운딩 박스 어태치먼트 이름(·`"*"`) → 애니메이션. 클릭한 곳이 그 다각형 안이면 클릭 트랙에서 한 번 재생 후 대기로 돌아감 |
| `blink` | 공통 필드 + `animation`(눈 깜빡임 애니메이션 이름). 위 트랙에서 `interval`마다 재생 |
| `physics` | 물리 제약(4.2) 적용 `{ enabled, wind, gravity }`. `wind`·`gravity`는 스켈레톤 좌표 `[x, y]` |

Spine 런타임 라이선스 때문에 기본 빌드에는 들어 있지 않을 수 있습니다([roadmap.md](roadmap.md#m11--spine-레이어-설계) M11).
빠진 빌드에서는 `spine` 레이어가 이유와 함께 거절됩니다.

#### 빛 가리기 (빛줄기와 캐릭터·레이어)

창문에서 들어오는 빛줄기를 캐릭터나 다른 레이어가 가리게 하는 패턴입니다. 지금 있는 기능(shader 레이어 + 레이어 `id` 입력)으로 되며,
캐릭터 레이어도 레이어 입력이 될 수 있으므로 그대로 적용됩니다.

[예제 빛줄기 셰이더](../examples/scenes/character/sunlight.glsl)는 픽셀마다 광원 쪽으로 샘플을 찍어 밝은 픽셀을 더합니다.
`"below"`를 입력으로 쓰면 광원 앞에 있는 캐릭터의 어두운 픽셀이 빛을 내지 않아 그 모양대로 빛줄기에 그림자 줄이 생기지만,
캐릭터의 밝은 부분(흰 옷, 하이라이트)도 빛을 냅니다. **빛을 내는 입력과 가리는 입력을 나누면** 이 문제가 없습니다.

```toml
[[layer]]
type    = "shader"
id      = "sunrays"
source  = "sunrays.glsl"
inputs  = { iChannel0 = "sky", iChannel1 = "wall", iChannel2 = "curtain", iChannel3 = "miku" }
blend   = "add"
```

```
샘플마다:  빛 = 광원 입력의 밝은 픽셀 × Π (1 − 가리는 입력의 알파)
```

- 투명한 곳(알파 0)은 그대로 통과합니다. 벽 사진에 창문 모양을 뚫어 두면 거기로만 빛이 들어옵니다.
- 반투명(예: 커튼 알파 0.5)은 일부만 통과합니다. 가리는 입력의 색을 곱하면 스테인드글라스처럼 빛에 색이 입혀집니다.
- 레이어 입력은 패럴랙스·`follow`·캐릭터 모션이 반영된 현재 모습이라, 움직이면 그림자 줄도 같이 움직입니다.
- 가리는 레이어 수는 채널 수만큼입니다. 지금은 광원 1 + 가림 3이 최대이고, [채널 개수 제한 해제](#채널-개수-제한-해제-설계) 후에는 GPU 한도까지입니다.
  움직이지 않는 사진들은 미리 합친 마스크 PNG 하나를 경로 입력으로 넣어도 됩니다.

**되지 않는 것:**
- 캐릭터 몸에 비치는 빛(창문 쪽이 밝고 반대쪽이 어두운 음영). live2d·spine 레이어는 `[[light]]`를 받지 않습니다. `lit`을
  줄지는 열린 질문입니다(보통 노멀맵이 없어 방향감이 약함). vrm은 MToon `light`로 음영을 받습니다.
- 바닥·벽에 떨어지는 캐릭터 그림자. 화면 공간 계산이라 투영 그림자는 없습니다. 그림자 이미지를 `multiply` 레이어로 두거나,
  vrm에 그림자 맵을 넣는 것(열린 질문)으로 다룹니다.

### 상호작용 요약
- 커서 이동: `parallax`, `follow`, `light.position = "pointer"`, `particles.pointer`, 캐릭터 레이어 `look_at`(설계)
- 클릭: `emitter.burst`, 셰이더 레이어의 `iClick`, 캐릭터 레이어 `on_click`(설계)

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
- 오디오(`<audio>`, `<video>`, WebAudio) 재생 가능. 음소거는 모든 소리에, 볼륨은 `<audio>`/`<video>` 요소에 적용됩니다
  (WebAudio는 음소거만 따름. 페이지가 `waywall-volume` 이벤트로 직접 맞출 수 있음)

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
어느 경로로 재생 중인지는 `waywall list`의 `dmabuf`(복사 없음) / `shm-fallback`(소프트웨어)로 확인합니다.

- 오디오는 출력마다 따로 재생됩니다. 같은 영상을 두 모니터에 띄우면 소리도 두 번 나오므로 한쪽을
  `[output."X"] mute = true`로 두세요. 볼륨은 최상위 `volume` < `[default]` < `[output."X"]` < `waywall volume` 순입니다.
- 영상의 `audio = false`이면 볼륨과 관계없이 항상 무음입니다.

## 이미지 배경

정적 이미지는 한 번 그리고 나면 아무 비용이 없습니다. 크기 맞춤은 컴포지터가 처리합니다.
GIF는 프레임이 2장 이상이면 영상처럼(GStreamer, 소프트웨어 디코딩) 재생되며 pause 시 멈춥니다.
옵션은 `[types.image]`를 따르고 항상 반복·무음입니다.
