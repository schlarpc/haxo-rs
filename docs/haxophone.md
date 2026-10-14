# Haxophone

NixOS for a Haxophone (digital saxophone HAT) on a Raspberry Pi Zero 2 W:
512 MB RAM, 4x Cortex-A53, onboard CYW43436 (2.4 GHz Wi-Fi + BT 4.2 on one
antenna), one OTG-capable micro-USB port. This records the decisions made so
far.

Until 2026-10-13 the host lived in schlarpc-flake (`hosts/haxophone`, booted
from nixpkgs' SD image). It moved to this repo with the storage layout
below; the sections before "Layout" describe that bootstrap.

## TODO

Roughly in order. Details are in the sections named.

1. **First card on the new layout.** Nothing in "Layout" has booted yet;
   the board still runs the bootstrap card (user `schlarpc`, ext4 root).
   - Keep the host key: `ssh schlarpc@10.68.0.1 sudo cat
     /etc/ssh/ssh_host_ed25519_key > key` while the bootstrap card runs.
   - Mint a second card (keep the bootstrap card as the fallback):
     `nix run .#mint-sd -- --host-key key --config haxophone.toml <dev>`,
     with your ssh keys and Wi-Fi in the TOML. Shred `key` afterwards.
   - On the first boot, check in this order: the firmware starts at all
     (nothing on the mini UART points at the hybrid MBR first); U-Boot
     finds extlinux.conf on GPT partition 1; the initrd mounts btrfs and
     the tmpfs root; `ssh haxophone@10.68.0.1` works with the card's keys
     and the old fingerprint; Wi-Fi joins from the TOML; MemTotal is up
     from 376 MB (`gpu_mem=16`, no vc4); the gadget's four functions
     enumerate; haxo plays. Then reboot once and check that machine-id,
     the host key and /home/haxophone survived.
   - Record what happened in "Layout", and drop its "untested" notes.
2. **USB gadget stalls** under sustained bulk transfers ("Open issue"):
   reproduce over Wi-Fi with the kernel log live.
3. **Bluetooth audio bench test** ("Bluetooth"): pair a receiver,
   measure latency per codec and CPU next to haxo, listen for radio noise.
4. **haxo-rs patches** ("Patches to make"), each reproduced and measured
   on the board first ("Upstream issues"). What's left there needs someone
   playing (idle wake-up, BLE MIDI from a phone, the pressure spikes) or
   step 3 below.
5. **Bluetooth MIDI** is in haxo-rs (193ea18); try it from a phone or a
   DAW and settle pairing.
6. **Own render loop with the Bluetooth bridge** ("Outputs", build order
   step 3).
7. **USB audio out** (UAC2, "TODO: USB audio out"), and the **web UI**
   ("TODO: web UI"): a frontend spike with a mock backend is in
   `webui/`, and its API contract in docs/webui.md; the backend and the
   haxo-rs hooks it needs are next.

## Bootstrap (schlarpc-flake hosts/haxophone, 2026-10-12)

nixpkgs' aarch64 SD image module with the mainline kernel (6.18, from
cache.nixos.org; dwc2 is built in as dual-role, and MAX98357A and bcm2835 I2S
are modules), built on bagel through aarch64 binfmt. One USB cable to the
inner "USB" port powers the board and carries the legacy `g_cdc` gadget:
ECM network (the Zero is 10.68.0.1 and hands the host a DHCP lease with no
router or DNS) plus ACM serial (`ttyGS0`, autologin). The mini UART
(`ttyS1`) is a second console. First boot worked: SSH over the gadget at
10.68.0.1. Only 376 MB of RAM is visible (MemTotal 385004 kB): the firmware's
GPU memory split and the vc4 KMS driver's CMA take the rest; `gpu_mem=16`
and dropping vc4 get most of it back. Throwaway: the btrfs/tmpfs layout and the rpi
kernel (cross-built with ccacheStdenv like schlarpc-flake's hosts/devterm)
replace it. (The rpi kernel part was later dropped: see "Kernel: mainline".)

Milestone (2026-10-13): playable on NixOS. Mainline kernel, our HAT
overlay, haxo-rs with patches 0001-0002, deployed with deploy-rs over the
gadget link. Sound, keys, breath and re-articulation all verified by
playing it.

## Open issue: the USB gadget data path stalls (2026-10-13)

Twice, a deploy's `nix copy` over the gadget link died: first with g_cdc
(ECM; sshd reset connections, ping still answered, the ACM console went
silent), then with the composite gadget (NCM; bagel logged repeated
`cdc_ncm ... NETDEV WATCHDOG: transmit queue 0 timed out`, then the device
stopped answering control requests: `can't set config #1, error -110`).
The board itself stayed up the second time: the hardware watchdog (now on)
never fired. So it's the dwc2 gadget side stalling under sustained bulk
traffic, not a kernel hang. An earlier reading of the first incident as a
hard hang was wrong: that boot's journal ends mid-play simply because
nothing more was logged.

Next: get in over Wi-Fi (nixos/wifi.nix) while the link is
stalled and read the kernel log live. Suspects: dwc2 gadget DMA/FIFO
handling on bcm2835 in mainline, NCM/ECM under load, power on the shared
USB supply.

## Layout (this repo, 2026-10-13)

What the "Decided" items below turned into. Not yet booted on the board.

- **Partitions** (nixos/disko.nix): GPT, 512 MB FAT at /boot, btrfs for
  the rest with subvolumes `/nix` and `/persist` (zstd, noatime). The FAT
  partition is also entry 1 of a hybrid MBR, type 0x0c: the BCM2837 boot
  ROM is Pi 3 class and reads only MBR. Untested on the board; if the
  firmware never starts (no activity LED, nothing on the mini UART), this is
  the first suspect.
- **Boot chain** (nixos/boot.nix): nixos-hardware's Raspberry Pi firmware
  module stages the GPU firmware, `u-boot.bin` (nixpkgs' rpi_3 build, which
  the bootstrap booted with) and config.txt onto /boot on every switch.
  config.txt is written in full, not on top of nixos-hardware's Raspberry Pi
  OS defaults: `enable_uart`, `core_freq=250` (the mini UART console),
  `gpu_mem=16`. U-Boot loads the generation's own device tree (FDTDIR), so
  the HAT overlay comes from `hardware.deviceTree`, never config.txt.
  `configurationLimit = 8` generations on the FAT partition.
- **Root**: tmpfs, 128 MB. preservation (not impermanence; it is what
  schlarpc-flake's newer hosts use) keeps machine-id, the ed25519 host key
  (symlinked from the initrd),
  /var/lib/{nixos,systemd,bluetooth,haxophone} and /home/haxophone. Only an
  ed25519 host key is generated: an RSA one would change on every boot.
- **Firmware**: only linux-firmware's `brcm/` and `cypress/` directories
  (the Wi-Fi blobs the bootstrap verified), not all of linux-firmware: the
  closure went from 2.7 to 1.9 GB. Plus the Bluetooth patch files (see
  "Bluetooth").
- **Journal**: volatile, 32 MB, so nothing survives a crash. Debug hangs
  live (over Wi-Fi) or on the UART.
- **Users**: immutable (`users.mutableUsers = false`); /etc/shadow is
  rebuilt every boot anyway. One user, `haxophone` (uid 1000), with
  autologin on the consoles; also the deploy user. Its ssh keys come from
  the card (see "Settings file"), not the flake.
- **Nix**: stock nixpkgs Nix, not Determinate (which the schlarpc-flake
  hosts get); nothing builds on the board.
- **Minting** (packages/mint-sd.nix, see README.md): format the card in a
  reader with the disko CLI, put the host key and machine-id on /persist,
  `nixos-install --system` the prebuilt closure, and write the settings
  file. Verified against a
  loop device on bagel, not yet with a card on the board.

## Decided

- **Storage**: FAT firmware partition holding the kernels (256-512 MB,
  sized to `configurationLimit` x ~50 MB per generation), btrfs for the rest
  (zstd, snapshots), tmpfs root with impermanence. No ZFS: RAM, U-Boot cannot
  read it, and the zfs module would have to be cross-built against linux_rpi.
- **Memory**: zram instead of SD swap; haxo-rs mlocks itself. Volatile
  journal capped at 16-32 MB. tmpfs root capped at 64-128 MB.
- **Bluetooth**: used for performing. The onboard controller is BT 4.2, so
  LE Audio / LC3 is out (needs 5.2 isochronous channels), and the only USB
  port is the gadget, so no dongle. Best available is aptX LL or FastStream
  through PipeWire, which needs a receiver that supports one of them.
  PipeWire cannot encode aptX Adaptive.
- **UART**: PL011 stays on Bluetooth (A2DP needs it); serial console on the
  mini-UART (`enable_uart=1`).
- **Kernel: mainline** (nixpkgs default, from cache.nixos.org), not the
  rpi kernel. Verified on the board with 6.18.48: dwc2 gadget, Wi-Fi
  (brcmfmac, BCM43430/1, firmware 7.45.96), Bluetooth (`hci0` on the PL011,
  mini-UART console on ttyS1, which is our planned split), and cpufreq-dt
  600-1000 MHz all work. `linux_rpi02w` (6.12.75) is not cached, so it would
  be a cross-build after every nixpkgs bump. The one thing it offers is the
  ready-made `max98357a` config.txt overlay; we write our own device tree
  overlay instead (`hardware.deviceTree.overlays`). PREEMPT_RT is a config
  option away on mainline if latency ever needs it.
- **haxo-rs is ours to change**: our fork schlarpc/haxo-rs (upstream:
  cardonabits/haxo-rs). See "Packaging haxo-rs".

## Outputs: IEM on the jack and Bluetooth for the crowd, at the same time

The jack feeds the player's in-ear monitors; Bluetooth feeds a speaker for
the audience. Both play at once (a monitor/front-of-house split, so the
Bluetooth delay doesn't matter to the player).

Plan: one synth, rendered once by haxo-rs's own ALSA loop on the I2S clock
via `fluid_synth_process()`. The IEM buffers go straight to ALSA
`hw:MAX98357A`. The crowd buffers go into a ring buffer (written without
blocking; drop on overflow), and a PipeWire stream reads them through a
`rubato` adaptive resampler, with a control loop holding the ring buffer near
half full to absorb I2S/BT clock drift (the zita-ajbridge / `alsa_out`
pattern). Use a cheap interpolation mode: the BT codec is lossy anyway.

Why: one soundfont load, one render pass (CPU, not RAM, is the scarce
resource with a big soundfont plus reverb on the A53), and the IEM thread
never waits on Bluetooth. Different mixes per output, if wanted later, come
from `synth.audio-groups` / `synth.effects-groups`: each note on two MIDI
channels, both reading the same samples, at twice the voice cost (use
per-channel mono/legato mode instead of global polyphony 1).

Rejected:
- Two synth instances: double render CPU, and double sample RAM.
- fluidsynth's JACK driver with `audio.jack.multi=yes` on PipeWire's JACK
  layer, linking each group to a different sink: no code, but the IEM path
  shares a graph with the BT sink. Kept as the bench test for the BT chain.

Build order:
1. IEM only: upstream haxo-rs plus dynamic sample loading, FluidR3 unload,
   idle mode.
2. Prove the BT chain (codec, receiver, latency) with the JACK-multi setup.
3. Replace it with the own-loop bridge.
4. Second audio group only when different mixes are wanted.

Soundfont memory: `synth.dynamic-sample-loading` (in nixpkgs fluidsynth
2.5.6) loads a preset's samples only when a channel selects it, so RAM tracks
the active preset, not the file. Preset switches read from the SD card, so
switch between songs. SF3 saves disk only; samples are decoded to 16-bit on
load. nixpkgs' fluidsynth has no PipeWire driver (JACK and PulseAudio only),
which the own-loop design doesn't need.

## TODO: web UI (listen to the mix, change settings)

Spike (2026-10-13): `webui/index.html`, frontend only with an in-page mock;
the API it needs is in docs/webui.md.

A web server on the board, reachable over the USB gadget and Wi-Fi
(haxophone.local), with:
- A page that plays the device's audio mix live in the browser. Take it
  from the own render loop as one more tap, like the Bluetooth bridge (see
  Outputs): a ring buffer written without blocking, so a slow or vanished
  listener can never stall the IEM path. Latency matters less here than
  for the player (listening, recording a take), so encoding (e.g. Opus
  over WebRTC or WebSocket) is acceptable if the CPU allows it next to the
  synth.
- Settings: Wi-Fi networks, plus the instrument settings the key combos
  reach today (program/soundfont, transpose, volume) and later ones (output
  levels, backing tracks).

Open questions:
- Wi-Fi networks come from the settings file on the card (see "Settings
  file"). The web UI could edit that same file, which keeps one source of
  truth.
- Auth: anyone on the same Wi-Fi could reach it. At least a password, and
  maybe settings only over the USB link.
- CPU cost of the server and the encoder, measured against the synth's
  headroom (see the measurement rule under "Upstream issues").

## HAT audio hardware (from haxo-hw `haxophone001.kicad_sch` netlist)

- J5 "Line Out" (TRS): tip and ring both on MAX98357 OUTP (through ferrite
  FB1, 220 pF C4 to GND); **sleeve on OUTN, not ground**. Bridge-tied
  output, both legs biased at about VDD/2.
- So a grounded input (mixer, powered amp) is only safe while the haxophone
  is otherwise floating. If its ground also reaches the amp another way
  (USB to a laptop on the same rig, or a shared supply), OUTN is shorted to
  ground. Passive wired IEMs are fine.
- Gain jumper JP1 on GAIN_SLOT: to GND 12 dB, to +5V 6 dB, open 9 dB. Full
  scale is far too loud for sensitive IEMs; cap the level in software.
- SD_MODE is on GPIO4 (`CODEC_ENABLE`), and upstream uses
  `dtoverlay=max98357a,sdmode-pin=4`, so the kernel codec driver turns the
  amp on and off with the PCM stream. Closing the stream when idle is enough.
- Upstream provisioning also sets `disable-wifi` and `disable-bt`. We run
  both radios, so listen for radio noise on the analog output and watch for
  audio glitches.

## Stock image (as found on the Samsung PRO Endurance 128 GB, 2026-10-12)

Raspbian 11 (bullseye), kernel 6.1.21, `haxo001` built 2023-12-21. Useful as
the reference for what the hardware expects:
- `config.txt`: `dtparam=i2c_arm=on`, `i2c_arm_baudrate=400000`,
  `dtoverlay=max98357a,sdmode-pin=4`, `dtoverlay=dwc2`, `enable_uart=1`,
  `disable-wifi`, `disable-bt`, `vc4-kms-v3d`, `disable_overscan=1`.
- `cmdline.txt`: `boot=overlay` (read-only overlayfs root),
  `console=serial0,115200 console=tty1`.
- `haxo.service`: root, group audio, `Type=idle`, `Restart=always`,
  `ExecStartPre=create_midi_gadget.sh`, then `haxo001 --notemap-file
  /usr/share/haxo/notemap.json --prog-number 66 --sf2-file
  /usr/share/sounds/sf2/TimGM6mb.sf2` (6 MB GM soundfont; program 66 is
  GM tenor sax, zero-based).
- `/usr/share/sounds/sf2` also holds FluidR3_GM (148 MB), Tenor_Sax (47 KB)
  and Mister_Tenor_Sax (8.7 MB, added by us).
- 1.6 GB of on-device Rust toolchain under /home/pi; rootfs 96% full.
- This card fails every data read in the Realtek 0bda:0306 USB3 reader on
  bagel ("Hardware Error" sense) but reads fine in an SMI 090c:3350 (USB 2).

## SD card: Samsung PRO Endurance 128 GB (U3, V30)

Tested 2026-10-12 on bagel through a SanDisk SDDR-C531 (USB 3,
usb-storage, so no command queueing; QD16 matches QD1). The card was wiped
in the process.
- `f3probe --destructive`: genuine, the full 128 GB (119.38 GiB) is real.
- fio, `direct=1`, 30 s per test over an 8 GiB region:

| Test | Result |
|---|---|
| Sequential read, 1 MiB | 53.7 MB/s |
| Sequential write, 1 MiB | 49.9 MB/s |
| Random read, 4K QD1 | 2668 IOPS (0.37 ms mean) |
| Random write, 4K QD1 | 1871 IOPS (0.53 ms mean, p99 3.5 ms) |

Random write sits just under the A2 class floor (2000 IOPS), far above A1
(500), and several times a typical non-endurance card; that is the number
that sets deploy speed. Sequential is capped by the reader's bus mode and doesn't
matter: the Zero 2 W's SD host runs well below 50 MB/s anyway.

## haxo-rs as it is upstream

- Embeds fluidsynth as a library: `audio.driver=alsa`, 3 x 64-frame periods,
  RT prio 99, opens `hw:N` of the card named `MAX98357A` directly. There is
  no separate fluidsynth service.
- MIDI out (feature `midi`) goes through midir to ALSA sequencer ports; the
  f_midi gadget appears as one, so USB MIDI needs no PipeWire bridging. With
  more than one seq output port it picks by index, so other seq clients
  (e.g. PipeWire's bridge) change its choice.
- Loads FluidR3_GM.sf2 (~140 MB) for the startup jingle and never unloads it.
- Main loop is a fixed 500 Hz tick (`TICK_USECS = 2000`): key matrix scan
  (8 rows, 10 us settle sleeps each) plus an I2C pressure read, forever,
  whether or not anyone is playing. The audio driver also runs continuously.
- Feature `instrumentation` toggles UART RXD, so it conflicts with a UART
  console.

## Packaging haxo-rs

`packages/haxo-rs`, built with crane from the source-only `haxo-rs` flake
input, which is our fork `github:schlarpc/haxo-rs` (clone in
`~/src/haxo-rs`, with cardonabits as the `upstream` remote). Our changes are
commits on the fork's `main`; `nix flake update haxo-rs` picks them up. To
test unpushed work: `--override-input haxo-rs git+file:///home/schlarpc/src/haxo-rs`.
Pull upstream fixes by merging or cherry-picking from `upstream/main`.
crane vendors from the fork's Cargo.lock at eval time, so there is no
cargoHash and no import-from-derivation. Built with `--features midi`.

Until 2026-10-13 the changes were `git format-patch` files in
`packages/haxo-rs/patches` plus a replacement Cargo.lock; they are now the
fork's first commits (cc0c336 through 9e06de6, source tree verified
identical to the old patched source).

## Upstream issues, diagnosed (2026-10-13)

Read from the code and the issue threads; nothing here is tested on the
board yet. Nothing was posted upstream.

These are hypotheses until proven. Each fix must come with: the failure
reproduced on the board first (if it can't be reproduced, the diagnosis is
wrong), a measurement before and after, and the numbers recorded here.
Ways to measure each:
- #24: fluidsynth's reported period/buffer sizes, plus key-to-sound
  latency measured end to end (e.g. the `instrumentation` noteon pin, or a
  key-click mic and the line out on two scope or audio-interface channels).
  Compare against plain `hw:` and count xruns while a track plays under
  load.
- #29: Ekolide's `stereo.sf2` (programs 3, 5, 6, 7) on the board, plus
  `fluid_synth_get_active_voice_count` per note, before and after.
- #41: message rate and queue depth on the MIDI port while breathing, and
  note-on latency over BLE with fast passages, before and after the cap.
- #26: `counting.sf2` from the issue, stepping past its last preset.
- Main loop: tick jitter (e.g. the `busy` instrumentation pin, or logged
  tick intervals) under Wi-Fi and USB load, before and after.

- **#24, backing tracks: dmix latency is dmix's defaults.** Both period
  sizes in the thread match ALSA's dmix defaults exactly. `plug:dmix` gave
  940 frames: dmix defaults to `rate 48000`, `period_size 1024`, and 1024
  frames at 48 kHz is 940 at fluidsynth's 44.1 kHz. The hand-written
  `dmix0` set the rate but no period, so dmix used its 125 ms period time
  (44100 x 0.125 = 5513). dmix sets the hardware period, so fluidsynth's
  64-frame request is ignored. Setting `period_size 64`, `buffer_size 192`,
  `rate 48000` in the dmix slave (and `synth.sample-rate` 48000 in
  fluidsynth) should restore the latency; the card already takes 64-frame
  periods from haxo directly. Untested catch: with
  `defaults.pcm.dmix.max_periods 0` every dmix client gets the same tiny
  buffer, so a non-realtime `aplay` would drop out under load (only the
  track, never the sax). Fine as an experiment; the real fix is mixing the
  track into our own render loop (see Outputs), decoded into a large ring
  buffer by a normal thread.
- **#29, stereo: most symptoms are `set_polyphony(1)`.** The silent right
  channel is the hardware: SD_MODE is driven straight from GPIO4 at 3.3 V
  with no resistor, which puts the MAX98357A in left-only mode. Summing in
  software adds no buffering (a `plug` with `ttable { 0.0 0.5 1.0 0.5 }`,
  or `L = (L+R)/2` in our own loop). But "program 3 randomly picks one
  sound" and "programs 6/7 mostly silent" come from `synth.rs` setting
  polyphony to 1 for monophony: a layered preset or a stereo sample pair
  needs two voices, fluidsynth keeps one, and when it keeps the right-hand
  voice nothing is heard. `main.rs` already sends note-off before every
  note-on, so the limit only does harm. Fix: polyphony 8-16 plus
  fluidsynth's per-channel mono/legato mode (`fluid_synth_set_basic_channel`,
  not wrapped by the rust-fluidsynth binding, so declare it `extern`).
- **#41, BT MIDI: the latency is a backlog.** `main.rs` sends CC#2 (breath)
  on every pressure change at the 500 Hz tick, up to ~500 messages/s, while
  a BLE link sends a packet every 15-50 ms. The queue grows without bound,
  so latency climbs to seconds and worsens with faster playing, exactly as
  reported; "publish only the most recent message" helped because it
  dropped stale messages. Fix: cap breath CC at ~100 Hz (or send only on a
  change of a few counts), never drop note on/off; then expect roughly
  10-20 ms. BlueZ's MIDI plugin is central-only (it connects out to BLE MIDI
  devices); for phones and DAWs to connect to the Haxophone it needs its own
  BLE MIDI GATT service, e.g. with the `bluer` crate.
- **#26, program change falls through to the default soundfont.** FluidR3
  is loaded first for the jingle and never unloaded; presets missing from
  the user's soundfont resolve to it. Unloading it (`sfunload` is wrapped)
  fixes this; stepping through the presets that exist, instead of clamping
  to 0-127, fixes the rest.
- **Main loop scheduling.** The loop runs at normal priority, so Wi-Fi or
  USB load can delay note onsets; `beep()` and `change_program()` also
  sleep 100 ms inside it. Nothing calls `mlockall` (the Memory decision
  above is a plan, not current behavior).

## Patches to make

- Own render loop with the BT bridge (see Outputs). PipeWire must never
  open the I2S card. Sum L+R to the left channel there, and mix backing
  tracks in it (#24, #29). Waits for the Bluetooth audio bench test
  (build order step 2).
- Per-channel mono/legato mode (`fluid_synth_set_basic_channel`), if
  legato phrasing is wanted: polyphony is fixed (8a9ae25), and the main
  loop's note-off before every note-on already keeps the sax monophonic.
- Idle mode on in the service (`--idle-secs`), once waking up has been
  tried on the board: see f65a60f below.
- BLE MIDI from a phone or a DAW, and pairing: see 193ea18 below. The
  board isn't pairable; a central that insists on pairing (bagel's
  BlueZ+PipeWire did, for its own MIDI service) gets disconnected. Decide
  whether to allow "just works" pairing.
- Play through 85984f9's median filter and check that breath onsets
  don't feel late (2 ms of lag by design).

Done (commits on the fork; cc0c336 bumps `time`, which no longer
compiled):
- 778d4c2: rppal 0.13.1 -> 0.22.1. 0.13.1 identifies the board only by the rpi
  kernel's compatible (`raspberrypi,model-zero-2`) and failed with
  `UnknownModel` on mainline (`raspberrypi,model-zero-2-w`, matched from
  rppal 0.16.0). No API changes needed.
- c6d1c84: the pressure baseline was one startup sample. Resting noise is about
  +/-8 counts (raw readings 2-19 around ~10), so a low first sample made
  the main loop retrigger the note over 100 times a second. Now the
  baseline averages 32 samples, and +/-10 counts around it count as no
  breath (both sides, so the negative-pressure gesture still works).
- 5a84d46: key scan through `/dev/gpiochip0` (gpiocdev), one line request for
  the life of the process. Replaces rppal's register mapping, which needed
  `iomem=relaxed` on mainline and was redone 500 times a second. haxo went
  from about 62% to about 25% of one core at rest.
- 9e06de6: poll up to 30 s for the sound card at startup instead of panicking
  while the I2S card probes.
- edba249: `RUST_LOG=debug` made `Pressure::init` fail (reproduced: panic,
  `NotFound`): it evaluated `i2c.clock_speed()?` inside a `debug!`, and
  rppal reads that from a sysfs file the mainline I2C driver doesn't have.
  The clock speed is now logged only when it can be read.
- a1d21ea: breath CC#2 at most every 10 ms, always the latest value, and a
  held-back value flushed before each note on/off (#41). Measured
  2026-10-14 on bagel, reading the gadget's rawmidi device (`midiC4D0`)
  with host timestamps, about 40 s of playing each: before, 196 msgs/s
  while breathing, median gap 2.2 ms, peaks of 42 per 100 ms; after,
  79 msgs/s, median gap 11.8 ms, 13 of 1240 gaps under 10 ms that aren't
  next to a note (USB batching). The playing differed between runs (14 vs
  187 note events). BLE latency itself waits for Bluetooth MIDI. The same
  commit makes MIDI out optional: with no port, haxo warns and keeps
  playing (not reproduced on the board: `snd_seq_dummy` always provides
  a port).
- 8a9ae25: polyphony stays at 16 instead of 1 (#29). Measured 2026-10-14,
  GeneralUser GS program 66, about 40 s of playing each: with polyphony 1,
  64 "Failed to allocate a synthesis process" per 81 note-ons; with 16,
  none per 26 note-ons. Every note started at least 2 voices (a stereo
  pair; fluidsynth had kept only one), at most 10 with release tails
  overlapping. By ear: "much richer".
- 6bf810a: the startup soundfont is unloaded after the jingle, once the user's soundfont
  is loaded, then `malloc_trim(0)` (#26). On this host the startup path is
  a tmpfiles link to the same GeneralUser GS file, so this dropped a second
  copy of it, not FluidR3. haxo's RSS went from 103 MB to
  57 MB. Unloading before the load reached 60 MB but left every channel
  without a preset for a moment (16 "No preset found" warnings); unloading
  after it without the trim stayed at 73 MB, freed pages stuck in the heap.
- 1759daf: tick timing in the log (`RUST_LOG=info,ticks=debug`): every
  5000 ticks, the longest interval, how many ran over 1.5/2.5/5 periods,
  and the longest tick's work.
- a46fac9: the main loop sleeps to its own deadlines (schedule_recv's
  timer thread is gone) at SCHED_FIFO 80, below fluidsynth's 99, with
  `mlockall`. Measured 2026-10-14 with 1759daf's log: 20 s idle, then 30 s
  of a Wi-Fi ssh upload, three busy loops and fsync'd 20 MB writes to the
  card. Before: idle max interval 2.4-6.1 ms; loaded 91-111 ms, 469-942
  ticks per 10 s over 3 ms, 75-179 over 10 ms, the delay mostly within a
  tick (max busy about equal to max interval: preempted around the key
  scan's sleeps). After: idle 2.0 ms; loaded 2.6-7.3 ms, 0-3 ticks over
  3 ms, none over 10 ms.
- 99a2cfb: feedback beeps are timed note on/off events that the loop plays
  as they come due (`cues.rs`), instead of `beep()` sleeping 100 ms and
  the mode switches, program change and transpose sleeping up to 300 ms
  inside the loop. Same timing as before. Tested on bagel against
  GeneralUser GS with no audio driver (`HAXO_TEST_SF2=... cargo test
  cues`); the mode switches themselves need someone at the keys.
- 4302285: `--startup-sf2-file` and `--startup-midi-file` (empty: no
  jingle) replace the /usr/share paths; the jingle wait polls every 10 ms
  instead of spinning a core. `shutdown()` (sudo + halt) is never called,
  so those paths needed nothing. The host's tmpfiles links under /usr are
  gone.
- 102241d: Control mode's program change moves to the nearest program the
  soundfont has in bank 0 (fluidsynth 2's preset iteration, declared
  `extern`: the binding only has fluidsynth 1's structs). A full GM bank
  steps as before; GeneralUser GS lists 128 (and 13 kits in bank 128).
- bf1ddeb: MIDI out goes to the rawmidi device `--midi-out`
  (`hw:CARD=haxophone`), opened non-blocking, a message that doesn't fit
  dropped whole; the sequencer port pick stays as the fallback. All Notes
  Off at startup, seen on bagel's `midiC4D0`. While the sequencer bridge
  holds the gadget's single rawmidi substream, `amidi` gets "busy".
- e64084c, f65a60f: `--idle-secs N` closes the audio driver after N s
  without key changes or breath (pressure on two ticks in a row: the
  sensor spikes for single ticks at rest), and slows the tick to 20 ms;
  the next key change or breath reopens it. Measured 2026-10-14 with
  `RUST_LOG=idle=debug` at rest: 0 key changes, 13-17 single-tick
  pressure spikes per 10 s, which kept the first version awake. While
  awake haxo uses about 20% of a core and SD_MODE (GPIO4) is high.
  With 85984f9's median filter and `--idle-secs 15` at rest (2026-10-14):
  idle after 15 s, SD_MODE low (amp off), haxo from about 20% of a core
  to 1%. Waking (a key change or breath, then the audio driver reopening)
  needs someone at the instrument, so the service doesn't pass
  `--idle-secs` yet; the log says how long the reopen took.
- 193ea18: Bluetooth LE MIDI (`ble` feature, `--ble-midi NAME`): the BLE
  MIDI service and characteristic through BlueZ (bluer), advertised as
  NAME. Messages reach a single-threaded tokio thread through a bounded
  queue without blocking the loop; it sends one packet at most every
  10 ms, collapsing breath values in a row to the latest, so the stack's
  queue can't build up (#41). A new subscriber gets All Notes Off.
  Tested 2026-10-14 from bagel: BlueZ there connected (MTU 512) and its
  MIDI plugin made an ALSA sequencer port, "haxophone Bluetooth", that
  `aseqdump` read haxo's messages from; haxo logged the subscribe and,
  on disconnect, the broken pipe. The first tries dropped the link: the
  board's own BlueZ MIDI plugin, as a client, found the BLE MIDI service
  of bagel's PipeWire, read it, got "Insufficient Encryption" and asked
  to pair, which the board refuses (btmon). Both MIDI pieces on the board
  are off now (bluetooth.nix), and haxo restarts with bluetoothd, which
  drops its registration. Not tried from a phone yet.
- 85984f9: the pressure reading is the median of the last three samples.
  Found through BLE: at rest, haxo sent the open-key note (59) at velocity
  127 with breath blips, 64 note-ons a minute over USB MIDI. Sampling
  the sensor at 500 Hz from a second reader: 14 spikes in 20 s while haxo
  runs, every one a single sample, and none with haxo stopped, amp on or
  off. Reading the sensor before the key scan didn't help (A/B, 55 s at
  rest each: 21 and 22 note-ons in 40 s); the median: 0, and no pressure
  ticks at all. One sample (2 ms) of lag on a breath onset.

## USB gadget (nixos/gadget.nix)

configfs composite: NCM (usb0, same fixed MACs and 10.68.0.1 DHCP setup),
ACM (ttyGS0 getty), MIDI (`f_midi`, ALSA id `haxophone`; hosts see a USB
MIDI 1.0 device) and MTP (umtprd over FunctionFS, storage "Files" at
/var/lib/haxophone/files). umtprd is `Type=notify` and signals after
writing its descriptors; `usb-gadget-bind` then binds the UDC and is PartOf
umtprd, so restarting umtprd rebinds (tested: under a second, host
re-enumerates). If the bind fails, `usb-gadget-fallback` tears the gadget
down and loads `g_cdc`, keeping network and serial. haxo opens the
gadget's rawmidi device (`hw:CARD=haxophone`) at startup and is PartOf the
bind unit, since a rebind recreates that device. FunctionFS is mounted `no_disconnect=1`: while umtprd is down
only MTP stalls, but when it reopens ep0 the kernel unbinds the whole
gadget anyway (`ffs_closed` -> `unregister_gadget_item`), so a successful
umtprd restart costs one re-enumeration. The gadget units are
`restartIfChanged = false`; gadget changes take effect at the next boot,
because a live rebind cuts the link deploy-rs is using.

## Wi-Fi (nixos/wifi.nix)

iwd associates, networkd runs DHCP on wlan0, Avahi publishes
haxophone.local. The networks come from the settings file on the card (next
section). iwd needs `brcmfmac feature_disable=0x82000` on this chip: with the
firmware's SAE and 4-way-handshake offload (FWSUP) enabled it stays in
"connecting" forever (verified). wpa_supplicant, which Raspberry Pi OS uses,
handles that offload path, so it would likely work without the option
(untested); we keep iwd, and the host doing the handshake costs nothing.
Deploys can go over Wi-Fi (`deploy --hostname <wlan0 address>`), which keeps
large copies off the stall-prone USB link.

## Bluetooth (nixos/bluetooth.nix, 2026-10-13)

Step 2 of the Outputs build order, configured but not yet run on the board:
BlueZ, plus PipeWire with its Bluetooth audio support, so a speaker can be
paired and the chain measured. Nothing in haxo-rs feeds it audio yet.
haxo-rs serves Bluetooth MIDI itself (`--ble-midi haxophone`, 193ea18), so
BlueZ's MIDI plugin is disabled, and so is PipeWire's BLE MIDI server
(`monitor.bluez-midi`); see "Patches to make" for why.

- **Firmware**: the controller's patch RAM files `BCM43430A1.hcd` (our
  chip) and `BCM43430B0.hcd` (the other Zero 2 W revision), from
  nixos-hardware's Raspberry Pi wireless firmware package (RPi-Distro's
  bluez-firmware). linux-firmware doesn't carry them; without one the chip
  runs its ROM firmware. Only these two files: the Wi-Fi blobs stay
  linux-firmware's, which are the ones verified on the board.
- **PipeWire** runs in the haxophone user's session, with linger so it
  starts at boot, and wireplumber started with the session instead of on a
  client's first connection. Chosen over `services.pipewire.systemWide`,
  which upstream discourages; the cost is four wireplumber settings:
  - `monitor.alsa` disabled: PipeWire must never open the I2S card.
  - `monitor.alsa-midi` disabled: its sequencer ports would change which
    port haxo-rs picks for MIDI out when it falls back to the sequencer.
  - `monitor.bluez-midi` disabled: a second BLE MIDI service next to
    haxo-rs's.
  - `monitor.bluez.seat-monitoring` disabled: headless, there is never an
    active seat, and wireplumber turns Bluetooth off for inactive ones.
- Codecs in nixpkgs' PipeWire 1.6.6 include SBC, AAC, aptX (with aptX LL),
  FastStream, LDAC and Opus. The receiver decides what is usable.

Bench test, to run on the board (record the numbers here):
1. `bluetoothctl`: `scan on`, `pair`, `trust`, `connect` the receiver.
2. `wpctl status` shows it as a sink; `wpctl inspect` on it shows the
   codec in use.
3. Play: `pw-jack fluidsynth -a jack -m alsa_seq <soundfont>`, notes in
   from `aconnect` or a MIDI file. Measure latency (key click vs. receiver
   output) per codec, and CPU next to haxo running.
4. Listen for radio noise on the line out with Wi-Fi and Bluetooth both
   busy (upstream disables both radios).

## Settings file (nixos/config-file.nix, packages/haxophone-config)

Decided 2026-10-13: nothing about who uses the instrument is in the flake.
The ssh keys and Wi-Fi networks live in `config/haxophone.toml` on the FAT
boot partition, editable from any computer with the card in a reader, so a
new network (a venue, a phone hotspot) or a new laptop needs no rebuild.
Before this, the Wi-Fi network was an agenix secret and the keys came from
github.com/schlarpc.keys; both inputs and agenix are gone.

- TOML, parsed by a small Python tool (`tomllib`, stdlib). Python was
  already in the closure. Errors come with line numbers.
- `haxophone-config apply` runs as a oneshot at every boot, before iwd and
  sshd. It writes iwd's network files into /var/lib/iwd and the keys to
  /run/haxophone-config/authorized_keys/haxophone, which sshd reads through
  `AuthorizedKeysFile` (`%u`, so the keys can't log in as root). Both are
  on tmpfs, so the file is the whole lifecycle. A bad entry is skipped with
  a message; a file that doesn't parse applies nothing.
- When the file is missing, the board writes a commented template.
  mint-sd writes the template with `--ssh-keys` filled in, or a given
  `--config`, and checks it before wiping the card.
- With no usable key, the consoles' autologin is the way in, which is why
  `users.allowNoPasswordLogin` is set: NixOS sees no password and no key in
  the config and would refuse to build.
- The file is plain text on the card, PSKs included, which is no worse
  than before: the card holds everything else unencrypted too.

## USB gadget endpoint budget

The dwc2 core logs `EPs: 8, dedicated fifos, 4080 entries in SPRAM`: ep0
plus 7 endpoint numbers, each able to carry a separate IN and OUT endpoint.
With dedicated FIFOs every IN endpoint needs its own TX FIFO, and the dwc2
overlay defines 7 (`g-tx-fifo-size = <512 512 512 512 512 256 256>`). So the
limit is 7 IN + 7 OUT.

| Function | IN | OUT |
|---|---|---|
| NCM | 2 | 1 |
| MIDI | 1 | 1 |
| MTP (umtprd) | 2 | 1 |
| ACM serial | 2 | 1 |
| All four | 7 | 4 |

Verified on the board (2026-10-12, mainline 6.18): `hw_params` shows
`num_dev_ep 7`, `dev_ep_dirs 0x0` (every endpoint number bidirectional) and
`en_multiple_tx_fifo 1`; `params` shows seven TX FIFOs (256 256 512 512 512
768 768 words) out of 4080 total. So 7 IN + 7 OUT holds, and all four
functions fit.

### TODO: USB audio out (UAC2), to the host as a sound card

All 7 IN endpoints are taken, so this means dropping a function. ACM is the
candidate (frees 2 IN, 1 OUT): the mini-UART console and SSH over NCM
cover it.

The configfs `uac2` function works on the stock kernel even though nixpkgs
leaves `CONFIG_USB_CONFIGFS_F_UAC2` unset: `usb_f_uac2.ko` is built for the
legacy `g_audio`, and configfs finds it by name. Verified 2026-10-13: modprobe
plus `mkdir functions/uac2.t` in a scratch, unbound gadget shows the full
attribute set. No kernel rebuild needed.

Device-to-host only costs 1 IN endpoint, but only if the defaults are
overridden. In f_uac2, `p_` is the gadget's playback (IN, what the host
records) and `c_` is host-to-gadget (OUT). The defaults enable both sides,
mute and volume controls (an interrupt IN), and async feedback (another
IN), so 3 IN. Set:
`p_chmask=3 c_chmask=0 p_mute_present=0 p_volume_present=0 p_srate=48000`.

On the Pi it appears as a second ALSA playback card. Feed it from the own
render loop as one more tap, with the same non-blocking ring buffer and
drift correction as the Bluetooth bridge: the host's USB clock is not the
I2S clock (u_audio also offers a "Playback Pitch" control to trim the rate).
This turns the haxophone into a USB audio + MIDI interface for a DAW.
Measure latency and CPU before calling it done.

## Gadget decisions

- **Ethernet: NCM.** Built-in drivers on Linux, macOS and Windows 10 2004+,
  and it batches frames into fewer USB transfers. Fixed `host_addr`/`dev_addr`
  so Windows doesn't create a new adapter each boot. IPv6 + IPv4 link-local
  with mDNS; no DHCP server.
- **MTP: umtprd** over cmtp-responder. cmtp-responder hardcodes its single
  store to `/media/card` at compile time and hasn't had a commit since
  2024-02; umtprd has runtime config, multiple stores, `sync_when_close`,
  inotify events, and active maintenance. Neither is in nixpkgs. Borrow
  cmtp's systemd unit layout for the FunctionFS mount and bind ordering.
