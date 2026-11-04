# 개발 환경

waywall의 개발·빌드 환경은 **Nix flake 하나로 완결**됩니다. 시스템에 Rust, cargo,
pkg-config, Vulkan SDK, GStreamer, GTK, WebKit 등을 따로 설치할 필요가 없고, 설치되어 있어도
사용하지 않습니다. 필요한 것은 Nix(flakes 활성화)와 git뿐입니다.

## 재현성 보장 방식

| 대상 | 고정 방법 |
|---|---|
| 시스템 라이브러리 (Vulkan loader, GStreamer, GTK4, WebKitGTK, …) | `flake.lock`의 nixpkgs 커밋 |
| Rust 컴파일러/cargo/clippy/rustfmt | `rust-toolchain.toml`의 정확한 버전 + `flake.lock`의 rust-overlay 커밋 |
| Rust 크레이트 | `Cargo.lock` (`nix build`는 이를 그대로 사용) |

- devShell과 `nix build`가 **같은 툴체인 derivation**을 씁니다
  (`rust-bin.fromRustupToolchainFile ./rust-toolchain.toml` → `makeRustPlatform`).
- `rust-toolchain.toml`은 rustup 사용자에게도 같은 버전을 강제하지만, 공식 경로는 Nix입니다.

## 준비

### Nix 설치 (Nix가 없는 시스템)
```sh
sh <(curl -L https://nixos.org/nix/install) --daemon
```
flakes 활성화 (`~/.config/nix/nix.conf` 또는 `/etc/nix/nix.conf`):
```
experimental-features = nix-command flakes
```

### 소스 받기
```sh
git clone <repo> waywall && cd waywall
```

> **주의:** flake는 git에 추적된 파일만 봅니다. 새 파일을 만들었으면 커밋 전이라도
> `git add`해야 `nix build` / `nix develop`에서 보입니다.

## 사용법

```sh
nix develop                       # 개발 셸 (툴체인 + 라이브러리 + 도구)
cargo build                       # 워크스페이스 전체
cargo run -p waywall -- daemon    # 데몬 실행 (웹 헬퍼는 target/ 에서 자동으로 찾음)
cargo run -p waywall -- set examples/plasma.glsl

nix build                         # 릴리스 패키지 → ./result/bin/{waywall,waywall-web}
nix run . -- daemon
nix flake check                   # fmt + clippy(-D warnings) + test
nix develop -c cargo test
```

direnv 사용자는 `.envrc`에 `use flake`를 두면 디렉터리 진입 시 자동으로 셸이 로드됩니다.


## flake 구성

```
inputs
├─ nixpkgs        (nixos-unstable, flake.lock으로 커밋 고정)
├─ rust-overlay   (oxalica, nixpkgs follows)
└─ flake-utils

outputs (x86_64-linux, aarch64-linux)
├─ packages.default / packages.waywall   데몬 + 웹 헬퍼
├─ packages.waywall-full                 + features "niri", "shaderc"
├─ devShells.default
├─ checks.{fmt, clippy, test}
└─ formatter                             nixfmt
```

### 빌드 입력

| 구분 | 패키지 | 용도 |
|---|---|---|
| native | 툴체인, `pkg-config`, `makeWrapper`, `wrapGAppsHook4` | 빌드 도구, 런타임 경로 래핑 |
| 데몬 | `wayland` | libwayland-client |
| 데몬 | `vulkan-loader` | wgpu가 런타임에 `dlopen`하는 libvulkan |
| 데몬 | `gst_all_1.gstreamer`, `gst-plugins-base`, `gst-plugins-good`, `gst-plugins-bad`(va 디코더), `gst-libav`(소프트웨어 폴백, GIF) | 영상 |
| 데몬 | `libva` | VA-API |
| 웹 헬퍼 | `gtk4`, `gtk4-layer-shell`, `webkitgtk_6_0`, `glib` | 웹 배경 |
| 선택 | `shaderc` | feature `shaderc` |

### 런타임 경로
- 데몬: wgpu는 libvulkan을 `dlopen`하므로 `makeWrapper`로 `LD_LIBRARY_PATH`에 `vulkan-loader`를,
  `GST_PLUGIN_SYSTEM_PATH_1_0`에 GStreamer 플러그인을 넣습니다.
- 웹 헬퍼: `wrapGAppsHook4`가 GSettings 스키마, GIO 모듈, GStreamer 플러그인 경로를 넣습니다.
- 데몬은 웹 헬퍼를 `WAYWALL_WEB_HELPER` 환경변수 → 같은 디렉터리의 `waywall-web` 순으로 찾습니다.
  패키지에서는 래퍼가 절대 경로를 넣습니다.
- devShell은 같은 변수들을 export해서 `cargo run`이 그대로 동작합니다.

### 개발 셸 도구
`rust-analyzer`, `vulkan-tools`(`vulkaninfo`), `vulkan-validation-layers`(디버그 빌드에서 자동 활성),
`wayland-utils`(`wayland-info` — 컴포지터가 광고하는 프로토콜 확인), `libva-utils`(`vainfo`),
`gst_all_1.gstreamer`의 `gst-inspect-1.0`, `gst-launch-1.0`, `renderdoc`(GPU 프레임 디버깅)

## NixOS가 아닌 배포판에서 실행할 때 (GPU 드라이버)

Nix로 빌드한 프로그램은 시스템의 Vulkan ICD/VA-API 드라이버(`/usr/lib/...`)를 찾지 못합니다.
**빌드는 문제없지만 실행 시** GPU 초기화나 하드웨어 디코딩이 실패할 수 있습니다.

- NixOS: `hardware.graphics.enable = true;` (VA-API는 GPU에 맞는 드라이버를 `hardware.graphics.extraPackages`에 추가)
- 그 외: [nixGL](https://github.com/nix-community/nixGL)로 감싸서 실행
  ```sh
  nix run --impure github:nix-community/nixGL#nixVulkanIntel -- ./result/bin/waywall daemon
  ```

## Live2D Cubism Core (설계, M9)

[M9b](roadmap.md#m9--live2d-레이어-설계)의 방침입니다. waywall은 Cubism Core C ABI 하나만 씁니다.

- **기본: PurismCore.** [SakuraMotion/PurismCore](https://github.com/SakuraMotion/PurismCore)(MIT, C99)는 Cubism Core를 API·ABI 그대로 대신하는
  오픈소스 재구현입니다. flake 입력(태그 커밋 고정)으로 받아 Nix에서 빌드하고 데몬에 링크하므로 사용자가 따로 받을 파일이 없습니다.
  ABI는 v6(Core 6.0, Purism 기본값)을 씁니다.
- **선택: 공식 Cubism Core.** 독점 라이선스(Live2D Proprietary Software License)이고 Live2D 사이트에서 약관에 동의해야 받을 수 있어
  **저장소·flake 입력·Nix 패키지에 넣지 않습니다.** 사용자가 config [`[live2d] core`](config.md#live2d-설계-m9)에 경로를 적으면 데몬이 그 라이브러리를
  실행 중에 `dlopen`해 PurismCore 대신 씁니다(같은 ABI라 호출 코드는 하나).
- `nix build`와 `nix flake check`는 공식 Core 없이 통과해야 합니다.
- 공식 Core를 쓸 때의 배포 라이선스: 개인·소규모 사업자(연 매출 1천만 엔 미만)는 면제지만 "확장 가능한 앱"은 예외입니다.
  waywall은 공식 Core를 배포하지 않고 사용자가 받은 파일을 읽기만 합니다.
- mocari(순수 Rust)는 제작자가 행방불명되어 취소했습니다([roadmap.md](roadmap.md#m9--live2d-레이어-설계)).

## Spine 런타임 (설계, M11)

[M11](roadmap.md#m11--spine-레이어-설계)의 방침입니다.

- `spine-c`(4.2)는 GitHub에 소스가 공개되어 있으므로 flake 입력(커밋 고정)으로 받아 Nix에서 빌드합니다. Cubism Core와 달리
  사용자가 따로 받을 파일이 없습니다.
- 라이선스는 Spine Runtimes License(소스 공개, 오픈소스 아님)입니다. 런타임을 제품에 넣는 시점에 넣는 사람이 유효한 Spine Editor
  라이선스를 가져야 하고(체험판 불가), 배포물에 Spine Runtimes License를 함께 넣어야 합니다.
- 그래서 cargo feature `spine`으로 분리합니다. 기본 패키지에 넣을지, `packages.waywall-full`처럼 별도 패키지로 둘지는 M11에서 정합니다.
  feature가 꺼진 빌드는 `nix build`·`nix flake check`가 Spine 소스 없이 통과해야 합니다.

## 툴체인/의존성 업데이트

```sh
# Rust 버전 변경: rust-toolchain.toml의 channel 수정 후
nix flake update rust-overlay   # 새 버전이 overlay에 없을 때만

nix flake update nixpkgs        # 시스템 라이브러리
nix develop -c cargo update     # 크레이트
```

업데이트 후 `nix flake check`와 실제 실행을 확인하고 `flake.lock` / `Cargo.lock`을 함께 커밋합니다.

## 디렉터리 구조

```
waywall/
├─ flake.nix, flake.lock
├─ rust-toolchain.toml
├─ Cargo.toml, Cargo.lock        # 워크스페이스
├─ crates/
│  ├─ waywall/                   # 데몬 + CLI
│  ├─ waywall-web/               # 웹 헬퍼
│  └─ waywall-proto/             # 공유 타입
├─ assets/                       # defaults.toml, 내장 셰이더, 스프라이트 (데이터)
├─ contrib/                      # 자동 시작 예제: systemd user 서비스, niri 설정 조각 (docs/setup.md)
├─ docs/
└─ examples/                      # 예제 배경 (examples/README.md). 전부 테스트에서 열어 봄
```

모듈 구성은 [architecture.md](architecture.md#데몬-모듈) 참고.

## 테스트 방침

- 단위 테스트 (`cargo test`, 디스플레이·GPU 불필요)
  - **defaults 완전성:** `assets/defaults.toml`이 설정 스키마의 모든 키를 빠짐없이 갖는지
    (설정 타입은 `deny_unknown_fields` + 코드 내 기본값 없음 → 누락 시 역직렬화 실패)
  - **하드코딩 금지 검사:** clippy 설정과 리뷰 기준 — 렌더/외형 관련 모듈에 색·크기·시간 리터럴 금지,
    모든 값은 병합된 설정 구조체에서 읽음
  - config 파싱/병합, 배경 타입 판별, `project.toml`·`scene.toml` 검증, `"$속성"` 치환
  - IPC·헬퍼 메시지 직렬화
  - 가시성 사유 합성 로직, niri 이벤트 → 판정 (JSON 픽스처)
  - Shadertoy 프리앰블 래핑 → naga 파싱·검증 (예제 셰이더 전부)
  - 씬 셰이더(WGSL) naga 검증
- 수동 통합 테스트 (실제 Wayland 세션): [roadmap.md](roadmap.md)의 각 단계 완료 기준

## 로그

`RUST_LOG=waywall=debug waywall daemon` — `tracing` 기반. 가시성 판정 변화, 백엔드 교체,
렌더 경로 선택(dmabuf/shm 폴백), 프레임 타이밍, IPC 요청을 기록합니다.
