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
GIF는 아직 첫 프레임만 표시(애니메이션은 M4의 영상 경로에서).

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
**미확인:** HiDPI(분수 스케일) 출력에서의 선명도 — 이 환경의 두 출력은 모두 스케일 1.

## M4 — 동영상 (zero-copy)
- GStreamer 파이프라인, VA-API 디코딩 → dmabuf → `zwp_linux_dmabuf_v1` → 서브서피스
- dmabuf feedback 기반 caps, shm 폴백, 반복 재생, GIF
- 오디오 + `waywall volume`

**완료 기준:** 4K60 영상이 끊김 없이 반복, `RUST_LOG`에 dmabuf 경로 확인, CPU 사용량이 디코딩 수준에 머무름.

## M5 — 가림 감지
- frame 타임아웃 → foreign_toplevel → (feature) niri
- niri의 frame 콜백 동작 측정 후 `protocols.md` 갱신

**완료 기준:** 전체화면 창 진입 시 `list`에 사유 표시 + CPU/GPU 사용 0 수준, 해제 시 재개.

## M6 — 씬 렌더러
- M6a: 씬 로더, image 레이어, 패럴랙스, HDR 버퍼 + 톤매핑
- M6b: 파티클(compute 시뮬레이션, 이미터, 포인터 상호작용, 클릭 burst)
- M6c: 조명(ambient/point/directional, 노멀맵), 블룸, 비네트, shader 레이어 + 텍스처 입력(`inputs`: `"below"` · 레이어 · 이미지 → `iChannel0..3`).
  셰이더 배경(단일 파일)에서도 `iChannel`을 쓸 수 있게 프리앰블 확장
- M6d: 레이어 효과(`effects`, 내장 `sway` + 사용자 `shader` 효과, `mask`), `clip`, 포인터 따라가기(`follow`),
  예제 씬 "햇빛 + 흔들리는 머리카락 + 커서를 따라보는 눈"

**완료 기준:** 예제 씬(반딧불 숲)이 60fps, 파티클 10만 개에서도 CPU 사용 미미, 커서를 따라가는 빛 동작.
M6c: 사진 위 햇빛 셰이더가 `"below"`의 밝은 부분에서 빛줄기를 냄. M6d: 예제 캐릭터 씬에서 머리카락만 흔들리고(마스크 밖은 고정),
눈동자가 흰자 밖으로 나가지 않으면서 커서를 따라봄.

## M7 — 웹 헬퍼
- `waywall-web` 크레이트, 헬퍼 프로토콜, 프로세스 수명 관리
- `window.waywall` API, 오디오/음소거, pause 이벤트, 헬퍼 측 frame 기아 보고

**완료 기준:** 예제 HTML이 마우스에 반응, 웹 배경을 지우면 헬퍼 종료, 헬퍼 강제 종료 시 재시작.

## M8 — 예제와 마무리
- `examples/`: 셰이더, 씬(반딧불 숲), 마우스 반응 HTML, 디렉터리형 배경(`project.toml` + properties)
- 문서 갱신, systemd user 서비스 예제, niri `spawn-at-startup` 예제

## 이후
- GUI 매니저(썸네일 라이브러리, 속성 편집) — IPC `subscribe` 스트림 기반
- `ext_idle_notify_v1` 기반 유휴 정지
- 배터리 모드(UPower D-Bus) — fps 제한/정지
- 씬 확장: 타임라인 애니메이션, 스프라이트 시트, 메시 변형·본(퍼펫) 애니메이션(머리 기울이기 같은 큰 움직임), 오디오 반응(스펙트럼 → uniform)
- 재생목록/시간대별 자동 변경
- Wallpaper Engine 프로젝트 일부 가져오기(video/web 타입)
