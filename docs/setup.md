# 설치와 자동 시작

## 설치

waywall은 Nix flake로 빌드합니다([development.md](development.md)). 결과물은 `bin/waywall`(데몬 + CLI)과
`bin/waywall-web`(웹 배경 헬퍼) 두 개이며, 래퍼가 필요한 라이브러리 경로를 넣어 두므로 그대로 실행하면 됩니다.

```sh
nix build                      # ./result/bin/waywall
nix profile install .          # ~/.nix-profile/bin/waywall
```

NixOS나 home-manager 설정에서는 flake 입력으로 넣습니다.

```nix
# flake.nix
inputs.waywall.url = "path:/home/me/waywall";   # 또는 git URL

# home-manager
home.packages = [ inputs.waywall.packages.${pkgs.system}.default ];
```

NixOS가 아닌 배포판에서는 GPU 드라이버 경로에 주의하세요([development.md](development.md#nixos가-아닌-배포판에서-실행할-때-gpu-드라이버)).

## 데몬 실행

```sh
waywall daemon &          # 설정: $XDG_CONFIG_HOME/waywall/config.toml (없으면 내장 기본값)
waywall set ~/Wallpapers/forest
waywall list
```

데몬은 Wayland 세션 안에서 실행해야 합니다(`WAYLAND_DISPLAY` 필요). 같은 소켓으로 두 번째 데몬을 띄우면
"already running"으로 끝납니다. 설정 파일은 [config.md](config.md), 명령은 [cli-ipc.md](cli-ipc.md)에 있습니다.

## 자동 시작

아래 둘 중 **하나만** 씁니다. 예제 파일은 저장소의 `contrib/`에 있으며, 직접 복사해서 설치합니다.

### systemd user 서비스

`contrib/systemd/waywall.service` — 그래픽 세션(`graphical-session.target`)과 함께 시작하고, 비정상 종료 시 다시 시작합니다.

```sh
mkdir -p ~/.config/systemd/user
cp contrib/systemd/waywall.service ~/.config/systemd/user/
# ExecStart 경로를 설치 방법에 맞게 고친 뒤
systemctl --user daemon-reload
systemctl --user enable --now waywall.service
journalctl --user -u waywall -f      # 로그
```

- 컴포지터가 세션을 시작할 때 `WAYLAND_DISPLAY`를 systemd user 환경에 넣어야 합니다. niri는 `niri-session`으로 시작하면
  자동으로 넣고 `graphical-session.target`을 올립니다. 다른 컴포지터는
  `systemctl --user import-environment WAYLAND_DISPLAY`를 세션 시작 스크립트에 넣으세요.
- `waywall quit`으로 끝내면 다시 시작하지 않습니다(`Restart=on-failure`).
- home-manager에서는 같은 내용을 `systemd.user.services.waywall`로 선언할 수 있습니다.

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
