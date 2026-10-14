# The web UI: haxo-web (crates/haxo-web in the haxo-rs workspace) on port
# 80, over the USB link (http://10.68.0.1) and Wi-Fi (http://haxophone.local).
# Design and API: docs/webui.md.
#
# The USB link needs no login; Wi-Fi needs the password under [web] in the
# settings file, and is refused while there is none.
#
# It runs as root: it edits the settings file on the FAT partition, restarts
# haxophone-config to apply it, and drives iwd and BlueZ over D-Bus, whose
# policies admit root.
{
  lib,
  pkgs,
  inputs,
  haxo,
  ...
}:
let
  haxo-web = pkgs.callPackage ../packages/haxo-web.nix {
    craneLib = inputs.crane.mkLib pkgs;
    src = inputs.haxo-rs;
  };
  haxophone-config = pkgs.callPackage ../packages/haxophone-config { };
in
{
  systemd.services.haxo-web = {
    description = "Haxophone web UI";
    wantedBy = [ "multi-user.target" ];
    after = [
      "network.target"
      "haxo.service"
    ];
    serviceConfig = {
      Restart = "always";
      RestartSec = 2;
      Environment = "RUST_LOG=info";
      ExecStart = lib.concatStringsSep " " [
        (lib.getExe haxo-web)
        "--listen 0.0.0.0:80"
        "--control-socket ${haxo.controlSocket}"
        "--config /boot/config/haxophone.toml"
        "--haxophone-config ${lib.getExe haxophone-config}"
        "--config-unit haxophone-config.service"
        "--files-dir ${haxo.filesDir}"
        "--builtin-soundfont ${haxo.soundfont}"
        "--usb-interface usb0"
        "--wifi-interface wlan0"
      ];
      ProtectHome = true;
      PrivateTmp = true;
    };
  };

  networking.firewall.allowedTCPPorts = [ 80 ];
}
