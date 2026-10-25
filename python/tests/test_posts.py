"""Tests for the post-processors shipped in posts/: the library's formatting and modal output,
each controller's dialect, and the runner CosmicCAM starts."""

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
POSTS = os.path.join(ROOT, "posts")
sys.path.insert(0, POSTS)

import cosmiccam_post  # noqa: E402
import fanuc  # noqa: E402
import grbl  # noqa: E402
import haas  # noqa: E402
import linuxcnc  # noqa: E402
import mach  # noqa: E402


def cl(**job):
    """A small job: a profile with a quarter arc and a helical entry, then a pecked hole."""
    data = {
        "format": "CosmicCAM-CL/1",
        "job": dict({"name": "Bracket", "units": "mm", "program_number": 7, "line_numbers": False}, **job),
        "machine": {"name": "Test mill", "travel": [500, 400, 300], "max_spindle_rpm": 12000, "max_feed": 5000, "tool_capacity": 20},
        "tools": [
            {"number": 1, "name": "6 mm flat", "diameter": 6.0, "length_offset": 1},
            {"number": 2, "name": "5 mm drill", "diameter": 5.0, "length_offset": 2},
        ],
        "operations": [
            {
                "id": "toolpath.1",
                "name": "Profile",
                "tool": 1,
                "spindle_rpm": 10000,
                "coolant": "flood",
                "work_offset": 0,
                "start": [0.0, -40.0, 50.0],
                "moves": [
                    {"type": "rapid", "to": [0.0, -40.0, 5.0]},
                    {"type": "linear", "to": [0.0, -40.0, -2.0], "feed": 300.0},
                    {"type": "linear", "to": [20.0, -40.0, -2.0], "feed": 1000.0},
                    {"type": "arc", "to": [30.0, -30.0, -2.0], "center": [20.0, -30.0], "cw": False, "plane": "xy", "feed": 1000.0},
                    {"type": "arc", "to": [20.0, -30.0, -3.0], "center": [25.0, -30.0], "cw": False, "plane": "xy", "feed": 1000.0},
                    {"type": "rapid", "to": [20.0, -30.0, 50.0]},
                ],
                "drill": None,
            },
            {
                "id": "toolpath.2",
                "name": "Holes",
                "tool": 2,
                "spindle_rpm": 3000,
                "coolant": "flood",
                "work_offset": 0,
                "start": [10.0, 10.0, 50.0],
                "moves": [],
                "drill": {
                    "cycle": "peck",
                    "holes": [[10.0, 10.0, 0.0, -15.0], [30.0, 10.0, 0.0, -15.0]],
                    "retract": 2.0,
                    "clearance": 50.0,
                    "peck": 5.0,
                    "dwell": 0.0,
                    "feed": 150.0,
                    "pitch": 0.0,
                },
            },
        ],
        "warnings": [],
    }
    return data


def post(cls, data=None):
    p = cls()
    return p.run(data or cl()).splitlines(), p


class LibraryTest(unittest.TestCase):
    def test_numbers(self):
        n = cosmiccam_post.Number(3)
        self.assertEqual(n(1.5), "1.500")
        self.assertEqual(n(-0.0001), "0.000")
        self.assertEqual(cosmiccam_post.Number(4, trailing_zeros=False)(2.5), "2.5")
        self.assertEqual(cosmiccam_post.Number(3, trailing_zeros=False, force_decimal=False)(2.0), "2")

    def test_fanuc_program(self):
        lines, p = post(fanuc.FanucPost)
        text = "\n".join(lines)
        self.assertEqual(lines[0], "%")
        self.assertEqual(lines[1], "O0007")
        self.assertIn("G21", lines)
        self.assertIn("T1 M6", lines)
        self.assertIn("T2", lines, "the next tool is called after the change")
        self.assertIn("S10000 M3", lines)
        self.assertIn("G0 G43 Z50.000 H1", lines)
        self.assertIn("G3 X30.000 Y-30.000 I0.000 J10.000", lines)
        # A helix: Z changes along the arc.
        self.assertIn("G3 X20.000 Y-30.000 Z-3.000 I-5.000 J0.000", lines)
        self.assertIn("M1", lines)
        self.assertIn("G99 G83 X10.000 Y10.000 Z-15.000 R2.000 Q5.000 F150.0", lines)
        self.assertIn("G80", lines)
        self.assertEqual(lines[-2:], ["M30", "%"])
        # Feed is modal; coolant goes off for the tool change and back on after it.
        self.assertEqual(text.count("F1000.0"), 1)
        self.assertEqual(text.count("M8"), 2)
        self.assertLess(lines.index("M9"), lines.index("T2 M6"))
        # Every motion block names its motion.
        for line in lines:
            if line[:1] in "XYZ":
                self.fail("motion without a G code: " + line)

    def test_inches_and_line_numbers(self):
        lines, _ = post(fanuc.FanucPost, cl(units="in", line_numbers=True))
        self.assertTrue(any(l.endswith("G20") for l in lines))
        self.assertTrue(any("X0.7874" in l for l in lines), "20 mm is 0.7874 in")
        self.assertTrue(any("F39.37" in l for l in lines), "1000 mm/min is 39.37 in/min")
        numbered = [l for l in lines if l[:1] == "N"]
        self.assertEqual(numbered[0].split()[0], "N10")
        self.assertEqual(numbered[1].split()[0], "N20")

    def test_r_arcs_and_lines_for_machines_without_arcs(self):
        class RPost(fanuc.FanucPost):
            arc_format = "r"

        lines, _ = post(RPost)
        self.assertIn("G3 X30.000 Y-30.000 R10.000", lines)

        class NoArcs(fanuc.FanucPost):
            arcs = False

        lines, _ = post(NoArcs)
        self.assertFalse(any(l.startswith(("G2 ", "G3 ")) for l in lines))
        self.assertTrue(sum(1 for l in lines if l.startswith("G1 X")) > 20, "arcs become short lines")

    def test_canned_cycles(self):
        data = cl()
        for cycle, code in [("drill", "G81"), ("spot", "G82"), ("chip_break", "G73"), ("bore", "G85")]:
            d = copy.deepcopy(data)
            d["operations"][1]["drill"]["cycle"] = cycle
            if cycle == "spot":
                d["operations"][1]["drill"]["dwell"] = 0.5
            lines, _ = post(fanuc.FanucPost, d)
            hole = next(l for l in lines if code in l)
            self.assertIn("X10.000", hole)
            if cycle == "spot":
                self.assertIn("P500", hole, "dwell in milliseconds")
        d = copy.deepcopy(data)
        d["operations"][1]["drill"].update(cycle="tap", pitch=1.0)
        lines, _ = post(fanuc.FanucPost, d)
        self.assertIn("M29 S3000", lines)
        self.assertTrue(any(l.startswith("G99 G84") and "F3000.0" in l for l in lines), "tap feed = pitch × rpm")

    def test_coolant_and_offsets(self):
        d = cl()
        d["operations"][0]["coolant"] = "through_tool"
        d["operations"][1]["work_offset"] = 7
        lines, _ = post(fanuc.FanucPost, d)
        self.assertIn("M88", lines)
        self.assertIn("M89", lines, "through-spindle coolant has its own off code")
        self.assertIn("G54.1 P2", lines)
        d["operations"][1]["work_offset"] = 60
        with self.assertRaises(cosmiccam_post.PostError):
            post(fanuc.FanucPost, d)

    def test_errors(self):
        d = cl()
        d["operations"][0]["spindle_rpm"] = 0
        with self.assertRaises(cosmiccam_post.PostError):
            post(fanuc.FanucPost, d)
        d = cl()
        d["operations"][0]["spindle_rpm"] = 20000
        with self.assertRaises(cosmiccam_post.PostError):
            post(fanuc.FanucPost, d)
        with self.assertRaises(cosmiccam_post.PostError):
            post(fanuc.FanucPost, dict(cl(), operations=[]))


class ControllerTest(unittest.TestCase):
    def test_haas(self):
        d = cl()
        d["operations"][0]["coolant"] = "air"
        d["operations"][1]["work_offset"] = 6
        lines, _ = post(haas.HaasPost, d)
        self.assertEqual(lines[1], "O00007")
        self.assertIn("M83", lines)
        self.assertIn("M84", lines)
        self.assertIn("G154 P1", lines)
        self.assertIn("G91 G28 Y0", lines)

    def test_linuxcnc(self):
        d = cl()
        d["operations"][1]["work_offset"] = 6
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertFalse(any(l.startswith("O") for l in lines), "O words are subroutines in LinuxCNC")
        self.assertIn("G64 P0.010", lines)
        self.assertIn("G59.1", lines)
        self.assertIn("G53 G0 Z0", lines)
        self.assertEqual(lines[-2:], ["M2", "%"])
        d["operations"][1]["work_offset"] = 9
        with self.assertRaises(cosmiccam_post.PostError):
            post(linuxcnc.LinuxCNCPost, d)

    def test_grbl(self):
        lines, _ = post(grbl.GrblPost)
        text = "\n".join(lines)
        self.assertNotIn("%", lines)
        self.assertNotIn("M6", text)
        self.assertNotIn("G43", text)
        self.assertIn("M0", lines, "manual tool change")
        self.assertNotIn("G83", text)
        self.assertIn("G1 Z-5.000 F150.0", lines, "pecks written as moves")
        self.assertIn("G1 Z-15.000", lines)
        d = cl()
        d["operations"][1]["drill"].update(cycle="tap", pitch=1.0)
        with self.assertRaises(cosmiccam_post.PostError):
            post(grbl.GrblPost, d)

    def test_mach(self):
        d = cl()
        d["operations"][1]["work_offset"] = 6
        lines, p = post(mach.MachPost, d)
        self.assertIn("G59 P7", lines)
        self.assertEqual(p.extension, "tap")


class PropertyTest(unittest.TestCase):
    def test_settings_are_applied_and_checked(self):
        d = cl()
        d["properties"] = {"line_numbers": "true", "line_number_start": "100", "line_number_step": "5", "separate_words": "false"}
        lines, _ = post(fanuc.FanucPost, d)
        numbered = [l for l in lines if l.startswith("N")]
        self.assertEqual(numbered[0], "N100G90G94G17G40G49G80")
        self.assertTrue(numbered[1].startswith("N105"))
        for bad in ({"line_numbers": "maybe"}, {"line_number_step": "x"}, {"comment_style": "fancy"}):
            d = cl()
            d["properties"] = bad
            with self.assertRaises(cosmiccam_post.PostError):
                post(fanuc.FanucPost, d)
        d = cl()
        d["properties"] = {"nonesuch": "1"}
        _, p = post(fanuc.FanucPost, d)
        self.assertTrue(any("nonesuch" in w for w in p.warnings))

    def test_described_settings_follow_each_posts_defaults(self):
        props = {p["name"]: p for p in linuxcnc.LinuxCNCPost.describe_properties()}
        self.assertEqual(props["blend_finish"]["kind"], "float")
        self.assertEqual(props["tapping"]["choices"], ["g33.1", "g84"])
        self.assertEqual(props["preselect_next_tool"]["default"], True)
        self.assertNotIn("safe_retract", props, "LinuxCNC's retract_mode replaces it")
        self.assertEqual(props["rapid_feed"]["default"], 0.0)
        self.assertEqual({p["name"]: p for p in fanuc.FanucPost.describe_properties()}["optional_stop"]["default"], True)

    def test_operation_options_restart_blocks_and_tool_change_position(self):
        d = cl()
        d["operations"][1].update(stop_before=True, notes="Turn the part over", code_before="M100 P2", code_after="M101")
        d["properties"] = {"restart_blocks": "true", "tool_change_xy": "-10,0"}
        lines, _ = post(fanuc.FanucPost, d)
        self.assertEqual(lines.count("(TURN THE PART OVER)"), 1)
        self.assertLess(lines.index("M0"), lines.index("M100 P2"))
        self.assertIn("M101", lines)
        self.assertEqual(lines.count("G90 G94 G17 G40 G49 G80"), 2, "the safe start again at the second operation")
        self.assertIn("G53 G0 X-10.000 Y0.000", lines)


class LinuxCNCTest(unittest.TestCase):
    def test_blending_follows_roughing_and_finishing(self):
        d = cl()
        d["operations"][0]["strategy"] = "rough3d"
        d["operations"][1]["strategy"] = "drill"
        d["properties"] = {"naive_tolerance": "0.005"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertIn("G64 P0.050 Q0.005", lines)
        self.assertIn("G64 P0.010 Q0.005", lines)
        d["properties"] = {"blending": "g61"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertIn("G61", lines)

    def test_rigid_tapping_and_coolant_outputs(self):
        d = cl()
        d["operations"][1]["drill"].update(cycle="tap", pitch=1.25)
        d["operations"][0]["coolant"] = "through_tool"
        d["properties"] = {"through_tool_pin": "2"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertIn("G33.1 Z-15.000 K1.250", lines)
        self.assertIn("M64 P2", lines)
        self.assertIn("M65 P2", lines)
        d["properties"] = {"tapping": "g84"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertTrue(any(l.startswith("G99 G84") for l in lines))
        # Without an output for it, through-tool coolant is left off with a warning.
        d["properties"] = {}
        _, p = post(linuxcnc.LinuxCNCPost, d)
        self.assertTrue(any("coolant" in w for w in p.warnings))

    def test_subroutines_are_defined_before_they_are_called(self):
        d = cl()
        d["properties"] = {"subroutines": "true", "end_code": "M30"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertLess(lines.index("o100 sub"), lines.index("o100 endsub"))
        self.assertLess(lines.index("o101 endsub"), lines.index("o100 call"))
        self.assertLess(lines.index("o100 call"), lines.index("o101 call"))
        self.assertEqual(lines[-2:], ["M30", "%"])

    def test_retract_modes_and_manual_tool_changes(self):
        d = cl()
        d["properties"] = {"retract_mode": "g28", "return_home_xy": "true"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertIn("G28", lines)
        self.assertIn("G53 G0 X0 Y0", lines)
        d["operations"][0].update(stop_before=True, notes="Clamp the part\nCheck the vise")
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertIn("(MSG, Clamp the part)", lines)
        self.assertIn("(MSG, Check the vise)", lines)
        self.assertNotIn("(CLAMP THE PART)", lines, "shown once, as the message")
        d["operations"][0].update(stop_before=False, notes="")
        d["properties"] = {"retract_mode": "clearance", "manual_tool_change": "true"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertNotIn("G53 G0 Z0", lines)
        self.assertNotIn("T2 M6", lines)
        self.assertTrue(any(l.startswith("(MSG, Fit T2") for l in lines))
        self.assertIn("M0", lines)


def five_axis_cl():
    """The small job on a trunnion (A tilts, C turns): a 3+2 profile on a plane tilted 30° about X,
    then a simultaneous move that tilts the tool."""
    d = cl()
    d["machine"]["kinematics"] = {
        "layout": "table_table",
        "axes": [{"name": "A", "min": -120, "max": 120}, {"name": "C", "min": -99999, "max": 99999}],
        "primary_centre": [0, 0, 0],
        "secondary_centre": [0, 0, 0],
        "pivot_length": 0,
    }
    plane = d["operations"][0]
    plane.update(
        move_frame="plane",
        rotary=[30.0, 0.0],
        tool_axis=[0.0, -0.5, 0.866025],
        work_plane={"origin": [0, 0, 0], "x": [1, 0, 0], "y": [0, 0.866025, 0.5], "z": [0, -0.5, 0.866025], "euler_zxz": [0.0, 30.0, 0.0]},
        wcs_start=[0.0, -50.0, 30.0],
        machine_start=[0.0, -40.0, 50.0],
        wcs_moves=[{"type": "rapid", "to": [0.0, -40.0, 5.0]}, {"type": "linear", "to": [20.0, -40.0, -2.0], "feed": 1000.0}],
        machine_moves=[{"type": "rapid", "to": [0.0, -37.0, 24.3]}, {"type": "linear", "to": [20.0, -33.6, 18.2], "feed": 1000.0}],
    )
    sim = d["operations"][1]
    sim.update(
        name="Swarf",
        drill=None,
        multi_axis=True,
        rotary=[0.0, 0.0],
        tool_axis=[0, 0, 1],
        start=[10.0, 10.0, 50.0],
        machine_start=[10.0, 10.0, 50.0],
        moves=[
            {"type": "rapid", "to": [10.0, 10.0, 5.0], "axis": [0, 0, 1], "rotary": [0.0, 0.0], "machine": [10.0, 10.0, 5.0]},
            {"type": "linear", "to": [12.0, 10.0, 0.0], "axis": [0, -0.17, 0.98], "rotary": [10.0, 0.0], "machine": [12.0, 9.85, 1.74], "feed": 800.0, "inverse_time": 148.6},
            {"type": "linear", "to": [14.0, 10.0, 0.0], "axis": [0, -0.34, 0.94], "rotary": [20.0, 0.0], "machine": [14.0, 9.4, 3.42], "feed": 800.0, "inverse_time": 400.0},
        ],
    )
    return d


class FiveAxisTest(unittest.TestCase):
    def test_fanuc_tilts_the_plane_and_follows_the_tool_tip(self):
        lines, _ = post(fanuc.FanucPost, five_axis_cl())
        self.assertIn("G0 A30.000 C0.000", lines)
        self.assertIn("G68.2 X0.000 Y0.000 Z0.000 I0.000 J30.000 K0.000", lines)
        self.assertLess(lines.index("G68.2 X0.000 Y0.000 Z0.000 I0.000 J30.000 K0.000"), lines.index("G53.1"))
        self.assertIn("G69", lines)
        self.assertIn("G43.4 H2", lines)
        self.assertIn("G1 X12.000 Z0.000 A10.000 F800.0", lines)
        # Rotary axes back to zero at the end.
        self.assertIn("G0 A0.000 C0.000", lines[lines.index("G49", lines.index("G43.4 H2")) :])

    def test_haas_uses_tcpc_and_dynamic_work_offsets(self):
        lines, _ = post(haas.HaasPost, five_axis_cl())
        self.assertIn("G254", lines)
        self.assertIn("G255", lines)
        self.assertIn("G234 H2", lines)
        # Under G254 the moves are in the WCS's coordinates.
        self.assertIn("G1 X20.000 Z-2.000 F1000.0", lines)

    def test_linuxcnc_writes_machine_positions_with_inverse_time(self):
        lines, _ = post(linuxcnc.LinuxCNCPost, five_axis_cl())
        self.assertNotIn("G43.4 H2", lines)
        self.assertIn("G1 X20.000 Y-33.600 Z18.200 F1000.0", lines)
        g93 = lines.index("G93")
        self.assertEqual(lines[g93 + 1 : g93 + 5], ["G0 Z5.000", "G1 X12.000 Y9.850 Z1.740 A10.000 F148.6", "G1 X14.000 Y9.400 Z3.420 A20.000 F400.0", "G94"])
        # With TCP kinematics and their codes set, tool tip positions instead.
        d = five_axis_cl()
        d["properties"] = {"plane_output": "tcp", "tcp_on_code": "M428", "tcp_off_code": "M429"}
        lines, _ = post(linuxcnc.LinuxCNCPost, d)
        self.assertIn("M428", lines)
        self.assertIn("G1 X20.000 Z-2.000 F1000.0", lines)
        # TCP without the codes cannot be written.
        d["properties"] = {"plane_output": "tcp"}
        with self.assertRaises(cosmiccam_post.PostError):
            post(linuxcnc.LinuxCNCPost, d)

    def test_three_axis_controls_refuse_tilted_work(self):
        with self.assertRaises(cosmiccam_post.PostError):
            post(grbl.GrblPost, five_axis_cl())
        self.assertNotIn("multi_axis_output", {p["name"] for p in grbl.GrblPost.describe_properties()})
        lines, _ = post(mach.MachPost, five_axis_cl())
        self.assertIn("G93", lines)
        # A tilted operation for a machine with no rotary axes.
        d = five_axis_cl()
        del d["machine"]["kinematics"]
        with self.assertRaises(cosmiccam_post.PostError):
            post(fanuc.FanucPost, d)


class RunnerTest(unittest.TestCase):
    def run_post(self, request):
        out = subprocess.run(
            [sys.executable, "-I", "-B", os.path.join(POSTS, "run_post.py")],
            input=json.dumps(request),
            capture_output=True,
            text=True,
            timeout=60,
        )
        return json.loads(out.stdout)

    def test_posts_and_describes(self):
        answer = self.run_post({"action": "post", "post": os.path.join(POSTS, "fanuc.py"), "cl": cl()})
        self.assertIn("G21", answer["gcode"])
        self.assertEqual(answer["extension"], "nc")
        info = self.run_post({"action": "describe", "post": os.path.join(POSTS, "grbl.py")})
        self.assertEqual(info["name"], "Grbl")

    def test_a_broken_post_is_reported_not_crashed(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "broken.py")
            with open(path, "w") as f:
                f.write("from cosmiccam_post import Post\n\nclass Broken(Post):\n    def write_program_start(self, job):\n        raise ValueError('oops')\n")
            answer = self.run_post({"action": "post", "post": path, "cl": cl()})
            self.assertIn("oops", answer["error"])
            answer = self.run_post({"action": "post", "post": os.path.join(d, "missing.py"), "cl": cl()})
            self.assertIn("no post", answer["error"])
        answer = self.run_post({"action": "post", "post": os.path.join(POSTS, "fanuc.py"), "cl": dict(cl(), operations=[])})
        self.assertIn("no operations", answer["error"])


if __name__ == "__main__":
    unittest.main()
