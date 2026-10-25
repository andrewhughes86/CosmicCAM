"""Unit tests for the Python client against a fake control channel (no app needed)."""

import inspect
import json
import socket
import threading
import unittest

import cosmiccam
from cosmiccam._commands import Commands
from cosmiccam.client import CosmicCAM, CosmicCAMError

TOKEN = "t" * 64


class FakeApp:
    """Speaks the control protocol: checks the token, records requests, answers from a table."""

    def __init__(self):
        self.requests = []
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen()
        self.port = self.listener.getsockname()[1]
        threading.Thread(target=self.serve, daemon=True).start()

    def serve(self):
        conn, _ = self.listener.accept()
        # Close everything when the client hangs up, so no thread outlives the test run.
        with self.listener, conn, conn.makefile("rwb") as f:
            self.answer(f)

    def answer(self, f):
        first = True
        for line in f:
            req = json.loads(line)
            self.requests.append(req)
            if first and req.get("token") != TOKEN:
                reply = {"id": req["id"], "error": {"kind": "unauthorized", "message": "wrong token"}}
            elif req["method"] == "engine.execute" and req["params"]["command"] == "nope":
                reply = {"id": req["id"], "error": {"kind": "unknown_command", "message": "unknown command: nope"}}
            else:
                reply = {"id": req["id"], "result": {"echo": req["method"], "params": req.get("params")}}
            first = False
            f.write(json.dumps(reply).encode() + b"\n")
            f.flush()


class ClientTests(unittest.TestCase):
    def test_token_goes_only_on_the_first_request(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.ping()
        cc.state()
        self.assertEqual(app.requests[0]["token"], TOKEN)
        self.assertNotIn("token", app.requests[1])
        self.assertEqual([r["id"] for r in app.requests], [1, 2])
        cc.close()

    def test_wrong_token_raises_unauthorized(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, "wrong", timeout=5)
        with self.assertRaises(CosmicCAMError) as ctx:
            cc.ping()
        self.assertEqual(ctx.exception.kind, "unauthorized")
        cc.close()

    def test_errors_carry_their_kind(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        with self.assertRaises(CosmicCAMError) as ctx:
            cc.run("nope")
        self.assertEqual(ctx.exception.kind, "unknown_command")
        cc.close()

    def test_generated_methods_send_only_given_parameters(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.view_camera_set(yaw=-90, target=[1, 2, 3])
        cc.view_fit()
        sent = [r["params"] for r in app.requests]
        self.assertEqual(sent[0], {"command": "view.camera.set", "params": {"target": [1, 2, 3], "yaw": -90}})
        self.assertEqual(sent[1], {"command": "view.fit", "params": {}})
        cc.close()

    def test_model_import_sends_path_and_units(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.model_import(path="/parts/bracket.stl", units="in")
        cc.model_list()
        cc.model_remove(id="model.1")
        sent = [r["params"] for r in app.requests]
        self.assertEqual(
            sent[0], {"command": "model.import", "params": {"path": "/parts/bracket.stl", "units": "in"}}
        )
        self.assertEqual(sent[1], {"command": "model.list", "params": {}})
        self.assertEqual(sent[2], {"command": "model.remove", "params": {"id": "model.1"}})
        cc.close()

    def test_wcs_commands_send_vectors_and_choices(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.setup_wcs_create(origin="box", origin_box="top_center", orientation="z_x",
                            orientation_z="model.1/face.3", orientation_x="model.1/edge.7", flip_x=True)
        cc.setup_wcs_offset(delta=[1, 2, 3], frame="world")
        sent = [r["params"] for r in app.requests]
        self.assertEqual(
            sent[0],
            {
                "command": "setup.wcs.create",
                "params": {
                    "orientation": "z_x", "orientation_z": "model.1/face.3", "orientation_x": "model.1/edge.7",
                    "flip_x": True, "origin": "box", "origin_box": "top_center",
                },
            },
        )
        self.assertEqual(
            sent[1], {"command": "setup.wcs.offset", "params": {"delta": [1, 2, 3], "frame": "world"}}
        )
        cc.close()

    def test_tool_and_machine_commands(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.machine_create(name="Router", max_spindle_rpm=30000)
        cc.tool_create_flat(diameter=6, flutes=3, spindle_rpm=12000, feed=1200)
        cc.tool_edit(number=1, new_number=4)
        sent = [r["params"] for r in app.requests]
        self.assertEqual(sent[0], {"command": "machine.create", "params": {"name": "Router", "max_spindle_rpm": 30000}})
        self.assertEqual(
            sent[1],
            {"command": "tool.create.flat", "params": {"diameter": 6, "flutes": 3, "spindle_rpm": 12000, "feed": 1200}},
        )
        self.assertEqual(sent[2], {"command": "tool.edit", "params": {"number": 1, "new_number": 4}})
        cc.close()

    def test_contour_sends_geometry_lists(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.toolpath_contour(geometry=["model.1/face.2", "model.1/edge.4"], side="inside", multiple_depths=True)
        self.assertEqual(
            app.requests[0]["params"],
            {
                "command": "toolpath.contour",
                "params": {"geometry": ["model.1/face.2", "model.1/edge.4"], "side": "inside", "multiple_depths": True},
            },
        )
        cc.close()

    def test_gcode_export(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.post_gcode_export(path="part.nc", units="mm", line_numbers=True)
        self.assertEqual(
            app.requests[0]["params"],
            {"command": "post.gcode.export", "params": {"path": "part.nc", "units": "mm", "line_numbers": True}},
        )
        cc.close()

    def test_job_files_and_tabs(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.job_save(path="part.cosmic")
        cc.job_tab_close(index=1, discard=True)
        self.assertEqual(
            [r["params"] for r in app.requests],
            [
                {"command": "job.save", "params": {"path": "part.cosmic"}},
                {"command": "job.tab.close", "params": {"index": 1, "discard": True}},
            ],
        )
        cc.close()

    def test_setup_create(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.setup_create(name="Op 1", wcs="origin", stock="size", stock_size=[100, 60, 40])
        self.assertEqual(
            app.requests[0]["params"],
            {
                "command": "setup.create",
                "params": {"name": "Op 1", "wcs": "origin", "stock": "size", "stock_size": [100, 60, 40]},
            },
        )
        cc.close()

    def test_type_text(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.type_text("0.25", replace=True)
        self.assertEqual(app.requests[0]["method"], "ui.type")
        self.assertEqual(app.requests[0]["params"], {"text": "0.25", "replace": True})
        cc.close()

    def test_strategies(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.toolpath_pocket(geometry=["model.1/face.8"], stepover=2.5)
        cc.toolpath_drill(geometry=["model.1/edge.3"], cycle="peck")
        self.assertEqual(
            [r["params"]["command"] for r in app.requests],
            ["toolpath.pocket", "toolpath.drill"],
        )
        cc.close()

    def test_simulation(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.sim_seek(fraction=0.5)
        cc.sim_gouge_check(tolerance=0.02)
        self.assertEqual(
            [r["params"] for r in app.requests],
            [
                {"command": "sim.seek", "params": {"fraction": 0.5}},
                {"command": "sim.gouge.check", "params": {"tolerance": 0.02}},
            ],
        )
        cc.close()

    def test_settings_units(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.settings_units_set(units="mm")
        self.assertEqual(app.requests[0]["params"], {"command": "settings.units.set", "params": {"units": "mm"}})
        cc.close()

    def test_click_sends_ui_click(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.click(120, 340.5)
        self.assertEqual(app.requests[0]["method"], "ui.click")
        self.assertEqual(app.requests[0]["params"], {"x": 120, "y": 340.5})
        cc.close()

    def test_right_click(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.click(10, 20, button="right")
        self.assertEqual(app.requests[0]["params"], {"x": 10, "y": 20, "button": "right"})
        cc.close()

    def test_start_sends_ui_start(self):
        app = FakeApp()
        cc = CosmicCAM(app.port, TOKEN, timeout=5)
        cc.start("model.import")
        self.assertEqual(app.requests[0]["method"], "ui.start")
        self.assertEqual(app.requests[0]["params"], {"command": "model.import"})
        cc.close()


class GeneratedApiTests(unittest.TestCase):
    def test_every_method_is_keyword_only_and_documented(self):
        methods = [m for name, m in inspect.getmembers(Commands, inspect.isfunction) if name != "execute"]
        self.assertGreater(len(methods), 50)
        for m in methods:
            self.assertIn("Command: ``", m.__doc__ or "", m.__name__)
            for p in list(inspect.signature(m).parameters.values())[1:]:
                self.assertEqual(p.kind, inspect.Parameter.KEYWORD_ONLY, f"{m.__name__}.{p.name}")

    def test_model_import_signature(self):
        sig = inspect.signature(Commands.model_import)
        self.assertEqual(list(sig.parameters)[1:], ["path", "units", "name"])
        self.assertIs(sig.parameters["path"].default, inspect.Parameter.empty)

    def test_public_names(self):
        for name in ("launch", "connect", "CosmicCAM", "CosmicCAMError"):
            self.assertTrue(hasattr(cosmiccam, name))


if __name__ == "__main__":
    unittest.main()
