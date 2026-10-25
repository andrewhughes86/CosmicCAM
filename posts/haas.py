"""Haas mill (NGC and classic controls).

Extended offsets G154 P1-P99, through-spindle coolant M88 / M89, air blast M83 / M84, tool
preselect, Y home at the end to bring the table forward, and optional G187 smoothing.
Rigid tapping is set on the control, so G84 is written without M29. On 5-axis machines (UMC,
trunnions): TCPC (G234) for simultaneous moves and dynamic work offsets (G254 / G255) for 3+2.
"""

from cosmiccam_post import Post


class HaasPost(Post):
    name = "Haas"
    description = "Haas VF-series mill (NGC and classic controls)"
    extension = "nc"

    program_number_digits = 5
    preselect_next_tool = True
    optional_stop = True
    extended_offset = "G154 P{n}"
    max_extended_offsets = 99
    coolant_codes = {"flood": "M8", "air": "M83", "through_tool": "M88"}
    coolant_off_codes = {"through_tool": "M89", "air": "M84"}
    home_xy = ("G91 G28 Y0", "G90")
    return_home_xy = True
    #: G187 P1 (rough) to P3 (finish) motion smoothing at the start of each operation, or None.
    smoothing = None
    # Tool centre point control and dynamic work offsets: the program stays in part
    # coordinates while the rotary axes turn the part.
    tcp_on = ("G234 H{h}",)
    tcp_off = ("G49",)
    plane_on = ("G254",)
    plane_off = ("G255",)
    plane_coordinates = "wcs"

    def write_spindle(self, op, changed):
        super().write_spindle(op, changed)
        if self.smoothing and changed:
            self.block(self.smoothing)
