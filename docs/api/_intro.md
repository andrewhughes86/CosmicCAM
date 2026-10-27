# CosmicDMS Python API

Everything you can do in CosmicDMS's interface you can do from Python: each ribbon button,
console command and shortcut is a command with an id like `view.camera.preset`, and each
command is a method on the Python connection (`cc.view_camera_preset(preset="top")`).
The same commands are what agents use through the control channel.

## Install

The client is pure Python (3.9+) with no dependencies:

```bash
pip install -e python/
cargo build --release -p CosmicDMS     # the app the client drives
```

## Start or connect

```python
import cosmicdms

# Start the app with the control channel open; leaving the block quits it.
with cosmicdms.launch(sample=True) as cc:
    print(cc.ping())                      # {'name': 'CosmicDMS', 'version': ..., 'protocol': 1}

# Or connect to an app started with `CosmicDMS --control 5800`.
cc = cosmicdms.connect(5800)              # token from $CosmicDMS_CONTROL_TOKEN or the token file
```

`launch()` finds the executable through `$COSMICDMS_EXECUTABLE`, then `CosmicDMS` on `PATH`,
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
values raise `CosmicDMSError(kind="invalid_params")` without changing anything.

| `CosmicDMSError.kind` | Meaning |
| --- | --- |
| `invalid_params` | A parameter is missing, unknown, of the wrong type or out of range |
| `unknown_command` | No command has that id |
| `disabled` | The command exists but cannot run now (e.g. a planned feature) |
| `not_supported` | The feature is not implemented yet |
| `import` | A model file could not be read or parsed (the message names the line) |
| `internal` | The command failed unexpectedly; the app restored its state |
| `unauthorized`, `timeout`, `disconnected` | Connection problems |

## Importing models

CosmicDMS imports exact CAD models and meshes; `cc.model_formats()` lists them all:

| Kind | Formats |
| --- | --- |
| CAD (exact surfaces, tessellated on import) | STEP (`.step`, `.stp`, `.p21`; AP203, AP214, AP242 including assemblies and AP242 meshes), IGES (`.iges`, `.igs`), ACIS SAT (`.sat`; text files of any ACIS release) |
| Meshes | STL, Wavefront OBJ, 3MF, PLY, OFF, AMF, glTF (`.gltf`, `.glb`), COLLADA (`.dae`), VRML (`.wrl`), X3D, 3DS, DXF (3D faces and meshes) |

Any of them may also be gzipped (`part.stl.gz`, `.stpz`, `.x3dz`, `.wrz`). STEP, IGES and SAT curved
surfaces become triangles within `tolerance` of the exact surface (0.01 mm by default); every
edge is shared by its two faces, so a solid comes in watertight. Files that record their unit
(STEP, IGES, SAT, 3MF, AMF, glTF, COLLADA, DXF) are read in it; for the rest say which unit the file
was drawn in (`mm` by default). `units` overrides what the file says. The job is always in
millimetres. glTF and COLLADA models drawn Y-up are turned to CosmicDMS's Z-up.

```python
part = cc.model_import(path="parts/bracket.step")             # {'id': 'model.1', 'units': 'in', 'warnings': [], ...}
cc.model_import(path="parts/housing.igs", tolerance=0.005)    # finer facets on curved faces
cc.model_import(path="parts/fixture.obj", units="in", name="Vice jaws")
for m in cc.model_list():
    print(m["id"], m["name"], m["bounds"])
cc.selection_set(id=part["id"])     # models appear under Models in the job outline
cc.model_remove(id="model.2")
```

`warnings` says what an import skipped or approximated: faces drawn flat because their surface
could not be trimmed, geometry kinds not supported yet, construction surfaces left beside a
solid, ACIS solids in DXF files. The first import replaces the built-in sample part. A file
that cannot be read raises `CosmicDMSError(kind="import")` and leaves the job unchanged.
`cc.start("model.import")` opens the file dialog instead, as the ribbon button does; files
dropped on the window are imported too.

## Work coordinate systems

A WCS is the part zero the machine works from, with its axis directions. The active WCS is the
one toolpaths will be programmed in. The viewport draws one WCS at a time as a triad (axes
labelled X, Y and Z): the one a WCS tool or the toolpath being edited works in, else the one
of the selection, else the one switched on in the outline. The
model origin's triad is hidden until switched on (`view_display_set(origin=True)`).

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

### Tool types

Twenty tool types, each created with its own command that takes the values its geometry needs:

| Type | Command | Values beyond diameter, flutes and lengths |
| --- | --- | --- |
| Flat, ball end mill | `tool_create_flat`, `tool_create_ball` | neck and reach |
| Bull nose end mill | `tool_create_bull` | corner radius, neck and reach |
| Tapered ball mill | `tool_create_tapered_ball` | taper angle (the diameter is the ball's) |
| Lollipop cutter | `tool_create_lollipop` | neck (narrower than the ball) and reach |
| Chamfer mill, engraving tool, countersink | `tool_create_chamfer`, `…_engraving`, `…_countersink` | included angle, tip flat |
| Dovetail cutter | `tool_create_dovetail` | angle from the axis, corner radius, neck |
| Slot mill | `tool_create_slot_mill` | corner radius, neck and reach (flute length = width) |
| Corner rounding mill | `tool_create_radius_mill` | form radius, pilot diameter |
| Face mill | `tool_create_face_mill` | insert corner radius, cutting edge angle |
| Thread mill, tap | `tool_create_thread_mill`, `tool_create_tap` | pitch, hand |
| Drill, spot drill | `tool_create_drill`, `tool_create_spot_drill` | point angle |
| Centre drill, counterbore | `tool_create_center_drill`, `tool_create_counterbore` | pilot diameter and length |
| Reamer | `tool_create_reamer` | — |
| Boring bar | `tool_create_boring_bar` | insert radius, bar (neck) and reach |

Every tool also keeps its material, coating, coolant through the tool, vendor and product number,
offsets, the holder it sits in, cutting data and presets. Values left out start from what is
typical for the type and diameter (a 140° point on a drill, ISO coarse pitch on a tap, a 0.8 mm
insert radius on a face mill). `tool_profile_get` returns the outline the preview draws, the
simulation shows and the multi-axis collision check uses, with the holder's.

```python
cc.tool_create_bull(diameter=10, corner_radius=1, flute_length=22, overall_length=72)
cc.tool_create_lollipop(diameter=6, neck_diameter=3.5, shoulder_length=20)
cc.tool_create_drill(diameter=8.5, tip_angle=140, coolant_through=True, vendor="Acme", product_id="D-085")
cc.tool_create_tap(diameter=10)                      # "M10×1.5 tap"
cc.tool_list(type="drill")
cc.tool_list(search="lollipop")
```

### Cutting presets

A tool keeps cutting data per workpiece material. Give the surface speed and chip load and the
spindle speed, feed, plunge and ramp feeds are worked out (rpm = vc × 1000 / πD, feed = rpm × fz × z);
any of them can be given instead. A toolpath's `preset` takes its speed and feeds from one.

```python
cc.tool_preset_set(number=1, name="Aluminium", workpiece="aluminium", surface_speed=300, feed_per_tooth=0.05,
                   stepover=4, stepdown=6)
cc.tool_preset_apply(number=1, name="Aluminium")     # make it the tool's own data
cc.toolpath_pocket(geometry=[floor], tool=1, preset="Aluminium")
cc.tool_feeds(number=1, surface_speed=120, feed_per_tooth=0.04)   # work out the tool's own
```

### Holders

Holders are stacks of cones and cylinders from the holder face up, shared by every machine's
tools. A tool's holder is drawn in the preview and the simulation, checked for collisions by the
multi-axis finish and reported when it touches the stock (`holder_contact`).

```python
h = cc.holder_create(name="ER32 collet chuck", segment_a=[50, 50, 22], segment_b=[45, 45, 30],
                     segment_c=[63.5, 63.5, 16])
cc.tool_edit(number=1, holder=h["id"])
cc.holder_list()
```

### The sample library

`tool_library_sample()` adds the sample library to a machine: two tools of every type with
typical geometry and cutting data, aluminium and steel presets on the milling tools, and six
holders. Numbers the machine already has are skipped (or overwritten with `replace=True`).
The same library is `samples/tools.json`, for `tool_library_import`.

Tools are checked against their machine: a spindle speed or feed above the machine's limits
is refused, and so is lowering a machine's limits below a tool in its library. Tools are named
`<machine id>/tool.<number>` for `selection.set`, so the inspector can show one.

A machine's `post` field names its post-processor and `post_properties` keeps the post's
settings for that machine (see "G-code and post-processors").

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

`cc.job_demo_open()` opens the built-in demo project: a part with a dome,
a 45° slope with a hole drilled square to it and a Ø24 bore, on its own 5-axis trunnion ("Demo
5-axis trunnion", added to the library with the sample tools if missing, and made active). Its
toolpaths face, rough, finish the walls, helically bore the Ø24 hole, finish the dome with the
multi-axis finish, spot and drill the angled hole on the tilted plane (3+2) and profile the
outline; the whole program simulates without gouges and posts with Fanuc. `cosmicdms.launch(demo=True)` starts the app
with it open; `launch()` starts with an empty job.

## CAD mode: modelling the part

The mode button at the far left of the ribbon switches between **CAD** (model the part) and
**CAM** (machine it); each has its own ribbon. **Drawings** and **Slicer** are coming. `ui_mode_set` does
the same from a script, and a ribbon tab of the other mode switches to it.

A part is a history of features, built in order: **sketches** (closed profiles of lines,
rectangles, circles, arcs and polygons on the top, front or right plane, or any plane given by
a point and normal, optionally offset), then **extrusions** and **revolutions** of them that add
to the part, cut from it, or make a separate body. Every feature keeps its settings: change
one and the part is rebuilt from there (earlier features are not worked out again). The
part's solid is the job's model "Part", so CAM picks its faces and machines it like an import,
and the history is saved in the job file.

In the app, **Sketch** (or a plane's right-click menu, or double-clicking a plane) turns the
view square to the plane; click in the viewport to draw (clicks snap to the grid and to points
already drawn; lines chain end to end and close on their first point), and edit any number in
the panel's list. **OK, then extrude** goes straight on to the extrusion, which previews live.
Double-click a feature in the tree to change it; right-click it to suppress, rename or delete.
The part's solids come from a B-rep modelling kernel, so they are exact until tessellated.

```python
cc.ui_mode_set(mode="cad")
sk = cc.cad_sketch(plane="top", entities=[{"type": "rectangle", "corner": [0, 0], "opposite": [80, 50]}])
cc.cad_extrude(sketch=sk["feature"]["id"], distance=20)                  # joined (the first is the body)
hole = cc.cad_sketch(plane="top", offset=20)["feature"]["id"]
cc.cad_sketch_circle(id=hole, center=[40, 25, 0], radius=8)
cc.cad_extrude_cut(sketch=hole, through_all=True)
ring = cc.cad_sketch(plane="front", entities=[{"type": "rectangle", "corner": [90, 0], "opposite": [100, 30]}])
cc.cad_revolve(sketch=ring["feature"]["id"], axis="y", angle=180, operation="new")
cc.cad_extrude(id="feature.2", sketch=sk["feature"]["id"], distance=25)  # change a feature
cc.cad_feature_suppress(id="feature.4")
print(cc.cad_feature_list(), cc.cad_mass_get())                          # volume, area, centre of mass
```

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
`top`, `bottom`, each `<name>_from` a reference plus `<name>_offset`). In the app the toolpath
panel previews the result as you change settings, working it out in the background.

The ribbon groups them as a job uses them: **2D**, **Drilling**, **3D** and **Multi-axis**.

| Command | What it cuts | Picks (`geometry`) |
| --- | --- | --- |
| `toolpath_face` | rows across the stock top, down to a height | none (or faces with `boundary="selection"`) |
| `toolpath_contour` | along edges or a face outline, offset by the tool radius | edges or faces |
| `toolpath_pocket` | offset rings inside floor faces or closed edges, around islands | floor faces or closed edges |
| `toolpath_slot` | the tool's centre along edges, down in steps | edges |
| `toolpath_chamfer` | a chamfer of a given width along edges, from the chamfer mill's angle and tip | edges or faces |
| `toolpath_trace` | the tool's centre along edges at one depth: engraving, scribing | edges |
| `toolpath_drill` | drill, spot, peck, chip break, tap or bore cycles (canned cycles in the G-code) | hole walls, rims or points |
| `toolpath_bore` | helical boring: the tool spirals down inside each hole, then circles the floor | hole walls or rims |
| `toolpath_thread` | thread milling, internal or external, right- or left-hand, with arc leads | hole or boss walls or rims |
| `toolpath_adaptive` | adaptive clearing: a steady tool load, deep cuts, no buried corners; rest machining | none (3D), or pockets with `boundary="selection"` (2D) |
| `toolpath_rough` | Z-level clearing of the stock around the whole part, rows and a pass around it | none |
| `toolpath_flat` | the model's flat floors at their exact heights | none (or faces to pick floors) |
| `toolpath_waterline` | Z-level passes around the part, for steep walls | none |
| `toolpath_raster` | parallel rows following the surface, for shallow areas | none (or faces with `boundary="selection"`) |
| `toolpath_scallop` | passes a constant stepover apart measured over the surface | none (or faces) |
| `toolpath_steep_shallow` | level passes on steep walls, scallop passes on shallow areas | none (or faces) |
| `toolpath_pencil` | one pass along the concave corners (ball-ended tools) | none (or faces) |
| `toolpath_spiral` | one pass spiralling out from (or in to) a centre | none (or faces) |
| `toolpath_radial` | passes radiating from a centre | none (or faces) |
| `toolpath_multiaxis` | a ball tilting as it follows the surface (see "4- and 5-axis machining") | none (or faces with `boundary="selection"`) |

```python
cc.toolpath_face(stepover=4, multiple_depths=True, stepdown=1)
cc.toolpath_adaptive(stepover=1.5, stepdown=12, stock_radial=0.3)         # 3D roughing
cc.toolpath_adaptive(tool=2, stepover=0.6, rest="previous")              # rest roughing
cc.toolpath_adaptive(boundary="selection", geometry=["model.1/face.8"])  # a 2D pocket
cc.toolpath_flat(tool=2, stepover=4)
cc.toolpath_scallop(tool=3, stepover=0.4)
cc.toolpath_steep_shallow(tool=3, stepover=0.4, threshold_angle=45)
cc.toolpath_pencil(tool=4)
cc.toolpath_pocket(geometry=["model.1/face.8"], stepover=2.5, stepdown=2, bottom_offset=-5)
cc.toolpath_chamfer(tool=11, geometry=["model.1/edge.3"], chamfer_width=0.5, tip_offset=0.25)
cc.toolpath_drill(geometry=["model.1/edge.12"], cycle="peck", peck_depth=3, bottom_offset=-12)
cc.toolpath_thread(tool=17, geometry=["model.1/face.1"], pitch=1.5, bottom_offset=-12)
cc.toolpath_bore(geometry=["model.1/face.1"], tool=1, pitch=1.5)   # a Ø24 hole with a Ø12 end mill
```

**Adaptive clearing** keeps the width of cut (the optimal load, `stepover`) steady by steering:
it spirals out from a helix entry, follows the walls, loops back through cleared ground in
narrow channels and links between areas at depth (routed through what is already cut, up to
`stay_down_distance`). A pass along the walls at each level takes off what is left there. Levels
follow the stepdown with extra ones at the model's flat areas (`flat_levels`). `rest="previous"`
clears only what the toolpaths before it leave, found by cutting them through a height map of
the stock.

**Scallop** and **steep and shallow** measure the stepover over the surface (on a grid of
tool-tip heights), so a wall gets as many passes as a floor of the same width. The finishing
strategies drop the tool onto the model, so they never gouge it; links between nearby passes
follow the surface too (`stay_down_distance`, default twice the diameter).

Each strategy takes the tool types that suit it, and picks the first such tool when none is named:

| Strategy | Tool types |
| --- | --- |
| contour, slot, helical bore | flat, bull nose, ball |
| face, flat | face mill, flat, bull nose |
| pocket, adaptive, 3D rough, waterline | flat, bull nose |
| raster, scallop, steep and shallow, spiral, radial | ball, bull nose, flat, tapered ball, lollipop |
| pencil, multi-axis finish | ball, tapered ball, lollipop |
| chamfer | chamfer mills, engravers, spot drills, countersinks |
| trace | engravers, chamfer mills, ball, tapered ball, flat, bull nose |
| thread | thread mills |
| drill | drills, spot and centre drills, reamers, taps, boring bars, counterbores, countersinks, flat end mills |

### Templates, order and regenerating

Save a toolpath's settings (not its geometry) as a template and start new toolpaths from it:
in the explorer (right-click a toolpath, **Save as template…**) and in the toolpath panel
(**Start from** on the Tool tab). Templates are kept with the machine library.

```python
cc.toolpath_template_save(id="toolpath.3", name="Aluminium adaptive")
cc.toolpath_template_apply(name="Aluminium adaptive", tool=2)
cc.toolpath_template_list(command="toolpath.adaptive")
cc.toolpath_move(id="toolpath.5", to=0)        # or by=-1 / by=1
cc.toolpath_regenerate()                       # after the model, stock or tools change
```

Drilling picks the cycle from the tool when none is given (tap for a tap, a dwell for spot and
centre drills, countersinks and counterbores, feed out for reamers and boring bars), and a tap's
pitch is the tool's.

### Trimming

`toolpath.trim` cuts finished toolpaths back to a boundary seen along the tool axis, keeping the
inside or the outside: a box (two `corners`), a polygon (`points`), a circle (`center`,
`radius`), the outline of selected faces or closed edges (`geometry`), the stock's or the
model's outline, and/or a band of heights (`top`, `bottom`, WCS Z). `containment` says what the
boundary holds: the tool centre (default), the whole cutter on the kept side (`tool_within`) or
the cutter reaching over by its radius (`tool_over`); `offset` grows or shrinks it. The pieces
left are linked by retracting to the retract (or clearance) height and plunging back, or, with
the default `link="auto"`, by feeding straight across short gaps (up to two tool diameters)
that stay on the kept side. `split=True` also adds a copy that keeps the other side.

Trims stay with the toolpath: regenerating applies them again, and removing one gives back what
it cut. Several toolpaths can be trimmed alike in one call. In the app, right-click toolpaths in
the explorer (Ctrl or Shift for several) and choose **Trim…**, or use **Trim** on the Toolpaths
tab: draw the box, polygon or circle in the viewport (Top view looks down the tool axis), and
the trimmed toolpaths preview in place before OK. A trimmed toolpath shows ✂ in the explorer.

```python
cc.toolpath_trim(id="toolpath.2", corners=[[-60, -40, 0], [0, 40, 0]])            # keep inside
cc.toolpath_trim(ids=["toolpath.3", "toolpath.4"], geometry=["model.1/face.12"],
                 keep="outside", containment="tool_within", offset=1)
cc.toolpath_trim(id="toolpath.5", center=[0, 0, 0], radius=20, split=True)         # both sides
cc.toolpath_trim(id="toolpath.6", top=-5)                                          # below Z -5
cc.toolpath_trim(id="toolpath.2", index=1, corners=[[-70, -40, 0], [0, 40, 0]])    # change trim 1
cc.toolpath_trim_list(id="toolpath.2")
cc.toolpath_trim_remove(id="toolpath.2")                                           # all of them
```

A drilling toolpath is trimmed by its holes: those whose centre is on the kept side stay.
On simultaneous multi-axis toolpaths links retract along the toolpath's Z.

## Showing, renaming, copying

Only the selected toolpath is drawn, plus any switched on. Models can be hidden too (hidden
models are still machined, but cannot be picked). Copy and paste work within a job and between
open jobs.

The explorer shows the job as Origin (the model origin's triad, hidden until switched on with
`view_display_set(origin=True)`), Models, Fixtures, NC Programs (one per machine, with its
post-processor) and Setups, each setup holding its stock, its WCS and its toolpaths in program
order, labelled with their tool (`[T1] Face top`). A WCS a toolpath alone uses (a 3+2
orientation, say) is part of that toolpath and not listed. Only one WCS marker shows at a time:
that of a toolpath being created or edited, else that of the selected setup, toolpath or WCS;
`view_wcs_visibility_set(id="wcs.2")` switches one on (replacing any other).
Right-click a toolpath to simulate just that one (`sim_start(toolpaths=["toolpath.2"])`). In the app, Ctrl and Shift pick several toolpaths
for Delete (the key or the menu), dragging one moves it, and a script does the same with
`toolpath_remove(ids=[…])` and `toolpath_move`.

```python
cc.toolpath_visibility_set(id="toolpath.2", visible=True)   # omit visible to toggle
cc.model_visibility_set(id="model.1", visible=False)
cc.toolpath_rename(id="toolpath.2", name="Rough the pocket")
cc.toolpath_duplicate(id="toolpath.2")
cc.job_clipboard_copy(id="toolpath.2")    # a toolpath, model or WCS; default: the selection
cc.job_tab_activate(index=1)
cc.job_clipboard_paste()
```

## Simulation

The simulation runs the toolpaths against the stock in machine time (feeds as programmed, rapids
at the machine's rapid rate) and shows the stock as it is cut, with flat and ball end mills.
When every toolpath works along one tool axis the stock is a height grid in that axis's frame;
when the tool tilts (multi-axis toolpaths, or 3+2 toolpaths along different axes) it is a voxel
model, cut by the tool swept along each move with its axis. `mode` forces one or the other.
Ball ends cut true round scallops in both. The voxel model keeps each cell's distance to the cut
surface (not just inside or outside), so domes, bores and scallops are drawn smooth between
cells; its default cell is a quarter of the smallest tool radius, coarser when the stock would
need more than about 8 million cells.

Simulating chosen toolpaths starts from the stock the toolpaths before them leave
(`start_from="previous"`, the default), so a finishing toolpath is seen cutting what the roughing
left; `start_from="stock"` starts from the raw stock. In the explorer, right-click a toolpath and
choose **Simulate** (that one alone) or **Simulate up to here**.

In the app the simulation has a timeline of the program, a band per toolpath with collisions
marked in red (hover for which toolpath runs, click or drag to go there), and under it: start,
previous toolpath, back a move, play/pause, forward a move, next toolpath, end, the time and a
playback speed from 0.5× to 1000×. The keys do the same while the pointer is over the view:
← and → step a move, Page Up and Page Down go by toolpath, Home and End to the ends, Space plays.
A readout at the top left shows the toolpath and tool, the move with its kind and feed, the tool
tip's position, the time left, the material removed and the collisions found (click one to go
there; they are also ringed in the view). Playback stops at a new collision unless told not to.
**Display…** chooses the toolpaths drawn (none, the current one up to the tool, or all), the
tool and holder, and the compare tolerance. **Compare** colours the stock by the thickness left
over the part (height-field simulations): red where it cuts into the part, green on size, yellow
to purple as more stock is left, with a legend.

```python
cc.sim_start(toolpaths=["toolpath.3"])  # from the stock toolpaths 1 and 2 leave; start_from="stock" for raw
cc.sim_start(cell=0.25)                 # all toolpaths; mode="voxel" forces voxels
cc.sim_seek(fraction=0.5)               # or time=seconds
cc.sim_jump(to="next_toolpath")         # start, end, previous_toolpath, next/previous_collision
state = cc.sim_get()                    # time, duration, move, toolpath, segments, removed_volume, events
cc.sim_speed(speed=50); cc.sim_play()   # plays in the app; sim_pause() stops
cc.sim_step(moves=-10)                  # back ten moves
cc.sim_display_set(paths="all", tool=False, stop_on_collision=False)
print(cc.sim_compare(tolerance=0.02))   # thinnest, thickest, gouged/on-size/stock-left areas
print(cc.sim_collision_check())         # rapids through material, the shank rubbing
print(cc.sim_gouge_check(tolerance=0.05))  # cells cut below the part, deepest first
cc.sim_stop()
```

Outside the simulation, selecting a toolpath shows the stock as the program leaves it after
that toolpath (the in-process stock, in purple), worked out in the background;
`view_display_set(in_process=False)` shows the stock's outline instead, and
`setup_stock_in_process_get(id="toolpath.3")` gives its volumes.

Any change to the job ends the simulation; start it again to run the changed program.

## G-code and post-processors

`post_gcode_export` posts the toolpaths with the machine's post-processor (see
[posts.md](../posts.md)): a Python post for a controller (Fanuc, Haas, LinuxCNC, Grbl, Mach) or,
when none is chosen, the built-in generic writer.

```python
cc.post_processor_select(post="fanuc")                # the active machine's post
cc.post_gcode_export(path="part.nc")                  # all toolpaths, in the units setting
cc.post_gcode_export(path="part.ngc", post="linuxcnc", units="mm", arc_tolerance=0.005)
text = cc.post_gcode_get(toolpaths=["toolpath.2"])["text"]
print(cc.post_limits_check())                         # travel, spindle and feed problems
cc.toolpath_cl_export(path="part.cl.json")            # the CL data a post reads
```

Python posts fit arcs (G2/G3, helical too) within `arc_tolerance`, write each controller's work
offsets, canned cycles, tool changes and coolant codes, and need Python 3 on the machine
running CosmicDMS. Check new programs against your machine before running them.

Each post offers settings (sequence numbers, tool preselection, LinuxCNC path blending, rigid
tapping, subroutines, …), kept per machine, and each toolpath can add a stop, an operator note
and pass-through G-code:

```python
for p in cc.post_properties_get()["properties"]:
    print(p["name"], p["value"], p["choices"])
cc.post_properties_set(name="line_numbers", value="true")
cc.post_properties_set(name="blend_finish", value="0.005")   # empty value: the default again
settings = cc.toolpath_edit(id="toolpath.2")                # regenerate with a stop and a note
cc.toolpath_contour(**dict(settings, id="toolpath.2", post_stop=True, post_comment="Flip the part"))
```

## 4- and 5-axis machining

A machine of kind `mill_4axis` or `mill_5axis` has rotary axes: their layout (both on the table,
table and head, or both on the head), names, travel and centres. A new 5-axis machine starts as a
trunnion with A tilting ±120° and a continuous C.

```python
cc.machine_create(name="UMC", kind="mill_5axis", max_feed=20000)
cc.machine_edit(primary_min=-35, primary_max=120, primary_centre=[0, 0, -60], secondary_centre=[0, 0, -60])
cc.machine_edit(layout="head_table", primary_axis="C", secondary_axis="B", pivot_length=180)
```

**3+2**: any strategy takes an `orientation`, another WCS whose Z is the tool axis; the program
stays in the toolpath's `wcs`, and the post turns the rotary axes once for it.

```python
cc.setup_wcs_create(name="Angled face", activate=False)
cc.setup_wcs_rotate(id="wcs.2", axis="x", degrees=30)
cc.toolpath_pocket(geometry=[face], orientation="wcs.2")
```

**Simultaneous**: `toolpath_multiaxis` finishes with a ball end mill that tilts as it follows the
surface; `tool_axis` says how (tilted from Z, from the surface normal, towards or away from a point
or line, or fixed), within `max_tilt` and turning at most `max_axis_rate` degrees per mm, leaning
back where its flutes or shank would touch the part.

```python
cc.tool_create_ball(diameter=6, feed=1500, spindle_rpm=12000)
tp = cc.toolpath_multiaxis(stepover=0.5, tool_axis="tilt", tilt_angle=15, lead_angle=5)
print(cc.toolpath_get(id=tp["id"], moves=True)["move_list"][10])   # {kind, to, feed, axis}
cc.sim_start()          # tilting tools are simulated on voxels ("mode": "voxel")
cc.sim_gouge_check()
cc.post_gcode_export(path="part.nc", post="fanuc")
```

How rotary work is written (tool centre point control, tilted planes or the machine's own
positions) is a post setting; see [posts.md](../posts.md#4--and-5-axis-output).

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
import cosmicdms

with cosmicdms.launch(sample=True) as cc:
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
