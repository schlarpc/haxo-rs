# The composite USB gadget on the Zero 2 W's OTG port, built through configfs:
#
#   NCM  network (usb0)            2 IN + 1 OUT endpoints
#   ACM  serial console (ttyGS0)   2 IN + 1 OUT
#   MIDI the instrument, for DAWs  1 IN + 1 OUT
#   MTP  file access (umtprd)      2 IN + 1 OUT
#
# 7 IN + 4 OUT, which is exactly the dwc2 core's 7 IN endpoints (verified on
# the board, docs/haxophone.md "USB gadget endpoint budget").
#
# Ordering: MTP is a FunctionFS function, and a gadget with an FFS function
# can only bind to the UDC after the daemon has written its descriptors.
# umtprd signals that with sd_notify (Type=notify), so:
#   usb-gadget.service       builds the gadget, mounts FunctionFS; unbound
#   umtprd.service           writes the MTP descriptors, then READY=1
#   usb-gadget-bind.service  binds the UDC; PartOf umtprd, so restarting
#                            umtprd rebinds
#
# Any rebind re-enumerates the whole device: USB can't change a live
# device's interfaces, and when umtprd reopens ep0 FunctionFS resets the
# function and unbinds the gadget (ffs_closed -> unregister_gadget_item).
# FunctionFS is mounted with no_disconnect=1, so while umtprd is down (a
# crash, a failing restart) the gadget stays bound and only MTP stalls; the
# rest of the link survives until umtprd comes back. None of these units
# restart on a deploy (restartIfChanged = false): a live rebind would cut
# the link deploy-rs is using, so gadget changes take effect at the next
# boot.
#
# If binding fails (setup error, umtprd never ready), usb-gadget-fallback
# tears the configfs gadget down and loads the legacy g_cdc gadget instead,
# so the network link and serial console still come up: this is the only
# way into the board short of the UART or pulling the SD card.
{
  lib,
  pkgs,
  ...
}:
let
  umtprd = pkgs.callPackage ../packages/umtprd.nix { };

  gadget = "/sys/kernel/config/usb_gadget/haxophone";
  ffsMount = "/dev/ffs-mtp";
  mtpRoot = "/var/lib/haxophone/files";

  # Fixed, locally administered MACs for the NCM link, so the host sees the
  # same interface every boot.
  hostMac = "02:68:61:78:6f:01";
  devMac = "02:68:61:78:6f:02";

  umtprdConf = pkgs.writeText "umtprd.conf" ''
    # Keep serving across host disconnects instead of exiting.
    loop_on_disconnect 1
    # Hosts show device, then storage (Dolphin: Haxophone/Files).
    storage "${mtpRoot}" "Files" "rw"
    manufacturer "Haxophone"
    product "Haxophone"
    serial "haxophone"
    interface "MTP"
    # Flush uploads to the SD card before reporting them done: the
    # instrument may be unplugged right after a copy.
    sync_when_close 0x1
    usb_functionfs_mode 0x1
    usb_dev_path   "${ffsMount}/ep0"
    usb_epin_path  "${ffsMount}/ep1"
    usb_epout_path "${ffsMount}/ep2"
    usb_epint_path "${ffsMount}/ep3"
    usb_max_packet_size 0x200
  '';

  setup = pkgs.writeShellScript "usb-gadget-setup" ''
    set -euo pipefail
    g=${gadget}

    if [ -d "$g" ]; then
      echo "gadget already exists" >&2
      exit 0
    fi

    serial=$(tr -d '\0' < /sys/firmware/devicetree/base/serial-number 2>/dev/null || true)
    [ -n "$serial" ] || serial=$(cat /etc/machine-id)

    mkdir "$g"
    cd "$g"
    echo 0x1d6b > idVendor  # Linux Foundation
    echo 0x0104 > idProduct # Multifunction Composite Gadget
    echo 0x0100 > bcdDevice
    echo 0x0200 > bcdUSB
    # Interface Association Descriptors, so hosts (Windows especially)
    # treat each function separately.
    echo 0xEF > bDeviceClass
    echo 0x02 > bDeviceSubClass
    echo 0x01 > bDeviceProtocol

    mkdir strings/0x409
    echo "Haxophone" > strings/0x409/manufacturer
    echo "Haxophone" > strings/0x409/product
    echo "$serial" > strings/0x409/serialnumber

    mkdir configs/c.1
    mkdir configs/c.1/strings/0x409
    echo "NCM + ACM + MIDI + MTP" > configs/c.1/strings/0x409/configuration
    echo 250 > configs/c.1/MaxPower

    mkdir functions/ncm.usb0
    echo ${hostMac} > functions/ncm.usb0/host_addr
    echo ${devMac} > functions/ncm.usb0/dev_addr

    mkdir functions/acm.GS0

    mkdir functions/midi.usb0
    # ALSA card id on this side, and the port name hosts show.
    echo haxophone > functions/midi.usb0/id
    echo 1 > functions/midi.usb0/in_ports
    echo 1 > functions/midi.usb0/out_ports

    mkdir functions/ffs.mtp

    ln -s functions/ncm.usb0 configs/c.1/
    ln -s functions/acm.GS0 configs/c.1/
    ln -s functions/midi.usb0 configs/c.1/
    ln -s functions/ffs.mtp configs/c.1/

    mkdir -p ${ffsMount}
    mount -t functionfs -o no_disconnect=1 mtp ${ffsMount}
  '';

  teardown = pkgs.writeShellScript "usb-gadget-teardown" ''
    set -u
    g=${gadget}
    [ -d "$g" ] || exit 0
    echo "" > "$g/UDC" 2>/dev/null || true
    umount ${ffsMount} 2>/dev/null || true
    cd "$g"
    find configs -maxdepth 2 -type l -delete
    rmdir configs/c.1/strings/0x409 configs/c.1 2>/dev/null || true
    for f in functions/*; do rmdir "$f" 2>/dev/null || true; done
    rmdir strings/0x409 2>/dev/null || true
    cd /
    rmdir "$g"
  '';
in
{
  boot.kernelModules = [
    "libcomposite"
    # Exposes the MIDI function's rawmidi port to the ALSA sequencer, which
    # is where haxo-rs (midir) looks for MIDI outputs.
    "snd_seq_midi"
    # "Midi Through". haxo-rs exits without any sequencer output port, and
    # with several it takes the last, which is the gadget's when the gadget
    # is up (it registers later). In the g_cdc fallback there's no MIDI
    # function, so this keeps haxo-rs running.
    "snd_seq_dummy"
  ];

  systemd.services.usb-gadget = {
    description = "USB gadget (NCM, ACM, MIDI, MTP)";
    wantedBy = [ "multi-user.target" ];
    after = [ "sys-kernel-config.mount" ];
    requires = [ "sys-kernel-config.mount" ];
    path = [ pkgs.util-linux ];
    restartIfChanged = false;
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = setup;
      ExecStop = teardown;
    };
  };

  systemd.services.umtprd = {
    description = "MTP responder for the USB gadget";
    wantedBy = [ "multi-user.target" ];
    after = [ "usb-gadget.service" ];
    requires = [ "usb-gadget.service" ];
    restartIfChanged = false;
    serviceConfig = {
      Type = "notify";
      ExecStart = "${lib.getExe umtprd} -conf ${umtprdConf}";
      Restart = "always";
      RestartSec = 1;
    };
  };

  systemd.services.usb-gadget-bind = {
    description = "Bind the USB gadget to the UDC";
    wantedBy = [ "umtprd.service" ];
    after = [ "umtprd.service" ];
    partOf = [ "umtprd.service" ];
    onFailure = [ "usb-gadget-fallback.service" ];
    restartIfChanged = false;
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = pkgs.writeShellScript "usb-gadget-bind" ''
        set -euo pipefail
        udc=$(ls /sys/class/udc | head -n1)
        echo "$udc" > ${gadget}/UDC
      '';
      ExecStop = pkgs.writeShellScript "usb-gadget-unbind" ''
        echo "" > ${gadget}/UDC || true
      '';
    };
  };

  systemd.services.usb-gadget-fallback = {
    description = "Fall back to the legacy g_cdc gadget";
    conflicts = [
      "usb-gadget.service"
      "umtprd.service"
    ];
    path = [ pkgs.util-linux ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = [
        teardown
        "${pkgs.kmod}/bin/modprobe g_cdc host_addr=${hostMac} dev_addr=${devMac}"
      ];
    };
  };

  # Group haxo-web, setgid: the web UI (webui.nix) uploads soundfonts here.
  systemd.tmpfiles.rules = [ "d ${mtpRoot} 2775 root haxo-web -" ];

  # Login on the USB serial console (autologin is set in configuration.nix).
  systemd.services."serial-getty@ttyGS0" = {
    wantedBy = [ "multi-user.target" ];
    after = [ "usb-gadget-bind.service" ];
  };
}
