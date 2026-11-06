{
  description = "waywall — live wallpapers for Wayland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
    # Live2D 런타임: Cubism Core 호환 오픈소스 재구현 (MIT). build.rs가 C 소스를 컴파일해 정적 링크 (docs/development.md#live2d-purismcore-m9)
    purism-core = {
      url = "github:SakuraMotion/PurismCore/v1.1.0";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      purism-core,
    }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        lib = pkgs.lib;

        # devShell과 패키지 빌드가 같은 툴체인을 쓴다 (docs/development.md)
        toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = toolchain;
          rustc = toolchain;
        };

        nativeBuildInputs = with pkgs; [
          pkg-config
          makeWrapper
          wrapGAppsHook4
        ];
        gst = pkgs.gst_all_1;
        buildInputs = [
          pkgs.wayland
          gst.gstreamer
          gst.gst-plugins-base
          # 웹 헬퍼 (waywall-web)
          pkgs.gtk4
          pkgs.gtk4-layer-shell
          pkgs.webkitgtk_6_0
          pkgs.glib-networking # https
        ];
        # 영상 재생에 쓰는 GStreamer 플러그인 (docs/development.md). 시스템 플러그인은 쓰지 않는다.
        gstPlugins = [
          gst.gstreamer
          gst.gst-plugins-base # playbin3, typefind, videoconvert, appsink
          gst.gst-plugins-good # mp4/matroska 디먹서, pulsesink
          gst.gst-plugins-bad # va 하드웨어 디코더
          gst.gst-libav # 소프트웨어 폴백, GIF
        ];
        gstPluginPath = lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gstPlugins;
        # wgpu가 dlopen하는 Vulkan 로더. GPU 드라이버(ICD)는 시스템 것 (docs/development.md)
        runtimeLibs = with pkgs; [
          wayland
          vulkan-loader
        ];

        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./LICENSE
            ./crates
            ./assets
            ./examples # 예제 셰이더 컴파일 테스트
          ];
        };

        common = {
          pname = "waywall";
          version = (lib.importTOML ./Cargo.toml).workspace.package.version;
          inherit src nativeBuildInputs buildInputs;
          cargoLock.lockFile = ./Cargo.lock;
          env.PURISM_CORE_SRC = "${purism-core}";
        };

        waywall = rustPlatform.buildRustPackage (
          common
          // {
            # 데몬은 GTK를 쓰지 않으므로 GApps 래핑은 헬퍼에만 (docs/development.md#런타임-경로)
            dontWrapGApps = true;
            # PurismCore(MIT)를 데몬에 정적 링크하므로 라이선스 문구를 함께 배포한다.
            # 예제 배경은 share/waywall/examples에 설치한다 (docs/setup.md#예제). Live2D 모델(model/)은 git 밖이라 들어가지 않는다.
            postInstall = ''
              install -Dm644 LICENSE $out/share/licenses/waywall/LICENSE
              install -Dm644 ${purism-core}/LICENSE $out/share/licenses/waywall/PurismCore.LICENSE
              mkdir -p $out/share/waywall
              cp -r examples $out/share/waywall/examples
            '';
            postFixup = ''
              wrapProgram $out/bin/waywall \
                --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeLibs} \
                --set GST_PLUGIN_SYSTEM_PATH_1_0 ${gstPluginPath} \
                --set WAYWALL_WEB_HELPER $out/bin/waywall-web
              wrapGApp $out/bin/waywall-web \
                --set GST_PLUGIN_SYSTEM_PATH_1_0 ${gstPluginPath}
            '';
            meta = {
              description = "Live wallpaper daemon for Wayland compositors";
              homepage = "https://github.com/uskdk1/waywall";
              license = lib.licenses.asl20;
              mainProgram = "waywall";
              platforms = lib.platforms.linux;
            };
          }
        );
      in
      {
        packages = {
          default = waywall;
          inherit waywall;
        };

        checks = {
          inherit waywall;
          fmt = pkgs.runCommand "waywall-fmt" { nativeBuildInputs = [ toolchain ]; } ''
            cd ${src}
            cargo fmt --all --check
            touch $out
          '';
          clippy = rustPlatform.buildRustPackage (
            common
            // {
              pname = "waywall-clippy";
              buildPhase = ''
                runHook preBuild
                cargo clippy --offline --workspace --all-targets -- -D warnings
                runHook postBuild
              '';
              doCheck = false;
              installPhase = "touch $out";
            }
          );
        };

        devShells.default = pkgs.mkShell {
          packages = [
            toolchain
          ]
          ++ nativeBuildInputs
          ++ buildInputs
          ++ (with pkgs; [
            wayland-utils
            cargo-edit
            vulkan-tools
            vulkan-validation-layers
            libva-utils # vainfo
            gst_all_1.gstreamer # gst-inspect-1.0, gst-launch-1.0
          ]);
          LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
          GST_PLUGIN_SYSTEM_PATH_1_0 = gstPluginPath;
          PURISM_CORE_SRC = "${purism-core}";
          # 웹 헬퍼를 cargo run으로 돌릴 때: GTK 스키마, https(GIO 모듈)
          XDG_DATA_DIRS = "${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}";
          GIO_EXTRA_MODULES = "${pkgs.glib-networking}/lib/gio/modules";
          # 디버그 빌드의 wgpu가 검증 레이어를 찾도록
          VK_LAYER_PATH = "${pkgs.vulkan-validation-layers}/share/vulkan/explicit_layer.d";
        };

        formatter = pkgs.nixfmt;
      }
    )
    // {
      # home-manager 모듈 (docs/setup.md#home-manager-모듈)
      homeManagerModules.default = import ./nix/hm-module.nix self;
    };
}
