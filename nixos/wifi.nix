# Wi-Fi: a way in that doesn't share the USB gadget link, whose data path
# has stalled under large transfers (docs/haxophone.md). iwd associates,
# networkd runs DHCP, and Avahi publishes haxophone.local.
#
# Networks come from two places, and iwd gets all of them:
#   - the agenix secret, an env file with WIFI_SSID and WIFI_PSK, so the
#     home network is in the repo without being readable there;
#   - wifi.txt on the FAT boot partition, which any computer can edit with
#     the card in a reader (format in ./wifi.txt, which is also written there
#     as a template when the file is missing). It is plain text on the card,
#     but so is everything else: whoever has the card has the board.
# iwd reads one file per network from /var/lib/iwd, which is on the tmpfs
# root, so a oneshot writes them all before iwd starts, on every boot.
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

  systemd.services.iwd-networks = {
    description = "Write iwd network files from the Wi-Fi secret and /boot/wifi.txt";
    wantedBy = [ "iwd.service" ];
    before = [ "iwd.service" ];
    # /boot is not needed for boot; without it only the secret's network
    # is written.
    wants = [ "boot.mount" ];
    after = [ "boot.mount" ];
    serviceConfig = {
      Type = "oneshot";
      # Optional: a newly minted card can't decrypt it until the secret is
      # rekeyed for its host key. wifi.txt is the way on the network then.
      EnvironmentFile = "-${config.age.secrets.spaceless-wifi.path}";
      UMask = "0077";
    };
    path = [
      pkgs.xxd
      pkgs.util-linux
    ];
    script = ''
      mkdir -p /var/lib/iwd

      write_network() {
        local ssid=$1 psk=$2 name
        if [ -z "$ssid" ] || [ -z "$psk" ]; then
          echo "skipping a network with an empty SSID or PSK" >&2
          return
        fi
        # iwd names the file after the SSID, hex-encoded with a leading "="
        # unless it is plain alphanumerics, space, "-" and "_".
        if [[ "$ssid" =~ ^[A-Za-z0-9_\ -]+$ ]]; then
          name="$ssid"
        else
          name="=$(printf '%s' "$ssid" | xxd -p | tr -d '\n')"
        fi
        # 64 hex digits is a raw key; anything else is a passphrase.
        if [[ "$psk" =~ ^[0-9A-Fa-f]{64}$ ]]; then
          printf '[Security]\nPreSharedKey=%s\n' "$psk" > "/var/lib/iwd/$name.psk"
        else
          printf '[Security]\nPassphrase=%s\n' "$psk" > "/var/lib/iwd/$name.psk"
        fi
        echo "network: $ssid"
      }

      if [ -n "''${WIFI_SSID:-}" ]; then
        write_network "$WIFI_SSID" "''${WIFI_PSK:-}"
      fi

      file=/boot/wifi.txt
      if ! mountpoint -q /boot; then
        echo "/boot is not mounted; skipping $file" >&2
        exit 0
      fi
      if [ ! -e "$file" ]; then
        cp ${./wifi.txt} "$file"
        exit 0
      fi

      # Trim surrounding whitespace (editors leave it behind), then drop one
      # pair of quotes, which keeps any spaces inside them.
      unquote() {
        local v=$1
        v=''${v#"''${v%%[![:space:]]*}"}
        v=''${v%"''${v##*[![:space:]]}"}
        if [[ "$v" =~ ^\"(.*)\"$ || "$v" =~ ^\'(.*)\'$ ]]; then
          v=''${BASH_REMATCH[1]}
        fi
        printf '%s' "$v"
      }

      ssid=
      n=0
      while IFS= read -r line || [ -n "$line" ]; do
        n=$((n + 1))
        line=''${line%$'\r'}
        [[ "$line" =~ ^[[:space:]]*(#|$) ]] && continue
        if [[ ! "$line" =~ ^[[:space:]]*([A-Za-z_]+)[[:space:]]*=(.*)$ ]]; then
          echo "$file:$n: not KEY=value, ignored" >&2
          continue
        fi
        key=''${BASH_REMATCH[1]}
        value=$(unquote "''${BASH_REMATCH[2]}")
        case "$key" in
          WIFI_SSID)
            [ -z "$ssid" ] || echo "$file:$n: no WIFI_PSK for '$ssid', skipped" >&2
            ssid=$value
            ;;
          WIFI_PSK)
            if [ -z "$ssid" ]; then
              echo "$file:$n: WIFI_PSK without a WIFI_SSID before it, ignored" >&2
            else
              write_network "$ssid" "$value"
            fi
            ssid=
            ;;
          *)
            echo "$file:$n: unknown key $key, ignored" >&2
            ;;
        esac
      done < "$file"
      [ -z "$ssid" ] || echo "$file: no WIFI_PSK for '$ssid', skipped" >&2
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
