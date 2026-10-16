# Mint an SD card for the haxophone from a card reader on another machine:
# partition and format it with disko (nix/nixos/disko.nix), give the host its
# identity on /persist, install the system closure, and write the settings
# file (config/haxophone.toml on the FAT partition). The card boots straight
# into the configured system, with no installer step on the Pi.
#
#   nix run .#mint-sd -- [options] /dev/sdX
#
#   --ssh-keys FILE   public keys allowed to log in (a .pub or an
#                     authorized_keys file); repeatable
#   --config FILE     a complete haxophone.toml instead of the template
#   --host-key FILE   the board's ssh host key, to keep its fingerprint;
#                     a new one is generated otherwise
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
  haxophone-config,
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
    haxophone-config
  ];
  text = ''
    usage() {
      echo "usage: mint-sd [--ssh-keys FILE]... [--config FILE] [--host-key FILE] <device>" >&2
      exit 2
    }

    if [ "$(id -u)" -ne 0 ]; then
      exec sudo -- "$0" "$@"
    fi

    dev="" key="" config=""
    keyfiles=()
    while [ $# -gt 0 ]; do
      case "$1" in
        --ssh-keys) [ $# -ge 2 ] || usage; keyfiles+=(--ssh-keys "$(readlink -f "$2")"); shift 2 ;;
        --config) [ $# -ge 2 ] || usage; config=$(readlink -f "$2"); shift 2 ;;
        --host-key) [ $# -ge 2 ] || usage; key=$(readlink -f "$2"); shift 2 ;;
        -*) usage ;;
        *) [ -z "$dev" ] || usage; dev=$(readlink -f "$1"); shift ;;
      esac
    done
    [ -n "$dev" ] || usage
    if [ -n "$config" ] && [ ''${#keyfiles[@]} -gt 0 ]; then
      echo "--config and --ssh-keys don't combine: put the keys in the config" >&2
      exit 2
    fi

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

    # The settings file, built and checked before anything is wiped.
    settings=$(mktemp)
    trap 'rm -f "$settings"' EXIT
    if [ -n "$config" ]; then
      cp "$config" "$settings"
    else
      haxophone-config template "''${keyfiles[@]}" > "$settings"
    fi
    haxophone-config check "$settings" || { echo "fix the settings first" >&2; exit 1; }

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

    install -D -m 0644 "$settings" "$mnt/boot/config/haxophone.toml"

    fingerprint=$(ssh-keygen -l -f "$mnt/persist/etc/ssh/ssh_host_ed25519_key.pub")
    umount -R "$mnt"
    rmdir "$mnt"
    sync

    echo
    echo "Done. $dev can be removed."
    echo "Host key: $fingerprint"
    echo "Settings (ssh keys, Wi-Fi): config/haxophone.toml on its FAT partition."
  '';
}
