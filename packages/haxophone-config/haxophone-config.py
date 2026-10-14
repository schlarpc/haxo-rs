"""Apply or check the Haxophone's settings file (haxophone.toml).

  haxophone-config apply [--config PATH] [--mount DIR] [--iwd-dir DIR] [--keys-dir DIR]
                         [--haxo-env FILE] [--soundfont-dir DIR]...
  haxophone-config check PATH
  haxophone-config template [--ssh-keys FILE]...
  haxophone-config show [--config PATH]
  haxophone-config wifi-add SSID [--psk-stdin] [--config PATH]
  haxophone-config wifi-remove SSID [--config PATH]
  haxophone-config set-instrument [--soundfont NAME] [--program N]
                                  [--transpose N] [--volume N] [--config PATH]

`apply` runs at boot (nixos/config-file.nix): it writes iwd's network files
and the haxophone user's authorized_keys from the settings file, and puts a
commented template in place when there is none. `check` validates a file
without applying it (mint-sd uses it). `template` prints the template,
with the public keys from each FILE (an authorized_keys or .pub file) filled
in.

`show` prints the parsed settings as JSON, and `wifi-add`, `wifi-remove`
and `set-instrument` edit the file; the web UI uses these. Edits change
only the lines they touch, so comments and layout stay, and the result is
checked before it replaces the file.
"""

import argparse
import json
import os
import re
import shutil
import sys
import tomllib
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parent.parent / "share" / "haxophone.toml"
USER = "haxophone"
KNOWN_KEYS = {"ssh_authorized_keys", "wifi", "instrument", "web"}
INSTRUMENT_KEYS = {"soundfont": str, "program": int, "transpose": int, "volume": int}
INSTRUMENT_RANGES = {"program": (0, 127), "transpose": (-48, 48), "volume": (0, 100)}
KEY_TYPES = re.compile(r"^(ssh-|ecdsa-|sk-ssh-|sk-ecdsa-)\S+ \S+")


class Settings:
    def __init__(self):
        self.keys = []
        self.networks = []  # (ssid, psk or None)
        self.instrument = {}  # soundfont (a file name), program, transpose, volume
        self.web_password = None


def load(path):
    """Parse and validate `path`. Returns (settings, problems)."""
    problems = []
    with open(path, "rb") as f:
        data = tomllib.loads(f.read().decode("utf-8-sig"))

    s = Settings()
    for key in sorted(set(data) - KNOWN_KEYS):
        problems.append(f"unknown setting {key!r}, ignored")

    keys = data.get("ssh_authorized_keys", [])
    if not isinstance(keys, list):
        problems.append("ssh_authorized_keys must be a list of strings")
        keys = []
    for i, k in enumerate(keys, 1):
        if not isinstance(k, str) or "\n" in k or not KEY_TYPES.match(k.strip()):
            problems.append(f"ssh_authorized_keys entry {i} is not an ssh public key, ignored")
            continue
        s.keys.append(k.strip())

    nets = data.get("wifi", [])
    if not isinstance(nets, list):
        problems.append("wifi must be [[wifi]] blocks")
        nets = []
    for i, net in enumerate(nets, 1):
        where = f"wifi block {i}"
        if not isinstance(net, dict):
            problems.append(f"{where} is not a [[wifi]] block, ignored")
            continue
        for extra in sorted(set(net) - {"ssid", "psk"}):
            problems.append(f"{where}: unknown setting {extra!r}, ignored")
        ssid, psk = net.get("ssid"), net.get("psk")
        if not isinstance(ssid, str) or not 1 <= len(ssid.encode()) <= 32:
            problems.append(f"{where}: ssid must be 1-32 bytes, block ignored")
            continue
        where = f"wifi {ssid!r}"
        if psk is not None:
            # 64 hex digits is a raw key; anything else is a WPA passphrase.
            if not isinstance(psk, str) or not (
                8 <= len(psk) <= 63 or re.fullmatch(r"[0-9A-Fa-f]{64}", psk)
            ):
                problems.append(f"{where}: psk must be 8-63 characters (or 64 hex digits), ignored")
                continue
            if "\n" in psk:
                problems.append(f"{where}: psk contains a newline, ignored")
                continue
        s.networks.append((ssid, psk))

    inst = data.get("instrument", {})
    if not isinstance(inst, dict):
        problems.append("instrument must be an [instrument] table")
        inst = {}
    for k, v in inst.items():
        kind = INSTRUMENT_KEYS.get(k)
        if kind is None:
            problems.append(f"instrument: unknown setting {k!r}, ignored")
        elif not isinstance(v, kind) or isinstance(v, bool):
            problems.append(f"instrument: {k} must be a{'n integer' if kind is int else ' file name'}, ignored")
        elif kind is int and not INSTRUMENT_RANGES[k][0] <= v <= INSTRUMENT_RANGES[k][1]:
            lo, hi = INSTRUMENT_RANGES[k]
            problems.append(f"instrument: {k} must be from {lo} to {hi}, ignored")
        elif kind is str and ("/" in v or not v.lower().endswith((".sf2", ".sf3"))):
            problems.append("instrument: soundfont must be a .sf2 or .sf3 file name, ignored")
        else:
            s.instrument[k] = v

    web = data.get("web", {})
    if not isinstance(web, dict):
        problems.append("web must be a [web] table")
        web = {}
    for k in sorted(set(web) - {"password"}):
        problems.append(f"web: unknown setting {k!r}, ignored")
    pw = web.get("password")
    if pw is not None:
        if not isinstance(pw, str) or len(pw) < 8:
            problems.append("web: password must be at least 8 characters; web access over Wi-Fi stays off")
        else:
            s.web_password = pw
    return s, problems


def iwd_name(ssid):
    # iwd names the file after the SSID, hex-encoded with a leading "="
    # unless it is plain alphanumerics, space, "-" and "_".
    if re.fullmatch(r"[A-Za-z0-9_ -]+", ssid):
        return ssid
    return "=" + ssid.encode().hex()


def write_private(path, text, mode):
    tmp = path.with_name(path.name + ".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, mode)
    with os.fdopen(fd, "w") as f:
        f.write(text)
    os.chmod(tmp, mode)
    os.replace(tmp, path)


def apply(args):
    config = Path(args.config)
    iwd_dir = Path(args.iwd_dir)
    keys_dir = Path(args.keys_dir)

    settings = Settings()
    status = 0
    if not os.path.ismount(args.mount):
        print(f"{args.mount} is not mounted; no settings applied", file=sys.stderr)
        status = 1
    else:
        if not config.exists():
            config.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(TEMPLATE, config)
            print(f"wrote a template to {config}")
        try:
            settings, problems = load(config)
            for p in problems:
                print(f"{config}: {p}", file=sys.stderr)
        except (tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
            print(f"{config}: {e}; no settings applied", file=sys.stderr)
            status = 1

    # Replace what a previous run wrote, so removed entries go away on a
    # rerun too, not only at reboot.
    iwd_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    for old in list(iwd_dir.glob("*.psk")) + list(iwd_dir.glob("*.open")):
        old.unlink()
    for ssid, psk in settings.networks:
        if psk is None:
            write_private(iwd_dir / f"{iwd_name(ssid)}.open", "", 0o600)
        else:
            field = "PreSharedKey" if len(psk) == 64 else "Passphrase"
            write_private(iwd_dir / f"{iwd_name(ssid)}.psk", f"[Security]\n{field}={psk}\n", 0o600)
        print(f"wifi: {ssid}")

    # haxo's startup settings, for its service's EnvironmentFile.
    if args.haxo_env:
        env = {}
        inst = settings.instrument
        if "soundfont" in inst:
            found = [Path(d) / inst["soundfont"] for d in args.soundfont_dir if (Path(d) / inst["soundfont"]).is_file()]
            if found:
                env["HAXO_SOUNDFONT"] = str(found[0])
            else:
                print(f"instrument: no soundfont {inst['soundfont']!r}; the default plays", file=sys.stderr)
        for k in ("program", "transpose", "volume"):
            if k in inst:
                env[f"HAXO_{k.upper()}"] = str(inst[k])
        Path(args.haxo_env).parent.mkdir(parents=True, exist_ok=True)
        write_private(Path(args.haxo_env), "".join(f"{k}={v}\n" for k, v in env.items()), 0o644)
        print(f"instrument: {inst or 'defaults'}")

    # sshd reads authorized keys as the user, so the file is world-readable
    # in a root-owned directory (sshd's StrictModes).
    keys_dir.mkdir(mode=0o755, parents=True, exist_ok=True)
    write_private(keys_dir / USER, "".join(k + "\n" for k in settings.keys), 0o644)
    print(f"ssh: {len(settings.keys)} authorized key(s) for {USER}")
    return status


def toml_value(v):
    # A JSON string is a valid TOML basic string; ints are the same.
    return json.dumps(v, ensure_ascii=False) if isinstance(v, str) else str(v)


HEADER = re.compile(r"^\s*\[\[?\s*([A-Za-z0-9_.-]+)\s*\]\]?\s*(#.*)?$")


def blocks(lines):
    """[(name, is_array, start, end)] for each table header in `lines`."""
    found = []
    for i, line in enumerate(lines):
        m = HEADER.match(line)
        if m:
            found.append([m.group(1), line.lstrip().startswith("[["), i, len(lines)])
    for prev, nxt in zip(found, found[1:]):
        prev[3] = nxt[2]
    return [tuple(b) for b in found]


def read_lines(path):
    text = Path(path).read_bytes().decode("utf-8-sig")
    lines = text.splitlines(keepends=True)
    if lines and not lines[-1].endswith("\n"):
        lines[-1] += "\n"
    return lines


def write_checked(path, lines):
    """Replace `path` with `lines`, if they still parse."""
    text = "".join(lines)
    tomllib.loads(text)
    path = Path(path)
    tmp = path.with_name(path.name + ".tmp")
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def wifi_blocks(lines):
    """[(ssid, start, end)] for each [[wifi]] block."""
    out = []
    for name, is_array, start, end in blocks(lines):
        if name == "wifi" and is_array:
            try:
                ssid = tomllib.loads("".join(lines[start:end]))["wifi"][0].get("ssid")
            except (tomllib.TOMLDecodeError, KeyError, IndexError):
                ssid = None
            out.append((ssid, start, end))
    return out


def wifi_add(args):
    psk = sys.stdin.readline().rstrip("\n") if args.psk_stdin else None
    if not 1 <= len(args.ssid.encode()) <= 32:
        print("ssid must be 1-32 bytes", file=sys.stderr)
        return 2
    if psk is not None and not (8 <= len(psk) <= 63 or re.fullmatch(r"[0-9A-Fa-f]{64}", psk)):
        print("psk must be 8-63 characters (or 64 hex digits)", file=sys.stderr)
        return 2
    lines = read_lines(args.config)
    block = ["[[wifi]]\n", f"ssid = {toml_value(args.ssid)}\n"]
    if psk is not None:
        block.append(f"psk = {toml_value(psk)}\n")
    existing = [b for b in wifi_blocks(lines) if b[0] == args.ssid]
    if existing:
        _, start, end = existing[0]
        # Keep the blank lines and comments that end the old block.
        tail = end
        while tail > start and (not lines[tail - 1].strip() or lines[tail - 1].lstrip().startswith("#")):
            tail -= 1
        lines[start:tail] = block
    else:
        if lines and lines[-1].strip():
            lines.append("\n")
        lines.extend(block)
    write_checked(args.config, lines)
    return 0


def wifi_remove(args):
    lines = read_lines(args.config)
    for ssid, start, end in reversed(wifi_blocks(lines)):
        if ssid == args.ssid:
            # Up to the comments and blank lines that lead into the next block.
            tail = end
            while tail > start + 1 and (not lines[tail - 1].strip() or lines[tail - 1].lstrip().startswith("#")):
                tail -= 1
            while tail < end and not lines[tail].strip():
                tail += 1
            del lines[start:tail]
            write_checked(args.config, lines)
            return 0
    print(f"no [[wifi]] block for {args.ssid!r}", file=sys.stderr)
    return 1


def set_instrument(args):
    values = {k: getattr(args, k) for k in INSTRUMENT_KEYS if getattr(args, k) is not None}
    for k, v in values.items():
        if k in INSTRUMENT_RANGES and not INSTRUMENT_RANGES[k][0] <= v <= INSTRUMENT_RANGES[k][1]:
            print(f"{k} must be from {INSTRUMENT_RANGES[k][0]} to {INSTRUMENT_RANGES[k][1]}", file=sys.stderr)
            return 2
    lines = read_lines(args.config)
    table = [b for b in blocks(lines) if b[0] == "instrument" and not b[1]]
    if not table:
        if lines and lines[-1].strip():
            lines.append("\n")
        lines.append("# The instrument's settings, also changed from the web UI.\n")
        lines.append("[instrument]\n")
        start = end = len(lines)
    else:
        _, _, start, end = table[0]
        start += 1
    for k, v in values.items():
        line = f"{k} = {toml_value(v)}\n"
        for i in range(start, end):
            if re.match(rf"^\s*{k}\s*=", lines[i]):
                lines[i] = line
                break
        else:
            lines.insert(start, line)
            end += 1
    write_checked(args.config, lines)
    return 0


def show(args):
    try:
        settings, problems = load(args.config)
    except (tomllib.TOMLDecodeError, UnicodeDecodeError, OSError) as e:
        print(json.dumps({"error": str(e)}))
        return 1
    print(json.dumps({
        "ssh_keys": len(settings.keys),
        "wifi": [{"ssid": ssid, "open": psk is None} for ssid, psk in settings.networks],
        "instrument": settings.instrument,
        "web": {"password": settings.web_password},
        "problems": problems,
    }, ensure_ascii=False))
    return 0


def check(args):
    try:
        settings, problems = load(args.path)
    except (tomllib.TOMLDecodeError, UnicodeDecodeError, OSError) as e:
        print(f"{args.path}: {e}", file=sys.stderr)
        return 1
    for p in problems:
        print(f"{args.path}: {p}", file=sys.stderr)
    print(f"{args.path}: {len(settings.keys)} ssh key(s), {len(settings.networks)} wifi network(s)")
    return 1 if problems else 0


def template(args):
    keys = []
    for path in args.ssh_keys:
        for line in Path(path).read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if not KEY_TYPES.match(line):
                print(f"{path}: not an ssh public key: {line[:40]}", file=sys.stderr)
                return 1
            keys.append(line)
    out = []
    for line in TEMPLATE.read_text().splitlines(keepends=True):
        out.append(line)
        if line.startswith("ssh_authorized_keys = ["):
            # A JSON string is a valid TOML basic string.
            out.extend(f"  {json.dumps(k, ensure_ascii=False)},\n" for k in keys)
    sys.stdout.write("".join(out))
    return 0


def main():
    parser = argparse.ArgumentParser(prog="haxophone-config")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("apply")
    p.add_argument("--config", default="/boot/config/haxophone.toml")
    # The FAT partition the config is on: not needed for boot, so it may be
    # missing.
    p.add_argument("--mount", default="/boot")
    p.add_argument("--iwd-dir", default="/var/lib/iwd")
    p.add_argument("--keys-dir", default="/run/haxophone-config/authorized_keys")
    p.add_argument("--haxo-env", help="write haxo's startup settings here")
    p.add_argument("--soundfont-dir", action="append", default=[], metavar="DIR",
                   help="where [instrument] soundfont names are looked up, in order")
    p.set_defaults(func=apply)
    p = sub.add_parser("check")
    p.add_argument("path")
    p.set_defaults(func=check)
    p = sub.add_parser("template")
    p.add_argument("--ssh-keys", action="append", default=[], metavar="FILE")
    p.set_defaults(func=template)
    p = sub.add_parser("show")
    p.add_argument("--config", default="/boot/config/haxophone.toml")
    p.set_defaults(func=show)
    p = sub.add_parser("wifi-add")
    p.add_argument("ssid")
    p.add_argument("--psk-stdin", action="store_true", help="read the passphrase from stdin")
    p.add_argument("--config", default="/boot/config/haxophone.toml")
    p.set_defaults(func=wifi_add)
    p = sub.add_parser("wifi-remove")
    p.add_argument("ssid")
    p.add_argument("--config", default="/boot/config/haxophone.toml")
    p.set_defaults(func=wifi_remove)
    p = sub.add_parser("set-instrument")
    p.add_argument("--soundfont")
    p.add_argument("--program", type=int)
    p.add_argument("--transpose", type=int)
    p.add_argument("--volume", type=int)
    p.add_argument("--config", default="/boot/config/haxophone.toml")
    p.set_defaults(func=set_instrument)
    args = parser.parse_args()
    sys.exit(args.func(args))


if __name__ == "__main__":
    main()
