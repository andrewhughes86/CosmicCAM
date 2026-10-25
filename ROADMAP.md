# CosmicCAM roadmap

| Phase | Milestone | Status |
| --- | --- | --- |
| 1 | Desktop UI & theme engine | **Done** (2026-10-24) |
| 2 | Model import & geometry ingestion | **Done except STEP/IGES** (2026-10-24) |
| 3 | Work coordinate system (WCS) creation | **Done** (2026-10-24) |
| 4 | Tool library & management | Planned |
| 5 | Stock setup & material definition | Planned |
| 6 | Toolpath creation engine | Planned |
| 7 | Material removal simulation & verification | Planned |
| 8 | Post-processor & G-code export | Planned |
| 9 | MCP agent integration | Planned |

## Phase 1 — what landed

- eframe 0.36 on the wgpu renderer; window title, app id, default and minimum sizes.
- `egui_dock` workspace: Viewport, Explorer, Inspector and Console panes that drag, stack,
  split and resize; `view.layout.reset` restores the default arrangement.
- Theme engine: semantic `theme::Tokens` with a modern dark palette (default) and a
  high-contrast light palette, applied over `egui::Visuals`; WCAG contrast is unit-tested.
  `view.theme.toggle` / `view.theme.set`.
- Contextual ribbon with Home, Setup, Tooling, Toolpaths, Simulation and Export tabs. Every
  button is a registered command; later-phase commands are listed but disabled and return
  "not supported yet".
- wgpu viewport: offscreen 4× MSAA render with depth, gradient background, grid, shaded sample
  part with edges, stock outline, WCS triad drawn on top, a clickable view cube, a screen
  orientation triad, orthographic/perspective projection, eased camera transitions and a
  frame-rate limit (`view.fps_limit.set`). Camera input goes only through `navigation::handle`.
- Engine: `Session::execute` runs commands under `catch_unwind` and restores state on a panic;
  `hostile_params_never_panic` covers every command.
- `cargo xtask ci`: fmt, clippy `-D warnings`, tests, assets, layers, ui-containers, wasm.

## Python API (2026-10-24)

- Control channel (`--control PORT`, token-authenticated JSON lines on 127.0.0.1); see
  `docs/control-protocol.md`.
- `python/cosmiccam`: stdlib-only client with `launch()` / `connect()` and one typed method per
  command, generated from the command registry by `cargo xtask api`.
- `docs/api/python.md`: the API guide, regenerated with the commands; CI fails when it is stale.
- Commands carry typed parameter schemas; unknown or mistyped parameters are rejected.
- New UI commands: camera get/set/orbit/pan/zoom, display toggles, ribbon tab, pane
  show/hide/focus, window size/full screen/maximise, quit, state/outline/selection/log queries,
  command listing and description; plus window screenshots over the protocol.

## Phase 2 — model import (2026-10-24)

- New L3 crate `CosmicCAM-io`: hostile-input-safe readers for binary and ASCII STL and
  Wavefront OBJ (polygons fanned, negative indices, `v/vt/vn` corners). File size, triangle and
  face-corner counts are capped; non-finite numbers and bad indices are errors naming the line.
- `geom::Mesh`: indexed triangle mesh with vertex welding, face normals and feature edges.
- Commands `model.import {path, units, name}`, `model.list`, `model.remove`. STL and OBJ carry no
  units, so `units` (mm, cm, m, in) scales to millimetres. Imported models appear in the job
  outline and inspector; the first import replaces the sample part. Failures use the new `import`
  error kind and leave the job unchanged.
- Viewport draws imported meshes flat shaded with feature edges (creases over 30°, open
  boundaries); large meshes are split across GPU buffers.
- The Import model button and Ctrl+I open a native file dialog (`rfd`); `ui.start` does the same
  over the control channel. Files dropped on the window are imported.
- Faces (planes, cylinders, freeform), edges (lines, circles) and vertices (corners) are
  recognised on every mesh. Clicking in the viewport selects a vertex, edge or face and the
  inspector shows its geometry; `selection.set` takes face, edge and vertex references.
- Still to do in Phase 2: the STEP/IGES B-Rep bridge (deferred). Shells are not modelled.

## Phase 3 — work coordinate systems (2026-10-24)

- A job holds up to 64 WCS (origin plus orthonormal X/Z axes, right-handed), one of them active.
- Commands: `setup.wcs.create`, `align` (see the two groups below), `top_center`,
  `bottom_corner`, `offset` (WCS or world axes), `rotate`, `rename`, `activate`, `remove`,
  `list`, `get`.
- Viewport draws a triad per WCS (active in full colour, others dimmed) with its name; WCS are
  listed under Setups in the outline and inspector.
- Ribbon Offset, Rotate and Rename open a dialog generated from the command's parameters.
- UI: Home now holds Import model and the WCS buttons; the view buttons moved off the ribbon to a
  floating column under the view cube (right-click Iso for every standard view).
- WCS definition in two groups, shared by `setup.wcs.create` and `setup.wcs.align`:
  **orientation** (`model`, `z_x`, `z_y`, `x_y` or `vectors`; a plane, cylinder or edge
  selected per axis; `flip_z` / `flip_x` / `flip_y`) and **origin** (`model`, `box` with 27
  box points, `point` from coordinates or a selection, `intersection` of three planes, a line
  and a plane, or two lines). Unused parameters are refused. The Create WCS and Align buttons
  open one panel with both groups, viewport selection and a live preview. Meshes carry
  recognised planes, cylinders and freeform patches, and straight and circular edges
  (`model.faces.list`, `model.edges.list`, `model.pick`, `model.edge.pick`).
- Every triad in the viewport labels its X, Y and Z arrows. The world (model) origin triad turns
  faint (short, thin, mostly grey) once a WCS exists, and stays there for reference.
- The viewport's view buttons are larger and sit against the right edge.
- `ui.click {x, y}` control method (and `cc.click`) for driving the UI in tests.
- Stock: `setup.stock.fit` sizes a box stock to the models with side, top and bottom margins;
  `setup.stock.get`; shown in the outline and viewport; WCS origin can sit on a stock box point.
- Later: dragging the triad in the viewport, saving WCS and stock in a job file (no job file
  format yet).

## Web app (2026-10-24)

- `apps/CosmicCAM-web`: the app compiled to WebAssembly with eframe's web runner, drawing with
  wgpu (WebGPU, or WebGL 2). Built with trunk from `index.html`.
- Browser file import: the Import button opens the browser's file picker and dropped files are
  read in the page; their bytes are staged in the session and imported as `upload://<name>`.
- The control channel is desktop-only (a stub in the browser).
- `.github/workflows/pages.yml` publishes the web app to GitHub Pages; `cargo xtask ci` lints
  the web build for wasm32.
- WebGL has no extra texture view formats, so the viewport renders straight into its texture
  there and encodes sRGB in the shader. GPU validation errors are logged and stop only the
  viewport (with a message) instead of panicking.
- In the browser a panic cannot be caught (wasm aborts), so the session's last-resort panic
  guard does not apply there; errors returned by commands work as on the desktop.

## Not started yet (from AGENTS.md)

Crash reports, `CosmicCAM-cli`, job file format.

## Ribbon icons (2026-10-24)

40 original SVG icons in `assets/icons/ui/design/svg/` following the approved
`assets/icons/ui/STYLE_GUIDE.md`; embedded at build time, coloured per theme and rasterised with resvg.
