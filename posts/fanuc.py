"""Fanuc-style mill, 3 to 5 axes (0i, 30i and compatible controls).

Program numbers O0001-O9999, G54-G59 then G54.1 P1-P48, canned cycles with rigid tapping (M29),
dwell in milliseconds, tool preselect, G28 home. On 4/5-axis machines: tool centre point control
(G43.4) for simultaneous moves and the tilted working plane (G68.2 ... G53.1, G69) for 3+2,
both control options; or the machine's own positions with inverse-time feeds (G93).
"""

from cosmicdms_post import Post


class FanucPost(Post):
    name = "Fanuc"
    description = "Fanuc-style mill, 3 to 5 axes (0i, 30i and compatibles)"
    extension = "nc"

    preselect_next_tool = True
    optional_stop = True
    rigid_tapping = "M29 S{rpm}"
    extended_offset = "G54.1 P{n}"
    max_extended_offsets = 48
