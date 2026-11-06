# waywall

[한국어](README.md) | **English**

A live wallpaper daemon for Wayland compositors. It puts videos, web pages (HTML/JS), shaders, images/GIFs, and scenes
that combine lighting, particles, bloom, and Live2D® characters on your desktop.

This program is not an official Live2D® product. Please do not contact Live2D® official support about this program.

```sh
nix run github:uskdk1/waywall -- daemon &
waywall set ~/Wallpapers/rain.mp4
```

## Features

- **Five wallpaper types**: video · web (WebKitGTK) · shader (Shadertoy GLSL, WGSL) · image/GIF · scene
- **Scenes**: image layers, parallax, particles, lighting (normal maps), bloom · vignette · tone mapping, shader layers,
  sway · mask · look-at-cursor effects, Live2D models ([PurismCore](https://github.com/SakuraMotion/PurismCore))
- **Mouse interaction**: cursor position and clicks are passed to shader, scene, and web wallpapers
- **Auto pause**: stops when windows cover the wallpaper; wallpapers with nothing moving use no GPU
- **Per-monitor wallpapers**, audio volume · mute, CLI and a JSON IPC socket
- **Config-driven**: colors, shapes, and numbers live in `config.toml` and wallpaper files (`project.toml`, `scene.toml`), not in code

## Requirements

Works on compositors that implement `wlr-layer-shell`: niri, sway, Hyprland, river, labwc, KWin, COSMIC, and others.
GNOME (Mutter) is not supported. Development and measurements are mainly done on niri. The GPU must support Vulkan.

## Installation

waywall is distributed as a Nix flake.

```sh
nix profile install github:uskdk1/waywall
```

With home-manager, a single module installs the package and enables autostart (a systemd user service).

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

Other installation methods, niri `spawn-at-startup`, and GPU driver setup on non-NixOS distributions are covered in
[docs/setup.md](docs/setup.md) (Korean).

## Usage

```sh
waywall daemon &                      # config: ~/.config/waywall/config.toml (built-in defaults if missing)
waywall set ~/Wallpapers/forest.mp4   # all monitors
waywall set -o DP-1 shader.glsl       # one monitor
waywall set --save ~/Wallpapers/stars # also write it to the config file
waywall pause                         # play again with resume
waywall volume 30                     # 0–100, --mute / --unmute
waywall list                          # wallpaper, state, and fps per monitor
```

The full command list is in [docs/cli-ipc.md](docs/cli-ipc.md) and the config file in [docs/config.md](docs/config.md).

## Examples

Example wallpapers are installed with the package (3 shaders, 6 scenes, 2 Live2D, 1 web).

```sh
EX=$(dirname "$(readlink -f "$(command -v waywall)")")/../share/waywall/examples
waywall set "$EX/scenes/fireflies"    # a firefly forest drawn toward the cursor
```

See [examples/README.md](examples/README.md) for the list. The Live2D examples do not include models; download them
yourself from [Live2D Sample Data](https://www.live2d.com/en/learn/sample/).

## Making your own

A wallpaper is a single file (`.mp4`, `.glsl`, `.html`, …) or a directory with a `project.toml`. Shader uniforms, scene
layers, and the web wallpaper API are documented in [docs/wallpaper-format.md](docs/wallpaper-format.md).

## Documentation

Developer documentation; most users won't need it. All documents under `docs/` are written in Korean.

| Document | Contents |
|---|---|
| [docs/README.md](docs/README.md) | Goals and table of contents |
| [docs/architecture.md](docs/architecture.md) | Architecture, data flow, performance measurements |
| [docs/protocols.md](docs/protocols.md) | Wayland protocols used and fallbacks |
| [docs/development.md](docs/development.md) | Nix development environment, build and test |
| [docs/roadmap.md](docs/roadmap.md) | Milestones (next: VRM layer) |

## Contributing

For now, contributions other than **critical bug fixes** (crashes, the daemon hanging or wallpapers not showing, security
issues) are generally not accepted. Pull requests for new features or refactoring may not be merged. Bug reports are
welcome as issues.

## License

[Apache License 2.0](LICENSE). PurismCore, statically linked into the daemon, is MIT ([docs/README.md](docs/README.md#라이선스)).

## Trademarks

- Live2D® and related logos are registered trademarks of Live2D Inc.
- This project does not use the official SDK and is not affiliated with or endorsed by Live2D Inc.
