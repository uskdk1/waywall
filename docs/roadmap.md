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

**결과 (2026-11-01, niri, Intel Iris Xe / Mesa):** wgpu 30(Vulkan) + naga 30. GLSL·WGSL 예제가 화면에 표시되고,
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

**결과 (2026-11-01, niri, Intel Iris Xe / iHD, GStreamer 1.28.7):** 4K60 HEVC·1080p60 H.264가 dmabuf 경로(`list`에 `dmabuf`, 로그 `mode=Dmabuf`)로
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

**M6a 결과 (2026-11-02, niri, Intel Iris Xe / Mesa):** 예제 씬이 두 출력에 `set` 후 142 ms에 표시(GPU 초기화 포함,
텍스처 4장). swapchain `Bgra8UnormSrgb`, HDR `Rgba16Float`. 화면 캡처의 픽셀이 텍스처 색과 정확히 같음
(`#120e24`, `#2b2350`, `tonemap = "none"`). 4:3 설계 + `contain`에서 설계 영역 밖은 `clear` 색만 있고 레이어가 그려지지 않음.
가상 포인터로 커서를 움직이면 깨어나 약 60fps로 그리고, 멈추면 정착 후 잠듦: 8초간 CPU tick 0, 자발적 문맥 전환 0회.
파티클 레이어가 있는 씬은 `[[layer]] \`fireflies\`: particles layers are not supported yet (M6b)`로 거절되고 이전 배경 유지.
셰이더 배경은 공용 `GpuView`로 옮김. 단위 테스트 43개(+씬 로더·변환·패럴랙스·내장 셰이더 검증·예제 로드).
**추가 확인 (2026-11-03):** 레이어마다 다른 깊이로 움직이는 모습을 육안으로 확인. 분수 스케일(1.5)에서도 선명(M3 추가 확인 참고).

**M6b 완료 기준:** 예제 씬(반딧불 숲, `examples/scenes/fireflies/`)이 60fps로 돌고 반딧불이 커서에 끌려옴, 클릭하면 그 자리에서
burst. 파티클 10만 개에서도 데몬 CPU가 파티클 수와 관계없이 미미. burst 전용 레이어만 있는 씬은 파티클이 다 사라지면 잠듦.
(커서를 따라가는 점광원은 M6c.)

**M6b 결과 (2026-11-02, niri, Intel Iris Xe / Mesa):** 예제 씬 `examples/scenes/fireflies/`(파티클 레이어 2개)가 eDP-1 60fps,
HDMI-A-1 약 119fps. 반딧불 3000개에서 데몬 CPU 17.8%, 같은 씬을 10만 개(`rate = 20000`)로 바꿔도 18.0%로 같은 fps(두 출력 합 약 180fps,
CPU는 프레임 수에 비례하고 파티클 수와 무관). burst 전용 씬은 표시 후 8초간 CPU tick 0, 자발적 문맥 전환 0회.
`circle`에 `radius`가 없는 씬은 `[[layer]] #1: \`emitter.radius\` is required for circle (> 0)`로 거절되고 이전 배경 유지.
방출 수(소수부 이월·고리 버퍼·`max` 상한)와 잠들기 판단은 단위 테스트. 단위 테스트 51개.
**추가 확인 (2026-11-03):** 커서 쪽으로 끌려오는 반딧불과 클릭 burst를 육안으로 확인.

**M6c 완료 기준:** 예제 씬 `examples/scenes/sunlight/`의 햇빛 셰이더 레이어가 `"below"`(아래 그림)의 밝은 부분에서
빛줄기를 냄. 예제 `examples/scenes/lights/`에서 노멀맵 벽이 커서를 따라가는 점광원에 입체적으로 반응하고 횃불이 흔들림.
반딧불 숲에 블룸이 보임. `[inputs]`가 있는 셰이더 배경이 `iChannel0`을 Shadertoy와 같은 방향·색으로 읽음.
`tonemap = "none"`, 노출 1, 블룸 0, 비네트 0에서 M6a와 같은 픽셀(회귀 없음).

**M6c 결과 (2026-11-02, niri, Intel Iris Xe / Mesa):** 햇빛 예제에서 빛줄기 셰이더(`"below"` 입력, `add`)가 해 주변 나무 사이로
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

**M6d 결과 (2026-11-02, niri, Intel Iris Xe / Mesa):** 캐릭터 예제가 두 출력에서 약 60/117fps. 빛줄기·follow를 뺀 변형에서 두 프레임의 차이가
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

## M8b — 셰이더 채널 개수 제한 해제 (설계)
`iChannel0`–`iChannel3` 고정(`backend/shader.rs`의 `CHANNELS = 4`, 프리앰블의 채널 선언 4쌍과 `iChannelResolution[4]`)을 없앤다.
4개는 Shadertoy를 따라 한 값일 뿐이고, waywall 배경을 Shadertoy 사이트로 되돌려 올릴 일은 없으므로 가져오는 쪽 호환(0–3 그대로)만 지킨다.
설계는 [wallpaper-format.md](wallpaper-format.md#채널-개수-제한-해제-설계).

- 채널 수를 셰이더마다 `max(4, 쓴 가장 큰 N + 1)`로 정하고, 상한은 GPU 어댑터 한도에서 읽음(코드에 숫자 없음)
- 프리앰블 GLSL·WGSL의 채널 선언을 표시 줄(`// @waywall:channels`) 자리에 생성, `iChannelResolution` 크기도 생성
- 셰이더 배경 `[inputs]`, 씬 shader 레이어 `inputs`, `shader` 효과 `inputs`(1번부터)에 모두 적용
- 고정 배열(`[_; CHANNELS]`)을 쓰는 곳: `backend/shader.rs`, `backend/scene/{mod, load, shader_layer, effects}.rs`, `daemon.rs`
- 테스트: `iChannel4` 거절 테스트를 "어댑터 한도 초과 거절"로 바꾸고, 0과 높은 번호만 지정한 입력·채널 수 계산·생성된 프리앰블의 naga 검증 추가
- 구현하면 문서 절 제목 `텍스처 입력 (iChannel0–iChannel3)`과 그 앵커를 쓰는 곳(문서 2곳, 프리앰블 주석 2곳, `examples/shaders/ripple/ripple.glsl`)을 함께 고침

**완료 기준:** 기존 예제(ripple, sunlight, character)가 그대로 동작하고 Shadertoy 셰이더가 수정 없이 컴파일됨. 광원 1 + 가림 레이어 4개 이상을
입력으로 받는 빛줄기 셰이더가 각 레이어의 투명한 곳으로만 빛을 통과시킴([빛 가리기](wallpaper-format.md#빛-가리기-빛줄기와-캐릭터레이어)).

## M9 — Live2D 레이어 (설계)
캐릭터 일러스트가 그대로 움직이는 2D 리깅 모델을 씬 레이어로 띄운다. 형식 설계는
[wallpaper-format.md](wallpaper-format.md#캐릭터-레이어-live2d--vrm--spine-설계).

`.moc3`(Cubism)가 사실상 표준이지만 공식 런타임 Cubism Core는 소스가 공개되지 않은 독점 라이선스라 저장소·Nix 패키지에 넣어
배포할 수 없다. 그래서 백엔드를 먼저 정한다.

- M9a: **mocari 검증** — 순수 Rust `.moc3` 런타임([docs.rs/mocari](https://docs.rs/mocari), wgpu 렌더러 있음)을 실제 모델로 확인:
  물리(physics3), 클리핑 마스크, 블렌드 모드, 모션·표정, 눈 깜빡임·숨쉬기, 성능. 라이선스와 아래 열린 질문도 함께 확인
- M9b: **백엔드 결정** — mocari, 또는 공식 Cubism Core를 런타임에 `dlopen`(사용자가 받아 둔 파일, 경로는 config
  [`[live2d] core`](config.md#live2d-설계-m9)). Core 경로에서는 waywall이 Core 없이 빌드·실행되고, live2d 레이어만 이유와 함께 거절한다
- M9c: 구현 — 씬 `live2d` 레이어, 포인터 시선, HitArea 클릭 모션, 예제 씬

**열린 질문:**
- mocari의 라이선스와 완성도. 비공식 런타임으로 `.moc3`를 읽는 것이 모델 판매처 약관이나 Live2D 측 입장에 걸리는지(걸리면 Core 경로)
- Cubism 배포 라이선스는 개인·소규모 사업자(연 매출 1천만 엔 미만)를 면제하지만 "확장 가능한 앱(Expandable Application)"은 예외다.
  아무 모델이나 불러오는 waywall이 여기에 해당하는지
- live2d 레이어에 `lit`(씬 `[[light]]` 음영)을 줄지. 노멀맵이 없어 방향감이 약하고, 빛 가리기는 `lit` 없이도 됨
  ([빛 가리기](wallpaper-format.md#빛-가리기-빛줄기와-캐릭터레이어))

**완료 기준:** 예제 씬에서 Live2D 모델이 대기 모션·눈 깜빡임·숨쉬기로 움직이고, 커서를 따라보며, 머리를 클릭하면 그 HitArea의 모션이 재생됨.
모델 뒤·앞의 파티클과 블룸이 함께 보임. Core 경로라면 Core가 없을 때 다른 배경에는 영향 없이 그 레이어만 거절.

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

## M11 — Spine 레이어 (설계)
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

**순서:** 사용자에게 미쿠 Live2D 예제 모델이 있으므로 **M9(Live2D)부터** 한다. M9a에서 라이선스나 기능 때문에 막히면 M10을 먼저 한다.
M11은 Spine 모델이 생기면 진행한다. 세 레이어는 같은 합성 경로(자기 텍스처에 그린 뒤 shader 레이어처럼 합성)를 쓰므로
먼저 하는 쪽이 그 경로를 만든다.

## 이후
- 씬 정지 레이어 캐시: 구현·측정했으나 Iris Xe에서 이득이 없어 보류. 설계·측정·되살릴 조건·주의점은 [architecture.md](architecture.md#보류한-최적화-정지-레이어-캐시)
- GUI 매니저(썸네일 라이브러리, 속성 편집) — IPC `subscribe` 스트림 기반
- `ext_idle_notify_v1` 기반 유휴 정지
- 배터리 모드(UPower D-Bus) — fps 제한/정지
- 씬 확장: 타임라인 애니메이션, 스프라이트 시트, 메시 변형·본(퍼펫) 애니메이션(머리 기울이기 같은 큰 움직임), 오디오 반응(스펙트럼 → uniform)
- 재생목록/시간대별 자동 변경
- Wallpaper Engine 프로젝트 일부 가져오기(video/web 타입)
