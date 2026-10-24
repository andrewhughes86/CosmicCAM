"""Import a model through the Python API and capture the result.

Writes a small binary STL (a stepped block with a through hole, built with the standard
library), imports it and saves screenshots. Run from the repository root after
`cargo build --release -p CosmicCAM`:

    python3 python/examples/import_model.py out/
"""

import math
import struct
import sys
import tempfile
from pathlib import Path

import cosmiccam


def box(x0, y0, z0, x1, y1, z1):
    """Triangles of an axis-aligned box, wound outwards."""
    c = [(x, y, z) for z in (z0, z1) for y in (y0, y1) for x in (x0, x1)]
    quads = [(0, 2, 3, 1), (4, 5, 7, 6), (0, 1, 5, 4), (2, 6, 7, 3), (1, 3, 7, 5), (0, 4, 6, 2)]
    for a, b, d, e in quads:
        yield c[a], c[b], c[d]
        yield c[a], c[d], c[e]


def cylinder(cx, cy, z0, z1, r, n=48):
    """Triangles of a vertical cylinder, wound outwards, with caps."""
    ring = [(cx + r * math.cos(2 * math.pi * i / n), cy + r * math.sin(2 * math.pi * i / n)) for i in range(n)]
    for i in range(n):
        (ax, ay), (bx, by) = ring[i], ring[(i + 1) % n]
        yield (ax, ay, z0), (bx, by, z0), (bx, by, z1)
        yield (ax, ay, z0), (bx, by, z1), (ax, ay, z1)
        yield (cx, cy, z1), (ax, ay, z1), (bx, by, z1)
        yield (cx, cy, z0), (bx, by, z0), (ax, ay, z0)


def write_stl(path, triangles):
    triangles = list(triangles)
    with open(path, "wb") as f:
        f.write(b"CosmicCAM example".ljust(80, b"\0"))
        f.write(struct.pack("<I", len(triangles)))
        for tri in triangles:
            f.write(struct.pack("<3f", 0, 0, 0))
            for v in tri:
                f.write(struct.pack("<3f", *v))
            f.write(b"\0\0")


def main(out_dir: str = ".") -> None:
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        stl = Path(tmp) / "stepped-block.stl"
        write_stl(stl, [*box(0, 0, 0, 80, 50, 15), *box(0, 0, 15, 40, 50, 30), *cylinder(60, 25, 15, 40, 8)])
        with cosmiccam.launch() as cc:
            cc.ui_window_size_set(width=1400, height=900)
            model = cc.model_import(path=str(stl))
            print("imported", model["id"], model["triangles"], "triangles, bounds", model["bounds"])
            cc.ui_ribbon_tab_set(tab="setup")
            cc.selection_set(id=model["id"])
            cc.view_camera_preset(preset="iso")
            cc.view_fit()
            cc.screenshot(out / "import-iso.png")


if __name__ == "__main__":
    main(*sys.argv[1:2])
