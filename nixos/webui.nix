# The web UI: haxo-web (crates/haxo-web in the haxo-rs workspace) on port
# 80, over the USB link (http://10.68.0.1) and Wi-Fi (http://haxophone.local).
# Design and API: docs/webui.md.
#
# The USB link needs no login; Wi-Fi needs the password under [web] in the
# settings file, and is refused while there is none.
#
# haxo-web runs as its own user with only CAP_NET_BIND_SERVICE. What it
# needs beyond that, each granted narrowly:
# - haxo001's control and audio sockets: group `haxo` (haxo's unit makes
#   them 0660).
# - The settings file is on the FAT boot partition, where permissions are
#   one mask for every file (kernels, extlinux.conf too), so it never
#   writes it: a root helper does, `haxophone-config serve`, one validated
#   request per connection on a socket only haxo-web may use. It edits the
#   one file, reapplies it, and keeps the web password to itself.
# - iwd's D-Bus policy admits root and wheel only; a policy file here
#   admits haxo-web too. BlueZ's admits everyone.
# - Uploaded soundfonts: the files directory is group haxo-web, setgid
#   (its tmpfiles rule is in gadget.nix, which owns the directory).
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
  settingsSocket = "/run/haxophone-config/settings.sock";

  iwdPolicy = pkgs.writeTextDir "share/dbus-1/system.d/haxo-web-iwd.conf" ''
    <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
     "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
    <busconfig>
      <policy user="haxo-web">
        <allow send_destination="net.connman.iwd"/>
      </policy>
    </busconfig>
  '';
in
{
  users.users.haxo-web = {
    isSystemUser = true;
    group = "haxo-web";
    extraGroups = [ "haxo" ];
  };
  users.groups.haxo-web = { };
  users.groups.haxo = { };

  services.dbus.packages = [ iwdPolicy ];

  # The settings helper: a root process per request, started by the socket.
  systemd.sockets.haxophone-settings = {
    description = "Haxophone settings file, for the web UI";
    wantedBy = [ "sockets.target" ];
    listenStreams = [ settingsSocket ];
    socketConfig = {
      Accept = true;
      SocketUser = "root";
      SocketGroup = "haxo-web";
      SocketMode = "0660";
    };
  };
  systemd.services."haxophone-settings@" = {
    description = "Haxophone settings request";
    # /boot holds the file; the helper fails cleanly without it.
    after = [ "boot.mount" ];
    serviceConfig = {
      ExecStart = lib.concatStringsSep " " [
        "${lib.getExe haxophone-config} serve"
        "--config /boot/config/haxophone.toml"
        "--apply-unit haxophone-config.service"
        "--lock /run/haxophone-config/edit.lock"
      ];
      StandardInput = "socket";
      StandardOutput = "socket";
      StandardError = "journal";
    };
  };

  systemd.services.haxo-web = {
    description = "Haxophone web UI";
    wantedBy = [ "multi-user.target" ];
    after = [
      "network.target"
      "haxo.service"
      "haxophone-settings.socket"
    ];
    wants = [ "haxophone-settings.socket" ];
    serviceConfig = {
      User = "haxo-web";
      Group = "haxo-web";
      Restart = "always";
      RestartSec = 2;
      Environment = "RUST_LOG=info";
      ExecStart = lib.concatStringsSep " " [
        (lib.getExe haxo-web)
        "--listen 0.0.0.0:80"
        "--control-socket ${haxo.controlSocket}"
        "--audio-socket ${haxo.audioSocket}"
        "--settings-socket ${settingsSocket}"
        "--files-dir ${haxo.filesDir}"
        "--builtin-soundfont ${haxo.soundfont}"
        "--usb-interface usb0"
        "--wifi-interface wlan0"
      ];

      AmbientCapabilities = [ "CAP_NET_BIND_SERVICE" ];
      CapabilityBoundingSet = [ "CAP_NET_BIND_SERVICE" ];
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ReadWritePaths = [ haxo.filesDir ];
      ProtectHome = true;
      PrivateTmp = true;
      PrivateDevices = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectKernelLogs = true;
      ProtectControlGroups = true;
      ProtectClock = true;
      ProtectHostname = true;
      ProtectProc = "invisible";
      # getifaddrs uses netlink.
      RestrictAddressFamilies = [
        "AF_UNIX"
        "AF_INET"
        "AF_INET6"
        "AF_NETLINK"
      ];
      RestrictNamespaces = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      MemoryDenyWriteExecute = true;
      SystemCallArchitectures = "native";
      SystemCallFilter = [ "@system-service" ];
      UMask = "0002";
    };
  };

  networking.firewall.allowedTCPPorts = [ 80 ];
}
