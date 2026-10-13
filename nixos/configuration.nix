# Haxophone: a Raspberry Pi Zero 2 W under the Haxophone HAT. Design notes
# and decisions are in docs/haxophone.md.
#
# Reached over the one USB cable to the host, which also powers the board
# and carries the composite gadget (gadget.nix): NCM network, ACM serial
# console (ttyGS0), USB MIDI and MTP. The mini UART on GPIO 14/15 is a
# second console, and Wi-Fi (wifi.nix) a way in that doesn't share the USB
# link. Audio comes from our own device tree overlay (haxophone-hat.dts)
# and haxo-rs as a service. Bluetooth and PipeWire are up for audio out
# (bluetooth.nix), but nothing feeds them yet.
{
  lib,
  pkgs,
  inputs,
  ...
}:
let
  haxo-rs = pkgs.callPackage ../packages/haxo-rs {
    craneLib = inputs.crane.mkLib pkgs;
    src = inputs.haxo-rs;
  };

  soundfont = "${pkgs.soundfont-generaluser-gs}/share/soundfonts/GeneralUser-GS.sf2";
in
{
  imports = [
    ./base.nix
    ./boot.nix
    ./storage.nix
    ./config-file.nix
    ./bluetooth.nix
    ./gadget.nix
    ./wifi.nix
  ];

  nixpkgs.hostPlatform = "aarch64-linux";

  # Upstream's service (scripts/systemd/haxo.service), minus its MIDI-only
  # gadget script: gadget.nix builds ours. Root for the GPIO and I2C
  # character devices and realtime priority.
  systemd.services.haxo = {
    description = "Haxophone";
    wantedBy = [ "multi-user.target" ];
    # haxo-rs picks its MIDI port once, at startup: let the gadget's exist.
    # Rebinding the gadget recreates that port, so restart with it.
    after = [
      "sound.target"
      "usb-gadget-bind.service"
    ];
    partOf = [ "usb-gadget-bind.service" ];
    startLimitIntervalSec = 0;
    serviceConfig = {
      Type = "idle";
      Restart = "always";
      RestartSec = 1;
      Environment = "RUST_LOG=info";
      ExecStart = lib.concatStringsSep " " [
        (lib.getExe haxo-rs)
        "--notemap-file ${haxo-rs}/share/haxo/notemap.json"
        "--prog-number 66" # GM tenor sax, zero-based
        "--sf2-file ${soundfont}"
      ];
    };
  };

  # haxo-rs hardcodes these paths (startup jingle and its soundfont, and the
  # shutdown key combo's sudo + halt). To be patched; until then, point them
  # into the store.
  systemd.tmpfiles.rules = [
    "L+ /usr/share/sounds/sf2/FluidR3_GM.sf2 - - - - ${soundfont}"
    "L+ /usr/share/haxo/Startup_Haxophone.mid - - - - ${haxo-rs}/share/haxo/Startup_Haxophone.mid"
    "L+ /usr/bin/sudo - - - - /run/wrappers/bin/sudo"
    "L+ /usr/sbin/halt - - - - ${pkgs.systemd}/bin/halt"
  ];

  networking = {
    hostName = "haxophone";
    useNetworkd = true;
    useDHCP = false;
    wireless.enable = false;
    firewall.allowedUDPPorts = [ 67 ];
  };

  # The gadget link: hand the host an address with no router and no DNS, so
  # it never routes anything else through the saxophone.
  systemd.network.networks."10-usb0" = {
    matchConfig.Name = "usb0";
    address = [ "10.68.0.1/24" ];
    networkConfig = {
      DHCPServer = true;
      LinkLocalAddressing = "yes";
      IPv6AcceptRA = false;
    };
    dhcpServerConfig = {
      PoolOffset = 10;
      PoolSize = 10;
      EmitRouter = false;
      EmitDNS = false;
    };
    linkConfig.RequiredForOnline = "no";
  };

  # Login on the serial consoles. Anyone with the cable already has the SD
  # card.
  services.getty.autologinUser = "haxophone";

  services.openssh.enable = true;

  zramSwap.enable = true;

  # The board hard-hung once (2026-10-13) with nothing in the journal: no
  # OOM, no under-voltage, no MMC errors. The BCM2835 watchdog reboots it
  # instead of leaving it wedged. The timer tops out at about 15 s.
  systemd.settings.Manager = {
    RuntimeWatchdogSec = "15s";
    RebootWatchdogSec = "30s";
  };

  environment.systemPackages = with pkgs; [
    alsa-utils
    i2c-tools
    libgpiod
    usbutils
  ];

  system.stateVersion = "26.05";
}
