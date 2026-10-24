# CosmicCAM icon style guide

> **Status: approved (2026-10-24).** The first 40 ribbon icons follow it.

CosmicCAM icons are **"shop drawing"** icons: clean technical line work, like a sketch on a
machinist's drawing, with a single accent that marks what the command does. They are drawn for
this project; they are not modelled on any other product's icons.

## 1. Grid and sizes
- Masters are **32 × 32** SVG (`viewBox="0 0 32 32"`), shown at 32 px on large ribbon buttons
  and gallery tiles, 16 px on small buttons and menus.
- **Live area 28 × 28** (2 px padding). Badges may use the padding at the bottom-right.
- Keyline shapes, centred: square 24 × 24, circle Ø 26, landscape 28 × 20, portrait 20 × 28.
- Snap geometry to whole or half pixels so 2 px strokes land crisply.

## 2. Line
- **2 px strokes** at 32 px, round caps and round joins. No hairlines, no strokes under 1.5 px.
- Corners of solid shapes get a 1 px radius; tools and paths keep their true geometry.
- Hidden or construction lines: 2 px dashed, 2 on / 2 off, in INK at 50 %.

## 3. Colour roles
Icons never contain literal colours. They use these placeholders, replaced from the active
theme's tokens at load time, so every icon works in the dark and light themes:

| Placeholder | Role | Dark theme | Light theme |
| --- | --- | --- | --- |
| `#INK` | All outlines and line work | `text` | `text` |
| `#FACE` | Lit faces of solids, sheets | `icon_face` | `icon_face` (white) |
| `#SHADE` | Faces turned from the light | `icon_shade` | `icon_shade` |
| `#ACCENT` | The one element that says what the command does | `accent` | `accent` |
| `#OK` / `#STOP` / `#WARN` | Verify, halt and caution states only | `success` / `error` / `warning` | same |
| `#BG` | Knock-out rings and glyphs on badges | `panel_bg` | `panel_bg` |

- **One accent region per icon** — the toolpath, the cutting edge, the new thing.
- Disabled icons are drawn by the UI at reduced opacity; never draw a separate disabled icon.
- No gradients, shadows, glows, textures or transparency other than the dashed construction line.

## 4. Projection and light
- 3D subjects (stock, parts, cutters in context) are **2:1 pixel isometric**: receding edges
  run 2 px across for every 1 px down (26.6°), so they land crisply on the pixel grid. Z is up;
  +X recedes to the lower right, +Y to the lower left.
- Light comes from the top: top faces `#FACE`, the two side faces `#SHADE`.
- 2D subjects (documents, transport controls, menus) are flat and front-on.

## 5. Canonical bases
Every icon starts from one of these bases. Copy the geometry from the icon in the last column:

| Base | Subject | Used by | Geometry from |
| --- | --- | --- | --- |
| Block | Isometric box, 20 × 14 footprint | job, model, stock, fit, margins | `setup_stock_box` |
| Cylinder | Isometric round bar | cylinder stock, round parts | `setup_stock_cylinder` |
| Cutter | Side view: shank, flutes as two diagonal strokes, tip variant (flat, ball, bull, chamfer, drill) | tools, library, feeds | `tool_create` |
| Path | Accent polyline over a faint Block top face: zig-zag (raster), offset loops (pocket), outline (profile), stacked slices (waterline), plunge dashes (drill) | toolpath strategies, CL data | `toolpath_pocket` |
| Triad | Three axis strokes from one origin dot; origin dot is `#ACCENT` | WCS create, align, offset | `setup_wcs_create` |
| Document | Sheet with folded top-right corner | open, save, import, export, G-code | `job_new` |
| Transport | Flat play / pause / step glyphs | simulation playback | `sim_play` |
## 6. Modifier badges
Badges sit in a **12 × 12** circle centred at (25, 25), ringed with a 1.5 px `#BG` stroke that
knocks the base out behind them:

| Badge | Meaning | Colour |
| --- | --- | --- |
| Plus | create / new | `#ACCENT` |
| Arrow in | import / load | `#ACCENT` |
| Arrow out | export / write | `#ACCENT` |
| Check | verify / passed | `#OK` |
| Cross | collision / gouge | `#STOP` |
| Dial | speed / limits | `#INK` |
| Cycle arrow | reset / restore | `#ACCENT` |

At most one badge per icon.

## 7. 16 px rule
The 16 px size uses the same master scaled down **unless** it has more than three separate
details or any gap under 2 px at 32 px. Then draw `<name>.16.svg` on a 16 grid with 1.5 px
strokes and drop the least important detail (usually the base face).

## 8. Don'ts
- No letters or words inside icons (the label does that), except an axis letter on a Triad.
- No product logos, mascots or imitations of another application's icon set.
- No more than one accent region; no badge stacking.
- No perspective other than isometric; no photographic detail.

## 9. Named finished references
Compare every new icon with these before drawing:

1. `job_new` — Document + Plus badge
2. `setup_stock_box` — Block
3. `tool_create` — Cutter (flat) + Plus badge
4. `toolpath_pocket` — Path (offset loops)
5. `sim_play` — Transport (play)

Review sheet: `ICON_SHEET=/tmp/sheet.png cargo test -p CosmicCAM-ui-egui icon_sheet` renders every
icon at 64, 32 and 16 px in both themes.
