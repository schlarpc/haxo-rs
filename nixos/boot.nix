# Boot chain: Pi firmware -> U-Boot -> extlinux.conf -> mainline kernel, all
# on the FAT partition at /boot (disko.nix). nixos-hardware's Raspberry Pi
# firmware module stages the GPU firmware, U-Boot and config.txt there on
# every switch; generic-extlinux-compatible writes the generations.
#
# Mainline kernel from cache.nixos.org, not the rpi kernel
# (docs/haxophone.md "Kernel: mainline").
{
  lib,
  pkgs,
  inputs,
  ...
}:
{
  imports = [
    "${inputs.nixos-hardware}/raspberry-pi/common/config-txt.nix"
    "${inputs.nixos-hardware}/raspberry-pi/common/firmware.nix"
  ];

  boot = {
    loader = {
      grub.enable = false;
      generic-extlinux-compatible = {
        enable = true;
        # Sized to the 512 MB firmware partition, ~50 MB per generation.
        configurationLimit = 8;
        # U-Boot loads the generation's own device tree (FDTDIR), which
        # carries our HAT overlay from hardware.deviceTree. The firmware
        # module defaults this off for config.txt overlays, which we don't
        # use.
        useGenerationDeviceTree = true;
      };
    };

    initrd.availableKernelModules = [ "mmc_block" ];

    # The mini UART is the GPIO 14/15 console on the Zero 2 W (the PL011
    # belongs to Bluetooth).
    kernelParams = [ "console=ttyS1,115200" ];

    # No display: the vc4 KMS driver only holds CMA, and snd_bcm2835 is the
    # VideoCore's HDMI audio.
    blacklistedKernelModules = [
      "vc4"
      "v3d"
      "snd_bcm2835"
    ];
  };

  hardware.deviceTree = {
    filter = "bcm2837-rpi-zero-2-w.dtb";
    overlays = [
      {
        name = "haxophone-hat";
        dtsFile = ./haxophone-hat.dts;
      }
    ];
  };

  # Only the Broadcom/Cypress Wi-Fi firmware (brcmfmac, BCM43430/1), not all
  # of linux-firmware, which was 782 MB of a 2.7 GB closure. Same blobs the
  # bootstrap image verified Wi-Fi with. linux-firmware has no Bluetooth
  # patch file (BCM43430A1.hcd) for this chip, so this drops nothing there.
  hardware.firmware = [
    (pkgs.runCommand "brcm-firmware" { } ''
      mkdir -p $out/lib/firmware
      cp -a ${pkgs.linux-firmware}/lib/firmware/{brcm,cypress} $out/lib/firmware/
    '')
  ];

  hardware.raspberry-pi = {
    firmware = {
      enable = true;
      # extlinux.conf and the kernels share the FAT partition.
      path = "/boot";
      uboot = {
        enable = true;
        # The rpi_3 build the bootstrap image booted with.
        package = pkgs.ubootRaspberryPi3_64bit;
      };
    };

    # Written out in full rather than on top of nixos-hardware's Raspberry Pi
    # OS defaults (camera and display probing, vc4-kms-v3d, HDMI audio),
    # none of which applies here. U-Boot replaces the firmware's device tree
    # with the generation's, so dtparam/dtoverlay lines would have no effect.
    configtxt.settings.all = {
      # U-Boot needs the UART enabled to boot, and the mini UART's baud
      # follows the core clock unless it is pinned.
      enable_uart = true;
      core_freq = 250;
      # The GPU only needs enough for the firmware; the rest goes to Linux.
      gpu_mem = 16;
      # Keep the firmware from drawing warnings over a framebuffer.
      avoid_warnings = true;
      boot_delay = 0;
      disable_splash = true;
    };
  };
}
