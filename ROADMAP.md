# CosmicCAM roadmap

| Phase | Milestone | Status |
| --- | --- | --- |
| 1 | Desktop UI & theme engine | **Done** (2026-10-24) |
| 2 | Model import & geometry ingestion | **In progress** (STL/OBJ landed 2026-10-24) |
| 3 | Work coordinate system (WCS) creation | Planned |
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
- Still to do in Phase 2: the STEP/IGES B-Rep bridge, topology mapping, viewport picking.

## Not started yet (from AGENTS.md)

Crash reports, `CosmicCAM-cli`, job file format.

## Ribbon icons (2026-10-24)

40 original SVG icons in `assets/icons/ui/design/svg/` following the approved
`assets/icons/ui/STYLE_GUIDE.md`; embedded at build time, coloured per theme and rasterised with resvg.
