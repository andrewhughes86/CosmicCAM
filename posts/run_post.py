"""Runs one CosmicCAM post-processor: CosmicCAM starts this with ``python -I`` and sends one
JSON request on standard input; the answer is one JSON object on standard output.

Requests:
    {"action": "post", "post": "<path to the post module>", "cl": {...CL data...}}
    {"action": "describe", "post": "<path>"}

Answers:
    {"gcode": "...", "warnings": [...], "extension": "nc", "name": "..."}
    {"name": "...", "description": "...", "extension": "nc", "properties": [...]}
    {"error": "..."}  when the post cannot run or reports a problem
"""

import importlib.util
import inspect
import json
import os
import sys
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import cosmiccam_post  # noqa: E402  (found next to this file)


def load_post(path):
    """The post class a module defines: ``POST`` if it names one, else its Post subclass."""
    if not os.path.isfile(path):
        raise cosmiccam_post.PostError("there is no post at {}".format(path))
    folder = os.path.dirname(os.path.abspath(path))
    if folder not in sys.path:
        sys.path.insert(1, folder)
    name = "cosmiccam_user_post_" + os.path.splitext(os.path.basename(path))[0]
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise cosmiccam_post.PostError("{} is not a Python module".format(path))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    chosen = getattr(module, "POST", None)
    if chosen is None:
        found = [
            c
            for _, c in inspect.getmembers(module, inspect.isclass)
            if issubclass(c, cosmiccam_post.Post) and c is not cosmiccam_post.Post and c.__module__ == module.__name__
        ]
        if not found:
            raise cosmiccam_post.PostError("{} defines no Post subclass".format(os.path.basename(path)))
        chosen = found[-1]
    return chosen


def answer(request):
    action = request.get("action", "post")
    cls = load_post(request.get("post", ""))
    if action == "describe":
        return {
            "name": cls.name,
            "description": cls.description,
            "extension": cls.extension,
            "properties": cls.describe_properties(),
        }
    post = cls()
    gcode = post.run(request.get("cl") or {})
    return {"gcode": gcode, "warnings": post.warnings, "extension": cls.extension, "name": cls.name}


def main():
    try:
        request = json.loads(sys.stdin.read() or "{}")
        result = answer(request)
    except cosmiccam_post.PostError as e:
        result = {"error": str(e)}
    except Exception:  # A faulty post is reported, with where it failed.
        lines = traceback.format_exc().strip().splitlines()
        result = {"error": "the post failed: " + " | ".join(lines[-6:])}
    sys.stdout.write(json.dumps(result))
    sys.stdout.flush()


if __name__ == "__main__":
    main()
