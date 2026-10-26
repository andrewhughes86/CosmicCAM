# CosmicCAM roadmap

| Phase | Milestone | Status |
| --- | --- | --- |
| 1 | Desktop UI & theme engine | **Done** (2026-10-24) |
| 2 | Model import & geometry ingestion | **Done except STEP/IGES** (2026-10-24) |
| 3 | Work coordinate system (WCS) creation | **Done** (2026-10-24) |
| 4 | Tool library & management | **Done** (20 tool types, presets, holders, sample library, 4/5-axis kinematics 2026-10-25) |
| 5 | Stock setup & material definition | **In progress** (setups, box stock options 2026-10-25) |
| 6 | Toolpath creation engine | **In progress** (contour, face, pocket, drill, helical bore, 3D rough, waterline, raster, 3+2, multi-axis finish 2026-10-25) |
| 7 | Material removal simulation & verification | **In progress** (height-field and voxel stock simulation, gouges, collisions 2026-10-25) |
| 8 | Post-processor & G-code export | **In progress** (Python posts, arc fitting, limits, 4/5-axis output 2026-10-25) |
| 9 | MCP agent integration | Planned |
| 10 | Scripting & add-ons (scripts panel, add-on packages, isolation) | Planned |
| 11 | VS Code integration & extension (run/debug scripts, build add-ons) | Planned |

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

## Post-processors (2026-10-25)

- Posts are Python modules run out of process (`python3 -I`, CL data JSON on stdin, two-minute
  timeout): a broken or missing post is an error, never a crash. The shipped posts (`posts/`)
  are copied to `<data folder>/posts/builtin/`; your own go in `<data folder>/posts/`.
- `cosmiccam_post.Post`: configuration as class attributes (units, framing, line numbers,
  comments, number formats, modal output, arcs IJK/R and helical, linearising, tool changes and
  preselect, length offsets, spindle and coolant codes, work offsets incl. extended ones, canned
  cycles or expanded drilling, rigid tapping, dwell units, homing) and overridable hooks.
- Posts: fanuc, haas, linuxcnc, grbl, mach (plus the Rust generic writer).
- Rust: CL data (`toolpath.cl.export`), arc fitting (`geom::arcfit`, lines kept where straight,
  chords checked against the tolerance, helices), limit checks (`post.limits.check`);
  `post.processor.list` / `select` / `folder`; machines carry their post.
- Post settings: posts declare `Property` settings (merged along the class hierarchy), shown in
  the Post-processor dialog by group and kept per machine (`post.properties.get` / `set`,
  `Machine::post_properties`). Common: sequence numbers, word spacing, comments, header tool list,
  arcs, rapids as feed, preselect, manual changes, tool-change position, restart blocks, safe
  retract, units. LinuxCNC: G64 P/Q per roughing or finishing or G61, G33.1 or G84 tapping, M64/M65
  coolant outputs, o-word subroutines, `(MSG, …)`, M2/M30, G53/G28/clearance retract.
- Per-operation post options (toolpath Post tab): stop before, operator note, G-code before and
  after.
- Still to do: more controllers, machine simulation of the rotary axes.

## 4- and 5-axis machining (2026-10-25)

- Machines: kinds `mill_4axis` and `mill_5axis` with their kinematics (`library::Kinematics`):
  table-table, head-table or head-head layout, A/B/C axes with travel and direction, rotary
  centres from the WCS origin and the head's pivot length; `machine.create` / `machine.edit`
  parameters and the generated dialog.
- `geom::kinematics`: the rotary angles for a tool direction (two solutions where there are, fitted
  to travel and nearest the current angles, the free axis kept at a singularity) and the machine's
  linear positions for given angles. `geom::collide`: a 3D-binned mesh index with segment–triangle
  clearance. `geom::voxel`: voxel stock cut by flat and ball tools swept along a move with any
  axis, part cells for gouges, surface nets for display. `geom::cutter`: a ball drop cutter that
  also gives the contact point.
- Toolpaths record their tool axis (`Toolpath::axis`) and, for simultaneous ones, each move's
  (`axes`, saved in job files). Any strategy takes `orientation` (3+2): it works along that WCS's
  Z while programming in its own WCS.
- Ball end mills (`tool.create.ball`), for Raster and the new **Multi-axis finish**
  (`toolpath.multiaxis`): raster rows with a ball drop cutter, the axis from a rule (tilt, normal,
  towards/away from a point or line, fixed) within a tilt limit and an axis rate, leaned back
  towards Z where the flutes or shank would touch the part (at its points and along each move
  between them). Rows (here and in Raster) are refined
  over edges so a move never cuts the corner between a sample on top and one below.
- CL data: rotary angles per operation, the tilted work plane (with ZXZ Euler angles), WCS and
  machine positions for 3+2, and per-move axis, angles, machine position and inverse-time feed for
  simultaneous operations, split where joint interpolation strays beyond the tolerance
  (`engine::fiveaxis`). The generic post and 3-axis machines refuse tilted work.
- Posts: tilted plane (Fanuc G68.2/G53.1, Haas G254/G255), tool centre point control (G43.4,
  G234) or machine positions with G93 (LinuxCNC, Mach); Grbl stays 3-axis.
- Simulation: tools are told apart by machine and number (two machines' T1 were mixed up);
  voxel stock in world coordinates when the tool tilts (`sim.start` `mode`), the tool
  drawn along its axis; gouges as part cells removed. The toolpath panel's Multi-axis tab picks the
  orientation and the tool axis rule; the viewport draws the tool axis along multi-axis toolpaths.
- Still to do: swarf and flowline strategies, 4-axis wrapping, holder and machine collision checks,
  the rotary axes in the simulation (the part turning), bull-nose tools.

## Simulation (2026-10-25)

- Stock simulation on a height grid ("Z-map", `geom::zmap`) in the toolpaths' WCS: each move
  sweeps the flat end mill's bottom through the grid. Playback in machine time (programmed feeds,
  rapids at the machine's rapid rate) with speed, scrubbing and stepping; going back replays.
- `sim.start`, `sim.play`, `sim.pause`, `sim.step`, `sim.seek`, `sim.speed`, `sim.stop`, `sim.get`;
  the Simulation tab and a playback bar over the viewport. The cut stock is drawn shaded (cut
  areas, uncut stock and gouges in their own colours) with the tool; the part, stock box and
  toolpaths are hidden while it runs.
- `sim.gouge.check`: cells cut below the part's surface (measured against the lowest surface
  around each cell, so cuts beside walls are not flagged); `sim.collision.check`: rapids that
  remove material and cuts deeper than the flutes (cells wholly under the cutter only).
- Still to come: bull-nose tools, holder geometry, fixtures, comparing the
  cut stock with the part (rest material), background computation for long programs.

## Tool library (2026-10-25)

- `engine::tooling`: twenty tool types, each with the values its geometry needs (corner/form
  radius, point angle, tip flat or pilot diameter and length, taper, neck and reach, thread pitch
  and hand), material, coating, coolant through, vendor and product number; typical values per
  type and diameter; per-type checks. `Tool::profile` is the outline as a solid of revolution:
  the preview, the simulation's tool and the multi-axis collision body are all drawn from it.
- Cutting presets per workpiece material (`tool.preset.set` / `remove` / `apply`), worked out from
  surface speed and chip load; `tool.feeds` does the same for the tool's own data; toolpaths take
  `preset`.
- Holders (`holder.create` / `edit` / `remove` / `list`), shared by the machines; a tool's holder
  is drawn, checked by the multi-axis finish and reported by the simulation (`holder_contact`).
- `tool.create.<type>` for every type with only its values; `tool.list` filters by type and
  search; `tool.profile.get`; export and import carry holders; `tool.library.sample` (and
  `samples/tools.json`): two tools of every type with presets and holders.
- Library pane: New tool menu of all types, type filter and search, columns for the type's
  values, the holder, a preview from the real outline with the holder, the type's values and the
  presets (Use, delete), the holder library; the tool dialog previews what is typed through the
  engine.
- Strategies take the tool types that suit them and pick a fitting tool by default; drilling picks
  its cycle and a tap's pitch from the tool.
- Face recognition finds spheres (domes, ball pockets) as one face.
- Still to do: bull nose and cone drop cutters (3D strategies with those), chamfer and thread
  milling toolpaths, holder collision checks in the 3-axis strategies.

## Demo project (2026-10-25)

- `samples/demo.cosmic` (from `samples/demo-part.obj`) is built in and opens at start-up while
  CosmicCAM is pre-release; `job.demo.open` (Home → Demo project) reopens it; `--empty` starts
  blank.
- Job files save the tools their toolpaths use; opening adds missing ones to the library and moves
  toolpaths on unknown machines to the active one.
- Faster screenshots for recordings (`settle: false`, background PNG writing).
- Rebuilt for 5-axis (`cargo xtask demo` makes the part, the job and `samples/tools.json`): a base
  with a dome, a block with a 45° slope carrying a Ø5 hole square to it and a Ø24 bore; the
  "Demo 5-axis trunnion" machine with the sample library (added on opening if missing); face,
  rough, waterline, helical bore, a 5-axis finish of the dome, 3+2 spot and peck drilling of the
  angled hole, profile; checked free of gouges and collisions when built.

## Toolpath strategies (2026-10-25)

Chosen from the strategy families CAM systems commonly offer (public documentation and general
CAM practice; no product's behaviour, text or names copied):

- `toolpath.face`: rows across the stock (or model, or selected faces) top, overrun past the edges,
  zigzag or one way, optional depth levels.
- `toolpath.pocket`: offset rings inside floor faces or closed edges, islands kept, from the
  middle out; helical entry, links inside the pocket, optional finishing pass.
- `toolpath.drill`: holes from cylinder walls, circular rims or points; drill, spot (dwell), peck,
  chip break, tap (pitch) and bore; canned cycles in the G-code.
- `toolpath.rough`: Z-level 3D roughing: at each level, the stock minus the part's shadow above
  it grown by the tool radius and stock, cleared in zones of back-and-forth rows plus a pass around
  the part.
- `toolpath.waterline`: Z-level finishing around the part (exact for flat end mills).
- `toolpath.raster`: parallel finishing with a drop cutter (flat end mills).
- geom: `region` (shapes with holes: offsets, booleans, hatching) and `cutter` (shadow above a
  height, drop cutter). Still to come: adaptive (constant engagement) clearing, scallop, pencil,
  rest machining, chamfer, thread milling, ball and bull-nose tools for the 3D strategies.

## Workflow recording (2026-10-25)

- README shows a recording of the whole workflow (new job → import → WCS → setup and stock →
  2D contour). The control channel gained `ui.type` for scripted typing.

## Setups and stock (2026-10-25)

- `setup.create` / `setup.edit` / `setup.activate` / `setup.remove` / `setup.list` / `setup.get`:
  a setup has a name, a machine, a WCS (or the origin) and its stock. Create setup leads the
  Setup tab and opens a panel at the right; setups appear under Setups in the explorer
  (double-click to activate and edit). Saved in job files.
- Box stock (`setup.stock.box`): fit to the models with side/top/bottom margins, a fixed size
  placed on the models (top, centre or bottom), or two corners; optional rounding up. The stock
  is drawn see-through while it is being defined.
- Contour tolerance also resamples circular edges and cylinder rims from the true circle, so a
  tighter tolerance gives closer points on curves.

## Job files and document tabs (2026-10-24)

- `.cosmic` job files (`CosmicCAM/1`, JSON with packed meshes and toolpath moves): `job.open`,
  `job.save`, `job.save.as`. Open, Save and Save as on Home (Ctrl+O, Ctrl+S, Ctrl+Shift+S);
  `.cosmic` files dropped on the window open too. Hostile files are refused, never crash.
- Each open document is a tab in the centre of the title strip; hovering shows its close
  button. Unsaved changes are marked and asked about on close. `job.tab.list`,
  `job.tab.activate`, `job.tab.close`.
- Grid and projection moved to the viewport's button row; layout restore moved to Preferences.
- `.cosmic` files have their own file-type icon (`assets/icons/file/cosmic-job.svg`);
  `packaging/linux/install-file-type.sh` registers it and CosmicCAM as their program, and
  `CosmicCAM job.cosmic` opens a file from the command line.
- 2D contour: entry point (automatic, nearest the origin, the WCS origin or a picked point);
  G-code names G0 / G1 on every line that moves.

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
  floating row at the bottom centre of the viewport (right-click Iso for every standard view).
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

## Phase 4 — machines and tool libraries (2026-10-24)

- Machine configurations (name, travel, spindle and feed limits, tool changer size), each with
  its own tool library; a Machine ribbon tab with New, Edit (Copy and Delete in the menu) and the
  library pane. Commands `machine.create/edit/duplicate/remove/activate/list/get/library.open`.
- Flat end mills: geometry (diameter, flutes, length of cut, stick-out, shank), material and
  coating, cutting data (spindle speed, feed, plunge feed, stepover, stepdown) and offset
  registers. Commands `tool.create`, `tool.create.flat`, `tool.edit`, `tool.feeds`,
  `tool.duplicate`, `tool.remove`, `tool.list`, `tool.get`, `tool.library.import/export` (JSON).
  Ball, bull, chamfer, drill, spot drill, face mill, tap and thread mill are placeholders.
- Tool library pane (a tab beside the Explorer): machine picker, tool table, new / edit /
  feeds / copy / delete; the selected tool shows in the inspector.
- The library is saved after every change (eframe storage: a file on the desktop, localStorage
  on the web) and loaded at start-up; saved data is validated and bad entries are skipped.
- To-scale tool drawing (shank, fluted length in the accent colour, Ø / flute / overall
  dimensions) in the New and Edit tool dialogs, updating as values are typed.
- The tool library is its own resizable window with a preview of the tool under the pointer or
  the selected one; Up / Down step through the list.
- Still to do in Phase 4: the other tool types, holders.

## Settings and units (2026-10-24)

- Settings on the Home tab (`settings.open`, `settings.get`, `settings.units.set`). Units are
  inches (default) or millimetres, saved with the app's storage.
- The engine stays in millimetres; the inspector, dialogs (typed values convert to mm), the tool
  library, the WCS panel, the camera readout, the status bar and the grid spacing (0.5 in / 10 mm)
  follow the preference. Commands and their results are always millimetres; length and feed
  parameters are marked `Length` / `Rate` in the schema.
- The Machine tab sits between Setup and Tooling.

## Phase 6 — first toolpath: 2D contour (2026-10-24)

- `toolpath.contour`: follows edges, a face outline or a hole/boss rim at depth, offset by the
  tool radius plus stock (round corners via the MIT/Apache `i_overlay` offset engine, guarded).
  Outside / inside / on, climb / conventional, reverse, radial passes and stepover, spring passes,
  radial and axial stock, multiple depths, feed reduction at direction changes (sharp corners and
  tight arcs), arc or line leads, ramping along the lead-in or plunging, retract to retract or
  clearance height or stay down, rapid or feed links, chord tolerance.
- Five heights (clearance, retract, feed, top, bottom), each a reference (selected contour, model
  top/bottom, stock top/bottom, origin) plus an offset, worked out in the WCS; new height icons.
- `toolpath.list`, `toolpath.get` (with moves), `toolpath.edit` (stored settings), `toolpath.remove`;
  toolpaths in the outline with length and time estimates, drawn in the viewport by move kind.
- The 2D contour tile opens a toolpath panel docked at the right: Tool, Multi-axis (placeholder),
  Geometry (click edges/faces), Heights, Passes, Linking tabs, a live preview, OK / Cancel.
  Double-clicking a toolpath in the explorer edits it.
- Still to do in Phase 6: pocketing, drilling, 3D strategies, CL data export, gouge checks.

## Phase 8 — generic G-code (2026-10-24)

- A built-in generic G-code writer (`crate::post`), until the Python posts and machine
  configurations replace it: safety block, tool change, spindle and coolant, the toolpath's WCS
  work offset (G54…), G43 length compensation, modal G0 / G1 moves in inches or millimetres, end
  block; optional line numbers and program number.
- `post.gcode.export` (Export tab: a save dialog on the desktop, a download in the browser) and
  `post.gcode.get` (the program as text). Toolpaths without a spindle speed are refused.
- Still to do in Phase 8: Python post-processors and machine configurations, arcs (G2/G3),
  limit checks.

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
