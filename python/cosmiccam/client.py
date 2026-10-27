"""Transport and session for the CosmicCAM control channel (docs/control-protocol.md)."""

from __future__ import annotations

import json
import os
import re
import secrets
import shutil
import socket
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

from ._commands import Commands

TOKEN_ENV = "CosmicCAM_CONTROL_TOKEN"
EXECUTABLE_ENV = "COSMICCAM_EXECUTABLE"
_LISTENING = re.compile(r"control channel listening on 127\.0\.0\.1:(\d+)")


class CosmicCAMError(Exception):
    """A request failed. ``kind`` is a stable machine-readable reason, e.g. ``invalid_params``,
    ``unknown_command``, ``disabled``, ``not_supported``, ``unauthorized``, ``timeout``."""

    def __init__(self, kind: str, message: str):
        super().__init__(f"{kind}: {message}")
        self.kind = kind
        self.message = message


def data_dir() -> Path:
    """CosmicCAM's per-user data folder (where control tokens are written)."""
    if sys.platform == "win32":
        return Path(os.environ.get("APPDATA", Path.home())) / "CosmicCAM"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "CosmicCAM"
    base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base) / "CosmicCAM"


class CosmicCAM(Commands):
    """A connection to a running CosmicCAM. Thread-safe; one request at a time.

    Every command is available as a method named after its id (``view.camera.set`` →
    :meth:`view_camera_set`), or generically through :meth:`execute` / :meth:`run`.
    """

    def __init__(
        self,
        port: int,
        token: str,
        host: str = "127.0.0.1",
        timeout: float = 60.0,
        process: Optional[subprocess.Popen] = None,
    ):
        self.port = port
        self.process = process
        self._lock = threading.Lock()
        self._next_id = 0
        self._token: Optional[str] = token
        self._sock = socket.create_connection((host, port), timeout=timeout)
        self._file = self._sock.makefile("rwb")

    # -- transport -------------------------------------------------------------------

    def request(self, method: str, params: Optional[Dict[str, Any]] = None) -> Any:
        """Sends one protocol request and returns its ``result``; raises :class:`CosmicCAMError`."""
        with self._lock:
            self._next_id += 1
            message: Dict[str, Any] = {"id": self._next_id, "method": method}
            if params is not None:
                message["params"] = params
            if self._token is not None:
                message["token"] = self._token
            self._file.write(json.dumps(message).encode() + b"\n")
            self._file.flush()
            line = self._file.readline()
            if not line:
                raise CosmicCAMError("disconnected", "CosmicCAM closed the connection")
            self._token = None  # only the first request needs it
            reply = json.loads(line)
        if "error" in reply:
            err = reply["error"] or {}
            raise CosmicCAMError(err.get("kind", "error"), err.get("message", ""))
        return reply.get("result")

    # -- protocol methods ------------------------------------------------------------

    def execute(self, command: str, params: Optional[Dict[str, Any]] = None) -> Any:
        """Runs a command by id with a dict of parameters and returns its result."""
        return self.request("engine.execute", {"command": command, "params": params or {}})

    def run(self, command: str, **params: Any) -> Any:
        """``cc.run("view.camera.preset", preset="top")`` — :meth:`execute` with keywords."""
        return self.execute(command, params)

    def ping(self) -> Dict[str, Any]:
        """``{"name", "version", "protocol"}`` of the connected app."""
        return self.request("app.ping")

    def commands(self) -> List[Dict[str, Any]]:
        """Every command with its parameters, return description, ribbon place and whether it can run now."""
        return self.request("engine.commands")

    def state(self) -> Dict[str, Any]:
        """Job, camera, display, theme, selection, ribbon tab, open panes and job outline."""
        return self.request("engine.state")

    def screenshot(self, path: "os.PathLike[str] | str", settle: bool = True) -> Dict[str, Any]:
        """Saves the whole window as PNG after the next frame. Returns ``{"path", "width", "height"}``.
        The window must be visible. ``settle=False`` captures without waiting for camera moves to
        finish, for recordings."""
        params: Dict[str, Any] = {"path": str(Path(path).resolve())}
        if not settle:
            params["settle"] = False
        return self.request("ui.screenshot", params)

    def click(self, x: float, y: float, button: str = "left", modifiers: Optional[List[str]] = None) -> Any:
        """Clicks a mouse button (``"left"``, ``"right"`` or ``"middle"``) at window point (x, y),
        as a user would, holding ``modifiers`` (``"ctrl"``, ``"shift"``, ``"alt"``, ``"command"``)."""
        params: Dict[str, Any] = {"x": x, "y": y}
        if button != "left":
            params["button"] = button
        if modifiers:
            params["modifiers"] = list(modifiers)
        return self.request("ui.click", params)

    def drag(self, x: float, y: float, to_x: float, to_y: float, modifiers: Optional[List[str]] = None) -> Any:
        """Presses the left button at (x, y), moves to (to_x, to_y) and lets go, as a user would."""
        params: Dict[str, Any] = {"x": x, "y": y, "to": {"x": to_x, "y": to_y}}
        if modifiers:
            params["modifiers"] = list(modifiers)
        return self.request("ui.click", params)

    def key(self, key: str, modifiers: Optional[List[str]] = None) -> Any:
        """Presses and releases a key by name (``"Delete"``, ``"Escape"``, ``"Enter"``…)."""
        params: Dict[str, Any] = {"key": key}
        if modifiers:
            params["modifiers"] = list(modifiers)
        return self.request("ui.key", params)

    def type_text(self, text: str, replace: bool = False) -> Any:
        """Types text into the focused field, as a user would; replace=True selects what it holds first."""
        return self.request("ui.type", {"text": text, "replace": replace})

    def start(self, command: str) -> Any:
        """Starts a command's interactive tool, as a toolbar click would."""
        return self.request("ui.start", {"command": command})

    # -- lifetime --------------------------------------------------------------------

    def close(self) -> None:
        """Closes the connection. An app started by :func:`launch` is asked to quit."""
        if self.process is not None and self.process.poll() is None:
            try:
                self.app_quit()
            except (CosmicCAMError, OSError):
                pass
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        try:
            self._file.close()
            self._sock.close()
        except OSError:
            pass

    def __enter__(self) -> "CosmicCAM":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()


def connect(port: int, token: Optional[str] = None, host: str = "127.0.0.1", timeout: float = 60.0) -> CosmicCAM:
    """Connects to a CosmicCAM started with ``--control PORT``.

    The token is taken from ``token``, else ``$CosmicCAM_CONTROL_TOKEN``, else the token file
    the app wrote to ``<data dir>/control/PORT.token``.
    """
    if token is None:
        token = os.environ.get(TOKEN_ENV)
    if token is None:
        path = data_dir() / "control" / f"{port}.token"
        try:
            token = path.read_text().strip()
        except OSError as e:
            raise CosmicCAMError("unauthorized", f"no token given and none readable at {path}: {e}") from e
    cc = CosmicCAM(port, token, host=host, timeout=timeout)
    cc.ping()  # authenticate now so a wrong token fails here
    return cc


def find_executable() -> str:
    """``$COSMICCAM_EXECUTABLE``, else ``CosmicCAM`` on PATH, else a release build in this checkout."""
    env = os.environ.get(EXECUTABLE_ENV)
    if env:
        return env
    on_path = shutil.which("CosmicCAM")
    if on_path:
        return on_path
    repo = Path(__file__).resolve().parents[2]
    for build in ("release", "debug"):
        candidate = repo / "target" / build / ("CosmicCAM.exe" if sys.platform == "win32" else "CosmicCAM")
        if candidate.exists():
            return str(candidate)
    raise CosmicCAMError(
        "not_found",
        f"CosmicCAM executable not found; set {EXECUTABLE_ENV} or build it with `cargo build --release -p CosmicCAM`",
    )


def launch(
    sample: bool = False,
    executable: Optional[str] = None,
    port: int = 0,
    args: Sequence[str] = (),
    timeout: float = 60.0,
    demo: bool = False,
) -> CosmicCAM:
    """Starts CosmicCAM with the control channel open and returns a connection to it.

    The app starts with an empty job, the built-in sample part (``sample=True``) or the demo
    project (``demo=True``). Closing the connection (or leaving a ``with`` block) quits the app.
    """
    token = secrets.token_hex(32)
    env = dict(os.environ, **{TOKEN_ENV: token})
    cmd = [executable or find_executable(), "--control", str(port)]
    if sample:
        cmd.append("--sample")
    elif not demo:
        cmd.append("--empty")
    cmd.extend(args)
    proc = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=None, text=True, bufsize=1)

    found: Dict[str, int] = {}
    ready = threading.Event()

    def pump() -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            m = _LISTENING.search(line)
            if m and not ready.is_set():
                found["port"] = int(m.group(1))
                ready.set()
        ready.set()

    threading.Thread(target=pump, name="cosmiccam-stdout", daemon=True).start()
    if not ready.wait(timeout) or "port" not in found:
        proc.kill()
        raise CosmicCAMError("timeout", "CosmicCAM did not open its control channel")
    cc = CosmicCAM(found["port"], token, timeout=timeout, process=proc)
    cc.ping()
    return cc
