# tmpfs root on the btrfs SD card (disko.nix), with what has to survive a
# reboot bind-mounted or symlinked from /persist by preservation. Decisions
# in docs/haxophone.md "Storage" and "Memory".
{ inputs, ... }:
{
  imports = [
    inputs.disko.nixosModules.disko
    inputs.preservation.nixosModules.preservation
    ./disko.nix
  ];

  boot = {
    supportedFilesystems = [ "btrfs" ];
    # preservation's units run in the systemd initrd.
    initrd.systemd.enable = true;
  };

  # preservation needs these in the initrd.
  fileSystems."/persist".neededForBoot = true;

  preservation = {
    enable = true;
    preserveAt."/persist" = {
      files = [
        # Written by the minting app, so it exists before the first boot.
        {
          file = "/etc/machine-id";
          inInitrd = true;
        }
        # The host key is agenix's identity, and agenix decrypts during
        # activation, before preservation's stage-2 units run. inInitrd puts
        # the symlinks in place from the initrd so every boot can decrypt.
        {
          file = "/etc/ssh/ssh_host_ed25519_key";
          how = "symlink";
          configureParent = true;
          inInitrd = true;
        }
        {
          file = "/etc/ssh/ssh_host_ed25519_key.pub";
          how = "symlink";
          configureParent = true;
          inInitrd = true;
        }
      ];
      directories = [
        {
          directory = "/var/lib/nixos";
          mode = "0755";
          inInitrd = true;
        }
        "/var/lib/systemd"
        "/var/lib/bluetooth"
        # MTP storage (gadget.nix).
        "/var/lib/haxophone"
        {
          directory = "/home/schlarpc";
          user = "schlarpc";
          group = "users";
          mode = "0700";
        }
      ];
    };
  };

  # machine-id comes from /persist; nothing to commit.
  systemd.services.systemd-machine-id-commit.enable = false;

  # Ed25519 only: the RSA key would be regenerated into the tmpfs on every
  # boot, changing the host's fingerprint.
  services.openssh.hostKeys = [
    {
      path = "/etc/ssh/ssh_host_ed25519_key";
      type = "ed25519";
    }
  ];

  # Volatile journal: in RAM under /run, capped, gone at reboot.
  services.journald = {
    storage = "volatile";
    extraConfig = ''
      RuntimeMaxUse=32M
    '';
  };

  services.fstrim.enable = true;
}
