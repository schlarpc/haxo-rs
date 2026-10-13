"""Apply or check the Haxophone's settings file (haxophone.toml).

  haxophone-config apply [--config PATH] [--mount DIR] [--iwd-dir DIR] [--keys-dir DIR]
  haxophone-config check PATH
  haxophone-config template [--ssh-keys FILE]...

`apply` runs at boot (nixos/config-file.nix): it writes iwd's network files
and the haxophone user's authorized_keys from the settings file, and puts a
commented template in place when there is none. `check` validates a file
without applying it (mint-sd uses it). `template` prints the template,
with the public keys from each FILE (an authorized_keys or .pub file) filled
in.
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
KNOWN_KEYS = {"ssh_authorized_keys", "wifi"}
KEY_TYPES = re.compile(r"^(ssh-|ecdsa-|sk-ssh-|sk-ecdsa-)\S+ \S+")


class Settings:
    def __init__(self):
        self.keys = []
        self.networks = []  # (ssid, psk or None)


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

    # sshd reads authorized keys as the user, so the file is world-readable
    # in a root-owned directory (sshd's StrictModes).
    keys_dir.mkdir(mode=0o755, parents=True, exist_ok=True)
    write_private(keys_dir / USER, "".join(k + "\n" for k in settings.keys), 0o644)
    print(f"ssh: {len(settings.keys)} authorized key(s) for {USER}")
    return status


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
    p.set_defaults(func=apply)
    p = sub.add_parser("check")
    p.add_argument("path")
    p.set_defaults(func=check)
    p = sub.add_parser("template")
    p.add_argument("--ssh-keys", action="append", default=[], metavar="FILE")
    p.set_defaults(func=template)
    args = parser.parse_args()
    sys.exit(args.func(args))


if __name__ == "__main__":
    main()
