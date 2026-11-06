# 설치와 자동 시작

## 설치

waywall은 Nix flake로 빌드합니다([development.md](development.md)). 패키지에는 `bin/waywall`(데몬 + CLI),
`bin/waywall-web`(웹 배경 헬퍼), `share/waywall/examples/`(예제 배경)가 들어 있습니다. 래퍼가 필요한 라이브러리 경로를
넣어 두므로 그대로 실행하면 됩니다.

```sh
nix run github:uskdk1/waywall -- daemon       # 설치하지 않고 실행
nix profile install github:uskdk1/waywall     # ~/.nix-profile/bin/waywall
```

NixOS나 home-manager 설정에서는 flake 입력으로 넣습니다.

```nix
# flake.nix
inputs.waywall = {
  url = "github:uskdk1/waywall";
  inputs.nixpkgs.follows = "nixpkgs";
};

# home-manager: 패키지만
home.packages = [ inputs.waywall.packages.${pkgs.stdenv.hostPlatform.system}.default ];

# home-manager: 패키지 + 자동 시작 (아래 "자동 시작" 참고)
imports = [ inputs.waywall.homeManagerModules.default ];
services.waywall.enable = true;
```

`inputs.nixpkgs.follows`를 쓰면 시스템과 같은 nixpkgs로 빌드합니다. 빼면 waywall의 `flake.lock`에 고정된 nixpkgs로
빌드합니다(검증된 조합이지만 라이브러리를 따로 내려받음).

NixOS가 아닌 배포판에서는 GPU 드라이버 경로에 주의하세요([development.md](development.md#nixos가-아닌-배포판에서-실행할-때-gpu-드라이버)).

### 예제

설치된 예제는 패키지의 `share/waywall/examples/`에 있습니다. 설치 방법과 상관없이 아래처럼 찾을 수 있습니다.

```sh
EX=$(dirname "$(readlink -f "$(command -v waywall)")")/../share/waywall/examples
waywall set "$EX/scenes/fireflies"
```

목록과 설명은 [examples/README.md](../examples/README.md)에 있습니다. Nix 스토어는 읽기 전용이므로, 고쳐 쓰거나
Live2D 모델을 넣으려면 먼저 복사합니다.

```sh
cp -r "$EX/live2d/miku" ~/Wallpapers/ && chmod -R u+w ~/Wallpapers/miku
```

## 데몬 실행

```sh
waywall daemon &          # 설정: $XDG_CONFIG_HOME/waywall/config.toml (없으면 내장 기본값)
waywall set ~/Wallpapers/forest
waywall list
```

데몬은 Wayland 세션 안에서 실행해야 합니다(`WAYLAND_DISPLAY` 필요). 같은 소켓으로 두 번째 데몬을 띄우면
"already running"으로 끝납니다. 설정 파일은 [config.md](config.md), 명령은 [cli-ipc.md](cli-ipc.md)에 있습니다.

## 자동 시작

아래 중 **하나만** 씁니다. 둘 이상 쓰면 두 번째 데몬은 "already running"으로 끝납니다.

### home-manager 모듈

flake의 `homeManagerModules.default`가 패키지 설치와 systemd user 서비스를 한 번에 선언합니다.

```nix
imports = [ inputs.waywall.homeManagerModules.default ];
services.waywall.enable = true;
```

| 옵션 | 기본값 | 설명 |
|---|---|---|
| `services.waywall.enable` | `false` | 패키지를 `home.packages`에 넣고 `waywall.service`를 만듦 |
| `services.waywall.package` | 이 flake의 패키지 | 다른 빌드를 쓸 때 |
| `services.waywall.systemdTarget` | `"graphical-session.target"` | 서비스를 묶을 target |

설정 파일(`~/.config/waywall/config.toml`)은 모듈이 만들지 않습니다. `waywall set --save`·`volume --save`가 이 파일을
고치므로, 읽기 전용 스토어 링크로 두면 저장이 실패합니다. 직접 쓰거나 `--save`로 만드세요([config.md](config.md)).
아래 세션 환경 조건(`WAYLAND_DISPLAY`)은 모듈에도 똑같이 적용됩니다.

### systemd user 서비스 (직접 설치)

`contrib/systemd/waywall.service` — 그래픽 세션(`graphical-session.target`)과 함께 시작하고, 비정상 종료 시 다시 시작합니다.
home-manager 모듈이 만드는 서비스와 같은 내용이며, 모듈을 쓰지 않을 때 복사해서 씁니다.

```sh
mkdir -p ~/.config/systemd/user
cp contrib/systemd/waywall.service ~/.config/systemd/user/
# ExecStart는 nix profile 기준(~/.nix-profile/bin/waywall). 다르게 설치했으면 고친 뒤
systemctl --user daemon-reload
systemctl --user enable --now waywall.service
journalctl --user -u waywall -f      # 로그
```

- 컴포지터가 세션을 시작할 때 `WAYLAND_DISPLAY`를 systemd user 환경에 넣어야 합니다. niri는 `niri-session`으로 시작하면
  자동으로 넣고 `graphical-session.target`을 올립니다. 다른 컴포지터는
  `systemctl --user import-environment WAYLAND_DISPLAY`를 세션 시작 스크립트에 넣으세요.
- `waywall quit`으로 끝내면 다시 시작하지 않습니다(`Restart=on-failure`).

### niri `spawn-at-startup`

`contrib/niri/config.kdl` — `~/.config/niri/config.kdl`에 넣는 조각입니다.

```kdl
spawn-at-startup "waywall" "daemon"
```

같은 파일에 단축키(일시정지·재개·배경 바꾸기) 예와, 오버뷰에서 배경을 보이게 하는 `layer-rule`
(`namespace="^waywall$"`, 설정 `[surface] namespace`와 맞춤) 예가 있습니다.

## 웹 배경

웹 배경은 `waywall-web` 헬퍼가 띄웁니다. 데몬이 같은 디렉터리(또는 `WAYWALL_WEB_HELPER`)에서 찾으므로 따로 설정할 것은 없고,
웹 배경을 쓰지 않으면 실행되지도 않습니다. 웹 배경 하나에 WebKit 프로세스 포함 약 0.5 GB의 메모리를 씁니다.
