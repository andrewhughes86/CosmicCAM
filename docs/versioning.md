# Versioning

CosmicCAM follows [Semantic Versioning](https://semver.org/): `MAJOR.MINOR.PATCH`.

## While the version starts with 0

CosmicCAM is pre-production: it can plan toolpaths and write generic G-code, but there are no
machine-specific post-processors, simulation or collision checks yet. **Do not run its output on a
machine without checking every line.** Until that changes the major version stays 0:

- **MINOR** (`0.2.0`, `0.3.0`, …) goes up when a milestone lands: new strategies, simulation,
  post-processors, a new job-file format. Scripts and job files may need updating between minor
  versions; the changelog says how.
- **PATCH** (`0.1.1`, …) is for fixes and small improvements that change nothing a script or job
  file depends on.

## 1.0.0

1.0.0 is released when CosmicCAM produces machine-ready G-code: Python post-processors and
machine configurations (Phase 8), material-removal simulation and gouge/collision checks (Phase 7).
From then on a major version change is required for anything that breaks scripts, the control
protocol or job files.

## What carries its own version

| What | Where | Changes when |
| --- | --- | --- |
| The application and every crate | `[workspace.package] version` in `Cargo.toml` | each release |
| Job files (`.cosmic`) | the `format` tag, `CosmicCAM/1` | the saved data changes shape |
| The control protocol | `PROTOCOL_VERSION`, reported by `app.ping` | requests or replies change |

## Making a release

1. Move the changelog's *Unreleased* entries under the new version and date.
2. Set `[workspace.package] version` in `Cargo.toml` and run `cargo update -w` so `Cargo.lock`
   follows.
3. Pass `cargo xtask ci`, commit, and tag the commit `v<version>`.
