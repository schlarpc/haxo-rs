# Wi-Fi: a way in that doesn't share the USB gadget link, whose data path
# has stalled under large transfers (docs/haxophone.md). iwd associates,
# networkd runs DHCP, and Avahi publishes haxophone.local.
#
# The networks come from config/haxophone.toml on the FAT boot partition
# (config-file.nix).
{
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
