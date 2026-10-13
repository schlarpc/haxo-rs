# haxo-nix

NixOS for a [Haxophone](https://github.com/cardonabits/haxo-hw) on a
Raspberry Pi Zero 2 W. Design notes and decisions: [docs/haxophone.md](docs/haxophone.md).

| Path | What |
|---|---|
| `nixos/configuration.nix` | The host: haxo-rs service, networking, consoles |
| `nixos/boot.nix` | Pi firmware -> U-Boot -> extlinux, mainline kernel, HAT overlay |
| `nixos/storage.nix`, `nixos/disko.nix` | SD layout: FAT /boot, btrfs `/nix` + `/persist`, tmpfs root |
| `nixos/gadget.nix` | USB gadget: NCM network, ACM console, MIDI, MTP |
| `nixos/wifi.nix` | iwd + networkd + Avahi |
| `nixos/bluetooth.nix` | BlueZ + PipeWire for Bluetooth audio out (not fed yet) |
| `nixos/config-file.nix` | Applies `config/haxophone.toml` from the card at boot |
| `packages/` | haxo-rs (from our fork), umtprd, the settings tool, the minting app |

`nix develop` provides `deploy`.

## Minting an SD card

On an x86_64 NixOS machine with aarch64 emulation
(`boot.binfmt.emulatedSystems = [ "aarch64-linux" ]` with
`preferStaticEmulators = true`; the install activates the aarch64 system in
a chroot), with the card in a USB reader:

```sh
nix run .#mint-sd -- --ssh-keys ~/.ssh/id_ed25519.pub /dev/disk/by-id/usb-<reader>-0:0
```

It builds the system, asks for sudo, and asks before it wipes the card. Then
it partitions and formats the card (disko), puts the host key and a
machine-id on `/persist`, installs the system with its firmware, U-Boot and
extlinux.conf, and writes the settings file. The card boots straight into
the configured system.

Options:
- `--ssh-keys FILE`: public keys that may log in, from a `.pub` or an
  `authorized_keys` file. Repeatable.
- `--config FILE`: a complete `haxophone.toml` (keys and Wi-Fi) instead of
  the template. It is checked before the card is touched.
- `--host-key FILE`: the board's ssh host key, so its fingerprint stays the
  same across a reflash. Without it a new key is generated. From a running
  board: `ssh haxophone@haxophone.local sudo cat /etc/ssh/ssh_host_ed25519_key > key`.

## Settings: `config/haxophone.toml`

The settings that belong to whoever has the instrument live on the card's
FAT partition, so any computer with a card reader can change them:

```toml
ssh_authorized_keys = [
  "ssh-ed25519 AAAA... you@laptop",
]

[[wifi]]
ssid = "My Network"
psk = "my passphrase"

[[wifi]]
ssid = "Open Cafe"   # no psk: an open network
```

The board reads the file at every boot; removing an entry removes it.
Mistakes are skipped with a message in `journalctl -u haxophone-config`.
On the board, after `sudo $EDITOR /boot/config/haxophone.toml`:
`sudo systemctl restart haxophone-config iwd`. To check a file elsewhere:
`nix run .#haxophone-config -- check haxophone.toml`.

With no usable key, log in on the USB serial console (`ttyGS0`) or the
mini UART: both autologin.

## Deploying

```sh
deploy .#haxophone
```

This goes over the USB gadget link (10.68.0.1). That link has stalled under
large copies (docs/haxophone.md), so for big closures go over Wi-Fi:
`deploy .#haxophone --hostname haxophone.local`. Changes to the USB gadget
take effect at the next boot; a live rebind would cut the link deploy-rs is
using.

## Updating

`nix flake update` for everything; `nix flake update haxo-rs` to pick up
what's pushed to the fork's `main`. To test unpushed haxo-rs work:
`--override-input haxo-rs git+file:///home/schlarpc/src/haxo-rs`.
