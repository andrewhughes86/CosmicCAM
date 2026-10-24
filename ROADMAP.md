# CosmicCAM roadmap

| Phase | Milestone | Status |
| --- | --- | --- |
| 1 | Desktop UI & theme engine | **Done** (2026-10-24) |
| 2 | Model import & geometry ingestion | Next |
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

## Not started yet (from AGENTS.md)

Crash reports, `CosmicCAM-cli`, job file format.

## Ribbon icons (2026-10-24)

40 original SVG icons in `assets/icons/ui/design/svg/` following the approved
`assets/icons/ui/STYLE_GUIDE.md`; embedded at build time, coloured per theme and rasterised with resvg.
