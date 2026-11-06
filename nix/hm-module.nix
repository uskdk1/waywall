# home-manager 모듈: 패키지 설치 + systemd user 서비스 (docs/setup.md#home-manager-모듈).
# 서비스 내용은 contrib/systemd/waywall.service와 같다.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.waywall;
in
{
  options.services.waywall = {
    enable = lib.mkEnableOption "waywall live wallpaper daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "inputs.waywall.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "waywall package to install and run.";
    };

    systemdTarget = lib.mkOption {
      type = lib.types.str;
      default = "graphical-session.target";
      description = "systemd user target the daemon starts with and is bound to.";
    };
  };

  # 설정 파일은 만들지 않는다: `set --save`·`volume --save`가 config.toml을 고치므로 스토어 링크면 저장이 실패한다.
  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    systemd.user.services.waywall = {
      Unit = {
        Description = "waywall live wallpaper daemon";
        Documentation = "https://github.com/uskdk1/waywall/blob/main/docs/setup.md";
        PartOf = [ cfg.systemdTarget ];
        After = [ cfg.systemdTarget ];
        Requisite = [ cfg.systemdTarget ];
      };
      Service = {
        ExecStart = "${lib.getExe cfg.package} daemon";
        # 비정상 종료 시에만 다시 시작한다. `waywall quit`으로 끝내면 그대로 둔다.
        Restart = "on-failure";
        RestartSec = 2;
      };
      Install.WantedBy = [ cfg.systemdTarget ];
    };
  };
}
