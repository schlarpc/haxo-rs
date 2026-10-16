# Bluetooth, and PipeWire for Bluetooth audio out: the first step of the
# Outputs plan in docs/haxophone.md, proving the chain (codec, receiver,
# latency) before haxo-rs grows its own bridge. Nothing feeds it yet; the
# bench test is fluidsynth on PipeWire's JACK layer (see the doc).
#
# The controller is on the PL011 (the console is on the mini UART for this
# reason), attached through the mainline device tree's serdev node.
{
  pkgs,
  inputs,
  ...
}:
let
  rpiWirelessFirmware =
    pkgs.callPackage "${inputs.nixos-hardware}/raspberry-pi/common/raspberry-pi-wireless-firmware.nix"
      { };
in
{
  # The controller's patch RAM file, which linux-firmware doesn't carry; it
  # runs on its ROM firmware without one. Zero 2 W boards have one of two
  # chip revisions (ours is 43430A1), and the files are tiny, so both. Only
  # these: the Wi-Fi blobs stay linux-firmware's (boot.nix).
  hardware.firmware = [
    (pkgs.runCommand "bcm43430-bt-firmware" { } ''
      install -Dm644 -t $out/lib/firmware/brcm \
        ${rpiWirelessFirmware}/lib/firmware/brcm/BCM43430A1.hcd \
        ${rpiWirelessFirmware}/lib/firmware/brcm/BCM43430B0.hcd
    '')
  ];

  hardware.bluetooth = {
    enable = true;
    powerOnBoot = true;
    # haxo-rs serves Bluetooth MIDI itself (--ble-midi). BlueZ's MIDI plugin
    # is a client: it connects to BLE MIDI services on the other side, and
    # a peer's (bagel's PipeWire) that wants encryption made it start
    # pairing, which the board refuses, so the link dropped.
    disabledPlugins = [ "midi" ];
  };

  security.rtkit.enable = true;

  services.pipewire = {
    enable = true;
    # For the bench test: `pw-jack fluidsynth -a jack ...`.
    jack.enable = true;
    wireplumber.extraConfig."50-haxophone" = {
      "wireplumber.profiles".main = {
        # PipeWire must never open the I2S card: haxo-rs drives
        # hw:MAX98357A directly, and only one of them can hold it.
        "monitor.alsa" = "disabled";
        # The ALSA sequencer bridge adds seq ports, which changes which
        # port haxo-rs picks for MIDI out (it takes the last one).
        "monitor.alsa-midi" = "disabled";
        # PipeWire's BLE MIDI server would publish a second MIDI service
        # next to haxo-rs's.
        "monitor.bluez-midi" = "disabled";
        # Headless: there is never an active seat, and with seat
        # monitoring wireplumber turns Bluetooth off for inactive ones.
        "monitor.bluez.seat-monitoring" = "disabled";
      };
      # The board only sends audio. With every role on offer, a computer
      # that pairs (bagel did) can pick it as a speaker and stream to it
      # instead (a bluez_input source appeared, no sink).
      "monitor.bluez.properties" = {
        "bluez5.roles" = [ "a2dp_source" ];
      };
    };
  };

  # PipeWire runs in the haxophone user's session. Linger starts that
  # session at boot with nobody logged in, and wireplumber starts with it
  # rather than on a client's first connection, so Bluetooth devices
  # connect and reconnect on their own. Pairings live in /var/lib/bluetooth,
  # wireplumber's state in ~/.local/state; both persist (storage.nix).
  users.users.haxophone.linger = true;
  systemd.user.services.wireplumber.wantedBy = [ "default.target" ];

  environment.systemPackages = [ pkgs.fluidsynth ];
}
