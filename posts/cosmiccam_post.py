"""CosmicCAM post-processor library.

A post-processor turns CosmicCAM's machine-neutral cutter location (CL) data into the G-code one
machine and controller expect. Every post is a Python module with a ``Post`` subclass; its class
attributes are the machine's configuration and its methods write each part of the program, so a
post changes only what differs from the defaults here.

CL data (see docs/posts.md) gives coordinates in each operation's work coordinate system in
millimetres, feeds in mm/min, arcs fitted by CosmicCAM within a tolerance, drilling cycles, tools,
spindle speeds and coolant. This module converts units, formats numbers, keeps modal state and
writes the program; ``run_post.py`` feeds it the CL data.

Standard library only; Python 3.9 or later.
"""

import math

MM_PER_INCH = 25.4

__all__ = ["Post", "PostError", "Number", "Property"]


class PostError(Exception):
    """A problem the post cannot write around: reported to the user, no program is written."""


class Number:
    """How one kind of value is written: decimals, trailing zeros, a forced decimal point."""

    def __init__(self, decimals=3, trailing_zeros=True, force_decimal=True, leading_zero=True):
        self.decimals = decimals
        self.trailing_zeros = trailing_zeros
        self.force_decimal = force_decimal
        self.leading_zero = leading_zero

    def __call__(self, value):
        text = "{:.{}f}".format(value, self.decimals)
        if text.lstrip("-").strip("0.") == "":
            text = "{:.{}f}".format(0.0, self.decimals)
        if not self.trailing_zeros and "." in text:
            text = text.rstrip("0")
            if text.endswith(".") and not self.force_decimal:
                text = text[:-1]
        if not self.leading_zero:
            text = text.replace("0.", ".", 1) if text.startswith(("0.", "-0.")) else text
        return text


class Property:
    """A setting a post offers the user. Its value lives in the class attribute of the same name
    (so a subclass changes the default by setting that attribute); the user's choice, saved with
    the machine, replaces it for a run."""

    def __init__(self, title, description="", group="General", choices=None, kind=None):
        self.title = title
        self.description = description
        self.group = group
        self.choices = tuple(choices) if choices else None
        #: "bool", "int", "float", "str" or "choice"; inferred from the default when None.
        self.kind = kind


def _kind_of(value, prop):
    if prop.kind:
        return prop.kind
    if prop.choices:
        return "choice"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    return "str"


class Post:
    """A Fanuc-style post. Subclasses override attributes and methods for their machine."""

    #: The settings this post offers, by attribute name. Subclasses add to them with
    #: ``properties = dict(Post.properties, my_setting=Property(...))``.
    properties = {
        "line_numbers": Property("Sequence numbers", "Number the blocks (N10, N20, ...)", "Format"),
        "line_number_start": Property("First sequence number", "", "Format"),
        "line_number_step": Property("Sequence number step", "", "Format"),
        "separate_words": Property("Spaces between words", "Write G0 X1 Y2 rather than G0X1Y2", "Format"),
        "comment_style": Property("Comments", "How comments are written", "Format", ("paren", "semicolon", "none")),
        "repeat_motion_codes": Property("Motion code on every block", "Write G0/G1/G2/G3 on every move", "Format"),
        "write_machine_info": Property("Machine and post in the header", "", "Format"),
        "write_tool_list": Property("Tool list in the header", "Each tool with its size and the Z range it works in", "Format"),
        "arcs": Property("Arcs", "Write G2/G3 (off writes short lines)", "Motion"),
        "arc_format": Property("Arc centre", "I/J incremental centre or R radius", "Motion", ("ijk", "r")),
        "max_arc_radius": Property("Largest arc radius (mm)", "Larger arcs are written as lines", "Motion"),
        "rapid_feed": Property("Rapids as feed (mm/min)", "Write rapids as G1 at this feed; 0 writes G0", "Motion", kind="float"),
        "preselect_next_tool": Property("Preselect the next tool", "Call the next tool after a change", "Tools"),
        "manual_tool_change": Property("Manual tool changes", "Stop for the operator to fit each tool (M0)", "Tools"),
        "tool_change_xy": Property("Tool change position (machine X,Y)", "e.g. 0,0; empty changes where the tool is", "Tools"),
        "spindle_dwell": Property("Spindle-up dwell (s)", "Wait after starting the spindle", "Tools"),
        "optional_stop": Property("Optional stop between operations", "M1 before each operation", "Program"),
        "restart_blocks": Property("Restart-safe operations", "Repeat the safe-start block at each operation so a run can start there", "Program"),
        "safe_retract": Property("Safe retract", "How the tool goes up before tool changes and at the end", "Program", ("home", "clearance")),
        "return_home_xy": Property("Return X and Y home at the end", "", "Program"),
        "units": Property("Units", "Follow the job, or always millimetres or inches", "Program", ("job", "mm", "in")),
        "multi_axis_output": Property(
            "Simultaneous moves",
            "tcp: tool tip positions, the control keeps the tip on the path; machine: the machine's own positions with inverse-time feeds",
            "Multi-axis",
            ("tcp", "machine"),
        ),
        "plane_output": Property(
            "3+2 operations",
            "plane: a tilted work plane; tcp or machine as for simultaneous moves",
            "Multi-axis",
            ("plane", "tcp", "machine"),
        ),
        "rotary_home_at_end": Property("Rotary axes to zero at the end", "", "Multi-axis"),
    }

    #: Spaces between the words of a block.
    separate_words = True
    #: Write the machine and post in the header.
    write_machine_info = True
    #: Write the tools in the header.
    write_tool_list = True
    #: Stop for the operator at each tool (M0) instead of changing tools.
    manual_tool_change = False
    #: Machine X,Y to move to (G53) before a tool change, e.g. "0,0"; empty for none.
    tool_change_xy = ""
    #: Repeat the safe-start block, units and work offset at each operation.
    restart_blocks = False
    #: "home" moves home (`home_z`) before tool changes and at the end; "clearance" stays at
    #: the clearance height.
    safe_retract = "home"
    #: Move X and Y home (`home_xy`) at the end.
    return_home_xy = False

    # ---- identity ----------------------------------------------------------------------
    name = "Generic"
    description = "Generic Fanuc-style 3-axis mill"
    #: File extension the program is saved with.
    extension = "nc"

    # ---- program framing ---------------------------------------------------------------
    #: Units written: "job" (or None) follows the job, or "mm" / "in".
    units = "job"
    percent_signs = True
    #: Write an O program number (Fanuc style). LinuxCNC uses O for subroutines: turn it off.
    program_number = True
    program_number_digits = 4
    #: The block that puts the machine in a known state at the start.
    safe_start = ("G90", "G94", "G17", "G40", "G49", "G80")
    #: Codes that end the program.
    program_end = ("M30",)

    # ---- blocks ------------------------------------------------------------------------
    line_numbers = False
    line_number_start = 10
    line_number_step = 10
    line_number_max = 99999
    #: "paren", "semicolon" or "none".
    comment_style = "paren"
    comments_upper = True
    max_comment = 70
    #: Write G0 / G1 / G2 / G3 on every motion block, not only when it changes.
    repeat_motion_codes = True
    #: Write only the axes that change.
    modal_axes = True

    # ---- numbers -----------------------------------------------------------------------
    decimals = {"mm": 3, "in": 4}
    feed_decimals = {"mm": 1, "in": 2}
    trailing_zeros = True

    # ---- motion ------------------------------------------------------------------------
    arcs = True
    #: "ijk" (incremental centre) or "r" (radius).
    arc_format = "ijk"
    helical_arcs = True
    #: Arcs outside this radius range (mm) are written as lines.
    min_arc_radius = 0.01
    max_arc_radius = 5000.0
    #: Chord tolerance (mm) when arcs have to be written as lines.
    linearize_tolerance = 0.005
    #: Write rapids as G1 at this feed (mm/min) for machines without reliable G0; None for G0.
    rapid_feed = 0.0
    #: Highest feed the machine takes (mm/min); None takes the machine configuration's.
    max_feed = None

    # ---- tools and spindle -------------------------------------------------------------
    tool_changer = True
    tool_change_code = "M6"
    #: Call the next tool after a change so the changer can fetch it.
    preselect_next_tool = False
    #: G43 H<n> tool length compensation.
    length_offset = True
    #: "tool" uses the tool number as the H register, "offset" the tool's length offset.
    length_register = "offset"
    #: Highest spindle speed; None takes the machine configuration's.
    max_rpm = None
    #: Seconds to wait after starting the spindle.
    spindle_dwell = 0.0
    #: Write M1 (optional stop) between operations.
    optional_stop = False

    # ---- coolant -----------------------------------------------------------------------
    coolant_codes = {"flood": "M8", "mist": "M7", "air": "M7", "through_tool": "M88"}
    #: Code that turns coolant off, and codes for kinds that need their own.
    coolant_off = "M9"
    coolant_off_codes = {"through_tool": "M89"}

    # ---- work offsets ------------------------------------------------------------------
    #: Number of standard offsets (G54...).
    standard_offsets = 6
    #: How offsets after the standard ones are written, or None: a format with {n} (the
    #: extended offset's number, from 1) and {p} (the WCS number, from 1).
    extended_offset = "G54.1 P{n}"
    max_extended_offsets = 48

    # ---- drilling ----------------------------------------------------------------------
    #: Canned cycle codes; a cycle missing here (or all, when None) is written as moves.
    canned_cycles = {"drill": "G81", "spot": "G82", "peck": "G83", "chip_break": "G73", "tap": "G84", "bore": "G85"}
    #: Return to the R plane (G99) or the initial plane (G98) between holes.
    cycle_return = "G99"
    #: "seconds" or "milliseconds" for P words in dwell and cycles.
    dwell_units = "milliseconds"
    #: Rigid tapping prefix (e.g. "M29 S{rpm}" on Fanuc); None for floating holders.
    rigid_tapping = None
    #: Write every hole's line in full (cycle code and all words).
    repeat_cycle_codes = True

    # ---- 4/5-axis ----------------------------------------------------------------------
    #: How simultaneous multi-axis moves are written: "tcp" (tool tip positions with the
    #: rotary angles; the control keeps the tip on the path, e.g. G43.4) or "machine" (the
    #: machine's own linear positions, worked out by CosmicCAM, with inverse-time feeds). None:
    #: the control does no multi-axis work.
    multi_axis_output = "tcp"
    #: How 3+2 operations are written: "plane" (a tilted work plane command), "tcp" or
    #: "machine" as above. None: the control does no 3+2 work.
    plane_output = "plane"
    #: Blocks that turn tool tip control on ({h}: length register) and off.
    tcp_on = ("G43.4 H{h}",)
    tcp_off = ("G49",)
    #: The tilted work plane on ({x} {y} {z}: its origin in the WCS; {a} {b} {c}: ZXZ Euler
    #: angles, degrees) and off.
    plane_on = ("G68.2 X{x} Y{y} Z{z} I{a} J{b} K{c}", "G53.1")
    plane_off = ("G69",)
    #: "plane": moves under the tilted plane are in its own coordinates (G68.2); "wcs": in
    #: the WCS's (dynamic work offsets).
    plane_coordinates = "plane"
    inverse_time_code = "G93"
    per_minute_code = "G94"
    rotary_decimals = 3
    #: Turn the rotary axes back to zero at the end of the program.
    rotary_home_at_end = True

    # ---- end of program ----------------------------------------------------------------
    #: Blocks that send the machine home at the end (and before tool changes).
    home_z = ("G91 G28 Z0", "G90")
    home_xy = ()

    # =====================================================================================

    def __init__(self):
        self.lines = []
        self.warnings = []
        self._line_number = None
        self.cl = None
        self.out_units = "mm"
        self.tool = None
        self.state = {}
        self.rotary_names = []
        self.rotary_state = None

    # ---- entry point -------------------------------------------------------------------

    # ---- properties --------------------------------------------------------------------

    @classmethod
    def all_properties(cls):
        """Every property this post offers, from the base class down. A subclass hides one it
        inherits by naming it with ``None``."""
        merged = {}
        for klass in reversed(cls.__mro__):
            merged.update(getattr(klass, "properties", {}) or {})
        return {name: prop for name, prop in merged.items() if prop is not None}

    @classmethod
    def describe_properties(cls):
        """The properties as CosmicCAM shows them: name, title, description, group, kind, default, choices."""
        out = []
        for name, prop in cls.all_properties().items():
            default = getattr(cls, name, None)
            if isinstance(default, (tuple, list, dict)) or default is None:
                default = "" if default is None else str(default)
            out.append(
                {
                    "name": name,
                    "title": prop.title,
                    "description": prop.description,
                    "group": prop.group,
                    "kind": _kind_of(default, prop),
                    "default": default,
                    "choices": list(prop.choices) if prop.choices else None,
                }
            )
        return out

    def apply_properties(self, values):
        """Sets the user's choices (text from the machine's settings) on this post."""
        props = self.all_properties()
        for name, text in (values or {}).items():
            prop = props.get(name)
            if prop is None:
                self.warn("the post has no setting {!r}; it was ignored".format(name))
                continue
            default = getattr(type(self), name, None)
            kind = _kind_of(default if default is not None else "", prop)
            text = str(text).strip()
            try:
                if kind == "bool":
                    if text.lower() not in ("true", "false", "1", "0", "yes", "no", "on", "off"):
                        raise ValueError(text)
                    value = text.lower() in ("true", "1", "yes", "on")
                elif kind == "int":
                    value = int(float(text))
                elif kind == "float":
                    value = float(text)
                    if not math.isfinite(value):
                        raise ValueError(text)
                elif kind == "choice":
                    if text not in prop.choices:
                        raise ValueError(text)
                    value = text
                else:
                    value = text
            except ValueError:
                raise PostError("setting {} ({}): {!r} is not a valid value".format(prop.title, name, text))
            setattr(self, name, value)

    # ---- entry point -------------------------------------------------------------------

    def run(self, cl):
        """Writes the program for CL data and returns it as text."""
        self.cl = cl
        self.apply_properties(cl.get("properties"))
        if self.manual_tool_change:
            self.tool_changer = False
        job = cl.get("job", {})
        self.out_units = self.units if self.units in ("mm", "in") else (job.get("units") or "mm")
        if self.out_units not in ("mm", "in"):
            raise PostError("units must be mm or in, not {!r}".format(self.out_units))
        self.machine = cl.get("machine", {})
        kinematics = self.machine.get("kinematics") or {}
        self.rotary_names = [a.get("name", "") for a in kinematics.get("axes", [])]
        self.angle = Number(self.rotary_decimals, self.trailing_zeros)
        self.tools = {t["number"]: t for t in cl.get("tools", [])}
        self.operations = cl.get("operations", [])
        if not self.operations:
            raise PostError("there are no operations to post")
        self._line_number = self.line_number_start if (self.line_numbers or job.get("line_numbers")) else None
        self.coord = Number(self.decimals[self.out_units], self.trailing_zeros)
        self.feed_number = Number(self.feed_decimals[self.out_units], self.trailing_zeros)
        self.reset_modal()
        self.warnings.extend(cl.get("warnings", []))
        self.write_program_start(job)
        previous = None
        for index, op in enumerate(self.operations):
            # The next operation with a different tool: the one to preselect.
            following = next((o for o in self.operations[index + 1 :] if o["tool"] != op["tool"]), None)
            self.write_operation(op, previous, following)
            previous = op
        self.write_program_end(job)
        return "\n".join(self.lines) + "\n"

    # ---- output helpers ----------------------------------------------------------------

    def reset_modal(self):
        """Forgets modal motion state: the next block writes everything it needs. Coolant, which
        stays on across operations, is kept."""
        coolant = self.state.get("coolant") if self.state else None
        self.state = {"motion": None, "X": None, "Y": None, "Z": None, "F": None, "coolant": coolant}

    def block(self, *words):
        """Writes one block of words (empty words are skipped), numbered if line numbers are on."""
        text = (" " if self.separate_words else "").join(w for w in words if w)
        if not text:
            return
        if self._line_number is not None and not text.startswith(("(", ";", "%")):
            text = "N{}{}{}".format(self._line_number, " " if self.separate_words else "", text)
            self._line_number += self.line_number_step
            if self._line_number > self.line_number_max:
                self._line_number = self.line_number_start
        self.lines.append(text)

    def comment(self, text):
        """Writes a comment in the controller's style (or nothing)."""
        if self.comment_style == "none" or not text:
            return
        text = str(text).replace("(", "[").replace(")", "]").replace(";", ",")
        text = "".join(c if 32 <= ord(c) < 127 else "?" for c in text.replace("Ø", "D"))
        if self.comments_upper:
            text = text.upper()
        text = text[: self.max_comment].strip()
        if self.comment_style == "semicolon":
            self.lines.append("; " + text)
        else:
            self.lines.append("(" + text + ")")

    def length(self, mm):
        """A length in output units."""
        return mm / MM_PER_INCH if self.out_units == "in" else mm

    def fmt(self, mm):
        return self.coord(self.length(mm))

    def fmt_feed(self, mm_per_min):
        limit = self.max_feed or self.machine.get("max_feed")
        if limit and mm_per_min > limit:
            self.warn("feed {:.0f} mm/min clamped to the machine's {:.0f}".format(mm_per_min, limit))
            mm_per_min = limit
        return self.feed_number(self.length(mm_per_min))

    def warn(self, text):
        if text not in self.warnings:
            self.warnings.append(text)

    def operator_message(self, text):
        """A message for the operator, shown when the controller can (a comment otherwise)."""
        self.comment(text)

    def raw(self, text):
        """G-code written as given, one block per line: the user's own code."""
        for line in str(text).splitlines():
            line = line.strip()
            if line:
                self.lines.append(line)

    def retract_home(self):
        """Up and out of the way before a tool change: home Z, or nothing at clearance."""
        if self.safe_retract == "home":
            for b in self.home_z:
                self.block(b)

    def dwell_word(self, seconds):
        if self.dwell_units == "seconds":
            return "P" + Number(3, trailing_zeros=False, force_decimal=False)(seconds)
        return "P{}".format(int(round(seconds * 1000)))

    # ---- program -----------------------------------------------------------------------

    def write_program_start(self, job):
        if self.percent_signs:
            self.lines.append("%")
        if self.program_number:
            number = int(job.get("program_number") or 1)
            if not 1 <= number < 10 ** self.program_number_digits:
                raise PostError("program number {} does not fit {} digits".format(number, self.program_number_digits))
            self.lines.append("O{:0{}d}".format(number, self.program_number_digits))
        self.comment(job.get("name", ""))
        if self.write_machine_info:
            self.comment("Post: {} - {}".format(self.name, self.description))
            self.comment("Machine: {}".format(self.machine.get("name", "")))
            self.comment("Units: {}".format("inches" if self.out_units == "in" else "millimetres"))
        if self.write_tool_list:
            self.write_tool_list_comments()
        self.block(*self.safe_start)
        self.block("G20" if self.out_units == "in" else "G21")

    def write_tool_list_comments(self):
        """Each tool: its number and name, size, and the lowest and highest Z it works at."""
        used = []
        for op in self.operations:
            if op["tool"] not in used:
                used.append(op["tool"])
        for number in used:
            t = self.tools.get(number, {})
            zs = [z for op in self.operations if op["tool"] == number for z in (op.get("bounds", {}).get("min", [0, 0, None])[2], op.get("bounds", {}).get("max", [0, 0, None])[2]) if z is not None]
            parts = ["T{}".format(number), t.get("name") or "", "D=" + self.fmt(t.get("diameter", 0.0))]
            if t.get("corner_radius"):
                parts.append("CR=" + self.fmt(t["corner_radius"]))
            if t.get("flutes"):
                parts.append("{} FL".format(t["flutes"]))
            if zs:
                parts.append("ZMIN=" + self.fmt(min(zs)))
            self.comment(" ".join(p for p in parts if p))

    def write_program_end(self, job):
        self.write_coolant(None)
        self.block("M5")
        self.retract_home()
        if self.rotary_names and self.rotary_home_at_end and self.rotary_state and any(
            abs(a) > 1e-9 for a in self.rotary_state
        ):
            self.write_rotary([0.0] * len(self.rotary_names))
        if self.return_home_xy:
            for b in self.home_xy:
                self.block(b)
        if self.length_offset:
            self.block("G49")
        self.block(*self.program_end)
        if self.percent_signs:
            self.lines.append("%")

    # ---- operations --------------------------------------------------------------------

    def write_operation(self, op, previous, following):
        self.comment(op.get("name", ""))
        notes = [n for n in str(op.get("notes") or "").splitlines() if n.strip()]
        if not op.get("stop_before"):
            for note in notes:
                self.comment(note)
        if previous is not None and self.optional_stop:
            self.block("M1")
        if op.get("stop_before"):
            # The note is the message the operator reads at the stop.
            for note in notes or ["Check before {}".format(op.get("name", "the next operation"))]:
                self.operator_message(note)
            self.block("M0")
        if self.restart_blocks and previous is not None:
            self.block(*self.safe_start)
            self.block("G20" if self.out_units == "in" else "G21")
            self.rotary_state = None
        self.raw(op.get("code_before") or "")
        new_tool = previous is None or previous["tool"] != op["tool"]
        if new_tool:
            if previous is not None:
                self.write_coolant(None)
                self.block("M5")
            self.write_tool_change(op, following)
        self.write_spindle(op, new_tool or self.restart_blocks or previous.get("spindle_rpm") != op.get("spindle_rpm"))
        self.block(self.work_offset(op.get("work_offset", 0)))
        self.write_smoothing(op)
        self.reset_modal()
        if self.rotary_names and op.get("rotary") is not None:
            self.write_oriented(op, previous, new_tool)
            self.raw(op.get("code_after") or "")
            return
        if op.get("multi_axis") or op.get("move_frame") == "plane":
            raise PostError("{} tilts the tool, but the machine has no rotary axes".format(op.get("name", "")))
        start = op.get("start") or [0.0, 0.0, 0.0]
        self.write_rapid(start[0], start[1], None)
        if self.length_offset and new_tool:
            self.block(self.motion_code("G0"), "G43", "Z" + self.fmt(start[2]), "H{}".format(self.length_register_for(op)))
            self.state["Z"] = self.fmt(start[2])
        else:
            self.write_rapid(None, None, start[2])
        if self.restart_blocks:
            self.state["coolant"] = None
        self.write_coolant(op.get("coolant"))
        if op.get("drill"):
            self.write_drill_cycle(op, op["drill"])
        else:
            self.write_moves(op)
        self.raw(op.get("code_after") or "")

    # ---- 4/5-axis ----------------------------------------------------------------------

    def rotary_words(self, angles):
        """A / B / C words for `angles` (degrees), only those that change."""
        words = []
        for name, value in zip(self.rotary_names, angles):
            text = self.angle(value)
            if not self.modal_axes or self.state.get(name) != text:
                words.append(name + text)
                self.state[name] = text
        return words

    def write_rotary(self, angles):
        """Turns the rotary axes to `angles` at rapid."""
        words = self.rotary_words(angles)
        if words:
            self.block(self.motion_code("G0"), *words)
        self.rotary_state = list(angles)

    def style_for(self, op):
        """How an operation with rotary angles is written: "plain" (tool along the WCS's Z,
        rotary axes at zero), "multi" with the output for simultaneous moves, or the 3+2 output."""
        if op.get("multi_axis"):
            if not self.multi_axis_output:
                raise PostError("{} is a simultaneous multi-axis operation; this post cannot write those".format(op.get("name", "")))
            return self.multi_axis_output
        if op.get("move_frame") == "plane":
            style = self.plane_output or self.multi_axis_output
            if not style:
                raise PostError("{} works on a tilted plane (3+2); this post cannot write those".format(op.get("name", "")))
            return style
        if op.get("machine_moves") is not None:
            # Along the WCS's Z, but with a rotary axis away from zero.
            return self.plane_output if self.plane_output in ("machine",) else "machine"
        return "plain"

    def write_oriented(self, op, previous, new_tool):
        """An operation on a machine with rotary axes: turn them (with the tool up), then write
        the moves the way `style_for` says."""
        angles = op["rotary"]
        if self.rotary_state is None or any(abs(a - b) > 1e-9 for a, b in zip(angles, self.rotary_state)):
            if previous is not None and not new_tool:
                self.retract_home()
            self.write_rotary(angles)
        style = self.style_for(op)
        h = self.length_register_for(op)
        if style == "plain":
            self.approach(op, op.get("start") or [0.0, 0.0, 0.0], new_tool)
            self.write_coolant(op.get("coolant"))
            if op.get("drill"):
                self.write_drill_cycle(op, op["drill"])
            else:
                self.write_moves(op)
            return
        if op.get("drill"):
            self.write_tilted_drilling(op, style, new_tool)
            return
        if style == "plane":
            plane = op.get("work_plane")
            if plane is None:
                raise PostError("{} has no work plane".format(op.get("name", "")))
            self.write_plane_on(plane)
            if self.plane_coordinates == "wcs":
                view = dict(op, start=op.get("wcs_start") or op["start"], moves=op.get("wcs_moves") or op["moves"])
            else:
                view = op
            self.approach(view, view.get("start") or [0.0, 0.0, 0.0], True)
            self.write_coolant(op.get("coolant"))
            self.write_moves(view)
            self.retract_home()
            for b in self.plane_off:
                self.block(b)
            self.reset_modal()
        elif style == "tcp":
            if not self.tcp_on:
                raise PostError("set the codes that turn tool tip control on and off for this machine")
            for b in self.tcp_on:
                self.block(b.format(h=h))
            if op.get("multi_axis"):
                start = op.get("start") or [0.0, 0.0, 0.0]
                self.write_rapid(start[0], start[1], None)
                self.write_rapid(None, None, start[2])
                self.write_coolant(op.get("coolant"))
                self.write_multi_moves(op, "to", per_minute=True)
            else:
                start = op.get("wcs_start") or op.get("start") or [0.0, 0.0, 0.0]
                self.write_rapid(start[0], start[1], None)
                self.write_rapid(None, None, start[2])
                self.write_coolant(op.get("coolant"))
                self.write_moves(dict(op, moves=op.get("wcs_moves") or op["moves"]))
            self.retract_home()
            for b in self.tcp_off:
                self.block(b)
            self.reset_modal()
        elif style == "machine":
            start = op.get("machine_start") or op.get("start") or [0.0, 0.0, 0.0]
            self.approach(op, start, True)
            self.write_coolant(op.get("coolant"))
            if op.get("multi_axis"):
                self.block(self.inverse_time_code)
                self.write_multi_moves(op, "machine", per_minute=False)
                self.block(self.per_minute_code)
                self.state["F"] = None
            else:
                self.write_moves(dict(op, moves=op.get("machine_moves") or op["moves"]))
        else:
            raise PostError("unknown multi-axis output {!r}".format(style))

    def approach(self, op, start, length_offset):
        """Over the start, then down to it, with tool length compensation when asked."""
        self.write_rapid(start[0], start[1], None)
        if self.length_offset and length_offset:
            self.block(self.motion_code("G0"), "G43", "Z" + self.fmt(start[2]), "H{}".format(self.length_register_for(op)))
            self.state["Z"] = self.fmt(start[2])
        else:
            self.write_rapid(None, None, start[2])

    def write_plane_on(self, plane):
        """The tilted work plane command."""
        o, (a, b, c) = plane["origin"], plane["euler_zxz"]
        values = dict(x=self.fmt(o[0]), y=self.fmt(o[1]), z=self.fmt(o[2]), a=self.angle(a), b=self.angle(b), c=self.angle(c))
        for line in self.plane_on:
            self.block(line.format(**values))

    def write_multi_moves(self, op, coordinates, per_minute):
        """Simultaneous moves: X Y Z (tool tip, or the machine's position) with the rotary
        angles, feeds per minute or inverse time."""
        for move in op.get("moves", []):
            p = move.get(coordinates) or move["to"]
            words = self.axes(p[0], p[1], p[2]) + self.rotary_words(move.get("rotary") or [])
            if not words:
                continue
            if move["type"] == "rapid":
                self.block(self.motion_code("G0"), *words)
            elif per_minute:
                self.block(self.motion_code("G1"), *words, self.feed_word(move["feed"]))
            else:
                # Inverse time: every block carries its own F.
                self.block(self.motion_code("G1"), *words, "F" + self.feed_number(move["inverse_time"]))
        rotary = (op.get("moves") or [{}])[-1].get("rotary")
        if rotary:
            self.rotary_state = list(rotary)

    def write_tilted_drilling(self, op, style, new_tool):
        """Drilling on a tilted plane: canned cycles in the tilted plane's coordinates, or in the
        machine's own when the spindle ends up vertical (a table that turns the part)."""
        if style == "plane" and self.plane_coordinates == "plane":
            self.write_plane_on(op["work_plane"])
            start = op.get("start") or [0.0, 0.0, 0.0]
            self.approach(op, start, True)
            self.write_coolant(op.get("coolant"))
            self.write_drill_cycle(op, op["drill"])
            self.retract_home()
            for b in self.plane_off:
                self.block(b)
            self.reset_modal()
            return
        md = op.get("machine_drill")
        if md is None:
            raise PostError(
                "{}: drilling on a tilted plane with this machine needs the tilted work plane output".format(op.get("name", ""))
            )
        d = dict(op["drill"], holes=md["holes"], retract=md["retract"], clearance=md["clearance"])
        first = md["holes"][0] if md["holes"] else [0.0, 0.0, md["clearance"], 0.0]
        self.approach(op, [first[0], first[1], md["clearance"]], True)
        self.write_coolant(op.get("coolant"))
        self.write_drill_cycle(op, d)

    def write_smoothing(self, op):
        """Motion smoothing for an operation (blending, high-speed modes); none by default."""

    def is_roughing(self, op):
        """Whether an operation roughs (clears material, leaves stock) rather than finishes."""
        stock = op.get("stock_to_leave") or [0.0, 0.0]
        return op.get("strategy") in ("rough3d", "pocket2d", "face") or any(v > 1e-6 for v in stock)

    def length_register_for(self, op):
        t = self.tools.get(op["tool"], {})
        if self.length_register == "tool":
            return op["tool"]
        return t.get("length_offset") or op["tool"]

    def write_tool_change(self, op, following):
        t = self.tools.get(op["tool"], {})
        self.comment("T{} {}".format(op["tool"], t.get("name", "")))
        if not self.tool_changer:
            self.write_manual_tool_change(op)
            return
        self.retract_home()
        self.move_to_tool_change_position()
        self.block("T{}".format(op["tool"]), self.tool_change_code)
        if self.preselect_next_tool and following is not None and following["tool"] != op["tool"]:
            self.block("T{}".format(following["tool"]))
        cap = self.machine.get("tool_capacity") or 0
        if cap and op["tool"] > cap:
            self.warn("T{} is beyond the {}-pocket tool changer".format(op["tool"], cap))

    def move_to_tool_change_position(self):
        """G53 to the machine X,Y set in `tool_change_xy`, if any."""
        text = str(self.tool_change_xy or "").strip()
        if not text:
            return
        try:
            x, y = (float(v) for v in text.replace(" ", "").split(","))
        except ValueError:
            raise PostError("tool change position {!r} is not X,Y".format(text))
        self.block("G53", "G0", "X" + self.fmt(x), "Y" + self.fmt(y))

    def write_manual_tool_change(self, op):
        """Machines without a changer stop for the operator to fit the tool."""
        self.retract_home()
        self.move_to_tool_change_position()
        t = self.tools.get(op["tool"], {})
        self.operator_message("Fit T{} {} and press cycle start".format(op["tool"], t.get("name", "")).strip())
        self.block("M0")

    def write_spindle(self, op, changed):
        rpm = op.get("spindle_rpm") or 0
        limit = self.max_rpm or self.machine.get("max_spindle_rpm")
        if rpm <= 0:
            raise PostError("{} has no spindle speed".format(op.get("name", "an operation")))
        if limit and rpm > limit:
            raise PostError("{}: {:.0f} rpm is above the machine's {:.0f}".format(op.get("name", ""), rpm, limit))
        if changed:
            self.block("S{}".format(int(round(rpm))), "M3")
            if self.spindle_dwell > 0:
                self.block("G4", self.dwell_word(self.spindle_dwell))

    def write_coolant(self, coolant):
        """Turns coolant to `coolant` (None or "off" for off), writing only what changes."""
        kind = coolant if coolant and coolant != "off" else None
        if kind and kind not in self.coolant_codes:
            self.warn("this machine has no {} coolant; it is left off".format(kind))
            kind = None
        last = self.state.get("coolant")
        if kind == last:
            return
        if last:
            self.block(self.coolant_off_codes.get(last, self.coolant_off))
        if kind:
            self.block(self.coolant_codes[kind])
        self.state["coolant"] = kind

    def work_offset(self, index):
        """The work offset code for the operation's WCS (0 = the first)."""
        if index < self.standard_offsets:
            return "G{}".format(54 + index) if index < 6 else self.extra_standard_offset(index)
        n = index - self.standard_offsets + 1
        if not self.extended_offset or n > self.max_extended_offsets:
            raise PostError("this machine has no work offset for WCS number {}".format(index + 1))
        return self.extended_offset.format(n=n, p=index + 1)

    def extra_standard_offset(self, index):
        """Standard offsets past G59 (LinuxCNC's G59.1...)."""
        return "G59.{}".format(index - 5)

    # ---- motion ------------------------------------------------------------------------

    def motion_code(self, code):
        if self.repeat_motion_codes or self.state.get("motion") != code:
            self.state["motion"] = code
            return code
        return ""

    def axes(self, x, y, z):
        words = []
        for axis, value in (("X", x), ("Y", y), ("Z", z)):
            if value is None:
                continue
            text = self.fmt(value)
            if not self.modal_axes or self.state.get(axis) != text:
                words.append(axis + text)
                self.state[axis] = text
        return words

    def write_rapid(self, x, y, z):
        words = self.axes(x, y, z)
        if not words:
            return
        if self.rapid_feed:
            self.block(self.motion_code("G1"), *words, self.feed_word(self.rapid_feed))
        else:
            self.block(self.motion_code("G0"), *words)

    def feed_word(self, feed):
        text = self.fmt_feed(feed)
        if self.state.get("F") == text:
            return ""
        self.state["F"] = text
        return "F" + text

    def write_linear(self, x, y, z, feed):
        words = self.axes(x, y, z)
        if words:
            self.block(self.motion_code("G1"), *words, self.feed_word(feed))

    def write_arc(self, start, move):
        """G2 / G3 from `start` along `move`; written as lines when the machine cannot."""
        to, centre = move["to"], move["center"]
        radius = math.hypot(start[0] - centre[0], start[1] - centre[1])
        helical = abs(to[2] - start[2]) > 1e-6
        if (
            not self.arcs
            or (helical and not self.helical_arcs)
            or not self.min_arc_radius <= radius <= self.max_arc_radius
            or (self.arc_format == "r" and self.sweep(start, move) > math.pi - 1e-6)
        ):
            for p in self.linearize(start, move):
                self.write_linear(p[0], p[1], p[2], move["feed"])
            return
        code = "G2" if move["cw"] else "G3"
        # Arcs always give both plane axes; Z only when it changes (a helix).
        self.state["X"], self.state["Y"] = None, None
        words = self.axes(to[0], to[1], to[2])
        if self.arc_format == "r":
            words.append("R" + self.fmt(radius))
        else:
            words.append("I" + self.fmt(centre[0] - start[0]))
            words.append("J" + self.fmt(centre[1] - start[1]))
        # An arc always names its motion, even when the motion code is modal.
        self.state["motion"] = code
        self.block(code, *words, self.feed_word(move["feed"]))

    def sweep(self, start, move):
        cx, cy = move["center"]
        a0 = math.atan2(start[1] - cy, start[0] - cx)
        a1 = math.atan2(move["to"][1] - cy, move["to"][0] - cx)
        d = a1 - a0
        if move["cw"]:
            d = -d
        while d <= 0:
            d += 2 * math.pi
        return d

    def linearize(self, start, move):
        """Points along an arc within `linearize_tolerance`."""
        cx, cy = move["center"]
        r = math.hypot(start[0] - cx, start[1] - cy)
        sweep = self.sweep(start, move)
        step = 2 * math.acos(max(-1.0, min(1.0, 1 - self.linearize_tolerance / max(r, 1e-9)))) if r > 0 else sweep
        n = max(1, int(math.ceil(sweep / max(step, 1e-6))))
        a0 = math.atan2(start[1] - cy, start[0] - cx)
        sign = -1 if move["cw"] else 1
        for k in range(1, n + 1):
            t = k / n
            a = a0 + sign * sweep * t
            yield (cx + r * math.cos(a), cy + r * math.sin(a), start[2] + (move["to"][2] - start[2]) * t)

    def write_moves(self, op):
        at = list(op.get("start") or [0.0, 0.0, 0.0])
        for move in op.get("moves", []):
            kind = move["type"]
            to = move["to"]
            if kind == "rapid":
                self.write_rapid(to[0], to[1], to[2])
            elif kind == "linear":
                self.write_linear(to[0], to[1], to[2], move["feed"])
            elif kind == "arc":
                self.write_arc(at, move)
            else:
                raise PostError("unknown move type {!r}".format(kind))
            at = list(to)

    # ---- drilling ----------------------------------------------------------------------

    def write_drill_cycle(self, op, d):
        cycle = d["cycle"]
        code = (self.canned_cycles or {}).get(cycle)
        if code is None:
            self.write_expanded_cycle(op, d)
            return
        rpm = op.get("spindle_rpm") or 0
        if cycle == "tap":
            if self.rigid_tapping:
                self.block(self.rigid_tapping.format(rpm=int(round(rpm))))
            feed = d["pitch"] * rpm
        else:
            feed = d["feed"]
        extra = []
        if cycle in ("peck", "chip_break"):
            extra.append("Q" + self.fmt(d["peck"]))
        if cycle == "spot" or (cycle == "drill" and d.get("dwell", 0) > 0):
            code = self.canned_cycles.get("spot", code)
            extra.append(self.dwell_word(max(d.get("dwell", 0.0), 0.0)))
        last = None
        for i, (x, y, top, bottom) in enumerate(d["holes"]):
            words = [self.cycle_return if i == 0 else "", code, "X" + self.fmt(x), "Y" + self.fmt(y)]
            full = self.repeat_cycle_codes or i == 0 or last != (bottom,)
            if full:
                words += ["Z" + self.fmt(bottom), "R" + self.fmt(d["retract"])] + extra + ["F" + self.fmt_feed(feed)]
            else:
                words = words[:1] + words[2:]
            self.block(*words)
            last = (bottom,)
        self.block("G80")
        self.reset_modal()
        self.write_rapid(None, None, d["clearance"])

    def write_expanded_cycle(self, op, d):
        """A drilling cycle as plain moves, for controllers without canned cycles."""
        cycle = d["cycle"]
        if cycle == "tap":
            raise PostError("this machine cannot tap without a tapping cycle; use a different cycle or post")
        r = d["retract"]
        for x, y, top, bottom in d["holes"]:
            self.write_rapid(x, y, None)
            self.write_rapid(None, None, r)
            z = top
            while True:
                nxt = max(z - d["peck"], bottom) if cycle in ("peck", "chip_break") else bottom
                self.write_linear(None, None, nxt, d["feed"])
                z = nxt
                if z <= bottom + 1e-9:
                    break
                if cycle == "peck":
                    self.write_rapid(None, None, r)
                    self.write_rapid(None, None, min(z + 0.5, top))
                else:
                    self.write_rapid(None, None, min(z + 0.5, top))
            if d.get("dwell", 0) > 0 or cycle == "spot":
                self.block("G4", self.dwell_word(max(d.get("dwell", 0.0), 0.5 if cycle == "spot" else 0.0)))
            if cycle == "bore":
                self.write_linear(None, None, r, d["feed"])
            else:
                self.write_rapid(None, None, r)
        self.write_rapid(None, None, d["clearance"])
