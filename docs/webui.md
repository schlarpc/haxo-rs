# Web UI

The board serves a web UI on port 80: http://10.68.0.1 over the USB cable,
http://haxophone.local over Wi-Fi. It shows the instrument live and
changes Wi-Fi, Bluetooth and sounds.

- **Server:** `haxo-web`, a second binary in the haxo-rs workspace
  (`crates/haxo-web` in our fork). Rust: axum, tokio on one thread, zbus
  for D-Bus. Packaged in `packages/haxo-web.nix`, run by `nixos/webui.nix`.
- **Page:** `crates/haxo-web/ui/index.html`, embedded in the binary. It is one
  self-contained file: no build step, no CDN at runtime (B612 from Google
  Fonts when reachable, system faces otherwise), because the board is often
  on a venue hotspot with no internet. It has no `<!doctype>` or
  `<html>`/`<body>` of its own so it can also be previewed as a claude.ai
  Artifact; the server wraps it. Served by the board it uses the real API.
  Anywhere else (a preview, a file) it keeps its in-page mock
  (`createMockApi()`), so the design can still be worked on without a board.
- **haxo001 hooks:** `--control-socket`, a Unix socket with JSON lines.
  The protocol is the `haxo-control` crate, which both binaries use (see
  "Control socket").

Started 2026-10-13 as a frontend spike with a mocked backend; plumbed in
2026-10-14.

## Sections

- **Play**: the board drawn from haxo-hw's PCB. **Board** mode shows the
  instrument's keys lighting as they're pressed, the note haxo-rs's notemap
  gives for that fingering, and breath as the CC2 value. **Demo tune** and
  **My fingers** run in the page, without the instrument, to look up any
  fingering. The audio tap ("Listen in") is not there yet; it needs the
  instrument's own render loop (docs/haxophone.md, "Outputs").
- **Wi-Fi**: current connection, a scan with signal and security, joining
  with a passphrase and optionally saving it, saved networks with Forget.
  802.1X networks are listed but marked unsupported.
- **Bluetooth**: adapter state, paired devices (speakers, and phones or
  computers using the Bluetooth MIDI) with connect/disconnect/remove, a
  scan, pairing.
- **Sounds**: soundfont files (upload by picker or drag-and-drop, delete
  with an inline confirm, storage used), and the instrument settings:
  active soundfont, preset, transpose, volume.

## Access

Anything that reaches the USB link (the usb0 subnet, or IPv6 link-local
scoped to usb0) needs no login: whoever has the cable has the instrument.
Everything else needs the password under `[web]` in the settings file:

```toml
[web]
password = "at least 8 characters"
```

With no password set, Wi-Fi access is refused. A login sets an HttpOnly,
SameSite=Strict session cookie, kept in memory until haxo-web restarts.
Wrong passwords wait a second. It is plain HTTP: the password crosses the
Wi-Fi in the clear (WPA protects it from outsiders, not from others on
the same network).

## Settings

Settings that outlive a boot live in the settings file
(`config/haxophone.toml` on the FAT partition), which a card reader can
also edit. The server never writes it directly: `haxophone-config` does
(`wifi-add`, `wifi-remove`, `set-instrument`, and `show` to read it).
Edits change only the lines they touch, so comments and layout stay, and
the result is checked before it replaces the file. The server serializes
its edits: two concurrent read-modify-writes lost one (seen with preset
and transpose changed back to back).

`[instrument]` (soundfont, program, transpose, volume) is what haxo001
starts with: `haxophone-config apply` writes it to
`/run/haxophone-config/haxo.env`, haxo's unit reads that as its
EnvironmentFile, and its ExecStart takes `${HAXO_SOUNDFONT}` and the rest,
with the old values as the unit's defaults. The soundfont is a file name,
looked up among the built-in soundfonts, then the uploaded ones.

## Control socket

`haxo001 --control-socket /run/haxo/control.sock`. One JSON object per
line, both ways (types in the `haxo-control` crate).

Out, to every client, when it changes, at most every 16 ms (latest wins,
never queued):

```json
{ "keys": 9344, "breath": 87, "note": 67, "program": 66, "transpose": -14,
  "volume": 100, "soundfont": "/nix/store/.../GeneralUser-GS.sf2" }
```

In, any of `{ "program": 0-127, "transpose": -48..48, "volume": 0-100,
"soundfont": "/path" }`. A soundfont loads, then `program` (or the current
one) is selected. The main loop stops for the load, so playing pauses.

The main loop never waits on a client: it publishes with `try_lock` and
takes commands with `try_recv`; each client has its own threads.

## API

JSON over HTTP, same origin as the page. Errors are
`{ "error": "..." }` with a message for the person reading the page.

### Session

| Call | Body / result |
|---|---|
| `GET /api/session` | `{ trusted, authenticated, password_set }` (no login needed) |
| `POST /api/login` | `{ password }` → 204 with the cookie, 401, or 403 when no password is set |

Everything else answers 401 without a session (off the USB link).

### Live state: `WS /api/live`

Text frames with the control socket's state as above, or `null` while
haxo001 isn't running, at most every 16 ms.

### Audio tap: `WS /api/audio` (not yet)

Binary frames of interleaved PCM from the own render loop's tap (see
docs/haxophone.md "Outputs"): the same non-blocking ring buffer as the
Bluetooth bridge, so a slow or vanished listener drops audio and never
stalls the in-ear path. First message is JSON
`{ "rate": 48000, "channels": 1, "format": "s16le" }`. The page plays it
through an AudioWorklet with a ~100 ms jitter buffer. Opus instead of PCM if
the Wi-Fi link needs it and the CPU allows (measure next to the synth).

### Wi-Fi (iwd)

| Call | Body / result |
|---|---|
| `GET /api/wifi` | `{ current: {ssid, signal, address, security} \| null, saved: [{ssid}] }` |
| `POST /api/wifi/scan` | `[{ ssid, signal, security }]`, strongest first |
| `POST /api/wifi/connect` | `{ ssid, psk?, save }` → `current`, or `400 { error }` |
| `DELETE /api/wifi/saved/{ssid}` | removes its `[[wifi]]` block and reapplies |

`signal` is dBm; `security` is `open`, `WPA2/3` (iwd doesn't tell them
apart), `802.1X` or `WEP`. Joining hands the passphrase to iwd through an
agent haxo-web registers. `save: true` first writes a `[[wifi]]` block and
restarts `haxophone-config`; without it the network lasts until the next
reboot. Joining over Wi-Fi can drop the page's own connection; the USB
link is the reliable place for it.

### Bluetooth (BlueZ)

| Call | Body / result |
|---|---|
| `GET /api/bluetooth` | `{ adapter: {powered, name, address, discoverable}, paired: [{address, name, connected, codec, audio}] }` |
| `POST /api/bluetooth/scan` | `[{ address, name, codecs, audio }]`, ~10 s discovery |
| `POST /api/bluetooth/{addr}/pair` | pairs, trusts, connects |
| `POST /api/bluetooth/{addr}/connect`, `/disconnect` | |
| `DELETE /api/bluetooth/{addr}` | removes the pairing |
| `PATCH /api/bluetooth` | `{ discoverable }` (BlueZ turns it off after 3 min) |

`audio` is whether the device has an A2DP sink. `codec` and `codecs` are
null and empty for now: the codec is PipeWire's (`wpctl inspect` on the
sink, in the haxophone user's session), and a device's codecs are only
known after connecting. Pairing uses the agent haxo001 registers for its
Bluetooth MIDI (it accepts).

### Sounds

| Call | Body / result |
|---|---|
| `GET /api/soundfonts` | `{ files: [{name, size, presets, builtin}], active, preset, presets: [{program, name}], transpose, volume, storage: {used, total}, running }` |
| `PUT /api/soundfonts/{name}` | raw file body, streamed to disk, checked as a soundfont; `.sf2`/`.sf3` only, up to 1 GB |
| `DELETE /api/soundfonts/{name}` | not for built-in ones; deleting the one in use switches to the first built-in one, program 66 |
| `PATCH /api/instrument` | any of `{ active, preset, transpose, volume }` → the status |

Files live in `/var/lib/haxophone/files` on /persist (the MTP storage, so
a computer on the USB cable sees the same folder). `presets` is bank 0 of
the active soundfont, parsed from its `phdr` chunk (the sample data is
skipped, so it is quick even on 140 MB files). `PATCH /api/instrument`
goes to haxo001 and into `[instrument]`.

## Board drawing

`crates/haxo-web/tools/gen-geometry.py <haxo-hw>/haxophone001.kicad_pcb`
regenerates the `GEOMETRY` block in the page: the main panel's Edge.Cuts
outline and each keyswitch's position, with its matrix bit traced from the
switch through its diode to the ROW/COL nets (bit = row * 3 + col, haxo-rs
keyscan's numbering). The side panels snap off the PCB and mount at right
angles, so they are clipped from the outline and their keys drawn edge-on
beside the main panel, positioned from haxo-hw's photos; the octave key is
on the back and drawn dashed beside it. Key names come from haxo-rs's
notemap fingerings, keycap colours from the photos (`KEYS` in the page).

A 3D view could come later from haxo-hw's STEP/STL models (`models/`).

## Tested

2026-10-14, on bagel, against a stand-in for haxo001's control socket and
the real `haxophone-config` on a scratch settings file: login (refused
without a cookie, wrong password, right password), live state over the
WebSocket and in the page's Board mode (headless Chromium over CDP),
preset, transpose and volume changes reaching the socket and the settings
file, soundfont switch, upload (and junk refused), delete (and the one in
use falling back), 24 concurrent changes without a lost write. haxo-web's
tests (`cargo test -p haxo-web`, with `HAXO_TEST_SF2` for the soundfont
parser) cover auth, cookies, device paths, upload names and the parser.
On the board (2026-10-14, fork 36e3b93): haxo-web at 4.8 MB RSS and
under 1% of a core, connected to haxo001's control socket at startup;
over USB the Sounds status (GeneralUser GS, program 66, transpose -14,
128 presets), Wi-Fi status (the joined network, its signal and address,
the saved one from the card) and a scan (10 networks); Bluetooth status
and a scan (26 devices, bagel flagged as an audio sink; the first
version read the results after stopping discovery, which drops their
RSSI, and found none). Over Wi-Fi with no `[web]` password: 401. Not
tried yet, to keep the board on the network: joining or forgetting a
Wi-Fi network, pairing a speaker, and the page in a browser on the
board itself.

## Open

- **Codec** in the Bluetooth tab, from PipeWire.
- **Audio tap**, after the own render loop.
- **Privilege**: haxo-web runs as root (settings file on FAT,
  `systemctl restart`, iwd and BlueZ policies). A user with the D-Bus
  policies, a group-writable mount and a polkit rule would do.
- **HTTPS**, if the password on a shared Wi-Fi matters.
