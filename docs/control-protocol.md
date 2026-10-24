# Control protocol

The control channel lets scripts, the Python API (`python/cosmiccam`) and agents drive a
running CosmicCAM. It is JSON lines over TCP on `127.0.0.1` only.

## Opening it

```bash
CosmicCAM --sample --control 5800     # 0 picks a free port
```

On start the app prints `CosmicCAM control channel listening on 127.0.0.1:PORT` to stdout.

**Token.** The first request on every connection must carry `"token"`. The token is
`$CosmicCAM_CONTROL_TOKEN` when set (at least 16 characters); otherwise the app generates a
random 256-bit token and writes it, readable by the user only, to
`<data dir>/CosmicCAM/control/PORT.token` (`~/.local/share` on Linux, `~/Library/Application Support`
on macOS, `%APPDATA%` on Windows). The file is removed when the app exits. A wrong or missing
token gets an `unauthorized` error and the connection is closed.

## Messages

One JSON object per line in each direction. Requests are answered in order.

```json
{"id": 1, "method": "engine.execute", "params": {"command": "view.camera.preset", "params": {"preset": "top"}}, "token": "…"}
{"id": 1, "result": {"preset": "top"}}
{"id": 2, "error": {"kind": "invalid_params", "message": "invalid parameters: `preset` must be one of: …"}}
```

`id` is echoed back unchanged. Lines are limited to 1 MiB and connections to 16 at a time.

## Methods

| Method | Params | Result |
| --- | --- | --- |
| `app.ping` | — | `{name, version, protocol}` (protocol is `1`) |
| `engine.execute` | `{command, params?}` | The command's result; see [api/python.md](api/python.md) for every command |
| `engine.commands` | — | Every command: `{id, label, tooltip, shortcut, enabled, params, returns, ribbon}` |
| `engine.state` | — | Same as the `session.state.get` command |
| `ui.screenshot` | `{path}` (ends in `.png`) | `{path, width, height}` once the next settled frame is saved; the window must be visible |
| `ui.start` | `{command}` | `{started}`: starts a command's interactive tool as a toolbar click would. `model.import` opens the file dialog and imports the chosen file. Other commands: `not_supported`; a dialog already open: `busy` |

Programmatic calls never open dialogs: `engine.execute` runs the command directly. Window and
pane commands (`ui.*`, `app.quit`) are carried out on the app's next frame. Requests are also
answered while the window is minimised.

## Error kinds

`unauthorized`, `invalid_request` (not JSON, no method), `unknown_method`, `unknown_command`,
`invalid_params`, `disabled`, `not_supported`, `import` (a model file could not be read or parsed), `busy` (`ui.start` while a dialog is open), `internal` (the command failed and the session was
restored), `io` (screenshot could not be written), `timeout` (no answer within 60 s),
`unavailable` (too many connections or shutting down).

## Parameter schema

`engine.commands` describes each parameter as `{name, type, required, doc}` plus `min`/`max` for
`number` and `integer`, and `choices` for `choice`. Types: `string`, `bool`, `number`,
`integer`, `choice`, `vec3` (`[x, y, z]` in mm). Unknown parameter names are rejected.
