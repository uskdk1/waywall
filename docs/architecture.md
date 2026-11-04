# 아키텍처

## 설계 목표

1. **상시 실행 비용 최소화** — 바탕화면은 하루 종일 돌아가므로 사소한 비용도 줄인다
2. **표현력** — 빛, 파티클, 후처리(블룸) 같은 GPU 효과를 자체 씬 렌더러로 표현
3. **표준 프로토콜 우선** — [protocols.md](protocols.md)
4. **설정 주도** — 코드에는 색·모양·수치를 하드코딩하지 않음. 기본값은 `assets/defaults.toml`, 내장 셰이더/스프라이트는
   `assets/` 파일이며 모두 사용자 설정/assets로 덮어쓸 수 있음 ([config.md](config.md#원칙-코드에는-모양도-숫자도-없다))

이를 위해 상주 데몬은 GTK 없이 Wayland와 Vulkan(wgpu)을 직접 다루고,
GTK/WebKit이 꼭 필요한 웹 배경만 별도 헬퍼 프로세스로 분리합니다.

## 프로세스 구성

```
 ┌──────────────┐  JSON-lines / Unix socket  ┌───────────────────────────────────────────────┐
 │ waywall set  │ ─────────────────────────▶ │ waywall daemon  (GTK 없음)                     │
 │ waywall list │ ◀───────────────────────── │                                               │
 └──────────────┘ $XDG_RUNTIME_DIR/         │  calloop 이벤트 루프 (단일 스레드)              │
                  waywall.sock              │   ├ Wayland (smithay-client-toolkit)          │
                                            │   ├ IPC 소켓                                   │
                                            │   ├ 타이머 (fps 제한, 가림 타임아웃)            │
                                            │   └ 채널 (GStreamer, 웹 헬퍼)                  │
                                            │                                               │
                                            │  Output(DP-1)          Output(eDP-1)          │
                                            │   ├ layer surface       ├ layer surface       │
                                            │   ├ Backend             ├ Backend             │
                                            │   │  image  → wl_shm + viewporter            │
                                            │   │  video  → GStreamer → dmabuf subsurface  │
                                            │   │  shader → wgpu (Vulkan)                  │
                                            │   │  scene  → wgpu (Vulkan)                  │
                                            │   │  web    → 헬퍼에 위임                     │
                                            │   └ Visibility 상태                           │
                                            │                                               │
                                            │  Gpu (지연 생성, 출력 간 공유)                  │
                                            └───────────────┬───────────────────────────────┘
                                                            │ stdin/stdout JSON-lines
                                                            ▼
                                            ┌───────────────────────────────────────────────┐
                                            │ waywall-web  (웹 배경이 있을 때만 실행)          │
                                            │  GTK4 + gtk4-layer-shell + WebKitGTK 6         │
                                            │  출력별 layer 창 + WebView                      │
                                            └───────────────────────────────────────────────┘
```

## Cargo 워크스페이스

```
crates/
├─ waywall/          # 데몬 + CLI 바이너리 (GTK에 링크하지 않음)
├─ waywall-web/      # 웹 헬퍼 바이너리 (GTK4, WebKitGTK)
└─ waywall-proto/    # 공유 타입: IPC 메시지, 헬퍼 프로토콜, 설정/배경 스키마 (serde)
assets/              # 데이터: defaults.toml, 내장 WGSL 셰이더, 스프라이트 (include_dir로 내장, 사용자 assets로 덮어씀)
```

### asset 해석
`Assets::open("shaders/tonemap/aces.wgsl")` 같은 상대 경로 요청은 `assets.dir`(사용자) → 내장 순으로 찾습니다.
셰이더 파일은 `reload` 시 다시 읽으므로 데몬 재시작 없이 렌더 단계를 바꿔 볼 수 있습니다.

데몬이 GTK를 링크하지 않으므로 웹 배경을 쓰지 않으면 GTK/WebKit 라이브러리는 메모리에
올라오지도 않습니다.

## 백엔드별 렌더 경로

배경 타입마다 **가장 싼 경로**를 고릅니다. GPU(wgpu) 장치는 shader/scene 배경이 처음 필요할 때
만들고, 쓰는 출력이 없어지면 해제합니다.

| 타입 | 경로 | 상시 비용 |
|---|---|---|
| image (정적) | 디코딩 1회 → `wl_shm` 버퍼 → `wp_viewporter`로 crop/scale | **0** — 한 번 커밋 후 아무것도 안 함. 스케일은 컴포지터가 처리 |
| image (GIF) | video 경로 재사용 (GStreamer 소프트웨어 디코딩 → `wl_shm`) | 프레임당 작은 CPU 복사 |
| video | GStreamer 하드웨어 디코딩(VA-API) → dmabuf → `zwp_linux_dmabuf_v1` 버퍼 → 서브서피스 + viewporter | 디코딩만. **우리 GPU 작업 0, 복사 0** |
| shader | wgpu 렌더 패스 1개, frame 콜백 + fps 제한으로 구동 | 셰이더 비용 그대로 |
| scene | wgpu 렌더 그래프 (HDR 오프스크린 → 레이어 → 파티클 compute → 조명 → 블룸 → 톤매핑) | 씬 복잡도에 비례, `render_scale`로 조절 |
| web | 웹 헬퍼 프로세스 | WebKit 비용 |

### 영상: 복사 없는 dmabuf 경로
```
playbin3 (file:// URI)
 ├ video-sink: appsink   caps = video/x-raw(memory:DMABuf), format=DMA_DRM, drm-format={컴포지터 feedback 포맷}
 │    └▶ (GStreamer 스레드) 샘플 → calloop 채널 → 메인: zwp_linux_buffer_params_v1 → wl_buffer → 서브서피스
 └ audio-sink: autoaudiosink (PipeWire/Pulse), playbin3의 volume / mute 속성
```
- 디코더는 playbin3가 고름. VA 디코더(`vah264dec` 등)가 dmabuf를 내면 복사 없이 그대로 컴포지터로 감
  (playbin3 `native-video` 플래그로 중간 변환 요소를 넣지 않음)
- 허용 포맷/모디파이어는 `zwp_linux_dmabuf_v1` v4 기본 feedback(포맷 테이블 + tranche)으로 받아 appsink caps에 넣음
- 샘플의 `VideoMeta`(plane별 offset/stride)와 `DmaBufMemory` fd로 `wl_buffer` 생성. 디코더 버퍼 풀이 같은 dmabuf를
  돌려 쓰므로 **dmabuf(inode, offset) 단위로 `wl_buffer`를 캐시**해 매 프레임 import하지 않음
- `GstSample`은 그 `wl_buffer`의 `release`가 올 때까지 잡아 둠(화면에 있는 것 + 대기 중 하나). 그 뒤 풀로 돌아감
- 타이밍: appsink `sync=true` → 파이프라인 시계(오디오)에 맞춰 샘플이 오면 attach + 커밋. frame 콜백은 쓰지 않음
- 서브서피스는 desync라서 프레임마다 부모를 커밋하지 않음. `wp_viewporter`의 source(cover 시 crop) / destination으로 맞춤
- 반복: segment seek. `SEGMENT_DONE`에서 다시 segment seek하므로 이음새가 없음. `speed`는 seek의 rate
- 일시정지: 파이프라인 `PAUSED` (`pause`, M5의 가림 정지)
- 버스 메시지는 sync handler에서 calloop 채널로 넘겨 메인 스레드에서 처리 (GLib 메인 루프 없음)
- **폴백:** dmabuf 협상이 실패하면(`not-negotiated`: 하드웨어 디코더 없음, 코덱 미지원 등) 같은 파일로 파이프라인을
  다시 만들어 `videoconvert ! appsink(BGRx)` → `wl_shm` 버퍼 링(프레임당 복사 1회). `list`의 `render_path`가 `shm-fallback`이 되고 경고 로그
- `hwdec = false`면 처음부터 shm 경로 + 디코더 선택에서 하드웨어 디코더 제외 (`render_path = shm`)
- YUV→RGB 변환은 컴포지터가 함. 색 공간 지정(`wp_color_representation_v1`, staging)은 로드맵
- 레터박스(`contain`)의 여백은 부모 surface에 `background` 옵션 색의 단색 버퍼(`wp_single_pixel_buffer_v1`, 없으면 1×1 `wl_shm`)를 viewporter로 늘려서 채움

### GIF
- 프레임이 2장 이상인 GIF만 영상 경로(GStreamer, shm 경로)로 재생하고, 1장짜리는 정적 이미지 경로로 표시
- 옵션은 `[types.image]`(fit, background, interactive)를 따르고, 항상 반복·무음
- 반복은 flush seek: GIF를 여는 libav 디먹서가 비flush segment seek를 처리하지 못해(시간이 0으로 돌아간 프레임이 한꺼번에 쏟아짐) 영상과 달리 이음새 없는 반복을 쓰지 않음
- GStreamer를 한 번 초기화하면 GLib 스레드 풀의 대기 스레드(`pool-spawner`) 하나가 데몬이 끝날 때까지 남음 (깨어나지 않음)

### 셰이더/씬: wgpu
- `wgpu::Instance`는 Vulkan 백엔드만 활성화, 어댑터는 설정 `render.gpu`(기본 `low-power`)로 선택
- `Gpu` 하나(Device/Queue)를 모든 출력이 공유, 출력마다 `wgpu::Surface` (raw-window-handle의 Wayland 핸들)
- `Gpu`는 shader/scene 배경이 처음 필요할 때 만들고(어댑터 선택에 블로킹 수십~수백 ms), GPU를 쓰는 출력이 하나도 없으면 해제
- GPU 내용은 layer surface 위에 놓인 **GPU 전용 서브서피스**(desync)에 그림. Vulkan present가 그 서브서피스를 직접 커밋하므로
  부모를 매 프레임 커밋할 필요가 없음. 부모에는 `background` 색의 단색 버퍼(첫 프레임 전에 보임)
- **frame 콜백으로 직접 페이싱:** present 직전에 `wl_surface.frame`을 요청해 두고(present의 커밋에 함께 실림),
  콜백을 받은 뒤에만 다음 프레임을 그림. 가려져서 콜백이 안 오면 렌더 루프가 저절로 멈춤
- present 모드는 `render.present_mode`(기본 `mailbox`, 지원하지 않으면 `fifo`). 우리가 콜백으로 페이싱하므로
  드라이버의 FIFO 대기에 막히지 않음
- fps 제한: 콜백이 왔는데 마지막 프레임 이후 `1/fps`가 안 지났으면 calloop 타이머로 남은 시간만큼 기다렸다 그림
- 분수 스케일·`render_scale`: swapchain을 `물리 픽셀 × render_scale` 크기로 만들고 viewporter destination을 논리 크기로 설정.
  확대는 컴포지터가 하므로 업스케일 패스가 필요 없음
- 셰이더 컴파일(naga 파싱·검증, 파이프라인 생성)은 메인 스레드에서 동기적으로 함. 오류는 `set` 응답과 `list`에 표시
- 채널 수([M8b](roadmap.md#m8b--셰이더-채널-개수-제한-해제-)): 셰이더마다 `max(4, 쓴 가장 큰 번호 + 1)`. `gpu/shadertoy.rs`가 프리앰블의
  `// waywall:channel_resolution`·`// waywall:channels` 줄을 그 수만큼의 선언으로 바꾸고, `ShaderRenderer`가 같은 수로 바인드 그룹 레이아웃을 만든다
  (바인딩 번호는 `texture_binding`·`sampler_binding` 한 곳). 상한은 장치 한도(`max_sampled_textures_per_shader_stage` · `max_samplers_per_shader_stage`,
  어댑터 값 그대로 요청)이고 넘으면 그 셰이더만 거절

shader와 scene은 위의 surface·페이싱·시계·포인터 처리를 `GpuView`(`backend/view.rs`) 하나로 공유하고,
"한 장 그리기"만 렌더러(`Shadertoy` / `Scene`)가 다릅니다.

### 씬 렌더러
형식은 [wallpaper-format.md](wallpaper-format.md#씬-배경).

```
 compute      레이어 입력 준비                   레이어 패스 (HDR, render.hdr_format)     블룸              후처리 (sRGB 뷰)
 particles ─▶ 레이어 id 입력: 그 레이어만 그림 ─▶ clear → 레이어 순서대로 draw     ─▶ 축소(threshold)  ─▶ (HDR + 블룸) × 노출
              shader 레이어: 자기 텍스처에 그림    "below"를 읽는 shader 레이어 앞에서     → 확대하며 더함      → tonemap → vignette
                                                 패스를 끊고 rect 영역을 잘라 냄                              → sRGB 인코딩
                                                 image(lit면 조명) · particles · shader 결과 합성
```

- **로드** (`set` 처리 중, 메인 스레드): `scene.toml` 파싱 → `"$속성"` 치환 → 레이어마다 defaults/작성자/사용자 계층 병합 후
  엄격한 스키마로 검사 → 텍스처 디코딩(텍스처마다 스레드 하나로 병렬, 끝날 때까지 기다림) → 선형 공간에서 알파를 곱해
  `Rgba8UnormSrgb`로 다시 인코딩. GPU 업로드와 파이프라인 생성은 출력마다.
- 블렌드 모드마다 image 파이프라인 하나(`scene/image.wgsl`, 고정 블렌드 상태). 레이어 변환(설계 px → clip)은 CPU가 계산해
  레이어마다 작은 uniform 버퍼에 쓴다. 변하지 않으면 쓰지 않는다.
- 후처리 셰이더는 `scene/post.wgsl` + `tonemap/<이름>.wgsl`을 이어 붙여 naga로 검증. 톤매핑 함수만 바꾸는 사용자 파일은 짧다.
- swapchain은 sRGB 포맷(없으면 UNORM + sRGB 뷰 포맷)으로 만들어 인코딩을 하드웨어가 한다. 셰이더 배경의 UNORM(값 그대로)과 다르다.
- **shader 레이어:** 셰이더 배경과 같은 컴파일 경로(`gpu/shadertoy.rs`, 같은 프리앰블·uniform). 그리는 방법은 두 가지다.
  - **직접:** `opacity = 1`, `render_scale = 1`, `clip` 없음이고 다른 레이어의 입력이 아니면, HDR 버퍼에 화면 삼각형을 그리되 scissor로 rect 영역만 남기고
    블렌드는 파이프라인의 고정 블렌드(`blend`)로 한다. 중간 텍스처의 쓰기·읽기와 합성이 없다. 렌더 타깃 기준 좌표를 레이어 기준으로
    바꾸는 값은 uniform의 origin(GLSL은 에필로그가 `fragCoord`에서 빼 주고, WGSL은 `g.origin`)이다. 이 레이어 앞에서 레이어 패스를 끊는다.
  - **텍스처 경유:** 그 밖에는 `rect`를 렌더 px로 옮긴 크기의 `Rgba16Float` 텍스처에 그린 뒤(origin = 0), image 레이어와 같은
    파이프라인(사각형 + blend)으로 합성한다. 불투명도는 셰이더 출력 전체에 곱해야 해서 고정 블렌드만으로 표현할 수 없고,
    레이어 입력은 결과 텍스처가 있어야 하기 때문이다. `render_scale`이 1이 아니면 텍스처를 그 배율 크기로 만들고 합성 사각형이
    선형 필터로 늘린다. 입력 잘라 내기는 원본에서 `1 / render_scale` 간격으로 가장 가까운 픽셀을 읽는다.
  채널 입력은 프레임마다 `scene/input.wgsl` 패스로 만든다: 원본(HDR 장면 또는 레이어 단독 텍스처)에서 이 레이어의 화면 rect를 잘라
  rect 크기의 `Rgba16Float`에 쓰고, GLSL이면 상하를 뒤집는다. 원본 포맷과 관계없이 필터링 가능한 포맷이 된다.
  `"below"`를 읽는 레이어가 있으면 레이어 패스를 그 앞에서 끊는다(같은 텍스처를 읽으며 쓸 수 없음). 레이어 id 입력은 그 레이어를
  투명 바탕의 장면 크기 텍스처에 따로 그린다.
- **효과 (`effects`):** image 레이어마다 이미지 크기의 `Rgba16Float` 텍스처 두 장을 번갈아 쓰며, 효과마다 화면 삼각형 패스 하나
  (원본 → 효과 1 → 효과 2 …). 레이어 사각형은 마지막 결과를 읽는다. 효과 파일의 uniform은 셰이더 배경과 같은 방식(naga로 멤버 이름·offset을
  읽음)으로 채운다: `time`·`size`·`resolution`은 waywall, 나머지는 효과 표의 같은 이름 필드. `shader` 효과는 Shadertoy 컴파일 경로를 쓰고
  결과를 마스크로 원본과 섞는 패스가 하나 더 붙는다.
- **`clip`:** 대상 레이어를 투명 바탕의 장면 크기 텍스처에 단독으로 그리고(shader 레이어 입력과 같은 `isolated`, 프레임 처음에 아래부터),
  image 셰이더가 그 텍스처의 같은 픽셀 알파를 곱한다(바인딩 5, `textureLoad`). 대상이 아래 레이어라 순서대로 그리면 늘 준비되어 있다.
  clip이 있는 shader 레이어는 합성 사각형이 필요하므로 텍스처 경유로 그린다.
- **`follow`:** 레이어마다 패럴랙스와 같은 지수 추적(`smoothing`, 정착 `settle_px`)을 하고, 이동량을 사각형 위치에 더한다.
- **조명:** `scene/image.wgsl` + `scene/lighting.wgsl`. 광원 목록은 storage 버퍼 하나(앰비언트 합 + 광원 배열)이며 CPU가 프레임마다
  `"pointer"` 위치와 `flicker`를 반영해 쓴다. 레이어 uniform에 설계 좌표 축(조명 위치 계산)과 회전(노멀맵 회전)을 넣는다.
  `lit`이 아닌 레이어는 같은 셰이더에서 조명 계산을 건너뛴다.
- **블룸:** compute 패스 하나 안의 디스패치들. 축소 체인 `down[i]`(`Rgba16Float` storage, 단계마다 절반)은 첫 단계에서
  threshold를 넘는 만큼만 남기고, 확대 체인 `up[i] = down[i] + 텐트필터(up[i+1])`(가장 작은 단계는 `down`을 그대로 읽음)를 작은 쪽부터 만든다.
  후처리가 `up[0]`을 읽어 `intensity`배로 더한다. `intensity = 0`이면 텍스처도 패스도 만들지 않는다.
  처음에는 단계마다 렌더 패스(additive blend)였으나, 패스마다 드라이버가 커맨드 버퍼를 초기화하는 CPU 비용(Intel anv의 `memset`)이
  커서 바꿨다(반딧불 씬 두 출력 180fps에서 CPU 약 19% → 측정은 로드맵).
- **파티클:** 레이어마다 storage 버퍼 하나(파티클당 위치·속도·나이·수명 24바이트 × `max`, 0으로 초기화 = 모두 죽음).
  CPU는 프레임마다 방출 수만 정한다: `rate × dt`의 정수부(소수부는 이월) + 새 왼쪽 클릭 수 × `burst`. 고리 버퍼의
  다음 자리 `[cursor, cursor + n)`을 compute 셰이더에 알려 주면, 그 범위의 스레드는 파티클을 새로 만들고(난수 = 인덱스와
  프레임 시드의 해시) 나머지는 살아 있으면 적분한다. 원자 연산도 GPU → CPU 읽기도 없다.
  그리기는 `particles/render.wgsl` + 스프라이트 파일을 이어 붙인 레이어 전용 파이프라인(블렌드·스프라이트가 레이어마다 다름)으로
  `draw(0..4, 0..max)`. 죽은 파티클은 버텍스 셰이더가 화면 밖으로 보낸다.
  CPU는 "마지막 방출 시각 + 최대 수명"을 기억해 모든 파티클이 죽었는지 안다(잠들기 판단).
- **캐릭터 레이어 (live2d · vrm · spine, 설계 — [roadmap.md](roadmap.md#m9--live2d-레이어-설계) M9–M11):** 텍스처 경유 shader 레이어와 같은 길을 쓴다.
  레이어마다 `rect` 렌더 크기의 `Rgba16Float` 텍스처에 모델을 그리고(투명 바탕, premultiplied), image 파이프라인 사각형으로
  `blend`·`opacity`·`parallax`·`clip`을 적용해 합성한다. 레이어 id 입력·`clip` 대상이 되면 그 텍스처를 그대로 넘긴다.
  - 시선·클릭은 메인 스레드의 레이어 상태가 처리한다: `follow`와 같은 지수 추적으로 시선 벡터를 만들고, 클릭은 렌더 px → 모델 공간으로
    바꿔 부위를 판정한 뒤 모션을 시작한다. 판정은 CPU에서 지금 프레임의 메시로 한다(GPU 읽기 없음): 부위 키의 메시(HitArea·`drawable:<ID>`·
    바운딩 박스·본 가중치) → `"*"`/문자열이면 보이는 메시 전체, 겹치면 그리기 순서가 위인 쪽. 어디에도 안 맞으면 그 클릭은 이 레이어가 쓰지 않는다. 모션·물리 적분은 프레임마다 CPU(`dt`는 파티클과 같은 `max_step` 상한).
  - **live2d:** 백엔드가 파라미터 → drawable 메시(정점·UV·불투명도·그리기 순서·마스크)를 만들고, 렌더는 waywall의 wgpu 파이프라인
    (`assets/shaders/live2d/`)이 한다. 클리핑 마스크는 마스크 drawable을 먼저 마스크 텍스처에 그려 알파를 곱한다. 백엔드는 정적 링크한
    PurismCore(Cubism Core C ABI v6) 하나다(M9b). Core는 메시 변형만 하므로
    `.motion3.json`·`.physics3.json`·`.pose3.json`·표정 적용과 모션 크로스페이드, 눈 깜빡임·숨쉬기는 waywall이 파라미터 배열에 쓴 뒤 `csmUpdateModel`을
    부른다. Core가 모델을 읽지 못하면(`csmReviveMocInPlace` 실패, 모르는 MOC 버전) 로드 단계에서 그 레이어만 거절.
  - **vrm:** 로드 시 glTF를 읽어 정점·인덱스·텍스처를 출력마다 업로드. 프레임마다 CPU가 `.vrma` 샘플링 → 휴머노이드 포즈 → LookAt → SpringBone
    → 본 행렬을 storage 버퍼로 쓰고, 버텍스 셰이더가 스키닝·모프를 한다(모프 가중치도 storage 버퍼). 깊이 버퍼(`Depth32Float`)는
    레이어 텍스처 크기로 레이어 안에서만 쓴다. MToon은 불투명 → 알파 컷 → 반투명(뒤에서 앞으로 정렬) 순서, 외곽선은 법선 방향으로 부풀린
    뒷면 패스.
  - **spine:** `spine-c`(FFI)가 프레임마다 애니메이션 상태 적용 → 물리 → 월드 변환을 하고, 그리기 순서대로 어태치먼트 정점·UV·색을 낸다.
    waywall은 이를 프레임마다 정점 버퍼 하나로 모아 블렌드 모드가 바뀌는 곳에서만 draw를 나눈다. 클리핑 어태치먼트는 런타임의 다각형 잘라 내기
    결과를 그대로 그린다. 아틀라스 `pma` 표시가 없으면 업로드 때 알파를 곱해 다른 레이어와 같은 premultiplied로 맞춘다.
- **잠들기:** `draw`가 "아직 움직이는 중"(패럴랙스가 목표에 도달하지 않음, 살아 있는 파티클이 있음, `rate > 0`, shader 레이어, 조명 `flicker`)을 돌려줄 때만 다음 frame 콜백에서 다시 그린다.
  아니면 렌더 루프를 멈추고, 포인터 이동(패럴랙스·파티클 `pointer`가 있을 때)·클릭(`burst`가 있을 때)·크기 변경·`reload`에서만 한 장 그린다. 정적인 씬은 표시 후 비용 0.

### 보류한 최적화: 정지 레이어 캐시

2026-11-02에 구현해 측정했지만 이득이 없어 넣지 않았습니다. 다른 GPU에서 다시 볼 때를 위한 기록입니다.

**아이디어.** 처음으로 시간에 따라 변하는 레이어(파티클 등) 아래의 레이어들은 패럴랙스가 멈춰 있으면 매 프레임 결과가 같습니다.
이 앞부분을 출력마다 캐시 HDR 텍스처에 한 번 그려 두고, 프레임마다 캐시를 HDR 버퍼로 복사(`copy_texture_to_texture`)한 뒤
나머지 레이어만 `LoadOp::Load`로 그립니다.

**구현했던 설계.**
- 설정 `render.layer_cache`(bool). 캐시 대상은 첫 변하는 레이어 앞까지. 변하는 레이어가 없는 씬은 어차피 잠들므로 캐시하지 않음.
- 캐시 키 = 패럴랙스 이동량(앞부분에 `parallax ≠ 0`인 레이어가 있을 때만, 아니면 상수).
  키가 바뀌는 동안(이동 중)은 캐시 없이 전부 그리고, 같은 키가 두 프레임 이어지면 다시 그림. 크기가 바뀌면 버림.
- 캐시 텍스처는 HDR 버퍼와 같은 크기·포맷(`RENDER_ATTACHMENT | COPY_SRC`), HDR 버퍼에 `COPY_DST` 추가.
  contain의 scissor는 캐시를 그릴 때도 같게 적용(레터박스 = 캐시의 clear 색).
- 검증: 패럴랙스를 끈 반딧불 씬에서 캐시를 켜든 끄든 화면 캡처가 픽셀 단위로 같았음.

**측정 (Intel Iris Xe / Mesa, 1920×1080 `rgba16float`, 반딧불 숲, GPU 타임스탬프로 잰 프레임당 GPU 시간).**

| 구성 | 프레임당 GPU 시간 |
|---|---|
| 셰이더 배경(단순 그라데이션) | 0.29–0.36 ms |
| 빈 씬(clear + 후처리) | 0.41 ms |
| 파티클만(반딧불 3000개) | 0.76 ms |
| 이미지 3장 + 파티클 1개 | 1.21 ms |
| 전체, 캐시 끔 | 1.18–1.37 ms |
| 전체, 캐시 켬 (하늘 + 먼 숲 캐시) | 1.17–1.32 ms |
| 전체, `rg11b10ufloat` | 1.16 ms |

캐시가 아끼는 것(화면 전체 레이어 블렌딩 1장 + 절반 크기 1장)과 새로 드는 것(화면 전체 복사 1번)이 이 GPU에서는 거의 같았습니다.
대신 출력마다 GPU 메모리를 HDR 버퍼 하나만큼(약 16 MB) 더 씁니다. 같은 측정에서 비용을 실제로 줄인 것은 초당 장수였습니다
(가려진 출력 정지, fps 상한, `render_scale`). 매 프레임 컴포지터가 다시 합성하는 비용도 장수에 비례합니다.

**다시 볼 조건.** 복사가 블렌딩보다 훨씬 싼 GPU(외장 GPU 등)에서 같은 측정으로 이득이 확인될 때.
캐시 앞부분이 길고 무거운 씬(화면 전체 레이어가 여러 장)에서 먼저 재 볼 것.

**넣을 때 따져야 할 것.** 캐시는 "앞부분은 캐시 키가 같으면 결과가 같다"는 가정에 기댑니다. 이후 기능이 이 가정을 깹니다.
- M6c 조명: `lit` 레이어는 `position = "pointer"`인 광원이 있으면 커서를 따라 바뀜 → 키에 광원 위치 포함, `flicker`가 있으면 캐시 불가.
- M6c shader 레이어: 시간에 따라 바뀌므로 변하는 레이어. `inputs`의 `"below"`·레이어 `id`는 캐시와 무관하지만 다른 레이어가 이 레이어를 참조하면 의존이 생김.
- M6d `effects`(`sway` 등)는 시간에 따라 바뀜 → 그 레이어부터 캐시 불가. `clip`·`follow`도 포인터를 따름 → 키에 포함하거나 캐시 불가.
- 따라서 레이어마다 "변하는지 / 무엇에 의존하는지"를 한곳(레이어 타입)에서 답하게 하고, 캐시 키를 그 합으로 만들 것.
  놓치면 화면이 갱신되지 않는 버그가 됨.

## 데몬 모듈

```
crates/waywall/src/
├─ main.rs            # clap: daemon | set | pause | resume | volume | list | reload | quit
├─ daemon.rs          # calloop 루프 구성, 출력 생성/제거, IPC 요청 처리
├─ wayland/
│  ├─ mod.rs          # SCTK 핸들러 (출력, layer, seat/포인터 + 커서, scale)
│  ├─ globals.rs      # SCTK가 감싸지 않는 전역 객체: viewporter, fractional-scale, single-pixel-buffer
│  ├─ output.rs       # Output: layer surface, 단색 버퍼, 이미지 서브서피스, viewport
│  └─ dmabuf.rs       # linux-dmabuf feedback, wl_buffer 생성/캐시
├─ fit.rs             # fit 모드 → viewporter source/destination, downscale 크기
├─ gpu/
│  ├─ mod.rs          # Gpu 지연 생성, Surface 구성
│  └─ shadertoy.rs    # 프리앰블/에필로그 래핑 → naga 파싱·검증, uniform 레이아웃
├─ backend/
│  ├─ mod.rs          # trait Backend
│  ├─ image.rs
│  ├─ video.rs        # GStreamer 파이프라인, 샘플 → 버퍼
│  ├─ view.rs         # GpuView: wgpu surface, frame 콜백 페이싱, 시계, 포인터 (shader·scene 공유)
│  ├─ shader.rs        # Shadertoy 렌더러(배경·씬 shader 레이어 공용) + 채널 입력
│  ├─ scene/          # 씬 로더(load.rs), 렌더러(render.rs: 레이어·조명·후처리), 파티클(particles.rs), shader 레이어(shader_layer.rs), 블룸(bloom.rs)
│  └─ web.rs          # 헬퍼 프로세스 관리 프록시
├─ visibility/
│  ├─ mod.rs          # 사유 합성
│  ├─ frame.rs        # wl_surface.frame 타임아웃
│  ├─ foreign_toplevel.rs
│  └─ niri.rs         # niri IPC EventStream (설정 visibility.niri, any-window 정책)
├─ config.rs          # 계층 병합: defaults.toml < 배경 작성자 < 사용자 < 출력 < CLI
├─ config_edit.rs     # `--save`: toml_edit로 주석·서식을 보존하며 config.toml 수정
├─ runtime.rs         # CLI 계층: set/volume/pause 값을 출력 이름 기준으로 기억 (재연결 시 복원)
├─ assets.rs          # 사용자 assets 디렉터리 → 내장 순으로 파일 해석
├─ wallpaper.rs       # 경로 → 타입 판별
└─ ipc.rs             # 소켓 서버(데몬, calloop 소스)와 클라이언트(CLI)
```

### Backend trait

```rust
trait Backend {
    /// frame 콜백 시 호출. 그릴 게 있으면 그리고 다음 콜백이 필요한지 반환
    fn frame(&mut self, out: &mut OutputCtx, now: Instant) -> NeedFrame;
    fn configure(&mut self, out: &mut OutputCtx, size: PhysicalSize, scale: f64);
    fn pause(&mut self);
    fn resume(&mut self);
    fn set_volume(&mut self, volume: f64, mute: bool) {}
    fn pointer(&mut self, ev: &PointerEvent) {}
}
```

`NeedFrame::No`(정적 이미지, 일시정지 상태)이면 frame 콜백을 요청하지 않아 **완전히 잠듭니다.**
video는 GStreamer 채널 이벤트로 깨어나 버퍼만 attach합니다.

### 입력
- `wl_pointer` enter/motion/button/axis → 출력 기준 좌표(물리 픽셀)의 `PointerEvent`로 변환 → 해당 출력 Backend
- 포인터가 들어오면 `wp_cursor_shape_v1`로 `cursor.shape` 설정값의 커서 지정 (없으면 xcursor 테마 폴백)
- `interactive = false`인 배경은 `wl_surface.set_input_region`을 빈 영역으로 설정해 입력 자체를 받지 않음
- 웹 배경은 헬퍼의 WebKit이 직접 입력을 받음

### 일시정지 사유
출력마다 사유 집합 `{User, Hidden, Covered}`을 둡니다. 하나라도 있으면 pause, 모두 사라지면 resume.
신호원은 [protocols.md](protocols.md#가림-감지) 참고.

## 웹 헬퍼

- 데몬이 웹 배경을 처음 설정할 때 `waywall-web`을 실행하고(`WAYWALL_WEB_HELPER` → 데몬 실행 파일과 같은 디렉터리),
  웹 배경이 하나도 없으면 `quit`을 보내 종료시킴
- 한 프로세스가 여러 출력의 웹 배경을 담당 (GTK 앱 하나, 출력마다 layer 창 하나 + WebView)
- 통신: 헬퍼의 stdin/stdout JSON-lines ([cli-ipc.md](cli-ipc.md#웹-헬퍼-프로토콜)). 데몬은 stdout을 읽는 스레드에서 calloop 채널로 받는다
- **출력 맞추기:** 헬퍼는 커넥터 이름(`GdkMonitor::connector`)으로 모니터를 찾아 gtk4-layer-shell 창을 그 출력에 띄운다.
  layer·namespace는 데몬 설정(`[surface]`)을 `show`로 받아 같은 값을 쓴다
- **데몬 surface:** 웹 배경 동안 데몬의 그 출력 layer surface는 버퍼를 떼어(null attach) 언맵한다. 같은 layer에 두 surface가 겹치면
  쌓임 순서가 정해져 있지 않기 때문이다. 다른 배경으로 바꿀 때는 초기 커밋 → configure를 다시 거친 뒤 그린다
- **입력:** `interactive = false`면 헬퍼 창의 입력 영역을 비운다. 키보드는 받지 않는다(layer-shell keyboard interactivity none)
- **가림 판정:** 웹 페이지는 정적이면 프레임이 없으므로, 헬퍼는 `frame_timeout_ms`마다 GTK `FrameClock`에 프레임 하나를 요청하고
  그 프레임이 다음 주기까지 오지 않으면 `visibility hidden`을 보고한다(비용: 주기마다 한 장). toplevel/niri 판정은 데몬이 하고,
  pause 여부의 최종 결정은 데몬이 사유를 합성해 `pause`/`resume`으로 내려보낸다
- **오디오:** 음소거는 WebView 전체(`WebView::set_is_muted`), 볼륨은 페이지의 `<audio>`/`<video>` 요소에 JS로 적용한다.
  WebKit에 페이지 전체 볼륨 API가 없어 WebAudio는 음소거만 따른다
- **재시작:** 헬퍼가 비정상 종료하면 데몬이 로그를 남기고 다시 실행해 웹 출력을 다시 보낸다. `[web_helper]`의
  `restart_window_s`초 안에 `restart_limit`번 죽으면 더 시도하지 않고 웹 출력을 `error` 상태로 둔다(`reload`·새 `set`으로 다시 시도)

## 이벤트 루프와 스레드

| 스레드 | 역할 |
|---|---|
| 메인 (calloop) | Wayland 디스패치, IPC, 타이머, Backend, wgpu 렌더 제출 |
| GStreamer 내부 | 디코딩, 오디오 출력, appsink 콜백 → calloop 채널로 샘플 전달 |
| 웹 헬퍼 프로세스 | 별도 프로세스 (GTK 메인 루프) |

wgpu 렌더는 메인 스레드에서 제출만 하고 GPU가 비동기로 처리합니다.
파티클 시뮬레이션은 compute 셰이더라서 CPU 비용이 거의 없습니다.

## 성능 원칙과 측정

- 가려졌거나 정지 중이면 **frame 콜백도 타이머도 없어야** 함 (가림 해제 감지용 콜백 하나만 대기)
- 정적 이미지만 있으면 GPU 장치를 만들지 않음
- 영상은 dmabuf 경로가 기본이고, 폴백 경로에 들어가면 로그로 경고
- 측정 도구: `top`/`htop`(CPU), `nvtop`/`intel_gpu_top`/`radeontop`(GPU),
  `powertop`(초당 깨어남 횟수), `RUST_LOG=waywall=debug`의 프레임 타이밍 로그
