"""A scripted tour of CosmicDMS through the Python API.

Run from the repository root after `cargo build --release -p CosmicDMS`:

    python3 python/examples/tour.py out/
"""

import sys
from pathlib import Path

import cosmicdms


def main(out_dir: str = ".") -> None:
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    with cosmicdms.launch(sample=True) as cc:
        print("connected to", cc.ping())
        cc.ui_window_size_set(width=1400, height=900)

        cc.view_camera_preset(preset="iso")
        cc.view_fit()
        cc.screenshot(out / "1-iso-dark.png")

        cc.view_theme_set(theme="light")
        cc.view_projection_set(projection="perspective")
        cc.view_camera_set(yaw=-60, pitch=25)
        cc.view_display_set(grid=False, readout=False)
        cc.ui_ribbon_tab_set(tab="toolpaths")
        cc.ui_pane_hide(pane="console")
        cc.selection_set(id="model.sample")
        cc.screenshot(out / "2-light-toolpaths.png")

        state = cc.state()
        print("camera:", state["camera"])
        print("open panes:", state["open_panes"])

        try:
            cc.toolpath_pocket()
        except cosmicdms.CosmicDMSError as e:
            print("as expected, planned command:", e.kind)


if __name__ == "__main__":
    main(*sys.argv[1:2])
