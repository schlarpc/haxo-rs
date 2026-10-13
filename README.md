# haxo-nix

NixOS for a [Haxophone](https://github.com/cardonabits/haxo-hw) on a
Raspberry Pi Zero 2 W. Design notes and decisions: [docs/haxophone.md](docs/haxophone.md).

| Path | What |
|---|---|
| `nixos/configuration.nix` | The host: haxo-rs service, networking, consoles |
| `nixos/boot.nix` | Pi firmware -> U-Boot -> extlinux, mainline kernel, HAT overlay |
| `nixos/storage.nix`, `nixos/disko.nix` | SD layout: FAT /boot, btrfs `/nix` + `/persist`, tmpfs root |
| `nixos/gadget.nix` | USB gadget: NCM network, ACM console, MIDI, MTP |
| `nixos/wifi.nix` | iwd + networkd, network from an agenix secret |
| `packages/` | haxo-rs (from our fork), umtprd, the minting app |
| `secrets/` | agenix secrets and their recipients |

`nix develop` provides `deploy` and `agenix`.

## Minting an SD card

On an x86_64 NixOS machine with aarch64 emulation
(`boot.binfmt.emulatedSystems = [ "aarch64-linux" ]` with
`preferStaticEmulators = true`; the install activates the aarch64 system in
a chroot), with the card in a USB reader:

```sh
nix run .#mint-sd -- /dev/disk/by-id/usb-<reader>-0:0 [ssh_host_ed25519_key]
```

It builds the system, asks for sudo, and asks before it wipes the card. Then
it partitions and formats the card (disko), puts the host key and a
machine-id on `/persist`, and installs the system with its firmware, U-Boot
and extlinux.conf. The card boots straight into the configured system.

Pass the board's existing host key to keep its identity, so agenix keeps
decrypting the Wi-Fi secret. From the running board:

```sh
ssh 10.68.0.1 sudo cat /etc/ssh/ssh_host_ed25519_key > key  # then shred it
```

Without a key the app generates one and prints its public half. Put that in
`secrets/secrets.nix` as `hostKey`, run `agenix -r`, and deploy (the first
boot comes up without Wi-Fi).

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
