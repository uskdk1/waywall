# 로드맵

각 단계는 "완료 기준"을 실제 Wayland 세션(niri)에서 확인해야 끝난 것으로 봅니다.

## M0 — 문서 ✅
`docs/` 설계 문서 작성.

## M1 — 기반 + 정적 이미지 ✅
- git 저장소, `flake.nix`, `rust-toolchain.toml`, Cargo 워크스페이스(`waywall`, `waywall-proto`)
- `assets/defaults.toml` + 설정 계층 병합 + asset 해석기 (처음부터 하드코딩 없는 구조로)
- SCTK: 출력별 layer surface, `wp_viewporter`, `wp_fractional_scale_v1`, 포인터 + `wp_cursor_shape_v1`
- image 백엔드(정적): `wl_shm` + viewporter, fit 모드

**완료 기준:** `nix build` 성공, 모든 모니터에 이미지 표시, 표시 후 CPU 사용 0 / 깨어남 없음(`powertop`).

**결과 (2026-11-01, niri, 2×1920x1080):** `nix build`·`nix flake check` 통과, 단위 테스트 13개. 표시 후 8초간
CPU tick 0, 자발적 문맥 전환 0회(깨어남 없음), 스레드 1개, RSS 3 MB. 디코딩(release) 30 ms, Lanczos 축소 포함 117 ms.
niri는 `wp_single_pixel_buffer_v1`을 광고하지 않아 단색은 1×1 `wl_shm` 폴백으로 동작.
GIF는 아직 첫 프레임만 표시(애니메이션은 M4의 영상 경로에서 — M4에서 해결).

## M2 — 설정 · IPC · CLI ✅
- `config.toml` 로드/병합, IPC 서버, `set` / `list` / `reload` / `quit` / `pause` / `resume`
- 모니터 연결/분리 처리

**완료 기준:** `waywall set -o <출력>`이 해당 모니터만 변경, 모니터 분리·재연결 후 설정대로 복원.

**결과 (2026-11-01, niri):** `set -o eDP-1`이 그 출력만 바꾸고 표시 완료 후 응답(33 ms). 잘못된 경로·타입·출력은
아무것도 바꾸지 않고 종료 코드 1. `niri msg output HDMI-A-1 off/on`으로 분리·재연결 시 CLI로 바꾼 배경·일시정지·볼륨까지
복원. `--save`는 주석을 보존하며 기록, `reload`는 오류 파일이면 기존 설정 유지. 두 번째 데몬은 "already running"으로 거부,
`quit` 후 소켓 삭제. IPC 소켓을 추가해도 유휴 8초간 CPU tick 0, 자발적 문맥 전환 0회, 스레드 1개 유지
(RSS 19 MB 중 약 6.6 MB는 출력 2개의 이미지 shm 버퍼). 단위 테스트 22개.
`volume`은 값만 기록하고, 실제 오디오는 M4부터 반영됩니다.

## M3 — GPU 기반 + 셰이더 + 입력 ✅
- wgpu(Vulkan) 지연 초기화, 출력별 Surface, frame 콜백 페이싱, fps 제한, `render_scale`
- Shadertoy GLSL 프리앰블 → naga, WGSL, uniform(`iMouse`, `iPointer`, `iClick`, `u_*`)
- `interactive = false` 빈 입력 영역, 컴파일 오류를 `list`에 표시

**완료 기준:** 예제 셰이더가 커서 호버/클릭에 반응, `fps = 30` 동작, HiDPI 출력에서 선명.

**결과 (2026-11-01, niri, Intel 내장 GPU / Mesa):** wgpu 30(Vulkan) + naga 30. GLSL·WGSL 예제가 화면에 표시되고,
`set` 후 GPU 초기화 포함 108 ms에 응답. 60fps 정확(5초 299–300프레임, CPU 약 4%), `fps = 30` + `render_scale = 0.5`에서
5초 150프레임·960×540 렌더. `pause` 시 프레임 0·모든 스레드 깨어남 0, `resume`으로 재개. 셰이더 → 이미지로 바꾸면 GPU 해제
(스레드 1개로 복귀). 컴파일 오류는 `bad.glsl:3:10: Unknown variable: …`처럼 사용자 파일 줄 번호로 `set`이 거절(이전 배경 유지).
swapchain은 Bgra8Unorm(Shadertoy와 같은 색). 단위 테스트 30개.
커서 호버·클릭 반응은 사용자가 직접 확인. `render.max_fps` 기본값을 0(모니터 주사율)으로 바꾼 뒤 240 Hz HDMI 출력에서 약 218fps(CPU 약 12%, 내장 GPU 거의 최대 클럭), `waywall list`에 실측 fps 표시.
**추가 확인 (2026-11-03):** `niri msg output eDP-1 scale 1.5`에서 GPU 배경이 `list`에 `1920x1080@1.5`(논리 1280x720)로 실제 픽셀 크기로
렌더되고 육안으로 선명함(공용 `GpuView`라 캐릭터 씬으로 확인).

## M4 — 동영상 (zero-copy) ✅
- GStreamer 파이프라인, VA-API 디코딩 → dmabuf → `zwp_linux_dmabuf_v1` → 서브서피스
- dmabuf feedback 기반 caps, shm 폴백, 반복 재생, GIF
- 오디오 + `waywall volume`

**완료 기준:** 4K60 영상이 끊김 없이 반복, `RUST_LOG`에 dmabuf 경로 확인, CPU 사용량이 디코딩 수준에 머무름.

**결과 (2026-11-01, niri, Intel 내장 GPU / iHD, GStreamer 1.28.7):** 4K60 HEVC·1080p60 H.264가 dmabuf 경로(`list`에 `dmabuf`, 로그 `mode=Dmabuf`)로
60fps, 데몬 CPU 2–4%. 6초 영상이 이음새 없이 반복(segment seek). dmabuf `wl_buffer`는 디코더 풀 크기(11개)에서 더 늘지 않음.
`hwdec = false`(소프트웨어 디코딩 + 복사)는 같은 1080p60에 CPU 69%. `pause` 시 CPU 0, 영상을 끄면 깨어남 0.
움직이는 GIF는 shm 경로로 약 10fps·CPU 1%. 오디오 스트림이 PipeWire에 `waywall`로 나감.
**구현 중 발견:** VA 디코더는 받는 쪽이 `VideoMeta`를 지원한다고 알려야 dmabuf를 내보내므로 appsink 패드의 allocation 쿼리에 추가.
libav GIF 디먹서는 비flush segment seek를 처리하지 못해 GIF만 flush seek로 반복.
**추가 확인 (2026-11-03):** 440 Hz 사인파 영상에서 `waywall volume` 100 → 30 → 5 → `--mute` → `--unmute`에 따라 소리 크기가 바뀌는 것을 귀로 확인.
`LIBVA_DRIVER_NAME=none`으로 VA-API를 막고 1080p60 H.264를 `set`하자 dmabuf 협상 실패 후 shm으로 다시 시작해
`list`에 `shm-fallback 58.5fps`, 데몬 CPU 약 58%(`hwdec = false`의 69%와 같은 소프트웨어 디코딩 수준).

## M5 — 가림 감지 ✅
- frame 타임아웃 → foreign_toplevel → niri (설정 `visibility.niri`)
- niri의 frame 콜백 동작 측정 후 `protocols.md` 갱신

**완료 기준:** 전체화면 창 진입 시 `list`에 사유 표시 + CPU/GPU 사용 0 수준, 해제 시 재개.

**결과 (2026-11-01, niri 26.04):** HDMI-A-1의 창을 전체화면으로 만들자 같은 순간(같은 이벤트 디스패치) 영상이 멈추고 `list`에
`paused … 0.0fps (covered:foreign_toplevel)`, 해제하자 즉시 재개되어 1초 뒤 60fps. `pause_when = "any-window"` + `visibility.niri = true`에서
창이 있는 워크스페이스로 옮기면 `covered:niri`로 멈추고 빈 워크스페이스로 옮기면 재개. frame 콜백 감시(`Hidden`)·정책 판정·niri 이벤트 반영은
단위 테스트. 
**추가 확인 (2026-11-03):** `pause_when = "never"`, `foreign_toplevel`·`niri` 신호원을 끈 상태에서 창을 전체화면으로 만들면
`frame_timeout_ms` 뒤 `list`에 `hidden`이 뜨고 멈춤. niri는 전체화면 창 아래 background layer에 frame 콜백을 보내지 않음.

## M6 — 씬 렌더러 ✅
- M6a: 씬 로더, image 레이어, 패럴랙스, HDR 버퍼 + 톤매핑
- M6b: 파티클(compute 시뮬레이션, 이미터, 포인터 상호작용, 클릭 burst)
- M6c: 조명(ambient/point/directional, 노멀맵), 블룸, 비네트, shader 레이어 + 텍스처 입력(`inputs`: `"below"` · 레이어 · 이미지 → `iChannel0..3`).
  셰이더 배경(단일 파일)에서도 `iChannel`을 쓸 수 있게 프리앰블 확장
- M6d: 레이어 효과(`effects`, 내장 `sway` + 사용자 `shader` 효과, `mask`), `clip`, 포인터 따라가기(`follow`),
  예제 씬 "햇빛 + 흔들리는 머리카락 + 커서를 따라보는 눈"

**M6a 완료 기준:** 예제 씬 `examples/scenes/parallax/`이 커서를 따라 레이어마다 다른 깊이로 움직이고, 커서를 멈추면
잠듦(프레임 0, CPU 0). `tonemap = "none"`, 노출 1.0에서 이미지 레이어의 색이 원본과 같음. `contain`에서 레터박스에 레이어가 그려지지 않음.
아직 없는 기능(파티클 등)을 쓴 씬은 이유와 함께 거절.

**M6a 결과 (2026-11-02, niri, Intel 내장 GPU / Mesa):** 예제 씬이 두 출력에 `set` 후 142 ms에 표시(GPU 초기화 포함,
텍스처 4장). swapchain `Bgra8UnormSrgb`, HDR `Rgba16Float`. 화면 캡처의 픽셀이 텍스처 색과 정확히 같음
(`#120e24`, `#2b2350`, `tonemap = "none"`). 4:3 설계 + `contain`에서 설계 영역 밖은 `clear` 색만 있고 레이어가 그려지지 않음.
가상 포인터로 커서를 움직이면 깨어나 약 60fps로 그리고, 멈추면 정착 후 잠듦: 8초간 CPU tick 0, 자발적 문맥 전환 0회.
파티클 레이어가 있는 씬은 `[[layer]] \`fireflies\`: particles layers are not supported yet (M6b)`로 거절되고 이전 배경 유지.
셰이더 배경은 공용 `GpuView`로 옮김. 단위 테스트 43개(+씬 로더·변환·패럴랙스·내장 셰이더 검증·예제 로드).
**추가 확인 (2026-11-03):** 레이어마다 다른 깊이로 움직이는 모습을 육안으로 확인. 분수 스케일(1.5)에서도 선명(M3 추가 확인 참고).

**M6b 완료 기준:** 예제 씬(반딧불 숲, `examples/scenes/fireflies/`)이 60fps로 돌고 반딧불이 커서에 끌려옴, 클릭하면 그 자리에서
burst. 파티클 10만 개에서도 데몬 CPU가 파티클 수와 관계없이 미미. burst 전용 레이어만 있는 씬은 파티클이 다 사라지면 잠듦.
(커서를 따라가는 점광원은 M6c.)

**M6b 결과 (2026-11-02, niri, Intel 내장 GPU / Mesa):** 예제 씬 `examples/scenes/fireflies/`(파티클 레이어 2개)가 eDP-1 60fps,
HDMI-A-1 약 119fps. 반딧불 3000개에서 데몬 CPU 17.8%, 같은 씬을 10만 개(`rate = 20000`)로 바꿔도 18.0%로 같은 fps(두 출력 합 약 180fps,
CPU는 프레임 수에 비례하고 파티클 수와 무관). burst 전용 씬은 표시 후 8초간 CPU tick 0, 자발적 문맥 전환 0회.
`circle`에 `radius`가 없는 씬은 `[[layer]] #1: \`emitter.radius\` is required for circle (> 0)`로 거절되고 이전 배경 유지.
방출 수(소수부 이월·고리 버퍼·`max` 상한)와 잠들기 판단은 단위 테스트. 단위 테스트 51개.
**추가 확인 (2026-11-03):** 커서 쪽으로 끌려오는 반딧불과 클릭 burst를 육안으로 확인.

**M6c 완료 기준:** 예제 씬 `examples/scenes/sunlight/`의 햇빛 셰이더 레이어가 `"below"`(아래 그림)의 밝은 부분에서
빛줄기를 냄. 예제 `examples/scenes/lights/`에서 노멀맵 벽이 커서를 따라가는 점광원에 입체적으로 반응하고 횃불이 흔들림.
반딧불 숲에 블룸이 보임. `[inputs]`가 있는 셰이더 배경이 `iChannel0`을 Shadertoy와 같은 방향·색으로 읽음.
`tonemap = "none"`, 노출 1, 블룸 0, 비네트 0에서 M6a와 같은 픽셀(회귀 없음).

**M6c 결과 (2026-11-02, niri, Intel 내장 GPU / Mesa):** 햇빛 예제에서 빛줄기 셰이더(`"below"` 입력, `add`)가 해 주변 나무 사이로
빛줄기를 냄(빛줄기 세기 0과 비교해 확인). 셰이더가 1080p 전체에 48번씩 샘플링해 무거워서 HDMI(120 Hz)가 약 64fps로 떨어짐(예제의 비용).
조명 예제에서 노멀맵 벽돌의 빛을 향한 가장자리가 밝아짐(방향 확인). 반딧불 숲에 기본 블룸(intensity 0.5)으로 빛 번짐이 생김.
`[inputs]`가 있는 셰이더 배경(`examples/shaders/ripple/`)이 그림 위쪽(빨강)을 화면 위에 그림(GLSL 상하 반전 확인).
회귀: 패럴랙스 예제(`tonemap = "none"`, 블룸 기본값, 패럴랙스 끔)를 M6b 빌드와 비교해 최대 1/255 차이가 0.04% 픽셀(191개)에서 남.
image 셰이더에 조명 분기·바인딩이 늘며 생긴 부동소수 반올림으로 봄. 단위 테스트 50개(+채널 입력 계층, `iChannelResolution` 배열 배치,
광원 버퍼·흔들림, 블룸 단계 크기).
**추가 확인 (2026-11-03):** 커서를 따라가는 점광원, 빛줄기 광원, 횃불 흔들림을 육안으로 확인. 분수 스케일은 M3 추가 확인 참고.

**M6c 이후 최적화 (2026-11-02):** perf로 보니 CPU의 큰 몫이 Intel Vulkan 드라이버가 커맨드 버퍼를 초기화할 때의 `memset`(16.7%)이었고,
블룸의 렌더 패스 7개(축소 4 + 확대 3)가 그 원인 중 하나. 블룸을 compute 패스 하나로 합치고, shader 레이어를 가능하면 HDR 버퍼에 직접
그리고(중간 텍스처·합성 생략), 프레임마다 하던 할당(uniform 슬롯 HashMap, 입력 바인드 그룹, 파티클 uniform Vec)을 없앰.
반딧불 숲(두 출력 합 약 180fps, 블룸 기본값)의 데몬 CPU가 25.6% → 19.6%(user 20.6% → 15.1%, 번갈아 두 번씩 측정).
블룸을 끈 경우와의 차이는 user 약 +6.7%p에서 +1.5%p로. 빛줄기 예제는 직접 경로로 바꾼 뒤에도 같은 화면(3% 허용 오차로 픽셀 차이 0),
WGSL 레이어의 `g.origin`은 일부 `rect`에서 모서리 값으로 확인.
shader 레이어 `render_scale`(레이어별 렌더 해상도)을 추가하고 빛줄기 예제에 `0.5`를 적용: HDMI(120 Hz)가 약 60fps → 119.7fps,
빛줄기 모양·방향은 같고 조금 부드러워짐(전체 해상도와 나란히 비교).

**M6d 완료 기준:** 예제 캐릭터 씬(`examples/scenes/character/`)에서 머리카락만 흔들리고(마스크 밖은 고정),
눈동자가 흰자 밖으로 나가지 않으면서 커서를 따라봄.

**M6d 결과 (2026-11-02, niri, Intel 내장 GPU / Mesa):** 캐릭터 예제가 두 출력에서 약 60/117fps. 빛줄기·follow를 뺀 변형에서 두 프레임의 차이가
머리카락 윤곽에만 생기고 얼굴 안쪽·배경은 그대로(sway가 마스크 안에서만 움직임). 눈동자를 옆으로 밀어 둔 변형에서 흰자 밖 부분이
잘림(clip). 효과 파일 uniform을 이름으로 채우기·`follow` 계산(최대 이동, 거리 비례, 패럴랙스 기준점, `on_leave`)·필드 검사는 단위 테스트.
빛줄기 예제의 일정한 샘플 간격 때문에 생긴 줄무늬(창문 모양이 사슬처럼 반복)를 픽셀마다 시작점 지터로 없앰. 단위 테스트 54개.
**추가 확인 (2026-11-03):** 커서를 따라보는 눈동자를 육안으로 확인(스케일 1.5 출력 포함).
`type = "shader"` 효과(GLSL, 물결 왜곡 + 1초마다 색 바꾸기 + `iChannel1` 체커)를 캐릭터 예제의 방 레이어에 왼쪽 절반 마스크로 걸자
왼쪽 절반만 효과가 적용되고 오른쪽·다른 레이어는 그대로. `iChannel1`은 기본 `wrap = "clamp"`라 `uv * 8`로 읽으면 왼쪽 아래 1/8에만
무늬가 생김(GLSL uv 원점이 왼쪽 아래인 것도 확인). `wrap = "repeat"`를 주면 전체에 반복.

## M7 — 웹 헬퍼 ✅
- `waywall-web` 크레이트, 헬퍼 프로토콜, 프로세스 수명 관리
- `window.waywall` API, 오디오/음소거, pause 이벤트, 헬퍼 측 frame 기아 보고

**완료 기준:** 예제 HTML이 마우스에 반응, 웹 배경을 지우면 헬퍼 종료, 헬퍼 강제 종료 시 재시작.

**결과 (2026-11-02, niri, GTK 4.22 · WebKitGTK 2.52 · gtk4-layer-shell 1.3):** 예제 `examples/web/pointer/`가 `set` 후 0.55초에
(페이지 로드 완료 시) 응답하고 마우스 이동·클릭에 반응. 웹 → 이미지로 바꾸면 데몬 surface를 다시 매핑해 그리고 헬퍼가 종료,
다시 웹으로 바꾸면 헬퍼 재실행. 헬퍼를 SIGKILL하면 새 헬퍼가 같은 페이지를 다시 띄움. 60초 안에 4번째로 죽으면 포기하고 오류 화면,
`reload`로 복구. `pause`가 페이지까지 전달되어 예제 페이지의 CPU가 헬퍼 6.2% + WebKit 렌더 프로세스 7.5% → 둘 다 0%, 데몬은 내내 0%.
메모리는 WebKit 특성상 큼: 헬퍼 236 MB + 렌더 프로세스 203 MB + 네트워크 프로세스 78 MB. 데몬 종료 시 헬퍼·WebKit 프로세스 모두 종료.
패키지 빌드(`nix build`)에서 래퍼가 헬퍼 경로를 넣고 GApps 환경으로 실행됨.
**구현 중 발견:** layer surface를 언맵(null attach)하면 상태가 만든 직후로 돌아가서, 다시 매핑할 때 앵커·크기를 다시 보내지 않으면
niri가 `width 0 requested without setting left and right anchors` 프로토콜 오류로 연결을 끊음.
**추가 확인 (2026-11-03):** 웹 배경 출력의 창을 전체화면으로 만들면 헬퍼의 frame 기아 보고로 `list`에 `hidden`이 뜨고 멈춤.
웹 배경 중 `niri msg output HDMI-A-1 off` → `on`으로 핫플러그를 흉내 내자 헬퍼 재시작 없이 다시 연결된 출력에 페이지가 뜨고
`shown`으로 `playing`. 다만 `show`가 GDK 모니터보다 먼저 와서 모니터 추가 시 띄우는 경로를 탔는지는 로그가 없어 구분하지 못함.
`interactive = false`로 바꾼 예제 사본은 마우스 이동·클릭에 전혀 반응하지 않음(입력 영역 비움).
**발견·수정 (2026-11-03):** 로컬 `<audio src="tone.wav">`가 `MEDIA_ERR_SRC_NOT_SUPPORTED`(networkState 3)로 재생되지 않음.
같은 파일을 `fetch` → blob URL로 재생하거나 WebAudio 오실레이터는 소리가 났음. 원인은 WebKit 웹 프로세스의 bubblewrap 샌드박스에
배경 디렉터리가 없는 것(페이지·이미지·`fetch`는 네트워크 프로세스가 읽어서 됨, 미디어 파일은 웹 프로세스가 직접 엶).
페이지마다 `WebContext`를 만들어 웹 프로세스가 뜨기 전에 배경 디렉터리를 읽기 전용으로 `add_path_to_sandbox`하자 PipeWire에 `WebKit` 스트림이 생김.
`waywall volume`·`--mute`의 값이 `window.waywall`과 `<audio>` 요소의 `volume`·`muted`에 반영되는 것을 확인.

## M8 — 예제와 마무리 ✅
- `examples/`: 셰이더, 씬(반딧불 숲), 마우스 반응 HTML, 디렉터리형 배경(`project.toml` + properties)
- 문서 갱신, systemd user 서비스 예제, niri `spawn-at-startup` 예제

**결과 (2026-11-02):** `examples/README.md`에 예제 10개(셰이더 3, 씬 5, 웹 1 + 디렉터리형 속성 덮어쓰기 방법) 정리.
모든 예제를 여는 테스트(`examples_resolve`: 경로 해석·옵션 스키마·속성)를 추가해, 셰이더 컴파일·씬 로드 테스트와 함께
예제가 깨지면 `nix flake check`가 실패함. 자동 시작 예제 `contrib/systemd/waywall.service`(`systemd-analyze verify` 통과,
실행 파일 경로만 설치에 맞게 고침)와 `contrib/niri/config.kdl`(`niri validate` 통과)을 두고 `docs/setup.md`(설치·실행·자동 시작)에 설명.
예제를 실제로 등록하지는 않음. 문서의 지난 마일스톤 표기(“M6c부터”, “M6d로”) 정리.

## M8b — 셰이더 채널 개수 제한 해제 ✅
`iChannel0`–`iChannel3` 고정(`backend/shader.rs`의 `CHANNELS = 4`, 프리앰블의 채널 선언 4쌍과 `iChannelResolution[4]`)을 없앤다.
4개는 Shadertoy를 따라 한 값일 뿐이고, waywall 배경을 Shadertoy 사이트로 되돌려 올릴 일은 없으므로 가져오는 쪽 호환(0–3 그대로)만 지킨다.
형식은 [wallpaper-format.md](wallpaper-format.md#4개를-넘는-채널).

- 채널 수를 셰이더마다 `max(4, 쓴 가장 큰 N + 1)`로 정하고(`parse_inputs`가 그 길이의 목록을 돌려줌), 상한은 GPU 한도
  (`max_sampled_textures_per_shader_stage`·`max_samplers_per_shader_stage`). 장치를 만들 때 이 두 한도를 어댑터 값 그대로 요청
- 프리앰블 GLSL·WGSL의 표시 줄 `// waywall:channel_resolution`(uniform 블록 안 해상도 배열)·`// waywall:channels`(텍스처·샘플러 선언)를
  채널 수만큼 생성. 줄 전체가 표시일 때만 바꿈(주석 안 이름은 그대로 — 속성 표시 줄 `// waywall:properties`도 같은 규칙으로 바꿈).
  두 줄이 없는 예전 교체 프리앰블은 4채널일 때만 그대로 씀
- 셰이더 배경 `[inputs]`, 씬 shader 레이어 `inputs`, `shader` 효과 `inputs`(1번부터) 모두 같은 경로. 고정 배열은 `Vec`으로
- 4는 `SHADERTOY_CHANNELS`(Shadertoy 호환 인터페이스 값) 한 곳에만 남음

**결과 (2026-11-04, niri, Intel 내장 GPU / Mesa):** 테스트 데몬(별도 소켓)에서 광원 1(하늘·해) + 가림 레이어 5(막대 4개 + 색유리 1개,
`iChannel1`–`iChannel5`)를 받는 빛줄기 씬이 두 출력에서 `playing`. 해를 덮은 막대 네 개가 각각 어두운 그림자 줄을 부채꼴로 내고,
5번 채널(4개를 넘는 채널)의 색유리를 지난 빛이 붉게 물듦(화면 캡처). 채널 알파를 그대로 출력한 디버그 셰이더로 1·4·5번 채널이 제자리에 묶인 것도 확인.
기존 예제 ripple·sunlight·character·pointer-glow는 그대로 `playing`, wgpu 오류 없음. 이 테스트 셰이더는 1080p 전체에 48 샘플 × 6 채널이라
약 22 fps(예제의 비용; `render_scale`로 줄일 수 있음). 단위 테스트 59개(+9채널 GLSL·WGSL 컴파일과 바인딩·해상도 배열 배치,
표시 줄 없는 예전 프리앰블, 주석 안 표시 이름, 채널 수 계산·중간 빈 채널, 잘못된 키 `iChannel01`·`iChannelX`).
처음 테스트 셰이더(전체 해상도, 48 샘플)는 GPU를 다 써서 컴포지터까지 버벅였고, `render_scale = 0.5`·32 샘플로 줄이자 두 출력 약 59 fps.
이를 예제 [`examples/scenes/stained-glass/`](../examples/scenes/stained-glass/)(하늘 + 창틀 막대 3 + 색유리 2, 채널 6개)로 정리:
막대 세 개의 그림자 줄과 빨간·파란 빛기둥이 보이고 두 출력 59 fps.
**구현 중 발견:** 화면 공간 빛줄기는 가리는 레이어가 화면에서 광원을 덮을 때만 그림자를 만든다(처음 테스트에서 막대가 해 밖에 있어 그림자가 없었음).
[빛 가리기](wallpaper-format.md#빛-가리기-빛줄기와-캐릭터레이어)에 적음.

**완료 기준:** 기존 예제(ripple, sunlight, character)가 그대로 동작하고 Shadertoy 셰이더가 수정 없이 컴파일됨. 광원 1 + 가림 레이어 4개 이상을
입력으로 받는 빛줄기 셰이더가 각 레이어의 투명한 곳으로만 빛을 통과시킴([빛 가리기](wallpaper-format.md#빛-가리기-빛줄기와-캐릭터레이어)).

## M8c — 셰이더 잠들기 ✅
시간을 읽는 셰이더만 계속 그린다. 전에는 셰이더 배경·shader 레이어·셰이더 효과가 있으면 항상 계속 그려서, 포인터에만 반응하는 셰이더(커서 아래만 다른 그림이
보이는 렌즈 등)도 커서가 멈춰 있는 동안 1080p 두 출력을 매 프레임 다시 그려 화면 전체가 버벅였다(2026-11-05, Intel 내장 GPU. 같은 효과를
`clip` + `follow`로 바꾸자 멈춰 있을 때 CPU 약 2%). 형식은 [wallpaper-format.md](wallpaper-format.md#잠들기-animate).

- **자동 판정** (`gpu/shadertoy.rs` `UniformReads`): naga 모듈의 모든 함수에서 uniform 전역의 멤버 포인터가 실제로 쓰이는지(그 포인터를
  `Load`하거나 더 들어가는 `AccessIndex`·`Access`) 본다. GLSL 프런트엔드는 익명 블록의 멤버 포인터를 쓰든 안 쓰든 미리 만들어 두므로
  포인터가 있다는 것만으로는 읽은 것이 아니다. 멤버는 이름으로 가린다(프리앰블 교체 대비). uniform 블록 전체 load·동적 인덱스는 "전부 읽음"
- 시간 멤버(`iTime`·`iTimeDelta`·`iFrame`·`iFrameRate`·`iDate`, WGSL은 `time`·`time_delta`·`frame`·`frame_rate`·`date`)를 안 읽으면
  `Activity::moving`이 거짓: 셰이더 배경은 입력 사건 때만 그리고, 씬에서는 레이어·효과가 "움직이는 것"에서 빠져 씬이 그릴 때 같이 그림.
  포인터 uniform(`iMouse`·`iPointer`·`iClick`)을 읽는 셰이더만 포인터 사건에 깨어남. `iMouse`를 읽으면 버튼을 누른 뒤 한 프레임 더
- 작성자 힌트 `animate = { click, pointer, start }`: 그 사건 뒤 적은 초 동안만 계속 그림(`Pointer::last_event`·`iClick.z` 기준).
  `"always"`로 강제, 사용자 `[types.shader]`·`[types.scene.shader]`·`[types.scene.effects.<type>]`의 `animate`가 우선.
  효과는 효과 섹션에 값이 없으면 `[types.scene.shader] animate`
- 내장값 `animate = "auto"`: `assets/defaults.toml`의 `[types.shader]`·`[types.scene.shader]`·`[types.scene.effects.sway]`
- 함께 고침: shader 레이어의 `follow`가 움직이는 중에도 씬이 잠들 수 있던 것(follow 결과를 `moving`에 넣지 않았음. 전에는 셰이더가 늘 깨워 둬서 드러나지 않음)

**결과 (2026-11-05, niri, Intel 내장 GPU / Mesa):** 테스트 데몬(별도 소켓)에서 `iTime`을 읽지 않는 렌즈 셰이더 씬(그림 두 장 + `"below"`·레이어 입력 +
`iPointer`)이 커서가 없는 출력(HDMI-A-1)에서 약 0.2 fps(잠듦), `iTime`을 읽는 `pointer-glow.glsl`은 두 출력 60·118 fps(주사율)로 계속.
`sunlight`(빛줄기, 포인터·아래 장면만 읽음)도 이제 커서가 없는 출력에서 잠듦. 커서가 있는 출력에서 렌즈를 움직이는 모습과
`animate = { click = 0.7 }`의 화면 동작은 직접 확인하지 못함(단위 테스트로만). 단위 테스트 76개
(+GLSL·WGSL 판정: 포인터만·선언만·시간 다섯 가지·함수 인자·다른 함수·블록 전체 load, 예제 셰이더, `Activity` 창·`iMouse` 한 프레임, `animate` 필드 우선순위·잘못된 값).

**완료 기준:** `iTime`을 읽지 않는 렌즈 셰이더(포인터 + 그림 두 장)가 커서가 멈추면 잠들어 정적 이미지 수준이고,
커서를 움직이면 바로 따라감. `iTime`을 읽는 기존 예제(pointer-glow, gradient, ripple)는 그대로 계속 그림.
`animate = { click = 0.7 }`인 클릭 파문 셰이더가 클릭 후 0.7초 뒤 잠듦. 판정 단위 테스트(GLSL·WGSL, 선언만 있는 경우, 함수 인자로 넘긴 경우).

## M9 — Live2D 레이어 ✅
캐릭터 일러스트가 그대로 움직이는 2D 리깅 모델을 씬 레이어로 띄운다. 형식 설계는
[wallpaper-format.md](wallpaper-format.md#캐릭터-레이어-live2d--vrm--spine).

`.moc3`(Cubism)가 사실상 표준이지만 공식 런타임 Cubism Core는 소스가 공개되지 않은 독점 라이선스라 저장소·Nix 패키지에 넣어
배포할 수 없다. 그래서 백엔드를 먼저 정한다.

검증 모델: 사용자가 가진 Live2D 공식 샘플 "初音ミク" FREE판·PRO판(`~/miku_ja`, PRO판은 스키닝·애니메이션 베이크 포함).
둘 다 HitArea가 없다(`"HitAreas": []`). 모델 파일은 저장소에 넣지 않고, 예제(`examples/live2d/`)는 배경 설정과 직접 그린 텍스처만 담아 사용자가 받은 모델을 `model/`에 넣게 한다.

- M9a: ~~**mocari 검증**~~ — **취소.** 순수 Rust `.moc3` 런타임 mocari는 기술적으로는 동작했으나(아래 결과),
  제작자가 행방불명되어(GitHub 계정·저장소 `Eatgrapes/Mocari`가 사라짐, 2026-07 이후 릴리스 없음) 유지될 수 없다고 보고 백엔드 후보에서 뺐다.
  결과는 비교 기준으로 남긴다
- M9a′: **PurismCore 검증** — [SakuraMotion/PurismCore](https://github.com/SakuraMotion/PurismCore)(MIT, C99, v1.1.0)는 Cubism Core를
  API·ABI 그대로 대신하는 오픈소스 재구현(Core 5.1 · 6.0 ABI)이다. 같은 미쿠 두 모델로 렌더·메시 변형을 확인하고, 같은 파라미터를 넣었을 때
  mocari 결과와 픽셀을 비교한다. 클리핑 마스크·블렌드 모드는 이걸 쓰는 모델로 따로 확인
- M9b: **백엔드 = PurismCore만**(결정) — Nix가 소스로 빌드해 데몬에 링크(사용자가 받을 파일 없음). 라이선스 허가가 필요한 의존은
  최대한 쓰지 않는다는 방침에 따라 공식 Cubism Core(독점 라이선스, 약관 동의 후 다운로드) 선택 경로는 두지 않는다.
  Core는 메시 변형만 하므로 모션·크로스페이드·물리·포즈·표정·눈 깜빡임·숨쉬기·시선은 waywall이 구현한다(mocari 0.4.0의 MIT 코드는 참고만)
- M9c: 구현 ✅ — 씬 `live2d` 레이어, 대기 모션·크로스페이드·눈 깜빡임·숨쉬기, 물리·포즈, 포인터 시선, 클릭 모션(모델 어디든 · HitArea · `drawable:<ID>` · `"*"`)

**열린 질문:**
- PurismCore는 비공식 재구현이다. 프로젝트는 Live2D 라이선스 없이 쓸 수 있다고 밝히지만, 모델 판매처 약관이 "공식 SDK로만 사용"을 요구하는
  경우가 있는지(그런 모델은 waywall에서 지원하지 않는 것으로 본다)
- live2d 레이어에 `lit`(씬 `[[light]]` 음영)을 줄지. 노멀맵이 없어 방향감이 약하고, 빛 가리기는 `lit` 없이도 됨
  ([빛 가리기](wallpaper-format.md#빛-가리기-빛줄기와-캐릭터레이어))

**M9a′ 결과 (2026-11-04, Intel 내장 GPU / Mesa, PurismCore v1.1.0 `1069334`, ABI v6 · compat 6.0.1):** M9a와 같은 검증 프로그램에서
PurismCore(정적 링크, 직접 쓴 최소 FFI)로 메시를 만들고 같은 렌더러로 그려 mocari 결과와 비교. 파라미터는 mocari가 계산한 값(모션·물리 적용 후)을 그대로 넣음.
- 두 모델 모두 MOC3 v3.x(`csmMocVersion_30`)로 읽힘. drawable 수·ID 순서·그리기 순서·렌더 순서가 mocari와 같음
- **좌표 규약:** 정점 위치는 같은 모델 공간. UV는 세로가 반대(Core `v` = mocari `1 − v`, Core는 왼쪽 아래 원점)
- **메시 일치:** 기본 자세 정점 위치 차이 최대 1.4e-3(FREE)·8.9e-4(PRO) 모델 단위. 렌더 결과 2% 넘게 다른 픽셀이
  기본 0.14%·0.19%, 대기 모션 0.98%·0.19%, 물리 후 0.007%·0.003%(FREE·PRO). 차이는 팔·소매 윤곽선의 1 px 미만 어긋남뿐이고 눈으로는 구별되지 않음.
  공식 Core가 없어 어느 쪽이 정답인지는 판정 못 함(Purism은 공식 Core와의 일치를 1만 개 이상 테스트로 확인했다고 밝힘)
- **성능:** `csmUpdateModel` + 메시 읽기 프레임당 CPU 0.06–0.07 ms(FREE)·0.08–0.10 ms(PRO)
- **클리핑 마스크 (2026-11-04 추가):** Live2D 공식 샘플 "모모세 히요리" 일반판 FREE·PRO(`~/hiyori_ko`, 마스크를 쓰는 drawable 4·8개,
  PRO는 텍스처 2장 + `pose3.json`)로 확인. **기본 자세는 mocari와 픽셀이 완전히 같음(차이 0)**, 눈동자·하이라이트가 흰자 안에만 그려짐.
  대기 모션 0.16%·0.15%, 고개 돌림 0.37%·0.24%(물리 끔 0.40%·0.28%)가 2% 넘게 다름 — 머리 윤곽선의 1 px 미만 어긋남으로, 물리를 꺼도 같아 회전 변형 계산의 미세 차이.
  포즈(PartArmA/B 전환)는 프레임워크 기능이라 mocari가 정한 파츠 불투명도를 옮겨 넣고 비교. 히요리 PRO 메시 갱신 0.14–0.18 ms
- **아직 확인 못 함:** 블렌드 모드(가산·곱하기)·multiply/screen 색 — 미쿠·히요리 네 모델 모두 쓰지 않음.
  ("모모세 히요리(영상용 버전)" `~/hiyori_movie_ko`는 Editor 원본만 있고 `.moc3`가 없어 쓸 수 없었음)
- 빌드는 `make CFLAGS="-O2 -fPIC"`로 `libPurismCore.a`·`.so`가 나옴(C99, 의존은 libc·libm). waywall에서는 flake 입력 + `cc` 빌드로 넣을 예정

**M9a 결과 — 취소된 mocari 검증, 비교 기준으로 보관 (2026-11-04, Intel 내장 GPU / Mesa, mocari 0.4.0):** 화면 없이 1024² 텍스처에 렌더해 PNG로 확인하는 검증 프로그램(저장소 밖)으로 확인.
- 라이선스 MIT, `forbid(unsafe_code)`, 의존 wgpu 30(waywall과 같음). 렌더러는 feature `wgpu`라 코어만 쓸 수 있음
- **렌더:** 두 모델 모두 기본 자세가 깨짐 없이 그려짐(FREE 88 drawable · 정점 1970, PRO 110 drawable · 정점 3507, 스키닝 포함 PRO도 정상). 로드 약 40 ms
- **모션:** 대기 모션 1.5초 후 화면의 27%(FREE)·15%(PRO) 픽셀이 바뀜
- **물리:** 고개를 오른쪽 → 왼쪽으로 돌린 직후, 물리를 켠 것과 끈 것의 차이가 **머리카락에만** 생김(몸·얼굴은 같음). `physics3.json`은 자동으로 읽힘
- **성능:** 모션 + 물리 + 메시 갱신이 프레임당 CPU 0.066 ms(FREE)·0.153 ms(PRO)
- **확인 못 함:** 클리핑 마스크·블렌드 모드(가산·곱하기)·multiply/screen 색 — 두 모델 모두 쓰지 않음(마스크 0, 가산·곱하기 0). 이걸 쓰는 다른 모델이 필요
- **없는 기능(직접 구현):** 모션 간 크로스페이드(`MotionPlayer`가 모션의 FadeIn/FadeOut 시간을 쓰지 않아 전환이 튐 — 두 플레이어를 가중치로 섞어야 함),
  눈 깜빡임·숨쉬기·시선(Cubism Framework 쪽 기능. `model3.json` Groups의 `EyeBlink` 파라미터 ID를 읽어 구현), `"*"`·`drawable:<ID>` 판정(메시 삼각형 판정, 정점·인덱스는 공개 API)
- **색:** 내장 렌더러는 텍스처를 `Rgba8Unorm`(선형화 없음)으로 올리고 셰이더에서 알파를 곱함. waywall HDR 경로(선형, premultiplied)와 맞지 않으므로
  설계대로 렌더는 waywall 파이프라인이 하고 mocari는 코어(메시 생성)만 쓴다
- **파라미터 이름:** FREE판은 옛 이름(`PARAM_ANGLE_X`), PRO판은 새 이름(`ParamAngleX`). `look_at.params` 기본값만으로는 FREE판이 안 움직이므로 모델별로 적어야 함
- **위험:** crates.io 첫 릴리스 2026-06-21, 마지막 0.4.0이 2026-07-13(이후 넉 달 릴리스 없음), README가 스스로 "experiment"라고 함.
  **GitHub 저장소(`Eatgrapes/Mocari`)가 404, 제작자 계정도 사라짐** → 취소. 남은 포크(`Initsnow/Mocari`)는 2026-10에 커밋 1개뿐이라 대안이 되지 못함
- 위 항목 중 크로스페이드·눈 깜빡임·숨쉬기·시선·클릭 판정·색 처리·파라미터 이름은 백엔드와 관계없이 M9c에서 그대로 해당한다

**M9c 결과 (2026-11-04, niri, Intel 내장 GPU / Mesa, PurismCore v1.1.0):**
- 빌드: flake 입력 `purism-core`(태그 커밋 `1069334` 고정) → `crates/waywall/build.rs`가 `cc`로 C 소스 17개를 컴파일해 정적 링크
  (`PURISM_CORE_SRC`, devShell·패키지 공통). 패키지에 PurismCore 라이선스(`share/licenses/waywall/PurismCore.LICENSE`)를 함께 넣음
- 렌더: 모델을 rect 렌더 크기의 `Rgba16Float` 텍스처에 그린 뒤 image 파이프라인 사각형으로 합성(텍스처 경유 shader 레이어와 같은 길).
  클리핑 마스크는 같은 마스크 묶음마다 레이어 해상도 그대로의 R8 텍스처(히요리 PRO: 묶음 2개). 테스트 씬(미쿠 PRO + 히요리 PRO)에서 두 모델이 그려지고
  히요리의 눈동자·하이라이트가 흰자 안에만 보임. 모델이 움직이지 않으면(대기 모션·깜빡임·숨쉬기·물리 모두 없음) 씬이 잠듦(0.3 fps)
- 모션: `motion3.json` 선형·베지어(제한/비제한)·계단·역계단, 모션·곡선별 페이드, 대기 모션이 끝나 갈 때 다음 모션을 겹쳐 들임.
  `PartOpacity` 곡선이 포즈 고르기 값으로 쓰여, 1단계에서 네 개로 보이던 히요리 팔이 한 세트만 보임. 프레임 비교에서 고개·눈·팔이 바뀜
- 물리: 같은 입력(머리 0 → 30 → −30 → 사인파, 60 fps 600프레임)에서 mocari 물리와 출력 파라미터 차이 최대 1.9e-4(히요리)·1.1e-4(미쿠)
- 시선·클릭: `follow`와 같은 추적(정착 허용 오차 = `settle_px` / `range`), 클릭은 Core 메시 삼각형 판정(부위가 겹치면 렌더 순서가 위인 쪽,
  `"*"`는 부위 규칙이 하나도 맞지 않을 때). 단위 테스트까지 확인, 실제 포인터로는 사용자 확인 대기
- 성능: 테스트 씬(모델 2 + 두 출력 58/104 fps)에서 데몬 CPU 약 39% → 프레임마다의 할당을 재사용 버퍼로 바꾼 뒤, 사용자 배경(미쿠 1 + 파티클 2 + 블룸, 두 출력 59/117 fps) 약 17%
- 사용자 배경 `~/waywall-wallpapers/miku/`(저장소 밖): 그라데이션 하늘 + 뒤·앞 반짝이 + 미쿠 PRO(시선, 클릭하면 Tap 모션) + 블룸
- 예제 `examples/live2d/miku`·`examples/live2d/hiyori`: 설정·직접 그린 텍스처만(히요리 배경은 샘플의 교실 그림 대신 ImageMagick 방), 모델은 `model/`(git 제외).
  테스트 `live2d_examples_need_only_the_model`: 모델이 없으면 모델 파일을 못 읽는다는 오류만, 있으면 끝까지 읽힘
- 단위 테스트 71개(+모션 세그먼트·페이드·비제한 베지어, 애니메이터 크로스페이드·깜빡임·잠들기, 물리 흔들림·정착·없는 파라미터, 포즈 전환,
  캔버스 → rect 변환, 삼각형 판정, 깨진 `.moc3` 거절)
- **아직 없는 것:** 표정(Expressions)·고정 파라미터 값(`parameters`)·립싱크, Cubism 5.3 확장 블렌드·오프스크린, 가산·곱하기 블렌드 drawable 실제 모델 확인

**완료 기준:** 예제 씬에서 Live2D 모델이 대기 모션·눈 깜빡임·숨쉬기로 움직이고, 커서를 따라보며, 모델을 클릭하면 모션이 재생되고
모델 밖 클릭은 무시됨. HitArea가 있는 모델은 HitArea별, 없는 모델은 `drawable:<ID>`로 지정한 부위별 모션이 재생됨.
모델 뒤·앞의 파티클과 블룸이 함께 보임. PurismCore가 읽지 못하는 모델은 다른 배경에는 영향 없이 그 레이어만 거절.

## M10 — VRM 레이어 (설계)
3D 툰 캐릭터를 씬 레이어로 띄운다. VRM은 glTF 2.0 기반 공개 표준이라 Nix 패키지에 그대로 들어간다.
형식 설계는 [wallpaper-format.md](wallpaper-format.md#vrm).

- glTF 로딩(`gltf` 크레이트): 메시, 텍스처, 스킨, 모프 타깃, VRM 확장(`VRMC_vrm`, `VRMC_materials_mtoon`, `VRMC_springBone`)
- GPU 스키닝 + 모프 타깃, 레이어 내부 깊이 버퍼
- MToon 셰이더(그림자 단계, 림라이트, 외곽선). `assets/shaders/vrm/` 파일이라 교체 가능
- SpringBone(머리카락·옷 흔들림, CPU), 표정 프리셋(눈 깜빡임 등), LookAt(포인터)
- VRM Animation(`.vrma`) 재생: 대기 모션, 클릭 모션

**열린 질문:** VRM 0.x 모델 지원 범위(1.0과 확장 이름·좌표계가 다름). 클릭 판정 방식(본 영역 레이캐스트로 충분한지).
바닥·벽에 떨어지는 그림자(그림자 맵)를 넣을지.

**완료 기준:** 예제 씬에서 VRM 모델이 `.vrma` 대기 모션으로 움직이고 머리카락이 SpringBone으로 흔들리며, 커서를 따라보고, 클릭하면
모션이 재생됨. MToon 외곽선·그림자가 표준 뷰어와 비슷하게 보임. 파티클·블룸과 함께 합성됨.

## M11 — Spine 레이어 (보류)
> **보류 (2026-11-04).** Spine Runtimes는 넣는 사람이 유료 Spine Editor 라이선스를 가져야 해서, 라이선스 허가가 필요한 의존은 최대한 쓰지
> 않는다는 방침과 맞지 않는다. 라이선스 허가 없이 쓸 수 있는 Spine 호환 런타임이 생기거나 방침이 바뀌면 다시 본다. 아래 설계는 그대로 남겨 둔다.

본·메시 변형 기반 2D 스켈레탈 애니메이션(Esoteric Software Spine)을 씬 레이어로 띄운다. 게임 일러스트(로비 화면 등)에 많이 쓰이는
방식이라 "일러스트가 움직이는" 배경에 맞는다. 형식 설계는 [wallpaper-format.md](wallpaper-format.md#spine).

- 런타임: 공식 `spine-c`(4.2). 소스가 GitHub에 공개되어 있어 flake 입력(커밋 고정)으로 받아 빌드하고 FFI로 부른다.
  Rust 바인딩 크레이트(`rusty_spine`)를 쓸지는 라이선스·유지 상태를 보고 정함
- 런타임은 스켈레톤 포즈 → 어태치먼트 정점·UV·색·블렌드 모드·클리핑을 내고, 렌더는 waywall의 wgpu 파이프라인(`assets/shaders/spine/`)
- 애니메이션 트랙(대기 0번, 겹침 트랙, 클릭 트랙), 전환 섞기(`mix`), 물리 제약, 스킨
- 시선: 지정한 본(IK 타깃)을 포인터 쪽으로 옮김. 클릭: 바운딩 박스 어태치먼트 다각형 판정
- cargo feature `spine`. 아래 라이선스 결론에 따라 기본 빌드 포함 여부를 정함(`packages.waywall-full`처럼 별도 패키지 가능)

**열린 질문:**
- 라이선스: Spine Runtimes는 소스 공개지만 오픈소스가 아니다. 런타임을 제품에 넣으려면 넣는 시점에 넣는 사람이 유효한 Spine Editor
  라이선스를 가져야 하고(체험판 불가), 제품에 Spine Runtimes License를 함께 넣어야 한다. 누가 라이선스를 갖고, 기본 빌드에 넣을지
- 다른 Spine 버전(4.1 이하) 파일을 받을지(런타임이 버전마다 다름 — 4.2만 받는 것으로 시작)
- 시선용 본 이름 규약을 둘지(모델마다 다름)

**완료 기준:** 예제 씬에서 Spine 스켈레톤이 대기 애니메이션과 겹침 트랙으로 움직이고, 지정한 본이 커서를 따라가며, 바운딩 박스를 클릭하면
애니메이션이 재생되고 대기로 섞여 돌아감. premultiplied 아틀라스와 아닌 아틀라스 모두 가장자리 색이 맞음. 파티클·블룸과 함께 합성됨.

**순서:** 사용자에게 미쿠 Live2D 예제 모델이 있으므로 **M9(Live2D)부터** 한다. M9a′에서 라이선스나 기능 때문에 막히면 M10을 먼저 한다.
M11은 보류. 캐릭터 레이어는 같은 합성 경로(자기 텍스처에 그린 뒤 shader 레이어처럼 합성)를 쓰므로
먼저 하는 쪽이 그 경로를 만든다.

## 이후
- 씬 정지 레이어 캐시: 구현·측정했으나 Intel 내장 GPU에서 이득이 없어 보류. 설계·측정·되살릴 조건·주의점은 [architecture.md](architecture.md#보류한-최적화-정지-레이어-캐시)
- GUI 매니저(썸네일 라이브러리, 속성 편집) — IPC `subscribe` 스트림 기반
- `ext_idle_notify_v1` 기반 유휴 정지
- 배터리 모드(UPower D-Bus) — fps 제한/정지
- 씬 확장: 타임라인 애니메이션, 스프라이트 시트, 메시 변형·본(퍼펫) 애니메이션(머리 기울이기 같은 큰 움직임), 오디오 반응(스펙트럼 → uniform)
- 재생목록/시간대별 자동 변경
- Wallpaper Engine 프로젝트 일부 가져오기(video/web 타입)
