# Haxophone: a Raspberry Pi Zero 2 W under the Haxophone HAT. Design notes
# and decisions are in docs/haxophone.md.
#
# Reached over the one USB cable to the host, which also powers the board
# and carries the composite gadget (gadget.nix): NCM network, USB audio
# (UAC2), USB MIDI and MTP. The mini UART on GPIO 14/15 is the console, and
# Wi-Fi (wifi.nix) a way in that doesn't share the USB link. Audio comes
# from our own device tree overlay (haxophone-hat.dts) and haxo-rs as a
# service; haxo-bridge copies it to USB audio and Bluetooth (audio-out.nix).
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

  soundfontDir = "${pkgs.soundfont-generaluser-gs}/share/soundfonts";
  soundfont = "${soundfontDir}/GeneralUser-GS.sf2";
in
{
  # Shared with config-file.nix and webui.nix.
  _module.args.haxo = {
    package = haxo-rs;
    inherit soundfont soundfontDir;
    # Uploaded soundfonts: the MTP storage (gadget.nix), so a computer on
    # the USB cable sees them too.
    filesDir = "/var/lib/haxophone/files";
    # The instrument's settings from the settings file ([instrument]),
    # written by haxophone-config at boot.
    envFile = "/run/haxophone-config/haxo.env";
    controlSocket = "/run/haxo/control.sock";
    audioSocket = "/run/haxo/audio.sock";
  };

  imports = [
    ./base.nix
    ./boot.nix
    ./storage.nix
    ./config-file.nix
    ./bluetooth.nix
    ./gadget.nix
    ./wifi.nix
    ./webui.nix
    ./audio-out.nix
  ];

  nixpkgs.hostPlatform = "aarch64-linux";

  # Upstream's service (scripts/systemd/haxo.service), minus its MIDI-only
  # gadget script: gadget.nix builds ours. Root for the GPIO and I2C
  # character devices and realtime priority.
  systemd.services.haxo = {
    description = "Haxophone";
    wantedBy = [ "multi-user.target" ];
    # haxo-rs opens the gadget's MIDI device (hw:CARD=haxophone) once, at
    # startup: let it exist. Rebinding the gadget recreates the device, so
    # restart with it.
    # Its Bluetooth MIDI service is registered with bluetoothd, and is gone
    # when bluetoothd restarts: restart with it too.
    after = [
      "sound.target"
      "usb-gadget-bind.service"
      "bluetooth.service"
      "haxophone-config.service"
    ];
    partOf = [
      "usb-gadget-bind.service"
      "bluetooth.service"
    ];
    startLimitIntervalSec = 0;
    serviceConfig = {
      Type = "idle";
      Restart = "always";
      RestartSec = 1;
      # The defaults; [instrument] in the settings file overrides them
      # (haxophone-config writes the EnvironmentFile), and the web UI
      # changes them live through the control socket.
      Environment = [
        "RUST_LOG=info"
        "HAXO_SOUNDFONT=${soundfont}"
        "HAXO_PROGRAM=66" # GM tenor sax, zero-based
        "HAXO_TRANSPOSE=-14" # tenor
        "HAXO_VOLUME=100"
      ];
      EnvironmentFile = "-/run/haxophone-config/haxo.env";
      RuntimeDirectory = "haxo";
      # Root, but with group haxo and this umask its control socket comes
      # out root:haxo 0660, for haxo-web (webui.nix).
      Group = "haxo";
      UMask = "0007";
      ExecStart = lib.concatStringsSep " " [
        (lib.getExe haxo-rs)
        "--notemap-file ${haxo-rs}/share/haxo/notemap.json"
        "--sf2-file \${HAXO_SOUNDFONT}"
        "--prog-number \${HAXO_PROGRAM}"
        "--transpose \${HAXO_TRANSPOSE}"
        "--volume \${HAXO_VOLUME}"
        "--startup-sf2-file ${soundfont}"
        "--startup-midi-file ${haxo-rs}/share/haxo/Startup_Haxophone.mid"
        "--ble-midi haxophone"
        "--control-socket /run/haxo/control.sock"
        "--audio-socket /run/haxo/audio.sock"
      ];
    };
  };

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
