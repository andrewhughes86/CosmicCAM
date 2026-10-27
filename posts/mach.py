"""Mach3 / Mach4 3-axis mill.

Fixture offsets beyond G59 as G59 P7-P253, canned cycles, incremental arc centres (Mach's default
IJ mode), dwell in seconds (check Mach's "G4 dwell in ms" setting matches), .tap files. Mach has
no tool tip control or tilted planes: rotary work is written in the machine's own positions with
inverse-time feeds.
"""

from cosmicdms_post import Post


class MachPost(Post):
    properties = {"multi_axis_output": None, "plane_output": None}
    multi_axis_output = "machine"
    plane_output = "machine"

    name = "Mach3/Mach4"
    description = "Mach3 / Mach4 3-axis mill"
    extension = "tap"

    program_number = False
    extended_offset = "G59 P{p}"
    max_extended_offsets = 247
    dwell_units = "seconds"
    coolant_codes = {"flood": "M8", "mist": "M7"}
