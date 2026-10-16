# Settings that belong to whoever has the instrument, not to this repo: a
# TOML file on the FAT boot partition, config/haxophone.toml, editable from
# any computer with the card in a reader. It holds the ssh keys that may log
# in as `haxophone` and the Wi-Fi networks to join (format and template in
# nix/packages/haxophone-config).
#
# A oneshot applies it at every boot: iwd's network files into /var/lib/iwd,
# the keys into /run, and haxo's instrument settings into an environment
# file, all on tmpfs, so the file is the whole lifecycle.
# After editing it on the board: `systemctl restart haxophone-config iwd`.
#
# The file is plain text on the card, but so is everything else: whoever
# has the card has the board.
{
  lib,
  pkgs,
  haxo,
  ...
}:
let
  inherit (pkgs) haxophone-config;
  keysDir = "/run/haxophone-config/authorized_keys";
in
{
  systemd.services.haxophone-config = {
    description = "Apply /boot/config/haxophone.toml";
    wantedBy = [ "multi-user.target" ];
    before = [
      "iwd.service"
      "sshd.service"
    ];
    # /boot is not needed for boot; without it nothing is applied, and the
    # consoles' autologin is the way in.
    wants = [ "boot.mount" ];
    after = [ "boot.mount" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = lib.concatStringsSep " " [
        "${lib.getExe haxophone-config} apply --keys-dir ${keysDir}"
        "--haxo-env ${haxo.envFile}"
        # Where [instrument] soundfont names are looked up.
        "--soundfont-dir ${haxo.soundfontDir}"
        "--soundfont-dir ${haxo.filesDir}"
      ];
    };
  };

  services.openssh.authorizedKeysFiles = [ "${keysDir}/%u" ];

  environment.systemPackages = [ haxophone-config ];
}
