# CosmicCAM Python API

Everything you can do in CosmicCAM's interface you can do from Python: each ribbon button,
console command and shortcut is a command with an id like `view.camera.preset`, and each
command is a method on the Python connection (`cc.view_camera_preset(preset="top")`).
The same commands are what agents use through the control channel.

## Install

The client is pure Python (3.9+) with no dependencies:

```bash
pip install -e python/
cargo build --release -p CosmicCAM     # the app the client drives
```

## Start or connect

```python
import cosmiccam

# Start the app with the control channel open; leaving the block quits it.
with cosmiccam.launch(sample=True) as cc:
    print(cc.ping())                      # {'name': 'CosmicCAM', 'version': ..., 'protocol': 1}

# Or connect to an app started with `CosmicCAM --control 5800`.
cc = cosmiccam.connect(5800)              # token from $CosmicCAM_CONTROL_TOKEN or the token file
```

`launch()` finds the executable through `$COSMICCAM_EXECUTABLE`, then `CosmicCAM` on `PATH`,
then `target/release` in this checkout.

## Calling commands

```python
cc.view_camera_preset(preset="iso")                 # generated, typed method
cc.run("view.camera.preset", preset="iso")          # same, by id
cc.execute("view.camera.preset", {"preset": "iso"}) # same, with a dict
```

Parameters are keyword-only and checked by the app: unknown names, wrong types and out-of-range
values raise `CosmicCAMError(kind="invalid_params")` without changing anything.

| `CosmicCAMError.kind` | Meaning |
| --- | --- |
| `invalid_params` | A parameter is missing, unknown, of the wrong type or out of range |
| `unknown_command` | No command has that id |
| `disabled` | The command exists but cannot run now (e.g. a planned feature) |
| `not_supported` | The feature is not implemented yet |
| `import` | A model file could not be read or parsed (the message names the line) |
| `internal` | The command failed unexpectedly; the app restored its state |
| `unauthorized`, `timeout`, `disconnected` | Connection problems |

## Importing models

CosmicCAM imports STL (binary and ASCII) and Wavefront OBJ meshes. Neither format stores units,
so say which unit the file was drawn in (`mm` by default); the job is always in millimetres.

```python
part = cc.model_import(path="parts/bracket.stl")              # {'id': 'model.1', 'triangles': ..., 'bounds': ...}
cc.model_import(path="parts/fixture.obj", units="in", name="Vice jaws")
for m in cc.model_list():
    print(m["id"], m["name"], m["bounds"])
cc.selection_set(id=part["id"])     # models appear under Models in the job outline
cc.model_remove(id="model.2")
```

The first import replaces the built-in sample part. A file that cannot be read raises
`CosmicCAMError(kind="import")` and leaves the job unchanged. `cc.start("model.import")` opens
the file dialog instead, as the ribbon button does; files dropped on the window are imported too.

## Work coordinate systems

A WCS is the part zero the machine works from, with its axis directions. The active WCS is the
one toolpaths will be programmed in; each WCS is drawn as a triad in the viewport (the active
one in full colour, axes labelled X, Y and Z) and listed under Setups in the job outline. Once
a WCS exists the world (model) origin triad is drawn faint.

`setup.wcs.create` defines a WCS from two independent groups of parameters. `setup.wcs.align`
takes the same groups and re-defines an existing WCS; a group left out is kept.

### Orientation: how the axes are set

| Parameter | Meaning |
| --- | --- |
| `orientation` | `model` (world axes, the default), `z_x`, `z_y`, `x_y` (select two axes; the third follows, right-handed) or `vectors` |
| `orientation_z`, `orientation_x`, `orientation_y` | The selection for each axis the mode uses: a plane (its outward normal), a cylinder (its axis), or an edge (a straight edge's direction, a circle's axis) |
| `orientation_z_vector`, `orientation_x_vector` | Directions, for `orientation = "vectors"` |
| `flip_z`, `flip_x`, `flip_y` | Reverse a selected axis (the derived one follows) |

Straight edges used for X or Y point away from the origin, along the edge. A hole or boss
selected for X or Y when its axis runs along Z points X (or Y) towards it, which is how a
second dowel hole sets X.

### Origin: where zero goes

| Parameter | Meaning |
| --- | --- |
| `origin` | `model` (world zero, the default), `box`, `point` or `intersection` |
| `origin_box` | For `box`: a point on the models' bounding box, `<level>_<position>` with level `top`, `middle`, `bottom` and position `front_left`, `front`, `front_right`, `left`, `center`, `right`, `back_left`, `back`, `back_right` (`center` alone is the box centre; default `top_center`) |
| `origin_point` / `origin_geometry` | For `point`: coordinates, or a selection (a circle's centre, a cylinder's upper axis end, a plane's centre, an edge's midpoint) |
| `origin_a`, `origin_b`, `origin_c` | For `intersection`: three planes (a corner), a line and a plane (an edge or a cylinder axis meeting a face), or two lines |

Parameters the chosen modes do not use are refused, so a script always gets what it asked for.

Selections name recognised geometry: `<model id>/face.<n>` (planes, cylinders and freeform
patches) and `<model id>/edge.<n>` (straight and circular edges). List them with
`cc.model_faces_list(id=..., kind="plane")` and `cc.model_edges_list(id=..., kind="line")`, or
find what a ray hits with `cc.model_pick(...)` and `cc.model_edge_pick(...)`, as viewport
clicks do.

```python
top = next(f for f in cc.model_faces_list(id="model.1", kind="plane") if f["normal"] == [0, 0, 1])
left = next(f for f in cc.model_faces_list(id="model.1", kind="plane") if f["normal"] == [-1, 0, 0])
front = next(f for f in cc.model_faces_list(id="model.1", kind="plane") if f["normal"] == [0, -1, 0])
edge = cc.model_edges_list(id="model.1", kind="line")[0]

# Corner of three faces; Z from the top face, X along an edge.
cc.setup_wcs_create(
    origin="intersection", origin_a=top["face"], origin_b=left["face"], origin_c=front["face"],
    orientation="z_x", orientation_z=top["face"], orientation_x=edge["edge"],
)
cc.setup_wcs_create(origin="box", origin_box="top_center")          # model axes, top centre
cc.setup_wcs_align(orientation="z_x", orientation_z=top["face"], orientation_x=edge["edge"], flip_x=True)
cc.setup_wcs_offset(delta=[0, 0, -2])                               # along the WCS's own axes
cc.setup_wcs_activate(id="wcs.2")
```

In the app, Create WCS and Align open a panel with the same two groups: choose a mode, click
each selection row and then the geometry in the viewport (picked faces and edges turn the
accent colour; things that cannot be used turn red under the pointer), tick Flip where needed,
check the preview triad and press OK. Offset, Rotate and Rename open a small dialog. Scripts
never get dialogs.

## State, screenshots and the log

```python
state = cc.state()                  # job, camera, display, theme, selection, ribbon_tab, open_panes, outline
cam = cc.view_camera_get()          # {'target': [...], 'distance': ..., 'yaw': ..., 'pitch': ..., ...}
cc.screenshot("view.png")           # whole window, after the next frame (window must be visible)
since = state["log_seq"]
cc.view_fit()
print(cc.console_log_get(since=since))   # commands run since, with results or errors
cc.commands()                       # every command with its parameter schema
```

## Example: a scripted tour

```python
import cosmiccam

with cosmiccam.launch(sample=True) as cc:
    cc.ui_window_size_set(width=1400, height=900)
    cc.view_theme_set(theme="light")
    cc.view_display_set(grid=False, readout=False)
    cc.view_camera_set(yaw=-60, pitch=25)
    cc.view_fit()
    cc.ui_ribbon_tab_set(tab="toolpaths")
    cc.ui_pane_hide(pane="console")
    cc.screenshot("tour.png")
```

`python/examples/tour.py` is a runnable version.

## Conventions

- Lengths are millimetres, angles are degrees, Z is up. `yaw` is measured around +Z from +X
  (−90° looks from the front); `pitch` is the elevation above the XY plane.
- Commands that change something return the new value; `*.get` commands only read.
- Window and pane commands (`ui.*`, `app.quit`) take effect on the app's next frame.
- The wire protocol underneath is documented in [docs/control-protocol.md](../control-protocol.md).
