# Wayland 프로토콜 사용 방침

## 원칙

필요한 기능마다 아래 우선순위로 프로토콜을 고릅니다.

1. **core / stable** — `wayland.xml`, wayland-protocols `stable/`
2. **staging** — wayland-protocols `staging/` (`ext-*`, `wp-*`)
3. **de-facto 확장** — wlr-protocols 등 여러 컴포지터가 구현한 비공식 프로토콜
4. **컴포지터 전용** — niri IPC 등. 위 단계로 해결이 안 될 때만, 기능 플래그 뒤에 둠

상위 단계로 해결되는 기능에 하위 단계를 쓰지 않습니다. 하위 단계를 쓸 때는 이 문서에
"왜 표준으로 안 되는지"를 기록합니다. 선택적 프로토콜은 컴포지터가 광고하지 않으면
해당 기능만 조용히 끄고 폴백합니다.

데몬은 GTK 없이 smithay-client-toolkit(SCTK)과 wayland-client로 아래 프로토콜을 직접 바인딩합니다.

## 요약 표 — 데몬

| 기능 | 프로토콜 | 단계 | 필수 |
|---|---|---|---|
| surface 생성 | `wl_compositor` | core | O |
| 영상용 하위 surface | `wl_subcompositor` | core | O |
| 모니터 열거, 이름(`DP-1`) | `wl_output` v4 `name`, `description` | core | O |
| CPU 버퍼 (이미지, 폴백) | `wl_shm` | core | O |
| 포인터 입력 | `wl_seat`, `wl_pointer` | core | O |
| 입력 영역 제어 | `wl_surface.set_input_region` | core | O |
| 프레임 페이싱, 가림 감지(1차) | `wl_surface.frame` | core | O |
| crop/scale (fit 모드, HiDPI) | `wp_viewporter` | stable | O |
| GPU 버퍼 공유 (영상 zero-copy) | `zwp_linux_dmabuf_v1` v4 (feedback) | stable | 권장 (없으면 shm 폴백) |
| 프레임 표시 시각 (선택) | `wp_presentation` | stable | X |
| 분수 스케일 | `wp_fractional_scale_v1` | staging | X (없으면 정수 `wl_output.scale`) |
| 커서 모양 | `wp_cursor_shape_v1` | staging | X (없으면 xcursor 테마) |
| 단색 버퍼 (배경색, 레터박스) | `wp_single_pixel_buffer_v1` | staging | X (없으면 1×1 `wl_shm`) |
| 유휴 시 정지 (로드맵) | `ext_idle_notify_v1` | staging | X |
| 배경 레이어 surface | `zwlr_layer_shell_v1` | de-facto | O |
| 가림 감지(보조) | `zwlr_foreign_toplevel_management_v1` | de-facto | X |
| 가림 감지(최후) | niri IPC | 컴포지터 전용 | X (feature `niri`) |

Vulkan 표시는 `VK_KHR_wayland_surface`로 우리 `wl_surface`에 직접 합니다.
(드라이버 WSI가 내부적으로 `zwp_linux_dmabuf_v1` 등을 사용)

웹 헬퍼는 GTK4가 쓰는 프로토콜(위의 core/stable/staging 대부분) + gtk4-layer-shell을 사용합니다.

## 항목별 설명

### 모니터 식별 — `wl_output` v4
버전 4부터 `name` 이벤트로 커넥터 이름(`DP-1`, `eDP-1`, `HDMI-A-1`)을 받습니다.
예전에 쓰이던 `zxdg_output_manager_v1`은 필요 없습니다. 이 이름을 설정 키로 씁니다.

### 배경 레이어 — `zwlr_layer_shell_v1` (불가피한 확장)
배경화면은 "모든 일반 창 아래에, 출력 전체를 덮는" surface가 필요합니다. `xdg-shell`은
일반 애플리케이션 창용이라 레이어/앵커 개념이 없고, 작성 시점 기준 공식 wayland-protocols에는
이를 대체하는 프로토콜이 없습니다. `wlr-layer-shell`은 niri, sway, Hyprland, river, labwc,
KWin, COSMIC 등이 구현한 사실상 표준입니다. GNOME(Mutter)은 지원하지 않으므로 지원 대상에서 제외합니다.

설정값: layer `background` · anchor 상하좌우 · exclusive zone `-1` · keyboard interactivity `none` ·
namespace `waywall`

### 크기 맞춤과 HiDPI — `wp_viewporter` + `wp_fractional_scale_v1`
- 버퍼 크기와 화면에 표시되는 크기를 분리합니다. 이미지·영상은 **원본 크기 버퍼**를 붙이고
  viewporter의 source(잘라낼 영역)와 destination(표시 크기)만 지정하면 컴포지터가 GPU로 스케일합니다.
  우리 쪽에서 리사이즈할 필요가 없습니다.
- 셰이더/씬은 `wp_fractional_scale_v1`이 알려주는 스케일(예: 1.25)로 물리 픽셀 크기 버퍼를 만들고
  destination을 논리 크기로 둡니다. 없으면 `wl_output.scale`(정수)로 폴백합니다.

### 영상 zero-copy — `zwp_linux_dmabuf_v1`
하드웨어 디코더가 만든 GPU 메모리(dmabuf)를 복사 없이 `wl_buffer`로 감싸 컴포지터에 넘깁니다.
v4의 feedback으로 컴포지터가 받을 수 있는 포맷/모디파이어를 알아내 디코더 출력과 맞춥니다.
컴포지터가 직접 스캔아웃할 수 있으면 GPU 합성조차 생략될 수 있습니다.

### 영상 레이어 — `wl_subcompositor`
이미지·영상은 layer surface의 서브서피스에 붙입니다. 부모 surface는 레터박스 배경(1×1 단색 버퍼 확대)을,
서브서피스는 영상 프레임을 담당해서 영상 프레임마다 부모를 다시 커밋할 필요가 없습니다.

### 마우스 입력 — `wl_pointer`, `wp_cursor_shape_v1`
- layer surface도 일반 surface처럼 enter/motion/button/axis 이벤트를 받습니다. 창이 없는 영역에서만
  이벤트가 오는 것은 Wayland 구조상 정상입니다.
- 포인터가 들어오면 클라이언트가 커서를 지정해야 합니다. `wp_cursor_shape_v1`(staging)로 설정
  `cursor.shape`의 모양을 요청하고, 없으면 `cursor.theme`/`cursor.size`의 xcursor 테마로 커서 surface를
  직접 만듭니다. `hidden`이면 커서 surface를 비웁니다.
- 상호작용 없는 배경은 빈 input region으로 입력을 아예 통과시킵니다.

### 가림 감지

**요구사항:** 창이 배경을 가리면 재생을 멈춰 CPU/GPU/배터리를 아낀다.

#### 1차: `wl_surface.frame` (core)
core 프로토콜은 surface가 어떤 식으로도 보이지 않으면 서버가 frame 콜백 신호를 피해야 한다고 권고합니다.

- 셰이더/씬은 frame 콜백으로만 그리므로, 가려지면 **렌더 루프가 자연히 멈춥니다.**
- 영상/웹은 디코딩·오디오·JS가 계속 돌기 때문에, frame 콜백이 **안 오는 것을 감지**해서 명시적으로 pause합니다.
  영상은 프레임을 붙일 때마다 그 서브서피스에 frame 콜백을 요청합니다(콜백으로 페이싱하지는 않고 감시에만 씀).
- 판정 (출력마다, 재생 중일 때만):
  - 요청한 콜백이 `visibility.frame_timeout_ms` 안에 오지 않으면 `Hidden`
  - 콜백 간격이 `frame_timeout_ms` 이상인 것이 `visibility.slow_frame_count`회 연속이어도 `Hidden`
    (가려진 surface에 저빈도 콜백을 보내는 컴포지터 보정)
  - `Hidden` 동안 콜백이 오면: 직전 콜백과의 간격이 `frame_timeout_ms` 미만이면 해제, 아니면 계속 `Hidden`
- pause 중에도 "다시 보이는지" 알기 위해 **빈 커밋 + frame 콜백 하나만** 대기시켜 둡니다.
  버퍼를 다시 그리지 않으므로 비용은 사실상 0입니다.
- 사용자 `pause`나 `Covered`로 멈춘 동안에는 콜백을 요청하지 않으므로 `Hidden` 판정도 하지 않습니다.
  재개할 때 판정을 새로 시작합니다.
- 셰이더는 `Hidden`이 되면 `iTime`도 멈춥니다.
- **niri 측정 (2026-11-01, M3):** 창들이 덮고 있는 출력(창 사이 간격으로 배경이 조금 보임)의 background layer에도
  주사율대로 frame 콜백을 보냈습니다. 이 경우 1차 신호만으로는 멈추지 않으므로 `pause_when` 정책(보조 신호)이 필요합니다.
  창이 출력 전체를 완전히 덮는 경우(전체화면)의 동작은 아직 확인하지 못했습니다(roadmap.md M5 참고).
- **niri 측정 (2026-11-01, M5):** 전체화면 + 포커스 창은 wlr-foreign-toplevel `state` 이벤트로 즉시 알려져,
  기본 정책(`fullscreen`)으로 같은 순간 정지·재개됩니다.

#### 보조: `zwlr_foreign_toplevel_management_v1` (de-facto)
"전체화면/최대화 창이 있으면 정지" 같은 **정책 기반** 판정용입니다. frame 신호만으로는
"배경이 조금 보이지만 사용자가 창에 집중하고 있다"는 상황을 알 수 없기 때문입니다.

- toplevel마다 `state`(maximized, minimized, activated, fullscreen)와 `output_enter/leave`를 받습니다.
- 판정: 출력에 `fullscreen`(정책에 따라 `maximized` 포함) + `activated` 창이 있으면 그 출력을 `Covered`로 봅니다.

**왜 표준으로 안 되나:** staging의 `ext-foreign-toplevel-list-v1`은 toplevel 식별자/제목/app_id만
제공하고 fullscreen·maximized 상태나 출력 정보가 없습니다. 상태를 다루는 공식 프로토콜이 생기면 그쪽으로 옮깁니다.

**한계:** 워크스페이스 개념이 없습니다. 비활성 워크스페이스의 fullscreen 창은 `activated` 조건으로
대부분 걸러지지만 완벽하지 않습니다.

#### 최후 수단: niri IPC (컴포지터 전용)
`$NIRI_SOCKET`에 `"EventStream"`을 요청해 워크스페이스/창 상태를 직접 받습니다. 위 두 신호로 판정할 수
없는 정책(`pause_when = "any-window"`)에만 쓰며, 설정 `visibility.niri = true`일 때만 연결합니다.

- 판정: 출력의 **활성 워크스페이스에 창이 하나라도**(플로팅 포함) 있으면 그 출력을 `Covered`(source `niri`)로 봅니다.
- 별도 스레드가 소켓을 blocking으로 읽고(이벤트가 없으면 잠듦), 출력별 결과가 바뀔 때만 calloop 채널로 보냅니다.
- 이벤트는 필요한 필드(`WorkspacesChanged`, `WorkspaceActivated`, `WindowsChanged`, `WindowOpenedOrChanged`,
  `WindowClosed`)만 느슨하게 읽어 niri 버전 차이를 견딥니다. 연결이 끊기면 경고를 남기고 이 신호원을 끕니다.
- 별도 크레이트 없이 Unix 소켓 + JSON이라 cargo feature로 나누지 않습니다.
- `any-window`인데 `visibility.niri = false`(또는 niri가 아님)면 시작 시 경고하고 `maximized`처럼 동작합니다.
- **측정 (2026-11-01, niri 26.04):** IPC의 창 정보에는 전체화면 여부가 없어 `fullscreen`/`maximized` 정책은 toplevel 신호원을 씁니다.

#### 신호 합성 규칙
- `Hidden` (frame) — 항상 pause 사유
- `Covered` (toplevel/niri) — 설정 `pause_when` 정책에 해당할 때만 pause 사유
- 어느 신호원이 사유를 걸었는지 `waywall list`에 표시 (디버깅용)

### 유휴 시 정지 — `ext_idle_notify_v1` (로드맵)
일정 시간 입력이 없으면 정지. staging 프로토콜이며 데몬이 직접 바인딩합니다.

## 지원 컴포지터 기대치

| 컴포지터 | layer-shell | linux-dmabuf | fractional-scale | cursor-shape | single-pixel-buffer | wlr-foreign-toplevel |
|---|---|---|---|---|---|---|
| niri | O | O | O (실측) | O | **X (실측)** | O |
| sway | O | O | O | O | O | O |
| Hyprland | O | O | O | O | O | O |
| KWin | O | O | O | O | O | X (자체 프로토콜) |
| GNOME | X | — | — | — | — | X |

설계 시점의 기대치이며, 구현하면서 실제 광고되는 전역 객체(`wayland-info`)로 확인해 갱신합니다.
