# Post-processors

A post-processor turns CosmicDMS's toolpaths into the G-code one machine expects. Posts are
Python modules you can read and change; CosmicDMS does the heavy work in Rust (arc fitting,
limit checks, coordinate transforms) and hands each post plain data.

## How posting works

1. CosmicDMS builds **CL data** (cutter location data, format `CosmicDMS-CL/1`): every
   operation's moves in its work coordinate system, in millimetres, with runs of short moves
   fitted as arcs within a tolerance (default 0.01 mm), drilling cycles, tools, spindle speeds,
   coolant and the machine's limits.
2. It starts `python3 -I -B run_post.py` as a separate process, sends the CL data on standard
   input and reads the program back. A missing interpreter, an error in a post or a post that
   runs for more than two minutes is reported as an error; it cannot take the app down.
3. The post's `Post` class converts units, formats numbers, keeps modal state and writes the
   program in its controller's dialect.

The **generic** post is written in Rust and needs no Python; it is what a machine uses until
another post is chosen.

## Choosing a post

- In the app: **Export → Post-processor** lists every post; the one ticked is used by **Export
  G-code** for the active machine.
- From Python or the console:

```python
cc.post_processor_list()                       # ids and descriptions
cc.post_processor_select(post="haas")          # the active machine's post
cc.post_gcode_export(path="part.nc")           # posts with the machine's post
cc.post_gcode_export(path="part.ngc", post="linuxcnc", arc_tolerance=0.005)
cc.post_limits_check()                         # travel, spindle speed and feed problems
cc.toolpath_cl_export(path="part.cl.json")     # the CL data itself
```

## Post settings

Each post offers settings: sequence numbers, comment style, arcs, tool preselection, a tool-change
position, manual tool changes, restart-safe operations, units and the post's own options. In the
app, **Export → Post-processor** shows the chosen post's settings below the list, grouped by
section; changes are kept with the machine (in your machine library), so every job posted for that
machine uses them. **Restore the post's defaults** clears them. Choosing another post starts its
settings afresh.

```python
cc.post_properties_get()                                    # each setting with its value
cc.post_properties_set(name="line_numbers", value="true")
cc.post_properties_set(name="blend_finish", value="0.005")
cc.post_properties_set(name="blend_finish", value="")      # back to the post's default
```

Values are text: `true`/`false`, numbers, or one of a setting's choices. The post checks them when
it runs and reports a value it cannot use as an error; a setting it does not know is a warning.

### Common settings

| Setting | Meaning |
| --- | --- |
| `line_numbers`, `line_number_start`, `line_number_step` | N words |
| `separate_words` | `G0 X1 Y2` or `G0X1Y2` |
| `comment_style` | `paren`, `semicolon` or `none` |
| `repeat_motion_codes` | G0/G1/G2/G3 on every move |
| `write_machine_info`, `write_tool_list` | Header comments: machine and post; each tool with D, corner radius, flutes and its lowest Z |
| `arcs`, `arc_format`, `max_arc_radius` | G2/G3 with I J or R; larger arcs become lines |
| `rapid_feed` | Rapids as G1 at this feed (0 writes G0) |
| `preselect_next_tool`, `manual_tool_change`, `tool_change_xy`, `spindle_dwell` | Tool changes |
| `optional_stop` | M1 before each operation |
| `restart_blocks` | The safe-start block, tool, spindle and coolant repeated at each operation so a run can start there |
| `safe_retract`, `return_home_xy` | Home (machine top) or clearance before tool changes and at the end; X/Y home at the end |
| `units` | Follow the job, or always `mm` or `in` |

### LinuxCNC settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `blending` | `g64` | `g64` path blending, `g61` exact stop, or `controller` (nothing written) |
| `blend_rough`, `blend_finish` | 0.05, 0.01 mm | G64 P for roughing operations (3D rough, pocket, face, or any with stock to leave) and for the rest |
| `naive_tolerance` | 0 | G64 Q, merging collinear moves; 0 leaves it out |
| `tapping` | `g33.1` | Spindle-synchronised G33.1 Z K rigid tapping, or the G84 cycle |
| `through_tool_pin`, `air_pin` | -1 | Digital outputs switched with M64 / M65 P for through-tool coolant and air blast |
| `subroutines` | off | Each operation as `o100 sub … o100 endsub`, defined after the header and called in order |
| `end_code` | `M2` | `M2` or `M30` |
| `retract_mode` | `g53` | `G53 G0 Z0`, `G28`, or stay at clearance |

Manual tool changes and operator notes on LinuxCNC are written as `(MSG, …)`, which the controller
shows on screen.

## 4- and 5-axis output

On a 4- or 5-axis machine (a machine whose kind is `mill_4axis` or `mill_5axis`, with its rotary
axes set on the Machine tab), every operation carries the rotary angles that point the tool where
it works, and each post writes them its controller's way. Two kinds of operation tilt the tool:

- **3+2** (positional): a toolpath with an `orientation` works along another WCS's Z. The rotary
  axes turn once, with the tool up, and stay put while it cuts.
- **Simultaneous**: the Multi-axis finish tilts the tool continuously.

Each is written in one of three styles, chosen in the post's settings (Multi-axis group):

| Style | 3+2 | Simultaneous | Needs |
| --- | --- | --- | --- |
| `plane` | A tilted work plane command (`G68.2 … G53.1` / `G69`, or `G254` / `G255` on Haas) and the moves in its coordinates, arcs and canned cycles included | — | The control's tilted-plane option |
| `tcp` | Tool tip positions in the WCS with tool centre point control (`G43.4 H`, `G234 H`) | Tool tip positions with the rotary angles each block, feeds per minute | The control's TCP option, set up with the machine's pivot distances |
| `machine` | The machine's own linear positions, worked out by CosmicDMS for the rotary angles | The machine's positions and rotary angles, `G93` inverse-time feeds | The machine's rotary centres (and head pivot length) in its kinematics settings |

In `machine` style CosmicDMS splits moves wherever moving the axes in step would take the tool tip
more than the arc tolerance off the path, and gives each block its inverse-time feed. Drilling on a
tilted plane is written as canned cycles in the tilted plane (`plane` style) or, when the spindle
ends up vertical (a table that turns the part), in the machine's positions.

CosmicDMS takes the WCS to be square to the machine's axes with every rotary axis at zero (the part
set up square on the table). The rotary centres are measured from the WCS origin. Angles follow
ISO 841: a positive angle tilts the tool the same way relative to the part whether the head or the
table moves. An axis that turns the other way is marked *reversed*. Rotary axes keep their angle
where it does not matter (C with the tool vertical) and never unwind more than they must; a tool
axis beyond an axis's travel is an error naming it.

Post settings for this:

| Setting | Meaning |
| --- | --- |
| `multi_axis_output` | `tcp` or `machine` for simultaneous moves |
| `plane_output` | `plane`, `tcp` or `machine` for 3+2 operations |
| `rotary_home_at_end` | Turn the rotary axes back to zero at the end |
| `tcp_on_code`, `tcp_off_code` (LinuxCNC) | The codes that switch a switchable-kinematics configuration to TCP and back |

Grbl has no rotary output; Mach writes machine positions. **Simulate every multi-axis program and
dry-run it with the rotary axes clear of the part before cutting**: the holder is not checked for
collisions, and how a control interprets tilted planes and TCP depends on its options and setup.

## Per-operation post options

A toolpath's **Post** tab (and its `post_stop`, `post_comment`, `post_code_before` and
`post_code_after` parameters) adds, for that operation only: a stop (M0) before it, a note for the
operator, and G-code lines written before and after it unchanged. Pass-through lines are not
checked: they go to the machine as written.

## Shipped posts

| Id | Controller | Notes |
| --- | --- | --- |
| `generic` | Any Fanuc-style control | Built in (Rust); straight moves only |
| `fanuc` | Fanuc 0i / 30i and compatibles | O0001, G54–G59 then G54.1 P1–P48, rigid tapping (M29), tool preselect, M1 between operations; 4/5-axis: G43.4 tool centre point control, G68.2 / G53.1 tilted working plane |
| `haas` | Haas NGC and classic | O00001, G154 P1–P99, M88/M89 through-spindle coolant, M83/M84 air blast, Y home at the end, optional G187 smoothing; 5-axis: G234 TCPC, G254 / G255 dynamic work offsets |
| `linuxcnc` | LinuxCNC, Tormach PathPilot | No O number, G54–G59.3, G64 P/Q blending per roughing or finishing (or G61), G33.1 rigid tapping, M64/M65 coolant outputs, o-word subroutines, `(MSG, …)` operator messages, dwell in seconds, G53 Z0 home, M2; 4/5-axis: machine positions with G93 inverse-time feeds, or TCP for switchable-kinematics configurations |
| `grbl` | Grbl 1.1 | No tool changer (stops with M0), no length offsets, drilling written as moves, G54–G59; 3 axes only |
| `mach` | Mach3 / Mach4 | G59 P7–P253 fixture offsets, dwell in seconds, .tap files; 4/5-axis as machine positions with G93 |

These follow each controller's published G-code documentation. Controls differ in options and
settings (dwell units, rigid tapping, offset ranges): **check a post's output against your
machine's manual and dry-run new programs before cutting.**

## Writing your own post

Posts live in your data folder's `posts/` (`~/.local/share/CosmicDMS/posts` on Linux,
`%APPDATA%\CosmicDMS\posts` on Windows, `~/Library/Application Support/CosmicDMS/posts` on macOS;
**Post-processor → Open the posts folder** shows it). The shipped posts are copied to
`posts/builtin/` and rewritten on every run; copy one up a level and rename it to make it yours. A
post of yours with the same name as a shipped one replaces it.

A post is a module with a `Post` subclass (or a `POST = MyPost` line). The first line of the
module's docstring is its description in the chooser.

```python
"""My shop's VMC: Fanuc-style, 5-digit programs, no rigid tapping."""

from cosmicdms_post import Post


class ShopPost(Post):
    name = "Shop VMC"
    description = "Our VMC"
    extension = "nc"
    program_number_digits = 5
    rigid_tapping = None
    max_rpm = 8000

    def write_program_start(self, job):
        super().write_program_start(job)
        self.comment("Clamp in the left vise")
```

### Configuration attributes

| Attribute | Default | Meaning |
| --- | --- | --- |
| `units` | `"job"` | `"job"` follows the job; `"mm"` or `"in"` forces |
| `percent_signs`, `program_number`, `program_number_digits` | `True`, `True`, 4 | Program framing |
| `safe_start`, `program_end` | `G90 G94 G17 G40 G49 G80`, `M30` | First and last blocks |
| `line_numbers`, `line_number_start`, `line_number_step`, `line_number_max` | off, 10, 10, 99999 | N words |
| `comment_style`, `comments_upper`, `max_comment` | `"paren"`, `True`, 70 | `"paren"`, `"semicolon"` or `"none"` |
| `repeat_motion_codes`, `modal_axes` | `True`, `True` | G0/G1 on every block; only changed axes |
| `decimals`, `feed_decimals`, `trailing_zeros` | 3 mm / 4 in, 1 / 2, `True` | Number formats |
| `arcs`, `arc_format`, `helical_arcs` | `True`, `"ijk"`, `True` | G2/G3 with I J (or `"r"`), helices |
| `min_arc_radius`, `max_arc_radius`, `linearize_tolerance` | 0.01, 5000, 0.005 mm | Arcs outside the range become lines |
| `rapid_feed`, `max_feed` | 0, `None` | Rapids as G1 at a feed (0: G0); feed clamp |
| `tool_changer`, `tool_change_code`, `preselect_next_tool` | `True`, `"M6"`, `False` | Without a changer the post stops (M0) for each tool |
| `length_offset`, `length_register` | `True`, `"offset"` | G43 H from the tool's offset (or `"tool"` number) |
| `max_rpm`, `spindle_dwell`, `optional_stop` | `None`, 0 s, `False` | |
| `coolant_codes`, `coolant_off`, `coolant_off_codes` | flood M8, mist/air M7, through-tool M88; M9; M89 | |
| `standard_offsets`, `extended_offset`, `max_extended_offsets` | 6, `"G54.1 P{n}"`, 48 | `{n}` extended number, `{p}` WCS number |
| `canned_cycles`, `cycle_return`, `dwell_units`, `rigid_tapping`, `repeat_cycle_codes` | G81–G85/G73, `"G99"`, ms, `None`, `True` | `canned_cycles = None` writes drilling as moves |
| `home_z`, `home_xy` | `G91 G28 Z0`, none | Before tool changes and at the end (`home_xy` when `return_home_xy`) |
| `multi_axis_output`, `plane_output` | `"tcp"`, `"plane"` | How simultaneous and 3+2 operations are written; `None` when the control cannot |
| `tcp_on`, `tcp_off` | `G43.4 H{h}`, `G49` | Tool tip control on (`{h}`: length register) and off |
| `plane_on`, `plane_off` | `G68.2 X{x} Y{y} Z{z} I{a} J{b} K{c}`, `G53.1`; `G69` | The tilted plane: origin in the WCS, ZXZ Euler angles |
| `plane_coordinates` | `"plane"` | Moves under the tilted plane in its coordinates (`"wcs"` for dynamic work offsets) |
| `inverse_time_code`, `per_minute_code`, `rotary_decimals` | `G93`, `G94`, 3 | |
| `properties` | the common settings | Settings offered in the app: name → `Property(title, description, group, choices)` |

### Settings of your own

A class attribute becomes a setting by naming it in `properties`; its value is the default and its
type (bool, int, float, text, or a list of choices) decides how the app shows it and how text is
converted.

```python
from cosmicdms_post import Post, Property


class ShopPost(Post):
    properties = {
        "probe_first": Property("Probe the part first", "Call O9810 before the first operation", "Program"),
    }
    probe_first = False

    def write_program_start(self, job):
        super().write_program_start(job)
        if self.probe_first:
            self.block("G65", "P9810")
```

Settings are merged along the class hierarchy, so a subclass lists only what it adds; naming an
inherited one with `None` (`"safe_retract": None`) hides it.

### Hooks

Override any of these and call `super()` for the default: `write_program_start`,
`write_tool_list`, `write_program_end`, `write_operation`, `write_tool_change`,
`write_manual_tool_change`, `write_spindle`, `write_coolant`, `work_offset`, `write_moves`,
`write_rapid`, `write_linear`, `write_arc`, `write_drill_cycle`, `write_expanded_cycle`,
`write_smoothing` (motion-control mode per operation), `write_tool_list_comments`,
`move_to_tool_change_position`, `apply_properties`, and for rotary work `write_oriented`,
`write_rotary`, `write_plane_on`, `write_multi_moves`, `write_tilted_drilling`, `style_for`.
Helpers: `block(*words)`, `comment(text)`, `operator_message(text)`, `raw(text)`, `fmt(mm)`,
`fmt_feed(mm_per_min)`, `warn(text)`, `dwell_word(seconds)`, `retract_home()`, `is_roughing(op)`. Raise `PostError("...")` for a problem the post cannot write around.

### Testing a post

```bash
cosmicdms: Toolpaths → CL data           # or cc.toolpath_cl_export(path="part.cl.json")
python3 -c 'import json,sys; sys.path.insert(0,"posts/builtin"); import run_post; \
  print(run_post.answer({"post": "myshop.py", "cl": json.load(open("part.cl.json"))})["gcode"])'
```

The shipped posts' tests (`python/tests/test_posts.py`) show how to check output line by line.

## CL data

```json
{
  "format": "CosmicDMS-CL/1",
  "job": {"name": "Bracket", "units": "in", "program_number": 1000, "line_numbers": false},
  "machine": {"id": "machine.1", "name": "VMC", "travel": [500, 400, 400], "max_spindle_rpm": 12000,
              "max_feed": 10000, "tool_capacity": 20,
              "kinematics": {"layout": "table_table", "axes": [{"name": "A", "min": -120, "max": 120, "reversed": false},
                             {"name": "C", "min": -100000, "max": 100000, "reversed": false}],
                             "primary_centre": [0, 0, -50], "secondary_centre": [0, 0, -50], "pivot_length": 0}},
  "tools": [{"number": 1, "name": "1/4 flat", "type": "flat", "diameter": 6.35, "length_offset": 1, "...": "..."}],
  "operations": [{
    "id": "toolpath.1", "name": "Profile", "strategy": "contour2d", "tool": 1,
    "notes": "", "stop_before": false, "code_before": "", "code_after": "",
    "stock_to_leave": [0.0, 0.0], "tolerance": 0.01,
    "tool_axis": [0, 0, 1], "multi_axis": false, "move_frame": "wcs", "work_plane": null, "rotary": [0, 0],
    "spindle_rpm": 10000, "coolant": "flood", "wcs": {"id": "wcs.1", "name": "Top", "index": 0},
    "work_offset": 0, "start": [x, y, z],
    "moves": [
      {"type": "rapid", "to": [x, y, z]},
      {"type": "linear", "to": [x, y, z], "feed": 1000.0},
      {"type": "arc", "to": [x, y, z], "center": [cx, cy], "cw": true, "plane": "xy", "feed": 1000.0}
    ],
    "drill": null,
    "bounds": {"min": [x, y, z], "max": [x, y, z]}
  }],
  "properties": {"line_numbers": "true"},
  "warnings": ["Profile spans 520.0 mm in X, beyond the machine's 500 mm travel"]
}
```

Coordinates are millimetres in the operation's WCS; feeds are mm/min. A drilling operation has no
moves and a `drill` object: `{cycle, holes: [[x, y, top, bottom], ...], retract, clearance, peck,
dwell, feed, pitch}`, with `cycle` one of `drill`, `spot`, `peck`, `chip_break`, `tap`, `bore`.

On a machine with rotary axes (`machine.kinematics` not null), every operation has `rotary`: its
angles in degrees, one per axis in `kinematics.axes` order. Besides:

- **3+2 operations** (`move_frame: "plane"`): `start`, `moves` and `drill` are in the tilted
  plane's coordinates, arcs fitted there. `work_plane` gives the plane in the WCS (`origin`, `x`,
  `y`, `z`, `euler_zxz`). `wcs_start` / `wcs_moves` are the same moves as lines in the WCS, and
  `machine_start` / `machine_moves` in the machine's own positions; `machine_drill` (`{holes,
  retract, clearance}`) is the drilling in machine positions when the spindle is vertical there.
- **Simultaneous operations** (`multi_axis: true`): each move has `to` (the tool tip in the WCS),
  `axis` (the tool axis), `rotary`, `machine` (the machine's linear position) and, for feed moves,
  `inverse_time` (1 / minutes). `machine_start` is where the machine starts.
- An operation along the WCS's Z with a rotary axis away from zero has `machine_start` and
  `machine_moves` too.
