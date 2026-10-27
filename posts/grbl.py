"""Grbl 1.1 (hobby CNC routers and mills).

Grbl has no tool changer, tool length offsets or canned cycles: tool changes stop for the operator
(M0), drilling is written as moves, and there are no program numbers or line numbers. Arcs are
written with I and J; dwell is in seconds; G54-G59 only.
"""

from cosmicdms_post import Post


class GrblPost(Post):
    # Three axes only.
    properties = {"multi_axis_output": None, "plane_output": None, "rotary_home_at_end": None}
    multi_axis_output = None
    plane_output = None

    name = "Grbl"
    description = "Grbl 1.1 router or mill (manual tool changes)"
    extension = "gcode"

    percent_signs = False
    program_number = False
    safe_start = ("G90", "G94", "G17")
    tool_changer = False
    length_offset = False
    canned_cycles = None
    dwell_units = "seconds"
    standard_offsets = 6
    extended_offset = None
    coolant_codes = {"flood": "M8", "mist": "M7"}
    home_z = ()
    program_end = ("M30",)
