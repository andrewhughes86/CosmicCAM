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
