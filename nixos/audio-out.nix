# The instrument's sound beyond the jack: haxo-bridge (crates/haxo-bridge in
# the haxo-rs workspace) reads haxo001's audio tap and plays it into a
# device on another clock, steering a small buffer against the drift.
#
# - USB audio: the gadget's UAC2 function (gadget.nix), a sound card the
#   host records from. Clocked by the host.
# - Bluetooth: PipeWire's ALSA plugin, in the haxophone user's PipeWire
#   session (bluetooth.nix), which plays to the connected speaker.
#   Clocked by the Bluetooth link.
#
# Each bridge is a listener on the tap like the web UI: it can drop its
# own audio, never delay the in-ear path.
{
  lib,
  pkgs,
  inputs,
  haxo,
  ...
}:
let
  haxo-bridge = pkgs.callPackage ../packages/haxo-bridge.nix {
    craneLib = inputs.crane.mkLib pkgs;
    src = inputs.haxo-rs;
  };
  bridge =
    device: extra:
    lib.concatStringsSep " " (
      [
        (lib.getExe haxo-bridge)
        "--tap ${haxo.audioSocket}"
        "--device ${device}"
        "--channels 2"
      ]
      ++ extra
    );
in
{
  users.users.haxo-bridge = {
    isSystemUser = true;
    group = "haxo-bridge";
    extraGroups = [
      "haxo"
      "audio"
    ];
  };
  users.groups.haxo-bridge = { };

  # USB audio. The gadget's card goes and comes back with the gadget, and
  # the bridge reopens it by itself.
  systemd.services.haxo-usb-audio = {
    description = "Haxophone sound to USB audio";
    wantedBy = [ "multi-user.target" ];
    after = [
      "haxo.service"
      "usb-gadget-bind.service"
    ];
    serviceConfig = {
      User = "haxo-bridge";
      Group = "haxo-bridge";
      ExecStart = bridge "hw:CARD=UAC2Gadget" [ ];
      Restart = "always";
      RestartSec = 2;
      Environment = "RUST_LOG=info";
      # Above everything ordinary, so load can't stall it into an
      # underrun; below haxo's main loop (80) and fluidsynth's audio
      # thread (99), so it can never delay the instrument.
      CPUSchedulingPolicy = "fifo";
      CPUSchedulingPriority = 10;
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
    };
  };

  # Bluetooth: PipeWire's ALSA plugin ("pipewire") for the bridge to play
  # into, as the user PipeWire runs as. WirePlumber links it to the
  # connected speaker; with none, the stream waits.
  services.pipewire.alsa.enable = true;
  users.users.haxophone.extraGroups = [ "haxo" ];
  systemd.user.services.haxo-bt-audio = {
    description = "Haxophone sound to Bluetooth (PipeWire)";
    wantedBy = [ "default.target" ];
    after = [
      "pipewire.service"
      "wireplumber.service"
    ];
    unitConfig.ConditionUser = "haxophone";
    serviceConfig = {
      ExecStart = bridge "pipewire" [ ];
      Restart = "always";
      RestartSec = 2;
      Environment = [
        "RUST_LOG=info"
        # PipeWire's own buffering for this stream: 256 frames.
        "PIPEWIRE_LATENCY=256/48000"
      ];
    };
  };
}
