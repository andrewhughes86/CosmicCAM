# assets/ — instructions for agents

Everything under `assets/` ships with CosmicCAM, so every file here must be ours to ship.

## Rules
- **Original or openly licensed only.** Draw it yourself, or use a file under a licence
  compatible with MIT OR Apache-2.0 (e.g. CC0, CC-BY 4.0, OFL for fonts).
- **Never** use, trace, redraw or "restyle" icons, images, fonts, artwork, templates or
  screenshots from commercial CAM or CAD products — not as input, not as a reference image
  open beside your editor. Our look comes from `icons/ui/STYLE_GUIDE.md` alone.
- **Every file needs a row in `/ATTRIBUTION.md`** (path, author or source, licence).
  `cargo xtask assets` fails the build otherwise. Markdown, Python and text files are exempt.
- No screenshots of other software, ever. Review evidence (screenshots of CosmicCAM itself)
  is attached to PRs, never committed.
- Keep source masters (SVG) under version control; generated rasters are build outputs.

## Tooling
Asset lint and review helpers are Python 3.11+ scripts kept beside the assets they check.
Python is a development tool only, never an application dependency.

## Layout
- `icons/ui/` — command and UI icons. Read `icons/ui/AGENTS.md` and
  `icons/ui/STYLE_GUIDE.md` before adding, changing or assigning any icon.
- `icons/file/` — file-type icons the desktop shows for CosmicCAM files (`cosmic-job.svg` for
  `.cosmic` jobs). These are full-colour artwork for file managers, not command icons: the UI
  style guide's colour roles do not apply, but the originality and attribution rules do.
  `packaging/linux/install-file-type.sh` registers them on Linux.
