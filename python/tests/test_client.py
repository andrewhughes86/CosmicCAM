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
