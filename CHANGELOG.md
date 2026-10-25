# Changelog

All notable changes to CosmicCAM. The format follows [Keep a Changelog](https://keepachangelog.com/)
and versions follow [Semantic Versioning](https://semver.org/) as described in
[docs/versioning.md](docs/versioning.md).

## [Unreleased]

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

[Unreleased]: https://github.com/andrewhughes86/CosmicCAM/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/andrewhughes86/CosmicCAM/releases/tag/v0.1.0
