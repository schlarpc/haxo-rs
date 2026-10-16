# The SD card layout (docs/haxophone.md "Storage").
#
#   firmware  512 MB FAT, mounted at /boot: Pi GPU firmware, config.txt,
#             U-Boot, and the extlinux.conf + kernels/initrds it reads. U-Boot
#             reads FAT, so this holds every generation's kernel: about 50 MB
#             each, which is what configurationLimit is sized against.
#   root      the rest, btrfs. Subvolumes nix and persist; / is a tmpfs.
#
# GPT with a hybrid MBR. The Zero 2 W's boot ROM (BCM2837, Pi 3 class) only
# reads MBR, so the firmware partition is also MBR entry 1, type 0x0c (FAT32
# LBA). Linux and disko use the GPT, so mounts go by partlabel.
#
# A function so the minting app (nix/packages/mint-sd.nix) can point it at the
# card reader with `disko --argstr device`. The installed system only uses it
# for the mounts below, which don't depend on `device`.
{
  device ? "/dev/mmcblk0",
  ...
}:
{
  disko.devices = {
    disk.sd = {
      type = "disk";
      inherit device;
      content = {
        type = "gpt";
        efiGptPartitionFirst = false;
        partitions = {
          firmware = {
            priority = 1;
            size = "512M";
            type = "EF00";
            hybrid = {
              mbrPartitionType = "0x0c";
              mbrBootableFlag = false;
            };
            content = {
              type = "filesystem";
              format = "vfat";
              mountpoint = "/boot";
              mountOptions = [ "umask=0077" ];
            };
          };
          root = {
            size = "100%";
            content = {
              type = "btrfs";
              extraArgs = [ "-f" ];
              subvolumes =
                let
                  # SD card: compress to cut write volume, no atime churn, no
                  # online discard (fstrim.timer instead).
                  opts = [
                    "compress=zstd"
                    "noatime"
                    "ssd"
                  ];
                in
                {
                  "/nix" = {
                    mountpoint = "/nix";
                    mountOptions = opts;
                  };
                  "/persist" = {
                    mountpoint = "/persist";
                    mountOptions = opts;
                  };
                };
            };
          };
        };
      };
    };

    nodev."/" = {
      fsType = "tmpfs";
      mountOptions = [
        "size=128M"
        "mode=755"
      ];
    };
  };
}
