# Users and Nix.
{ ... }:
{
  nix = {
    settings = {
      experimental-features = [
        "nix-command"
        "flakes"
      ];
      # deploy-rs copies unsigned paths as the ssh user.
      trusted-users = [ "@wheel" ];
      auto-optimise-store = true;
    };
    gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 14d";
    };
    # No channels: the system comes from this flake.
    channel.enable = false;
  };

  nixpkgs.config.allowUnfree = true;

  # /etc/passwd and /etc/shadow are rebuilt on every boot from the tmpfs
  # root, so a passwd change wouldn't survive anyway. Login is by ssh key,
  # or autologin on the consoles.
  users.mutableUsers = false;
  # No password and no ssh key in the config, which NixOS takes for a
  # lockout. The keys come from the card at boot (config-file.nix), and the
  # console (mini UART; USB serial in the g_cdc fallback) autologins.
  users.allowNoPasswordLogin = true;

  # The instrument's own user rather than a personal one. Its ssh keys come
  # from the settings file on the card (config-file.nix).
  users.users.haxophone = {
    isNormalUser = true;
    description = "Haxophone";
    uid = 1000;
    extraGroups = [
      "wheel"
      "systemd-journal"
    ];
  };

  security.sudo.wheelNeedsPassword = false;
}
