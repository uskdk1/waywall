# 예제 배경

각 예제는 `waywall set <경로>`로 바로 띄울 수 있습니다(데몬이 실행 중이어야 함). 형식은
[docs/wallpaper-format.md](../docs/wallpaper-format.md)에 있습니다.

```sh
waywall set examples/scenes/fireflies            # 모든 출력
waywall set -o DP-1 examples/web/pointer         # 한 출력만
waywall set --save examples/scenes/character     # 설정 파일에도 기록
```

## 셰이더 (`shaders/`)

| 예제 | 보여 주는 것 |
|---|---|
| `pointer-glow.glsl` | 단일 파일 Shadertoy GLSL. 커서 주변 빛(`iPointer`), 클릭 파문(`iClick`) |
| `gradient.wgsl` | 단일 파일 WGSL. `g.time`, `g.pointer` |
| `ripple/` | 디렉터리형 셰이더 + 텍스처 채널(`[inputs]` → `iChannel0`). 커서 주변이 물결처럼 일렁임 |

## 씬 (`scenes/`)

| 예제 | 보여 주는 것 | 속성 |
|---|---|---|
| `parallax/` | image 레이어, 패럴랙스(커서를 움직이면 레이어마다 다른 깊이), 멈추면 잠듦 | `moon_glow` |
| `fireflies/` | 파티클(방출기, curl noise, 커서로 끌어당김, 클릭하면 불꽃 burst), 블룸 | `density` |
| `sunlight/` | shader 레이어 + `"below"` 입력(아래 그림의 밝은 곳에서 빛줄기), `render_scale` | `strength`, `sun` |
| `lights/` | 조명: 노멀맵 벽, 커서를 따라가는 점광원, 흔들리는 횃불, 불씨 파티클 | `lamp` |
| `character/` | 레이어 효과 `sway`(머리카락만 흔들림), `clip` + `follow`(흰자 안에서 커서를 따라보는 눈), 빛줄기 | `strength`, `sun` |

## 웹 (`web/`)

| 예제 | 보여 주는 것 | 속성 |
|---|---|---|
| `pointer/` | 마우스 궤적과 클릭 효과(평범한 DOM 이벤트), `window.waywall`, `waywall-pause`/`waywall-resume` 처리 | `hue` |

## 속성 바꾸기

`project.toml`의 `[properties]`는 배경 작성자의 기본값입니다. 설정 파일에서 출력마다 덮어쓸 수 있습니다
([docs/config.md](../docs/config.md#default-output커넥터)).

```toml
[output."DP-1"]
wallpaper  = "~/waywall/examples/scenes/fireflies"
properties = { density = 150.0 }

[default]
properties = { hue = 320.0 }
```

## 예제 텍스처

예제 이미지는 모두 ImageMagick으로 그린 단순 도형입니다(저장소에 그대로 포함). 실제 그림으로 바꿔도 `scene.toml`의
좌표·크기만 맞추면 동작합니다.
