"""Generate the web UI's board drawing from haxo-hw's KiCad PCB.

    python3 webui/tools/gen-geometry.py path/to/haxo-hw/haxophone001.kicad_pcb

Rewrites the block between the GEOMETRY markers in webui/index.html with:
  - the main panel's outline (Edge.Cuts, as an SVG path in mm), with the two
    side panels (RHK, LHP) clipped off: they snap off and mount at right
    angles, so the page draws their keys edge-on beside the main panel;
  - every keyswitch's position and its matrix bit (row * 3 + col, as
    haxo-rs's keyscan numbers them), traced from the switch through its
    diode to the ROW and COL nets.
"""

import json
import math
import re
import subprocess
import sys
from pathlib import Path

TOKEN = re.compile(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()]+')

# The side panels and their breakout tabs, cut out of the fabricated panel
# outline (mm, PCB coordinates). Checked by eye against haxo-hw's renders.
# Non-overlapping, since the clip is even-odd.
SIDE_PANELS_CLIP = "M20 106 H55.2 V178.5 H20 Z M62 213.5 H200 V300 H62 Z M110.6 196 H200 V213.5 H110.6 Z"


def parse(text):
    stack = [[]]
    for m in TOKEN.finditer(text):
        t = m.group(0)
        if t == "(":
            stack.append([])
        elif t == ")":
            node = stack.pop()
            stack[-1].append(node)
        else:
            stack[-1].append(t[1:-1] if t.startswith('"') else t)
    return stack[0][0]


def children(node, name):
    return [c for c in node if isinstance(c, list) and c and c[0] == name]


def child(node, name):
    found = children(node, name)
    return found[0] if found else None


def reference(fp):
    for p in children(fp, "property"):
        if p[1] == "Reference":
            return p[2]
    for t in children(fp, "fp_text"):
        if t[1] == "reference":
            return t[2]
    return None


def position(fp):
    a = child(fp, "at")
    return float(a[1]), float(a[2]), float(a[3]) if len(a) > 3 else 0.0


def pad_nets(fp):
    nets = {}
    for p in children(fp, "pad"):
        n = child(p, "net")
        nets[p[1]] = n[2] if n else None
    return nets


def base_net(name):
    # The side panels' nets pass through the breakout tabs renamed.
    return re.sub(r"_(RHK|LHP)$", "", name or "")


def keys(tree):
    fps = children(tree, "footprint")
    diodes = {reference(fp): pad_nets(fp) for fp in fps if "Diode" in fp[1]}
    out = []
    for fp in fps:
        if not fp[1].startswith("MX_Only"):
            continue
        x, y, _ = position(fp)
        row = col = None
        for net in pad_nets(fp).values():
            if base_net(net).startswith("COL"):
                col = int(base_net(net)[3:])
            m = re.match(r"Net-\((D\d+)-A\)", net or "")
            if m:
                for dnet in diodes[m.group(1)].values():
                    if base_net(dnet).startswith("ROW"):
                        row = int(base_net(dnet)[3:])
        out.append({"ref": reference(fp), "bit": row * 3 + col, "x": x, "y": y})
    return sorted(out, key=lambda k: k["bit"])


def edge_segments(tree):
    segs = []

    def collect(node, prefix, place):
        for c in node:
            if not (isinstance(c, list) and c and isinstance(c[0], str) and c[0].startswith(prefix)):
                continue
            layer = child(c, "layer")
            if not layer or layer[1] != "Edge.Cuts":
                continue
            kind = c[0][len(prefix):]
            pt = lambda name: place(float(child(c, name)[1]), float(child(c, name)[2]))  # noqa: E731
            if kind == "line":
                segs.append(("L", pt("start"), pt("end")))
            elif kind == "arc":
                segs.append(("A", pt("start"), pt("mid"), pt("end")))
            else:
                raise SystemExit(f"unhandled Edge.Cuts {c[0]}")

    collect(tree, "gr_", lambda x, y: (x, y))
    for fp in children(tree, "footprint"):
        ox, oy, rot = position(fp)
        a = math.radians(-rot)

        def place(x, y, ox=ox, oy=oy, a=a):
            return ox + x * math.cos(a) - y * math.sin(a), oy + x * math.sin(a) + y * math.cos(a)

        collect(fp, "fp_", place)
    return segs


def loops(segs):
    def k(p):
        return (round(p[0], 2), round(p[1], 2))

    ends = {}
    for i, s in enumerate(segs):
        for p in (s[1], s[-1]):
            ends.setdefault(k(p), []).append(i)
    used, out = set(), []
    for i in range(len(segs)):
        if i in used:
            continue
        loop, cur, pt = [], i, k(segs[i][1])
        while cur is not None and cur not in used:
            used.add(cur)
            s = segs[cur]
            forward = k(s[1]) == pt
            loop.append(s if forward else (s[0], *s[1:][::-1]))
            pt = k(loop[-1][-1])
            nxt = [j for j in ends.get(pt, []) if j not in used]
            cur = nxt[0] if nxt else None
        out.append(loop)
    return out


def circle(a, m, b):
    (ax, ay), (bx, by), (cx, cy) = a, m, b
    d = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
    ux = ((ax * ax + ay * ay) * (by - cy) + (bx * bx + by * by) * (cy - ay) + (cx * cx + cy * cy) * (ay - by)) / d
    uy = ((ax * ax + ay * ay) * (cx - bx) + (bx * bx + by * by) * (ax - cx) + (cx * cx + cy * cy) * (bx - ax)) / d
    return ux, uy, math.hypot(ax - ux, ay - uy)


def svg_path(loop):
    f = lambda v: f"{v:.2f}".rstrip("0").rstrip(".")  # noqa: E731
    d = [f"M{f(loop[0][1][0])} {f(loop[0][1][1])}"]
    for s in loop:
        if s[0] == "L":
            d.append(f"L{f(s[2][0])} {f(s[2][1])}")
            continue
        a, m, b = s[1:]
        ux, uy, r = circle(a, m, b)
        cross = (m[0] - a[0]) * (b[1] - a[1]) - (m[1] - a[1]) * (b[0] - a[0])
        sweep = 0 if cross < 0 else 1
        ang = lambda p: math.atan2(p[1] - uy, p[0] - ux)  # noqa: E731
        extent = (ang(b) - ang(a)) % (2 * math.pi) if sweep else (ang(a) - ang(b)) % (2 * math.pi)
        d.append(f"A{f(r)} {f(r)} 0 {int(extent > math.pi)} {sweep} {f(b[0])} {f(b[1])}")
    return " ".join(d) + " Z"


def revision(pcb):
    try:
        out = subprocess.run(
            ["git", "-C", str(pcb.parent), "rev-parse", "--short", "HEAD"],
            capture_output=True, text=True, check=True,
        )
        return out.stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def main():
    pcb = Path(sys.argv[1])
    page = Path(__file__).resolve().parent.parent / "index.html"
    tree = parse(pcb.read_text())
    geometry = {
        "source": f"haxo-hw {revision(pcb)}/{pcb.name}",
        "outline": " ".join(svg_path(loop) for loop in loops(edge_segments(tree))),
        "clip": SIDE_PANELS_CLIP,
        "keys": [{"bit": k["bit"], "ref": k["ref"], "x": k["x"], "y": k["y"]} for k in keys(tree)],
    }
    block = "const GEOMETRY = " + json.dumps(geometry, indent=1) + ";"
    text = page.read_text()
    new, n = re.subn(
        r"(// BEGIN GEOMETRY[^\n]*\n).*?(\n\s*// END GEOMETRY)",
        lambda m: m.group(1) + block + m.group(2),
        text,
        flags=re.S,
    )
    if n != 1:
        raise SystemExit("GEOMETRY markers not found in index.html")
    page.write_text(new)
    print(f"{len(geometry['keys'])} keys written to {page}")


if __name__ == "__main__":
    main()
