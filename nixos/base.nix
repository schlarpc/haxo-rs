# Users and Nix: what the host got from schlarpc-flake's shared modules.
{
  lib,
  pkgs,
  inputs,
  ...
}:
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

  # The instrument's own user rather than a personal one; schlarpc's ssh
  # keys log in to it.
  users.users.haxophone = {
    isNormalUser = true;
    description = "Haxophone";
    uid = 1000;
    extraGroups = [
      "wheel"
      "systemd-journal"
    ];
    shell = pkgs.fish;
    openssh.authorizedKeys.keys = lib.filter (k: k != "") (
      lib.splitString "\n" (builtins.readFile inputs.schlarpc-keys)
    );
  };

  programs.fish.enable = true;
  security.sudo.wheelNeedsPassword = false;
}
