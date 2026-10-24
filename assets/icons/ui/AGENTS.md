# assets/icons/ui — instructions for agents

## Workflow for a new icon
1. Read `STYLE_GUIDE.md` in full and open the named finished references it lists.
2. Start from the canonical base for the subject (block, cylinder, cutter, path, triad,
   document, transport); copy its geometry from the icon the guide names for that base. Never start from a blank canvas when a base exists, and never invent
   a parallel style.
3. Draw the 32 px master as `design/svg/<name>.svg`; add `design/svg/<name>.16.svg` only
   when the guide's 16 px rule says the master will not survive reduction.
4. Name the file after the command id with dots replaced by underscores:
   `toolpath.pocket` → `toolpath_pocket.svg`.
5. Use only the colour role placeholders from the guide; never a literal colour.
6. Add the file's row to `/ATTRIBUTION.md` (author: CosmicCAM contributors, licence:
   MIT OR Apache-2.0).
7. A file named after the command id is picked up automatically. Set `icon: Some("<name>")` on
   the `CommandSpec` only to reuse another command's icon. Every ribbon command must have one
   (a test enforces it).
8. Check it in the app at 32 and 16 px, in both themes, enabled and disabled, and attach
   that screenshot to the PR.

## Compilation
`crates/ui-egui/build.rs` embeds every master; `crates/ui-egui/src/icon_svg.rs` substitutes the
colour roles from the active `theme::Tokens`, rasterises with resvg at the display scale and
caches the textures per theme and size. Its tests check that every master renders in both themes
and uses only colour roles.
