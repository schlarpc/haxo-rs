# Mint an SD card for the haxophone from a card reader on another machine:
# partition and format it with disko (nixos/disko.nix), give the host its
# identity on /persist, and install the system closure. The card boots
# straight into the configured system, with no installer step on the Pi.
#
#   nix run .#mint-sd -- /dev/sdX [ssh_host_ed25519_key]
#
# With a host key, the card keeps that identity (and agenix keeps
# decrypting). Without one it generates a new key and prints what to do.
#
# On x86_64 the install activates the aarch64 system in a chroot, so the
# machine needs aarch64 binfmt emulation with the fix-binary flag
# (boot.binfmt.emulatedSystems plus preferStaticEmulators on NixOS).
{
  lib,
  stdenv,
  writeShellApplication,
  coreutils,
  util-linux,
  openssh,
  systemd,
  nixos-install-tools,
  disko,
  diskoFile,
  toplevel,
  hostName,
}:
writeShellApplication {
  name = "mint-sd";
  runtimeInputs = [
    coreutils
    util-linux
    openssh
    systemd
    nixos-install-tools
    disko
  ];
  text = ''
    usage() {
      echo "usage: mint-sd <device> [ssh_host_ed25519_key]" >&2
      exit 2
    }
    [ $# -ge 1 ] && [ $# -le 2 ] || usage

    if [ "$(id -u)" -ne 0 ]; then
      exec sudo -- "$0" "$@"
    fi

    dev=$(readlink -f "$1")
    key=''${2:+$(readlink -f "$2")}

    [ -b "$dev" ] || { echo "$dev is not a block device" >&2; exit 1; }
    case "$(lsblk -dno TYPE "$dev")" in
      disk | loop) ;;
      *) echo "$dev is not a whole disk" >&2; exit 1 ;;
    esac
    if lsblk -no MOUNTPOINTS "$dev" | grep -q .; then
      echo "$dev has mounted partitions:" >&2
      lsblk "$dev" >&2
      exit 1
    fi
    if [ -n "$key" ] && ! ssh-keygen -y -f "$key" >/dev/null; then
      echo "$key is not a readable private key" >&2
      exit 1
    fi
    ${lib.optionalString (!stdenv.hostPlatform.isAarch64) ''
      if [ ! -e /proc/sys/fs/binfmt_misc/aarch64-linux ]; then
        echo "no aarch64-linux binfmt handler; the install can't activate the system" >&2
        exit 1
      fi
    ''}
    command -v nix >/dev/null || { echo "nix is not on PATH" >&2; exit 1; }

    lsblk -o NAME,SIZE,MODEL,SERIAL "$dev"
    echo
    echo "Everything on $dev will be destroyed."

    # A fresh mount point rather than /mnt, which may be in use.
    mnt=$(mktemp -d -p /run mint-sd.XXXXXX)

    # Prompts before wiping.
    disko --mode destroy,format,mount --root-mountpoint "$mnt" \
      --argstr device "$dev" ${diskoFile}

    install -d -m 0755 "$mnt/persist/etc/ssh"
    if [ -n "$key" ]; then
      install -m 0600 "$key" "$mnt/persist/etc/ssh/ssh_host_ed25519_key"
      ssh-keygen -y -f "$key" > "$mnt/persist/etc/ssh/ssh_host_ed25519_key.pub"
    else
      ssh-keygen -q -t ed25519 -N "" -C ${hostName} \
        -f "$mnt/persist/etc/ssh/ssh_host_ed25519_key"
    fi
    systemd-id128 new > "$mnt/persist/etc/machine-id"

    nixos-install --system ${toplevel} --root "$mnt" \
      --no-root-passwd --no-channel-copy

    pub=$(cat "$mnt/persist/etc/ssh/ssh_host_ed25519_key.pub")
    umount -R "$mnt"
    rmdir "$mnt"
    sync

    echo
    echo "Done. $dev can be removed."
    if [ -z "$key" ]; then
      echo
      echo "New host key; set hostKey in secrets/secrets.nix to:"
      echo "  $pub"
      echo "then run 'agenix -r' and deploy, or the Wi-Fi secret won't decrypt."
    fi
  '';
}
