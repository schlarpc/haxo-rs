# haxo-rs

The software for a [Haxophone](https://github.com/cardonabits/haxo-hw), a
digital saxophone HAT, on a Raspberry Pi Zero 2 W: the firmware that turns
key presses and breath into music, and the NixOS system that runs it. One
flake builds both, and mints an SD card that boots straight into it.

This is a fork of [cardonabits/haxo-rs](https://github.com/cardonabits/haxo-rs).
Design notes and decisions: [docs/haxophone.md](docs/haxophone.md); the web
UI: [docs/webui.md](docs/webui.md).

| Path | What |
|---|---|
| `src/` | haxo001, the instrument: key scan, breath sensor, fluidsynth, MIDI out, Bluetooth MIDI |
| `crates/haxo-web` | The web UI's server; the page is `ui/index.html` |
| `crates/haxo-bridge` | Plays haxo001's audio tap into USB audio and Bluetooth |
| `crates/haxo-control` | The control-socket protocol haxo001 and haxo-web share |
| `nix/nixos/configuration.nix` | The host: the haxo service, networking, consoles |
| `nix/nixos/boot.nix` | Pi firmware -> U-Boot -> extlinux, mainline kernel, HAT overlay |
| `nix/nixos/storage.nix`, `nix/nixos/disko.nix` | SD layout: FAT /boot, btrfs `/nix` + `/persist`, tmpfs root |
| `nix/nixos/gadget.nix` | USB gadget: NCM network, USB audio (UAC2), MIDI, MTP |
| `nix/nixos/audio-out.nix` | haxo-bridge: the instrument's sound to USB audio and Bluetooth |
| `nix/nixos/wifi.nix` | iwd + networkd + Avahi |
| `nix/nixos/bluetooth.nix` | BlueZ + PipeWire for Bluetooth audio out |
| `nix/nixos/config-file.nix` | Applies `config/haxophone.toml` from the card at boot |
| `nix/nixos/webui.nix` | The web UI on port 80 |
| `nix/overlay.nix`, `nix/packages/` | The packages: haxo-rs, haxo-web, haxo-bridge, umtprd, the settings tool, the minting app |

`nix develop` provides cargo with the C libraries, and `deploy`.

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

With no usable key, log in on the mini UART console (GPIO 14/15), which
autologins. The USB serial console made way for USB audio; it comes back
only in the g_cdc fallback, when the composite gadget fails to bind.

## Deploying

```sh
deploy .#haxophone
```

This goes over the USB gadget link (10.68.0.1). That link has stalled under
large copies (docs/haxophone.md), so for big closures go over Wi-Fi:
`deploy .#haxophone --hostname haxophone.local`. Changes to the USB gadget
take effect at the next boot; a live rebind would cut the link deploy-rs is
using.

Commit (or at least `git add`) new files first: the flake sees only what
git tracks. Uncommitted changes to tracked files are built as they are.

`nix flake update` updates nixpkgs and the other inputs.

## Working on the code

The Rust packages build from the workspace alone (Cargo.toml, Cargo.lock,
build.rs, `src/`, `crates/`, plus `notemap.json` and the startup tune), so a
change to the NixOS modules or the docs rebuilds none of them. Their
dependencies are built once per Cargo.lock.

```sh
nix build .#haxo-rs            # for this machine
nix build .#packages.aarch64-linux.haxo-rs   # for the board
nix develop -c cargo test -p haxo-web        # runs anywhere
```

### Logging

The programs use [`env_logger`](https://docs.rs/env_logger/0.9.0/env_logger/).
The services run with `RUST_LOG=info`; set `RUST_LOG=debug` for more.

### Testing on the board

haxo001's unit tests drive the real GPIO, I2C and ALSA hardware, so they run
on a Pi with the HAT, one at a time: `cargo test -- --test-threads=1`.

```
running 8 tests
test keyscan::tests::all_keys ... ignored
test keyscan::tests::init ... ok
test keyscan::tests::read ... ok
test notemap::tests::update ... ok
test pressure::tests::init ... ok
test pressure::tests::pressure_step ... ignored
test pressure::tests::read ... ok
test pressure::tests::read_io ... ignored

test result: ok. 5 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

The ignored tests need someone at the instrument, and are useful to check
the hardware (stuck keys, a dead pressure sensor). Run them with standard
output:

```
$ cargo test all_keys -- --nocapture --ignored
running 1 test
Press all the keys at least once, in any order...
01/22: detected_keys: 400 keys: 400
01/22: detected_keys: 0 keys: 400
(...)
21/22: detected_keys: 0 keys: 27773777
22/22: detected_keys: 8 keys: 2777377f
test keyscan::tests::all_keys ... ok
```

### Raspberry Pi OS

Upstream's way of running haxo-rs, on Raspberry Pi OS, is still here:
cross-compiling in `docker/`, the systemd units in `scripts/systemd/`, and
the network setup in [docs/zero-setup.md](docs/zero-setup.md). See
[upstream's README](https://github.com/cardonabits/haxo-rs#readme) for it.
