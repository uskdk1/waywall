# 설정

## 원칙: 코드에는 모양도 숫자도 없다

waywall 코드에는 **보이는 것과 조절 가능한 값을 하나도 하드코딩하지 않습니다.**
색, 커서, 스프라이트 모양, 톤매핑 곡선, 블룸 강도, fps, 타임아웃, 빈 화면에 무엇을 보여줄지까지
모두 설정 데이터에서 읽습니다.

- 모든 키의 기본값은 저장소의 **`assets/defaults.toml`** 한 파일에 있습니다. 이 파일은 빌드 시 바이너리에
  내장되지만 **코드가 아니라 데이터**이며, 사용자 설정이 그 위에 덮어씁니다.
- 내장 셰이더(톤매핑, 블룸, fit, 파티클 스프라이트 등)와 스프라이트 이미지도 `assets/` 아래 파일로 두고,
  사용자 assets 디렉터리에 같은 이름의 파일을 두면 그것을 대신 씁니다.
- 설정 구조체는 `#[serde(default = …)]` 같은 코드 내 기본값을 쓰지 않습니다. `defaults.toml`에 모든 키가
  빠짐없이 있어야 하며, 테스트가 이를 강제합니다([development.md](development.md#테스트-방침)).
- 코드에 남는 것은 **인터페이스 이름**(uniform 이름 `iTime`, 키 이름, IPC 필드)뿐입니다.

현재 적용된 값을 보려면:
```sh
waywall config defaults          # 내장 defaults.toml 출력 (복사해서 시작점으로 사용)
waywall config show [-o DP-1]    # 모든 계층을 합친 최종 값
```

## 계층과 우선순위

```
assets/defaults.toml (내장)
  < 배경의 project.toml [options] / scene.toml   ← 배경 작성자
    < config.toml 의 전역 값과 [types.*]          ← 사용자 (여기부터 전부 사용자)
      < config.toml [default]
        < config.toml [output."X"]
          < 런타임 CLI 명령 (--save 없으면 reload 시 사라짐)
```

**사용자가 항상 배경 작성자를 이깁니다.** 배경 작성자의 값은 내장 기본값만 덮어쓰고,
사용자가 설정 파일 어디에든 적은 값은 배경 작성자의 값보다 우선합니다.
예: `[types.video] fit = "contain"`을 적어 두면, `project.toml`이 `cover`를 지정한 배경도 `contain`으로 표시됩니다.

`defaults.toml`은 사용자 설정이 아닙니다 — 사용자가 적지 않은 키는 배경 작성자의 값이 쓰이고,
그것도 없을 때만 내장값이 쓰입니다.

## 파일 위치

| 파일 | 기본 위치 | 변경 |
|---|---|---|
| 사용자 설정 | `$XDG_CONFIG_HOME/waywall/config.toml` | `waywall daemon --config <경로>` |
| 사용자 assets | `$XDG_CONFIG_HOME/waywall/assets/` | `assets.dir` |
| 내장 기본값 | 바이너리에 내장 (`assets/defaults.toml`) | — |

`waywall reload`로 데몬을 재시작하지 않고 다시 읽습니다. `waywall set --save`는 해당 출력 항목을
갱신합니다(주석과 서식 보존, `toml_edit` 사용). 반영 범위와 예외(`[log]`, `[cursor].theme`/`size`는 재시작 필요)는
[cli-ipc.md](cli-ipc.md#reload)를 보세요.

## 예시 (사용자 config.toml)

필요한 키만 적으면 나머지는 `defaults.toml` 값이 쓰입니다.

```toml
volume     = 50
mute       = false
pause_when = "fullscreen"

[render]
gpu     = "low-power"
max_fps = 60                          # 배터리를 아끼려고 모든 모니터를 60fps로 제한 (기본 0 = 주사율)

[cursor]
shape = "crosshair"

[appearance]
empty = "~/Wallpapers/fallback.png"   # 배경이 지정되지 않은 출력
error = "previous"                    # 로딩 실패 시 이전 배경 유지

[types.video]
fit = "contain"
background = "#101018"                # 레터박스 색

[types.scene.post]
tonemap = "agx"                       # 모든 씬의 톤매핑
bloom = { threshold = 0.9, intensity = 0.8, radius = 6 }

[default]
wallpaper = "~/Wallpapers/bg.png"

[output."DP-1"]
wallpaper = "~/Wallpapers/rain.mp4"
volume    = 30
fit       = "cover"

[output."eDP-1"]
wallpaper = "~/Wallpapers/forest/"
mute      = true
fps       = 30

[output."eDP-1".properties]
density = 2.0
```

## 스키마

색은 `"#rrggbb"` 또는 `"#rrggbbaa"`. 경로는 `~` 확장을 지원하며, 상대 경로는 해당 파일 기준입니다.
아래 표의 "내장값"은 `assets/defaults.toml`에 들어 있는 값입니다.

### 최상위

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `volume` | int 0–100 | `50` | 전체 볼륨 |
| `mute` | bool | `false` | 전체 음소거 |
| `pause_when` | string | `"fullscreen"` | `never` · `fullscreen` · `maximized` · `any-window`(`visibility.niri = true` 필요, 없으면 `maximized`처럼 동작). 그 출력에 활성화(포커스)된 전체화면/최대화 창이 있거나, any-window면 활성 워크스페이스에 창이 있을 때 멈춤 |

`pause_when`과 관계없이, frame 콜백 기준으로 완전히 가려진(`Hidden`) 출력은 항상 정지합니다.

### `[log]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `level` | string | `"info"` | tracing 필터. `RUST_LOG` 환경변수가 있으면 그쪽이 우선 |

### `[surface]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `layer` | string | `"background"` | layer-shell 레이어: `background` · `bottom` · `top` · `overlay` |
| `namespace` | string | `"waywall"` | 컴포지터 규칙(niri `layer-rule` 등)에서 식별하는 네임스페이스 |

### `[render]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `gpu` | string | `"low-power"` | `low-power` · `high-performance` |
| `max_fps` | int | `0` | shader/scene fps 상한. `0`이면 상한 없이 모니터마다 자기 주사율(120 Hz 모니터는 120fps). 배터리·발열을 줄이려면 `60` 등으로 제한 |
| `render_scale` | float | `1.0` | 내부 렌더 해상도 배율. 배경의 `render_scale` 옵션과 곱해짐 |
| `present_mode` | string | `"mailbox"` | Vulkan present 모드: `mailbox` · `fifo` · `immediate`. 지원하지 않으면 `fifo` |
| `frame_latency` | int | `2` | swapchain이 미리 준비할 수 있는 최대 프레임 수 (작을수록 입력 지연↓) |
| `hdr_format` | string | `"rgba16float"` | 씬 내부 HDR 버퍼 포맷: `rgba16float` · `rg11b10ufloat`(메모리 절반, 정밀도↓, 알파 없음) · `rgba32float`. 뒤의 둘은 GPU 기능(`RG11B10UFLOAT_RENDERABLE`, `FLOAT32_BLENDABLE`)이 필요하며, 없으면 씬이 이유와 함께 거절됨 |

톤매핑은 씬 옵션 `[types.scene.post] tonemap`입니다(작성자가 씬마다 고르고, 사용자가 적으면 모든 씬에 적용).

### `[visibility]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `frame_timeout_ms` | int | `1500` | frame 콜백이 이 시간 이상 없으면 `Hidden` |
| `slow_frame_count` | int | `3` | 저빈도 콜백을 보내는 컴포지터용: 간격이 임계 이상인 콜백이 이 횟수 연속이면 `Hidden` |
| `foreign_toplevel` | bool | `true` | wlr-foreign-toplevel 신호원 사용 |
| `niri` | bool | `false` | niri IPC 신호원 사용. `pause_when = "any-window"`에 필요 |

### `[cursor]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `shape` | string | `"default"` | `wp_cursor_shape_v1`의 모양 이름(`default`, `crosshair`, `pointer`, …) 또는 `hidden` |
| `theme` | string | `""` | xcursor 폴백 테마. 빈 값이면 `XCURSOR_THEME` 환경변수 |
| `size` | int | `24` | xcursor 폴백 크기 |

출력별 섹션에서는 `cursor.shape`만 덮어쓸 수 있습니다(`theme`/`size`는 포인터 전체에 하나).

### `[appearance]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `empty` | 색 또는 경로 | `"#000000"` | 배경이 지정되지 않은 출력에 표시할 것 |
| `error` | `"previous"` · 색 · 경로 | `"previous"` | 배경 로딩/실행 실패 시 표시할 것 |
| `loading` | `"previous"` · 색 · 경로 | `"previous"` | 새 배경 준비 중 표시할 것 |

대체 순서: 배경 실패 → `error` → (`error`도 실패하거나 `previous`인데 이전 내용이 없으면) `empty` → (그것도 실패하면) 아무것도 표시하지 않고 로그만 남김.
`loading = "previous"`인데 이전 내용이 없으면, `empty`가 색일 때만 그 색을 먼저 보여줍니다.

### `[assets]`

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `dir` | 경로 | `"${XDG_CONFIG_HOME:-~/.config}/waywall/assets"` | 내장 asset을 덮어쓸 파일을 찾는 디렉터리 |

경로 값은 `~`, `$VAR`, `${VAR}`, `${VAR:-대체값}`을 확장합니다.

### `[detect]`

경로로 배경 타입을 판별하는 규칙입니다.

| 키 | 타입 | 내장값 | 설명 |
|---|---|---|---|
| `image` · `video` · `shader` · `web` | 문자열 배열 | `["png", …]` 등 | 그 타입으로 볼 확장자 (대소문자 무시) |
| `url_schemes` | 문자열 배열 | `["http://", "https://"]` | 이 접두사로 시작하면 `web` |
| `project_file` | string | `"project.toml"` | 디렉터리형 배경의 프로젝트 파일 이름 |

내장 asset 구성(같은 상대 경로로 덮어씀):
```
assets/
├─ defaults.toml
├─ shaders/
│  ├─ fullscreen.wgsl       # 화면을 덮는 삼각형 버텍스 셰이더 (셰이더 배경, 후처리 패스)
│  ├─ scene/image.wgsl      # 씬 image 레이어 (사각형 + 텍스처). 뒤에 scene/lighting.wgsl을 붙여 컴파일
│  ├─ scene/lighting.wgsl   # `lit` 레이어 조명: fn lighting(n, p) -> vec3<f32> (n: 법선, p: 설계 px 위치)
│  ├─ scene/input.wgsl      # shader 레이어 채널 입력: 장면/레이어에서 rect 영역 잘라 내기(GLSL이면 상하 반전)
│  ├─ scene/post.wgsl       # HDR + 블룸 → 노출 → tonemap() → vignette() → 화면. tonemap/<이름>.wgsl과 vignette.wgsl을 뒤에 붙여 컴파일
│  ├─ tonemap/{aces,agx,reinhard,none}.wgsl   # fn tonemap(c: vec3<f32>) -> vec3<f32>. 새 파일 = 새 이름
│  ├─ bloom/downsample.wgsl # compute: 첫 단계에서 threshold를 넘는 부분만 남기고 해상도 절반씩
│  ├─ bloom/upsample.wgsl   # compute: 한 단계 작은 결과를 텐트 필터로 키워 이 단계에 더함
│  ├─ vignette.wgsl         # fn vignette(uv: vec2<f32>, strength: f32) -> f32
│  ├─ particles/simulate.wgsl   # compute: 방출(고리 버퍼) · 적분 · curl noise · 포인터 힘
│  ├─ particles/render.wgsl     # 인스턴스 사각형. 뒤에 스프라이트 파일이 붙어 `fn sprite(uv) -> vec4<f32>`를 정의
│  ├─ effects/sway.wgsl     # 레이어 효과. 새 파일을 두면 새 효과 타입 (M6d)
│  ├─ clip.wgsl             # 레이어 clip (M6d)
│  └─ shadertoy/prelude.glsl, epilogue.glsl, prelude.wgsl   # 인터페이스 선언(uniform 블록)과 main() 래퍼
└─ sprites/
   └─ soft-circle.wgsl      # 무늬를 정하지 않은 파티클의 기본 스프라이트 (`fn sprite`)
```

### `[types.<타입>]` — 배경 타입별 기본 옵션

`image` · `video` · `shader` · `scene` · `web` 각각의 옵션 기본값입니다. 키는
[wallpaper-format.md](wallpaper-format.md)의 옵션 표와 같습니다.

```toml
[types.image]
fit = "cover"
background = "#000000"
interactive = false
downscale = true              # 출력보다 큰 이미지를 메모리에 올리기 전에 줄임
downscale_filter = "lanczos3" # nearest | triangle | catmull-rom | gaussian | lanczos3

[types.video]
fit = "cover"
background = "#000000"
loop = true
audio = true
speed = 1.0
hwdec = true
interactive = false

[types.shader]
fps = 0
render_scale = 1.0
interactive = true
background = "#000000"     # 첫 프레임이 그려지기 전에 보이는 색

[types.shader.channel]     # [inputs] 채널에서 생략한 값 (사용자가 적으면 배경의 값보다 우선)
filter = "linear"          # linear | nearest
wrap   = "repeat"          # repeat | clamp | mirror

[types.web]
background = "#000000"     # 페이지가 투명한 부분
audio = true
interactive = true

[types.scene]
fps = 0
render_scale = 1.0
interactive = true
clear = "#000000"
fit = "cover"
pointer_smoothing = 0.15   # 패럴랙스가 포인터를 따라가는 시간 상수(초). 0 = 즉시
settle_px = 0.25           # 목표와의 차이가 이보다 작으면(출력 px) 멈추고 잠듦

[types.scene.layer]        # 레이어 공통 필드 (사용자가 적으면 scene.toml 값보다 우선)
blend = "normal"
opacity = 1.0
parallax = 0.0
visible = true

[types.scene.image]        # image 레이어 필드
scale = 1.0
rotation = 0.0
lit = false

[types.scene.post]
exposure = 1.0
tonemap  = "aces"
bloom    = { threshold = 1.0, intensity = 0.5, radius = 4 }
vignette = 0.0

[types.scene.particles]    # particles 레이어 필드 (사용자가 적으면 scene.toml 값보다 우선. sprite만 예외: 무늬를 정하지 않은 레이어의 기본값)
max        = 1000
sprite     = "sprites/soft-circle.wgsl"   # assets 기준 경로
blend      = "add"
emitter    = { shape = "point", rate = 10.0, burst = 0 }
lifetime   = [1.0, 2.0]
velocity   = { min = [0.0, 0.0], max = [0.0, 0.0] }
gravity    = [0.0, 0.0]
drag       = 0.0
turbulence = { strength = 0.0, scale = 0.005, speed = 0.5 }
size       = 4.0
color      = "#ffffff"
emissive   = 1.0
pointer    = { mode = "none", radius = 200.0, strength = 300.0 }
max_step   = 0.1          # 한 프레임 시뮬레이션의 최대 dt(초)

[types.scene.shader]       # shader 레이어 필드
channel = { filter = "linear", wrap = "clamp" }

[types.scene.light.ambient]   # [[light]]에서 생략한 필드 (사용자가 적으면 scene.toml 값보다 우선)
color     = "#ffffff"
intensity = 1.0

[types.scene.light.point]
color     = "#ffffff"
intensity = 1.0
radius    = 300.0
height    = 100.0
flicker   = 0.0
flicker_speed = 8.0

[types.scene.light.directional]
color     = "#ffffff"
intensity = 1.0
direction = [0.0, 0.0, -1.0]

# 아래 두 섹션은 M6d에서 defaults.toml에 추가된다
[types.scene.effects.sway]  # sway 효과에서 생략한 필드
strength  = 4.0            # px
speed     = 0.6
scale     = 0.01
direction = [1.0, 0.0]

[types.scene.follow]        # follow에서 생략한 필드
range     = 600.0          # px
smoothing = 0.12           # 초
on_leave  = "center"       # center | stay
```

(실제 내장값의 정본은 `assets/defaults.toml`이며, 이 문서는 그 요약입니다.)

### `[default]`, `[output."<커넥터>"]`

커넥터 이름은 `waywall list`로 확인합니다.

| 키 | 타입 | 설명 |
|---|---|---|
| `wallpaper` | string | 파일/디렉터리 경로 또는 URL |
| `volume` | int 0–100 | 출력별 볼륨 |
| `mute` | bool | 출력별 음소거 |
| 배경 옵션 키 (`fit`, `fps`, `loop`, `render_scale`, `background`, …) | | 이 출력의 배경 옵션 덮어쓰기 |
| `properties` | table | 배경 `[properties]` 값 덮어쓰기 |
| `cursor`, `appearance` | table | 위 섹션을 이 출력에서만 덮어쓰기 |
