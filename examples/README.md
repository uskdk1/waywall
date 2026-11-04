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
| `stained-glass/` | 4개를 넘는 채널(`iChannel0`–`iChannel5`), 빛 가리기(광원과 가리는 레이어를 따로 입력): 해 앞 창틀 막대의 그림자 줄, 색유리를 지난 빨간·파란 빛기둥 | `strength`, `sun` |

## Live2D (`live2d/`)

**모델은 들어 있지 않습니다.** 배경 설정(`scene.toml`·`project.toml`)과 직접 그린 텍스처만 있고, Live2D 모델은 사용자가
[Live2D 샘플 데이터](https://www.live2d.com/ko/download/sample-data/) 페이지에서 이용 조건에 동의하고 받아 넣습니다.
받은 샘플의 `runtime` 폴더 **안의 내용**을 각 예제의 `model/`에 넣으면 됩니다(복사나 심볼릭 링크). `model/`은 git에 올라가지 않습니다.

| 예제 | 넣을 모델 (`model/`) | 보여 주는 것 | 속성 |
|---|---|---|---|
| `miku/` | "初音ミク" PRO판의 `runtime` (`miku_sample_t04.model3.json`이 바로 보이게) | 대기 모션·눈 깜빡임·숨쉬기·물리, 커서를 따라보기, 클릭하면 `Tap` 모션, 뒤·앞 반짝이(커서로 모이고 클릭하면 터짐), 블룸 | `sparkles` |
| `hiyori/` | "모모세 히요리" PRO판(영상용 아님)의 `runtime` (`hiyori_pro_t11.model3.json`) | 포즈(팔 두 세트 전환), HitArea 클릭(`Body` → `Tap@Body`, 나머지 → `Tap`), 창 빛줄기(`"below"` 입력), 햇살 속 먼지 | `rays`, `sun`, `dust` |

```sh
cp -r ~/Downloads/miku/runtime/. examples/live2d/miku/model/     # 받은 위치에 맞게
waywall set examples/live2d/miku
```

모델이 없으면 `set`이 `failed to read …/model/….model3.json`으로 거절합니다. 다른 모델을 쓰려면 `scene.toml`의 `model`·`scale`·`rect`와
`look_at.params`(파라미터 이름은 모델마다 다름), `on_click`(모션 그룹·HitArea 이름)을 그 모델에 맞게 고칩니다
([docs/wallpaper-format.md](../docs/wallpaper-format.md#live2d)).

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
