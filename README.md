# waywall

**한국어** | [English](README.en.md)

Wayland 컴포지터용 라이브 배경화면 데몬입니다. 동영상, 웹(HTML/JS), 셰이더, 이미지/GIF, 그리고 빛·파티클·블룸·Live2D®
캐릭터를 조합하는 씬을 바탕화면에 띄웁니다.

본 프로그램은 Live2D®의 공식 프로그램이 아닙니다. Live2D® 공식 창구에 이 프로그램에 관한 문의는 하지 말아주세요.

```sh
nix run github:uskdk1/waywall -- daemon &
waywall set ~/Wallpapers/rain.mp4
```

## 기능

- **배경 5종**: 동영상 · 웹(WebKitGTK) · 셰이더(Shadertoy GLSL, WGSL) · 이미지/GIF · 씬
- **씬**: 이미지 레이어, 패럴랙스, 파티클, 조명(노멀맵), 블룸·비네트·톤매핑, 셰이더 레이어, 흔들림·마스크·시선 따라가기 효과,
  Live2D 모델([PurismCore](https://github.com/SakuraMotion/PurismCore))
- **마우스 상호작용**: 커서 위치와 클릭을 셰이더·씬·웹 배경에 전달
- **자동 일시정지**: 창이 배경을 가리면 멈추고, 움직임이 없는 배경은 GPU를 쓰지 않음
- **모니터별 배경**, 오디오 볼륨·음소거, CLI와 JSON IPC 소켓
- **설정 주도**: 색·모양·수치는 코드가 아니라 `config.toml`과 배경 파일(`project.toml`, `scene.toml`)에 둠

## 지원 환경

`wlr-layer-shell`을 구현한 컴포지터에서 동작합니다. niri, sway, Hyprland, river, labwc, KWin, COSMIC 등이 해당하고,
GNOME(Mutter)은 지원하지 않습니다. 주로 niri에서 개발·측정합니다. GPU는 Vulkan을 지원해야 합니다.

## 설치

Nix flake로 배포합니다.

```sh
nix profile install github:uskdk1/waywall
```

home-manager에서는 모듈 하나로 패키지와 자동 시작(systemd user 서비스)을 함께 켤 수 있습니다.

```nix
# flake.nix
inputs.waywall = {
  url = "github:uskdk1/waywall";
  inputs.nixpkgs.follows = "nixpkgs";
};

# home-manager
imports = [ inputs.waywall.homeManagerModules.default ];
services.waywall.enable = true;
```

다른 설치 방법과 niri `spawn-at-startup`, NixOS가 아닌 배포판에서의 GPU 드라이버 설정은 [docs/setup.md](docs/setup.md)에 있습니다.

## 사용법

```sh
waywall daemon &                      # 설정: ~/.config/waywall/config.toml (없으면 내장 기본값)
waywall set ~/Wallpapers/forest.mp4   # 모든 모니터
waywall set -o DP-1 shader.glsl       # 한 모니터만
waywall set --save ~/Wallpapers/stars # 설정 파일에도 기록
waywall pause                         # resume으로 다시 재생
waywall volume 30                     # 0–100, --mute / --unmute
waywall list                          # 모니터별 배경·상태·fps
```

명령 전체는 [docs/cli-ipc.md](docs/cli-ipc.md), 설정 파일은 [docs/config.md](docs/config.md)에 있습니다.

## 예제

패키지에 예제 배경이 함께 설치됩니다(셰이더 3, 씬 6, Live2D 2, 웹 1).

```sh
EX=$(dirname "$(readlink -f "$(command -v waywall)")")/../share/waywall/examples
waywall set "$EX/scenes/fireflies"    # 커서에 끌려오는 반딧불 숲
```

목록은 [examples/README.md](examples/README.md)에 있습니다. Live2D 예제에는 모델이 들어 있지 않으므로,
[Live2D 샘플 데이터](https://www.live2d.com/ko/learn/sample/)에서 직접 받아 넣어야 합니다.

## 직접 만들기

배경은 파일 하나(`.mp4`, `.glsl`, `.html` 등)이거나, `project.toml`이 있는 디렉터리입니다. 셰이더 uniform, 씬 레이어,
웹 배경 API는 [docs/wallpaper-format.md](docs/wallpaper-format.md)에 정리되어 있습니다.

## 문서

일반 사용자는 볼 필요가 거의 없는 개발 문서입니다.

| 문서 | 내용 |
|---|---|
| [docs/README.md](docs/README.md) | 목표와 문서 목차 |
| [docs/architecture.md](docs/architecture.md) | 구조, 데이터 흐름, 성능 측정 |
| [docs/protocols.md](docs/protocols.md) | 사용하는 Wayland 프로토콜과 폴백 |
| [docs/development.md](docs/development.md) | Nix 개발 환경, 빌드·테스트 |
| [docs/roadmap.md](docs/roadmap.md) | 마일스톤 (다음: VRM 레이어) |

## 기여

현재는 **치명적인 버그 수정**(크래시, 데몬이 멈추거나 배경이 표시되지 않는 문제, 보안 문제) 외의 기여는 받지 않는 편입니다.
기능 추가나 리팩터링 PR은 병합하지 않을 수 있으니 양해 부탁드립니다. 버그 제보는 이슈로 환영합니다.

## 라이선스

[Apache License 2.0](LICENSE). 데몬에 정적 링크하는 PurismCore는 MIT입니다([docs/README.md](docs/README.md#라이선스)).

## Trademarks / 상표권 고지
- Live2D® 및 관련 로고는 Live2D Inc.의 등록 상표입니다.
- 본 프로젝트는 공식 SDK를 사용하지 않으며, 주식회사 Live2D와 어떠한 제휴나 후원 관계도 없습니다.
