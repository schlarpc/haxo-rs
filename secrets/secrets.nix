# agenix recipients. Run agenix from the repo root (`nix develop` provides
# it): `agenix -e secrets/<name>.age`, and `agenix -r` after a key changes.
let
  flake = builtins.getFlake (builtins.toString ./..);
  userKeys = builtins.filter (k: builtins.match "^ssh-(ed25519|rsa) .+" k != null) (
    builtins.filter builtins.isString (
      builtins.split "\n" (builtins.readFile flake.inputs.schlarpc-keys)
    )
  );
  # The board's identity, kept on /persist (nixos/storage.nix). The minting
  # app installs it on a new card, or prints a new one to put here.
  hostKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFB6icV0F8F/fuI+/4NYCDnlKebtpvgzSBSFPmJm3rHg";
in
{
  # Env file with WIFI_SSID and WIFI_PSK; nixos/wifi.nix.
  "spaceless-wifi.age".publicKeys = userKeys ++ [ hostKey ];
}
