# Wi-Fi: a way in that doesn't share the USB gadget link, whose data path
# has stalled under large transfers (docs/haxophone.md). iwd associates,
# networkd runs DHCP, and Avahi publishes haxophone.local.
#
# The network comes from an agenix secret, an env file with WIFI_SSID and
# WIFI_PSK, so neither is in the repo. iwd reads one file per network from
# /var/lib/iwd; a oneshot writes it from the secret before iwd starts.
{
  config,
  pkgs,
  ...
}:
{
  age.secrets.spaceless-wifi.file = ../secrets/spaceless-wifi.age;

  # Without this iwd sits in "connecting" forever on the Zero 2 W's
  # BCM43430/1: the firmware advertises SAE and 4-way-handshake offload, and
  # iwd trips over it. Disable both (verified on the board: connects within
  # seconds with it, never without).
  boot.extraModprobeConfig = ''
    options brcmfmac feature_disable=0x82000
  '';

  networking.wireless.iwd = {
    enable = true;
    # networkd owns addressing on every interface here.
    settings.General.EnableNetworkConfiguration = false;
  };

  systemd.services.iwd-network-from-secret = {
    description = "Write the iwd network file from the Wi-Fi secret";
    wantedBy = [ "iwd.service" ];
    before = [ "iwd.service" ];
    serviceConfig = {
      Type = "oneshot";
      EnvironmentFile = config.age.secrets.spaceless-wifi.path;
      UMask = "0077";
    };
    path = [ pkgs.xxd ];
    script = ''
      # iwd names the file after the SSID, hex-encoded with a leading "="
      # unless it is plain alphanumerics, space, "-" and "_".
      if [[ "$WIFI_SSID" =~ ^[A-Za-z0-9_\ -]+$ ]]; then
        name="$WIFI_SSID"
      else
        name="=$(printf '%s' "$WIFI_SSID" | xxd -p | tr -d '\n')"
      fi
      mkdir -p /var/lib/iwd
      printf '[Security]\nPassphrase=%s\n' "$WIFI_PSK" > "/var/lib/iwd/$name.psk"
    '';
  };

  systemd.network.networks."20-wlan0" = {
    matchConfig.Name = "wlan0";
    networkConfig = {
      DHCP = "yes";
      IPv6AcceptRA = true;
      MulticastDNS = false;
    };
    linkConfig.RequiredForOnline = "no";
  };

  services.avahi = {
    enable = true;
    nssmdns4 = true;
    publish = {
      enable = true;
      addresses = true;
    };
  };
}
