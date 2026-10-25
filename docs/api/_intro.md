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

## Units

Commands take and return **millimetres** (and mm/min for feeds), always. The units preference
(`cc.settings_units_set(units="in")`, or Settings on the Home tab; inches by default) only
changes what the app shows and what its dialogs accept, so a script means the same thing on
every machine. Parameters that are lengths or feeds say so in their schema (`"unit": "mm"` or
`"mm/min"` in `command.describe`).

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
| `origin` | `model` (world zero, the default), `box`, `stock`, `point` or `intersection` |
| `origin_box` | For `box` or `stock`: a point on the models' (or the stock's) bounding box, `<level>_<position>` with level `top`, `middle`, `bottom` and position `front_left`, `front`, `front_right`, `left`, `center`, `right`, `back_left`, `back`, `back_right` (`center` alone is the box centre; default `top_center`) |
| `origin_point` / `origin_geometry` | For `point`: coordinates, or a selection (a vertex, a circle's centre, a cylinder's upper axis end, a plane's centre, an edge's midpoint) |
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

## Stock

```python
cc.setup_stock_fit(margin_sides=5, margin_top=2, margin_bottom=0)   # box around the models
print(cc.setup_stock_get())        # {'shape': 'box', 'min': [...], 'max': [...], 'size': [...]}
cc.setup_wcs_create(origin="stock", origin_box="top_front_left")
```

The sample part comes with stock; importing a model replaces the sample and its stock.

## Machines and tool libraries

Every machine configuration owns its own tool library. Open it with `cc.tool_library_open()`
(its own window in the app, with a to-scale preview of the tool under the pointer). The active machine is the one the
Tooling tab and the tool commands work on; switch with `machine.activate`. The library is kept
across jobs and saved by the app after every change (a file in the user data folder on the
desktop, browser storage on the web). A new installation starts with one "Generic 3-axis mill".

```python
router = cc.machine_create(name="Router", travel_x=1200, travel_y=800, travel_z=150,
                           max_spindle_rpm=24000, max_feed=15000, tool_capacity=8)
cc.tool_create_flat(diameter=6, flutes=3, flute_length=18, overall_length=50,
                    spindle_rpm=12000, feed=1200, plunge_feed=300, stepover=2, stepdown=6)
cc.tool_create_flat(diameter=3.175, flutes=2, name="1/8in upcut", number=5)
for t in cc.tool_list():
    print(t["number"], t["name"], t["diameter"])
cc.tool_feeds(number=5, spindle_rpm=18000, feed=900)
cc.tool_edit(number=5, new_number=6)            # renumber
cc.tool_duplicate(number=6)                     # copy to the lowest free number
cc.tool_library_export(path="router-tools.json")
cc.machine_duplicate(name="Router (spare)")     # a copy with the whole library
cc.machine_activate(id="machine.1")
cc.tool_library_import(path="router-tools.json", replace=True)
```

Only flat end mills can be created so far. Ball, bull nose, chamfer, drill, spot drill, face
mill, tap and thread mill are listed (`tool.create.ball`, …) and report "not supported yet".
Tools are checked against their machine: a spindle speed or feed above the machine's limits
is refused, and so is lowering a machine's limits below a tool in its library. Tools are named
`<machine id>/tool.<number>` for `selection.set`, so the inspector can show one.

Controller dialect and post-processing are part of the Python machine configurations planned
for Phase 8; a machine's `post` field will name its post module.

## Toolpaths: 2D contour

`toolpath.contour` follows edges (or a face's outline, or the lower rim of a hole or boss) at
depth, offset by the tool radius. Every setting is a parameter; leave one out for its default.
In the app the 2D contour tile on the Toolpaths tab opens a panel at the right with the same
settings on tabs (Tool, Multi-axis, Geometry, Heights, Passes, Linking) and previews the result.

```python
bottom = next(f for f in cc.model_faces_list(id="model.1", kind="plane") if f["normal"] == [0, 0, -1])
tp = cc.toolpath_contour(
    geometry=[bottom["face"]], side="outside", tool=1,
    top_from="model_top", bottom_from="selection", bottom_offset=-0.5,  # 0.5 mm below the part
    multiple_depths=True, max_stepdown=6, stock_radial=0.3,
    radial_passes=2, radial_stepover=2, spring_passes=1,
    lead_in="arc", lead_in_radius=4, ramp="ramp", ramp_angle=3,
    feed_optimization=True, optimization_angle=45, optimization_feed=50,
)
print(tp["moves"], tp["cut_length"], tp["time"], tp["warnings"])
moves = cc.toolpath_get(id=tp["id"], moves=True)["move_list"]   # [{kind, to, feed}, ...]
cc.toolpath_contour(id=tp["id"], geometry=[bottom["face"]], direction="conventional")  # regenerate
```

Heights are measured in the WCS: each is a reference (`selection`, `model_top`,
`model_bottom`, `stock_top`, `stock_bottom`, `origin`) plus an offset. From top to bottom they are
clearance (moves between contours), retract (between passes), feed (rapid ends, feed begins),
top (where the material starts) and bottom (the final depth); they must not cross. Climb milling
keeps the material on the right of the tool's travel: clockwise outside a contour, anticlockwise
inside. Moves are `rapid`, `link`, `plunge` (plunges and ramps), `lead` and `cut`, in world mm.

## Setups and stock

A setup is one way the part is held: a name, the machine it runs on, the WCS it is programmed
in (or the world origin) and its stock. The active setup's stock and WCS are the current ones.

```python
cc.setup_create(name="Op 1", machine="machine.1", wcs="wcs.1",
                stock="fit", stock_margin_sides=2, stock_margin_top=1, stock_round=5)
cc.setup_create(name="Op 2", wcs="origin", stock="size", stock_size=[120, 80, 50], stock_align="top")
cc.setup_stock_box(stock="box", stock_min=[-60, -40, -5], stock_max=[60, 40, 45])  # active setup
cc.setup_activate(id="setup.1")
print(cc.setup_list())
```

## The demo project

`cc.job_demo_open()` opens the built-in demo project (also on the Home tab): a part, WCS, setup,
stock, tools and toolpaths, ready to change and post. `cosmiccam.launch(demo=True)` starts the app
with it open; `launch()` starts with an empty job.

## Job files and tabs

Jobs are saved as `.cosmic` files. Each open job is a document in its own tab; commands work on
the active one.

```python
cc.job_save(path="bracket")                 # writes bracket.cosmic; later saves need no path
cc.job_open(path="other.cosmic")            # opens in a new tab and shows it
for tab in cc.job_tab_list()["tabs"]:
    print(tab["index"], tab["name"], tab["modified"])
cc.job_tab_activate(index=0)
cc.job_tab_close(index=1)                   # refused while it has unsaved changes...
cc.job_tab_close(index=1, discard=True)     # ...unless they are discarded
```

## Toolpath strategies

Every strategy is a command that adds a toolpath (or regenerates one when given its `id`). They
share the tool, feeds, coolant, WCS and the five heights (`clearance`, `retract`, `feed_height`,
`top`, `bottom`, each `<name>_from` a reference plus `<name>_offset`).

| Command | What it cuts | Picks (`geometry`) |
| --- | --- | --- |
| `toolpath_contour` | along edges or a face outline, offset by the tool radius | edges or faces |
| `toolpath_face` | rows across the stock top, down to a height | none (or faces with `boundary="selection"`) |
| `toolpath_pocket` | offset rings inside floor faces or closed edges, around islands | floor faces or closed edges |
| `toolpath_drill` | drill, spot, peck, chip break, tap or bore cycles (canned cycles in the G-code) | hole walls, rims or points |
| `toolpath_rough` | Z-level clearing of the stock around the whole part | none |
| `toolpath_waterline` | Z-level passes around the part, for steep walls | none |
| `toolpath_raster` | parallel rows following the surface, for shallow areas | none (or faces with `boundary="selection"`) |

```python
cc.toolpath_face(stepover=4, multiple_depths=True, stepdown=1)
cc.toolpath_pocket(geometry=["model.1/face.8"], stepover=2.5, stepdown=2, bottom_offset=-5)
cc.toolpath_drill(geometry=["model.1/edge.12"], cycle="peck", peck_depth=3, bottom_offset=-12)
cc.toolpath_rough(stepdown=3, stock_radial=0.5, stock_axial=0.3)
cc.toolpath_waterline(stepdown=0.5)
cc.toolpath_raster(stepover=0.5, angle=45)
```

The 3D strategies (rough, waterline, raster) use flat end mills for now.

## Showing, renaming, copying

Only the selected toolpath is drawn, plus any switched on. Models can be hidden too (hidden
models are still machined, but cannot be picked). Copy and paste work within a job and between
open jobs.

```python
cc.toolpath_visibility_set(id="toolpath.2", visible=True)   # omit visible to toggle
cc.model_visibility_set(id="model.1", visible=False)
cc.toolpath_rename(id="toolpath.2", name="Rough the pocket")
cc.toolpath_duplicate(id="toolpath.2")
cc.job_clipboard_copy(id="toolpath.2")    # a toolpath, model or WCS; default: the selection
cc.job_tab_activate(index=1)
cc.job_clipboard_paste()
```

## G-code

Until the Python post-processors of Phase 8 exist, CosmicCAM writes plain, generic G-code:

```python
cc.post_gcode_export(path="part.nc")                       # all toolpaths, in the units setting
cc.post_gcode_export(path="part-mm.nc", units="mm", line_numbers=True, program_number=1234)
text = cc.post_gcode_get(toolpaths=["toolpath.2"])["text"]  # the same program as text
```

The program opens with a safety block (`G90 G94 G17 G40 G49 G80`, then `G20` or `G21`). Each
toolpath gets a tool change (`T# M6`, skipped when the tool stays), `S… M3`, its WCS's work
offset (`G54` for the job's first WCS, `G55` for the second, …), `G43 H#` on the first Z move,
coolant (`M8` flood, `M7` mist or air) and its moves as `G0` / `G1` in that WCS, modal values
written only when they change. It ends `M9`, `M5`, `G91 G28 Z0`, `G90 G49`, `M30`. A toolpath
whose tool has no spindle speed is refused. Check the program against your machine before
running it.

## Selecting geometry

Clicking in the viewport selects what is under the pointer: a vertex (corner) if one is close,
else an edge, else the face. The inspector shows its geometry (a face's kind, area, normal or
diameter and axis; an edge's length and ends or centre; a vertex's position). Clicking empty
space clears the selection. From a script:

```python
cc.selection_set(id="model.1/face.3")       # or model.1/edge.4, model.1/vertex.2, or an outline id
print(cc.selection_get())                   # {id, label, properties: [{name, value}], children}
cc.model_vertices_list(id="model.1")        # corners: {vertex, model, kind, position}
cc.model_vertex_pick(origin=[0, 0, 100], direction=[0, 0, -1], radius=1)
```

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
