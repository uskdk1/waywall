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

씬 렌더러 세부는 [wallpaper-format.md](wallpaper-format.md#씬-배경).

## 데몬 모듈

```
crates/waywall/src/
├─ main.rs            # clap: daemon | set | pause | resume | volume | list | reload | quit
├─ daemon.rs          # calloop 루프 구성, 출력 생성/제거, IPC 요청 처리
├─ wayland/
│  ├─ mod.rs          # SCTK 핸들러 (출력, layer, seat/포인터 + 커서, scale)
│  ├─ globals.rs      # SCTK가 감싸지 않는 전역 객체: viewporter, fractional-scale, single-pixel-buffer
│  ├─ output.rs       # Output: layer surface, 단색 버퍼, 이미지 서브서피스, viewport
│  └─ dmabuf.rs       # (M4) linux-dmabuf feedback, wl_buffer 생성/캐시
├─ fit.rs             # fit 모드 → viewporter source/destination, downscale 크기
├─ gpu/
│  ├─ mod.rs          # Gpu 지연 생성, Surface 구성
│  └─ shadertoy.rs    # 프리앰블/에필로그 래핑 → naga 파싱·검증, uniform 레이아웃
├─ backend/
│  ├─ mod.rs          # trait Backend
│  ├─ image.rs
│  ├─ video.rs        # GStreamer 파이프라인, 샘플 → 버퍼
│  ├─ shader.rs
│  ├─ scene/          # 씬 로더, 렌더 그래프, 파티클(compute), 조명, 블룸
│  └─ web.rs          # 헬퍼 프로세스 관리 프록시
├─ visibility/
│  ├─ mod.rs          # 사유 합성
│  ├─ frame.rs        # wl_surface.frame 타임아웃
│  ├─ foreign_toplevel.rs
│  └─ niri.rs         # feature "niri"
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

- 데몬이 웹 배경을 처음 설정할 때 `waywall-web`을 실행하고, 웹 배경이 하나도 없으면 종료시킴
- 한 프로세스가 여러 출력의 웹 배경을 담당 (GTK 앱 하나, 출력마다 창 하나)
- 통신: 헬퍼의 stdin/stdout JSON-lines ([cli-ipc.md](cli-ipc.md#웹-헬퍼-프로토콜))
- 해당 출력의 데몬 layer surface는 웹 배경 동안 제거하고, 헬퍼가 같은 출력에 자체 layer surface를 띄움
- 가림 판정: frame 기아는 헬퍼가 GTK `FrameClock`으로 감지해 보고하고, toplevel/niri 판정은 데몬이 해서 내려보냄
- 헬퍼가 비정상 종료하면 데몬이 로그를 남기고 최대 3회 재시작, 그래도 실패하면 해당 출력을 `error` 상태로 둠

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
