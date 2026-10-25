"""LinuxCNC 3-axis mill (also Tormach PathPilot, which runs LinuxCNC).

Written from LinuxCNC's G-code reference. Settings (Export -> Post-processor) cover:

- Path blending: G64 with separate P tolerances for roughing and finishing (and an optional Q
  naive-CAM tolerance), or G61 exact stop.
- Tool changes: T M6 with G43 H, the next tool preloaded, a tool-change position (G53 X Y),
  or manual changes with an operator message ((MSG, ...)) and M0.
- Safe retract: G53 G0 Z0 (machine top), G28, or staying at clearance; X/Y home at the end.
- Rigid tapping with G33.1 (spindle-synchronised) or the tapping canned cycle.
- Coolant: M7 mist, M8 flood, and through-tool or air blast on digital outputs (M64 / M65 P<pin>).
- Each operation as an o-word subroutine (o100 sub ... o100 endsub, then o100 call ...).
- Restart-safe operations, optional stops, operator notes and pass-through G-code per operation.
- No O program number (O words are subroutines in LinuxCNC), work offsets G54-G59 and G59.1-G59.3,
  dwell in seconds, M2 at the end.
- 4/5-axis: the machine's own positions with inverse-time feeds (G93), which any LinuxCNC
  configuration with rotary joints runs; or tool tip positions for a configuration with
  switchable TCP kinematics, with the codes that switch it set in the post's settings.
"""

from cosmiccam_post import Post, PostError, Property


class LinuxCNCPost(Post):
    name = "LinuxCNC"
    description = "LinuxCNC / PathPilot 3-axis mill"
    extension = "ngc"

    properties = {
        "blending": Property(
            "Path blending",
            "G64 blends moves within a tolerance; G61 stops exactly at every corner",
            "Motion",
            ("g64", "g61", "controller"),
        ),
        "blend_rough": Property("Roughing blend tolerance (mm)", "G64 P for roughing operations", "Motion"),
        "blend_finish": Property("Finishing blend tolerance (mm)", "G64 P for finishing operations", "Motion"),
        "naive_tolerance": Property(
            "Naive CAM tolerance (mm)", "G64 Q: collinear moves within this are merged; 0 leaves it out", "Motion"
        ),
        "tapping": Property(
            "Tapping",
            "G33.1 rigid (spindle-synchronised) tapping, or the G84 tapping cycle",
            "Drilling",
            ("g33.1", "g84"),
        ),
        "through_tool_pin": Property(
            "Through-tool coolant output", "Digital output pin switched with M64 / M65; -1 for none", "Coolant"
        ),
        "air_pin": Property("Air blast output", "Digital output pin switched with M64 / M65; -1 for none", "Coolant"),
        "subroutines": Property(
            "Operations as subroutines", "Write each operation as an o-word subroutine and call them in order", "Program"
        ),
        "end_code": Property("End of program", "M2 or M30", "Program", ("M2", "M30")),
        "retract_mode": Property(
            "Safe retract",
            "G53 G0 Z0 to the machine's top, G28 to its stored position, or stay at clearance",
            "Program",
            ("g53", "g28", "clearance"),
        ),
        # retract_mode decides this.
        "safe_retract": None,
        "plane_output": Property(
            "3+2 operations",
            "machine: the machine's own positions; tcp: tool tip positions (needs TCP kinematics)",
            "Multi-axis",
            ("machine", "tcp"),
        ),
        "tcp_on_code": Property("Code that turns TCP on", "For switchable kinematics configurations", "Multi-axis"),
        "tcp_off_code": Property("Code that turns TCP off", "", "Multi-axis"),
    }

    multi_axis_output = "machine"
    plane_output = "machine"
    tcp_on_code = ""
    tcp_off_code = ""

    program_number = False
    standard_offsets = 9
    extended_offset = None
    dwell_units = "seconds"
    canned_cycles = {"drill": "G81", "spot": "G82", "peck": "G83", "chip_break": "G73", "tap": "G84", "bore": "G85"}
    home_xy = ("G53 G0 X0 Y0",)
    preselect_next_tool = True

    blending = "g64"
    blend_rough = 0.05
    blend_finish = 0.01
    naive_tolerance = 0.0
    tapping = "g33.1"
    through_tool_pin = -1
    air_pin = -1
    subroutines = False
    end_code = "M2"
    retract_mode = "g53"

    # ---- framing ---------------------------------------------------------------------

    def run(self, cl):
        self.program_end = (self.end_code,)
        return super().run(cl)

    def apply_properties(self, values):
        super().apply_properties(values)
        self.program_end = (self.end_code,)
        self.safe_retract = "clearance" if self.retract_mode == "clearance" else "home"
        self.home_z = ("G28",) if self.retract_mode == "g28" else ("G53 G0 Z0",)
        codes = {"flood": "M8", "mist": "M7"}
        off = {}
        for kind, pin in (("through_tool", self.through_tool_pin), ("air", self.air_pin)):
            if int(pin) >= 0:
                codes[kind] = "M64 P{}".format(int(pin))
                off[kind] = "M65 P{}".format(int(pin))
        self.coolant_codes = codes
        self.coolant_off_codes = off
        self.tcp_on = (self.tcp_on_code,) if self.tcp_on_code.strip() else ()
        self.tcp_off = (self.tcp_off_code,) if self.tcp_off_code.strip() else ()

    def operator_message(self, text):
        # LinuxCNC shows (MSG, ...) comments to the operator.
        text = str(text).replace("(", "[").replace(")", "]")
        self.lines.append("(MSG, {})".format(text[:120]))

    # ---- motion smoothing --------------------------------------------------------------

    def write_smoothing(self, op):
        if self.blending == "g61":
            self.block("G61")
        elif self.blending == "g64":
            p = self.blend_rough if self.is_roughing(op) else self.blend_finish
            words = ["G64", "P" + self.fmt(p)]
            if self.naive_tolerance and self.naive_tolerance > 0:
                words.append("Q" + self.fmt(self.naive_tolerance))
            self.block(*words)

    # ---- drilling ----------------------------------------------------------------------

    def write_drill_cycle(self, op, d):
        if d["cycle"] == "tap" and self.tapping == "g33.1":
            self.write_rigid_tap(op, d)
            return
        super().write_drill_cycle(op, d)

    def write_rigid_tap(self, op, d):
        """G33.1: the spindle-synchronised move down at the thread's pitch and back out."""
        if not d.get("pitch"):
            raise PostError("{}: tapping needs the thread pitch".format(op.get("name", "")))
        for x, y, top, bottom in d["holes"]:
            self.write_rapid(x, y, None)
            self.write_rapid(None, None, d["retract"])
            self.block("G33.1", "Z" + self.fmt(bottom), "K" + self.fmt(d["pitch"]))
            # G33.1 returns to where it started.
            self.state["Z"] = self.fmt(d["retract"])
        self.write_rapid(None, None, d["clearance"])

    # ---- subroutines -------------------------------------------------------------------

    def write_operation(self, op, previous, following):
        if not self.subroutines:
            super().write_operation(op, previous, following)
            return
        # Each operation's body goes into its own subroutine; the main program calls it.
        number = 100 + len(self._subs)
        main, self.lines = self.lines, []
        self.lines.append("o{} sub".format(number))
        super().write_operation(op, previous, following)
        self.lines.append("o{} endsub".format(number))
        self._subs.append(self.lines)
        self.lines = main
        self.comment(op.get("name", ""))
        self.lines.append("o{} call".format(number))

    def write_program_start(self, job):
        self._subs = []
        super().write_program_start(job)
        self._header_end = len(self.lines)

    def write_program_end(self, job):
        super().write_program_end(job)
        if self.subroutines and self._subs:
            # LinuxCNC needs subroutines defined before they are called: after the header.
            defs = [line for sub in self._subs for line in sub]
            self.lines[self._header_end : self._header_end] = defs
