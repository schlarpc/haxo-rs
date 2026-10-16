# Wi-Fi: a way in that doesn't share the USB gadget link, whose data path
# has stalled under large transfers (docs/haxophone.md). iwd associates,
# networkd runs DHCP, and Avahi publishes haxophone.local.
#
# The networks come from config/haxophone.toml on the FAT boot partition
# (config-file.nix).
{
  config,
  lib,
  pkgs,
  ...
}:
let
  hostName = config.networking.hostName;

  # Avahi renames itself (haxophone-2, -3...) when it hears another host
  # claim its name, and never takes the name back. On this board that
  # happened several times a day, each time right as Wi-Fi came back from
  # an outage; in controlled reconnects (iwd disconnect, networkctl
  # reconfigure) nothing else ever claimed the name, so it looks like the
  # board's own announcement echoed back during a real outage (a mesh
  # roam or an access point restart). Every 30 s, if the name has drifted:
  # when only the board's own addresses answer for it, take it back; when
  # another host answers, log that host and leave the name be (two
  # Haxophones on one network shouldn't fight).
  mdnsName = pkgs.writeShellApplication {
    name = "haxophone-mdns-name";
    runtimeInputs = [
      pkgs.avahi
      pkgs.systemd
      pkgs.iproute2
      pkgs.gnugrep
      pkgs.gawk
      pkgs.coreutils
    ];
    text = ''
      want=${hostName}
      have=$(busctl call org.freedesktop.Avahi / org.freedesktop.Avahi.Server GetHostName | awk -F'"' '{print $2}')
      [ "$have" = "$want" ] && exit 0
      mine=$(ip -o addr show | awk '{split($4, a, "/"); print a[1]}')
      others=$(timeout 5 avahi-resolve -n "$want.local" 2>/dev/null | awk '{print $2}' | grep -vxF "$mine" || true)
      if [ -n "$others" ]; then
        echo "mDNS name is $have.local: $want.local is answered by $others, leaving it"
        exit 0
      fi
      echo "mDNS name drifted to $have.local with nobody else on $want.local; taking it back"
      avahi-set-host-name "$want"
    '';
  };
in
{
  # Without this iwd sits in "connecting" forever on the Zero 2 W's
  # BCM43430/1: the firmware advertises SAE and 4-way-handshake offload, and
  # iwd trips over it. Disable both (verified on the board: connects within
  # seconds with it, never without).
  boot.extraModprobeConfig = ''
    options brcmfmac feature_disable=0x82000
  '';

  networking.wireless.iwd = {
    enable = true;
    # networkd owns addressing on every interface here.
    settings.General.EnableNetworkConfiguration = false;
  };

  systemd.network.networks."20-wlan0" = {
    matchConfig.Name = "wlan0";
    networkConfig = {
      DHCP = "yes";
      IPv6AcceptRA = true;
      MulticastDNS = false;
    };
    linkConfig.RequiredForOnline = "no";
  };

  services.avahi = {
    enable = true;
    nssmdns4 = true;
    publish = {
      enable = true;
      addresses = true;
    };
  };

  systemd.services.haxophone-mdns-name = {
    description = "Take back ${hostName}.local after an mDNS rename";
    after = [ "avahi-daemon.service" ];
    requisite = [ "avahi-daemon.service" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = lib.getExe mdnsName;
    };
  };
  systemd.timers.haxophone-mdns-name = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnBootSec = "1min";
      OnUnitActiveSec = "30s";
      AccuracySec = "5s";
    };
  };
}
