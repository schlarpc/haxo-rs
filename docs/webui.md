# Web UI

`webui/index.html` is a frontend spike (2026-10-13): the whole UI, with
every call answered by an in-page mock (`createMockApi()`). Nothing is
served by the board yet. This file is the contract the mock stands in for,
so the backend can be written against it. Design questions it raised are at
the end.

The page is one self-contained file: no build step, no CDN at runtime (B612
from Google Fonts when reachable, system faces otherwise), because the
board is often on a venue hotspot with no internet. It has no `<!doctype>`
or `<html>`/`<body>` of its own so it can also be previewed as a claude.ai
Artifact; the board's server should wrap it in a standard HTML5 skeleton
(charset and viewport meta) when serving.

## Sections

- **Play**: the board drawn from haxo-hw's PCB, keys lighting as they are
  pressed; the note haxo-rs's notemap gives for that fingering; breath as
  the CC2 value; the audio tap with a scope and level meter. The demo plays
  a lick with real notemap fingerings; "My fingers" lets you press keys on
  the drawing (and set breath) to look up any fingering.
- **Wi-Fi**: current connection, a scan with signal and security, joining
  with a passphrase and optionally saving it, saved networks with Forget.
  802.1X networks are listed but marked unsupported.
- **Bluetooth**: adapter state, paired speakers with connect/disconnect/
  remove and the codec in use, a scan, pairing. Low-latency codecs are
  highlighted.
- **Sounds**: soundfont files (upload by picker or drag-and-drop, delete
  with an inline confirm, storage used), and the instrument settings:
  active soundfont, preset, transpose, volume.

## Board drawing

`webui/tools/gen-geometry.py <haxo-hw>/haxophone001.kicad_pcb` regenerates
the `GEOMETRY` block in the page: the main panel's Edge.Cuts outline and
each keyswitch's position, with its matrix bit traced from the switch
through its diode to the ROW/COL nets (bit = row * 3 + col, haxo-rs
keyscan's numbering). The side panels snap off the PCB and mount at right
angles, so they are clipped from the outline and their keys drawn edge-on
beside the main panel, positioned from haxo-hw's photos; the octave key is
on the back and drawn dashed beside it. Key names come from haxo-rs's
notemap fingerings, keycap colours from the photos (`KEYS` in the page).

A 3D view could come later from haxo-hw's STEP/STL models (`models/`).

## API

JSON over HTTP on the board, same origin as the page. Writes that change
the settings file (`config/haxophone.toml` on the FAT partition) go through
`haxophone-config`, so the file stays the single source of truth that a card
reader can also edit.

### Live state: `WS /api/live`

Server pushes, at most ~60 Hz (coalesced, latest wins; never queued):

```json
{ "keys": 9344, "breath": 87, "note": 67, "program": 66, "transpose": 0 }
```

- `keys`: the scanned bitmask (bit = row * 3 + col).
- `breath`: 0-127, the CC2 value haxo-rs computes.
- `note`: MIDI note sounding, or `null` (no breath, or no fingering).

haxo-rs has to publish this; today nothing leaves the process. A small
shared-memory or Unix-socket feed from its main loop, written without
blocking, keeps the 500 Hz loop safe from a slow server.

### Audio tap: `WS /api/audio`

Binary frames of interleaved PCM from the own render loop's tap (see
docs/haxophone.md "Outputs"): the same non-blocking ring buffer as the
Bluetooth bridge, so a slow or vanished listener drops audio and never
stalls the in-ear path. First message is JSON
`{ "rate": 48000, "channels": 1, "format": "s16le" }`. The page plays it
through an AudioWorklet with a ~100 ms jitter buffer. Opus instead of PCM if
the Wi-Fi link needs it and the CPU allows (measure next to the synth).

### Wi-Fi

| Call | Body / result |
|---|---|
| `GET /api/wifi` | `{ current: {ssid, signal, address, security} \| null, saved: [{ssid}] }` |
| `POST /api/wifi/scan` | `[{ ssid, signal, security }]` (iwd `Station.Scan`, then the ordered networks) |
| `POST /api/wifi/connect` | `{ ssid, psk?, save }` → `current`, or `400 { error }` with a human message |
| `DELETE /api/wifi/saved/{ssid}` | removes it from the settings file |

`save: true` adds a `[[wifi]]` block to the settings file and reruns
`haxophone-config apply`; without it the network is joined for this boot
only (iwd's runtime state is on tmpfs anyway).

### Bluetooth

| Call | Body / result |
|---|---|
| `GET /api/bluetooth` | `{ adapter: {powered, name, address, discoverable}, paired: [{address, name, connected, codec}] }` |
| `POST /api/bluetooth/scan` | `[{ address, name, codecs }]`, ~10 s discovery |
| `POST /api/bluetooth/{addr}/pair` | pairs, trusts, connects |
| `POST /api/bluetooth/{addr}/connect`, `/disconnect` | |
| `DELETE /api/bluetooth/{addr}` | removes the pairing |
| `PATCH /api/bluetooth` | `{ discoverable }` (times out after 3 min) |

BlueZ over D-Bus; the codec comes from PipeWire (`wpctl inspect` on the
sink). The codecs a device offers are only known after connecting, so the
scan's `codecs` may need to come from the device class or be dropped.

### Sounds

| Call | Body / result |
|---|---|
| `GET /api/soundfonts` | `{ files: [{name, size, presets, builtin}], active, preset, transpose, volume, storage: {used, total} }` |
| `PUT /api/soundfonts/{name}` | raw file body, streamed to disk; `.sf2`/`.sf3` only |
| `DELETE /api/soundfonts/{name}` | not for `builtin` (store) files |
| `PATCH /api/instrument` | any of `{ active, preset, transpose, volume }` |

Files live on /persist (the MTP storage, `/var/lib/haxophone/files`, so a
computer over USB sees the same folder). Preset names come from parsing
the soundfont's `phdr` chunk.

## Open questions

- **Auth.** Anyone on the same Wi-Fi can reach the board. At least a
  password (kept in the settings file); maybe settings writes only over the
  USB link.
- **Server.** Something small and memory-light next to the synth: a Rust
  binary (axum) or a Python one (the settings tool is already Python). Its
  CPU under the audio tap has to be measured against the synth's headroom.
- **haxo-rs hooks.** Live state out, audio tap, and instrument changes in
  (program, transpose, volume, soundfont) all need new interfaces in
  haxo-rs; the key combos already change program and transpose, so the two
  must agree.
- **Wi-Fi from the page itself.** Joining a network over Wi-Fi drops the
  page's own connection; the USB link (10.68.0.1) is the reliable place
  for that.
