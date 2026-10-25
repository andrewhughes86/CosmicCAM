# Changelog

All notable changes to CosmicCAM. The format follows [Keep a Changelog](https://keepachangelog.com/)
and versions follow [Semantic Versioning](https://semver.org/) as described in
[docs/versioning.md](docs/versioning.md).

## [Unreleased]

## [0.2.0] - 2026-10-25

Toolpath strategies beyond the 2D contour, a built-in demo project, and job files that carry
their tools. Still pre-production: check the G-code before running it.

### Added

- **Toolpath strategies:** Face (rows across the stock top), Pocket (offset rings around
  islands, helical entry), Drill (drill, spot, peck, chip break, tap and bore, written as canned
  cycles G81/G82/G83/G73/G84/G85), 3D rough (Z-level clearing by zones around the part), Waterline
  (Z-level finishing of steep walls) and Raster (parallel finishing that follows the surface).
  The toolpath panel adapts its tabs to each strategy.
- **Demo project:** the app opens with `samples/demo.cosmic` (a part with a pocket and a boss, a
  WCS, a setup with stock, two tools and five toolpaths); Home → Demo project reopens it;
  `--empty` starts blank.
- **Job files carry their tools:** tools a job's toolpaths use are saved with it and added to the
  library on open where missing, so a shared job posts anywhere.
- **Recording:** `ui.screenshot` takes `settle: false` to capture mid camera move; captures are
  written off the UI thread. The README recording now runs at about 13 frames a second.

## [0.1.0] - 2026-10-25

The first versioned CosmicCAM: it plans 2D contour toolpaths and writes generic G-code, but has no
machine-specific post-processors or simulation yet, so its output must be checked before use.

### Added

- **Desktop app:** ribbon (Home, Setup, Machine, Tooling, Toolpaths, Simulation, Export), dockable
  explorer, inspector, viewport and console; dark and light themes; inches or millimetres
  throughout. The window has no system title bar: the tab strip carries the window controls.
- **Viewport:** wgpu renderer with shaded models and edges, grid, stock and WCS triads; a
  chamfered view cube whose faces, edges and corners each give a view; a row of view, projection
  and grid buttons.
- **Models:** STL and OBJ import with face, edge and vertex recognition; selection and inspection
  of faces, edges and vertices.
- **Work coordinate systems:** from planes, cylinders, edges and points, with flips, offsets,
  rotation and alignment.
- **Setups and stock:** setups with a name, machine, WCS (or the origin) and stock; box stock
  fitted with margins, of a fixed size, or from two corners, with rounding up.
- **Machines and tools:** several machine configurations, each with its own tool library; flat end
  mills drawn to scale.
- **Toolpaths:** 2D contour along edges or a face outline: heights, multiple depths and radial
  passes, stock to leave, climb or conventional, lead-in and lead-out arcs or lines, ramping,
  feed optimisation, entry point, tolerance that also follows true circles.
- **G-code:** generic G-code export with a safety block, tool changes, work offsets (G54…), length
  compensation and a motion code on every move.
- **Job files:** `.cosmic` files (format `CosmicCAM/1`) with document tabs; Open, Save, Save as;
  a file-type icon and Linux file association (`packaging/linux/install-file-type.sh`).
- **Scripting:** every feature is a command, reachable from the Python client (`python/cosmiccam`),
  the console and the control channel (`--control PORT`), including `ui.click` and `ui.type`.
- **Web:** the same app built for the browser (WASM).

[Unreleased]: https://github.com/andrewhughes86/CosmicCAM/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/andrewhughes86/CosmicCAM/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/andrewhughes86/CosmicCAM/releases/tag/v0.1.0
