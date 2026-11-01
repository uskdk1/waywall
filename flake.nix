{
  description = "waywall — live wallpapers for Wayland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
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
        ];
        buildInputs = with pkgs; [ wayland ];
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
        };

        waywall = rustPlatform.buildRustPackage (
          common
          // {
            postFixup = ''
              wrapProgram $out/bin/waywall \
                --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeLibs}
            '';
            meta = {
              description = "Live wallpaper daemon for Wayland compositors";
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
          ]);
          LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
          # 디버그 빌드의 wgpu가 검증 레이어를 찾도록
          VK_LAYER_PATH = "${pkgs.vulkan-validation-layers}/share/vulkan/explicit_layer.d";
        };

        formatter = pkgs.nixfmt;
      }
    );
}
