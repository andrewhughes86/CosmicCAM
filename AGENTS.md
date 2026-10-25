# CosmicCAM — instructions for agents

CosmicCAM is a clean-room, open-source, pure-Rust Computer-Aided Manufacturing (CAM) engine and desktop application. 

The application transforms CAD geometry (B-Rep models, meshes, 2D boundaries) into physical CNC machining toolpaths, material removal simulations, and machine-specific G-Code.

Any code adapted from other projects (MIT OR Apache-2.0) is noted in ATTRIBUTION.md.

## Product name
CosmicCAM everywhere: crates `CosmicCAM-*`, binaries `CosmicCAM` and `CosmicCAM-cli`,
`.cosmic` job files, the `CosmicCAM/N` format tag, `CosmicCAM_*` environment variables,
settings and recovery folders, the MCP server name, docs, CI and packaging. 

## Start every session here
1. Read `plan/STATUS.md` (current milestone, next task), then the task in `plan/execution-plan.md`

`plan/` is gitignored (local only).

## Never crash
People trust CosmicCAM with their machining setups and physical machine code; a crash loses their work or risks machine collisions. **This outranks feature work.**
- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`,
  `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`). The exceptions, in the app crate and in
  `crates/threedx-client` (which deny it instead), have no safe API: the macOS-only
  `crates/threedx-client/src/macos.rs`, two functions in
  `apps/CosmicCAM/src/open_documents.rs`, the Windows-only `apps/CosmicCAM/src/native_crash.rs`,
  the Windows-only `apps/CosmicCAM/src/native_frame.rs` and the Windows-only
  `apps/CosmicCAM/src/space_mouse/windows_hid.rs`.
- **Errors are `Result<T, E>`** through the crate's error type and `?`. An unfinished feature
  returns an error ("not supported yet: …"); it never panics.
- **Input-derived numbers are hostile** (command parameters, job files, control-channel
  input): `get()` not `[i]`, finite checks, caps on sizes and loop counts.
- **Third-party geometry calls are guarded:** everything that interacts with external B-Rep/mesh geometry libraries goes through safe wrapper guards that translate panics into `EngineError::Internal`.
- **Last-resort guard:** `Session::execute` runs every command under `catch_unwind`; an escaped
  panic restores the session and reports `EngineError::Internal`.
- **Crash reports:** the desktop app writes every panic and every unclean exit to a local crash folder,
  with the job and the commands that reproduce it (`CosmicCAM_engine::crash`, docs/crash-reports.md).
  When someone says the app crashed, look there first: `CosmicCAM-cli crashes`, `crashes show <id>`,
  `crashes replay <id>` (or the `diagnostics.crash_reports` command / MCP `crash_reports` tool).
- **Prove it:** every crash fix lands with a regression test (see `hostile_params_never_panic`); a
  crash report's job file + `replay.json` is the test's starting point.
- **Where tests go:** a regression test goes into the existing test file for its area (adaptive clearing, pockets, waterline, stock sim, post-processor, collision check…), never a new file named after the bug (#3226).
- Every production crate root carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`.

## Non-negotiables
- **Independent.** CosmicCAM is independent. Behaviour is designed from public documentation,
  general CAM conventions and user feedback. Reference tests use analytic values and open-source
  CAM tools. Never copy another product's code, icons, artwork, help text, tooltips, templates or
  materials. File formats come from public specifications.
- **No commercial CAM products.** Agents must not drive, script, automate or extract data from
  commercial CAM products (Powermill, Fusion, MasterCAM, …) for this project: no test data,
  measurements, command lists, ids or files produced with them.
- **Never read, copy or link GPL/LGPL/AGPL code**
- **Assets — absolutely essential.** CosmicCAM contains **no iconography, images, fonts or
  artwork from commercial CAM products — ever.** Command icons are the original SVG masters in
  `assets/icons/ui/design/svg/`, compiled through `crates/ui-egui/src/icon_svg.rs`.
  **Before any icon, icon assignment or media edit, read `assets/AGENTS.md`,
  `assets/icons/ui/AGENTS.md` and `assets/icons/ui/STYLE_GUIDE.md`.** Use the canonical
  bases and named finished references there; never invent a parallel icon style. Any file asset must be original or openly licensed and **must have a row in ATTRIBUTION.md**
  (`cargo xtask assets` enforces it). Screenshots of commercial CAM software are never committed.
  Breaking this rule is the most serious mistake you can make in this repo.
- **Everything is a command.** User-visible behaviour = a command in `crates/engine/src/cmd/*`
  (`CommandSpec`: id = our own dotted, lower-case id `<area>.<thing>[.<variant>]` such as
  `toolpath.adaptive` or `setup.wcs.create`, never another product's command id; label;
  toolbar tab and panel; icon; shortcut; params doc; `enabled`; JSON `run`) + tests. The toolbar, dialogs, tools, palette, scripts, CLI and control channel all reach the same commands.
- **Programmatic calls never open dialogs.** `engine.execute` runs the JSON form; only toolbar
  clicks (`ui.start`) start interactive tools and dialogs, which then run commands.
- **Layering** is enforced by `cargo xtask layers`: L0 `geom` → L1 `stock`, `kernel`, `render`
  → L2 `job` → L3 `io` → L4 `engine` → L5 `ui-egui` → apps. Nothing below L5 depends on
  egui/eframe/winit/wgpu/rfd. **The UI crate is swappable.**
- **The UI is thin**: panels read engine state and act through `app.run(id, params)` /
  `app.start(id)`. Colours come from `theme::Tokens`.
- **One outer UI container**: all panels, floating sheets, menus, overlays and platform UI
  windows use `ui_kit::Container` (directly or through `Panel`, `Window`, `Confirm`, `Menu`).
  Declare the semantic `Surface`; workspace panels use its shared layout policy and chrome.
  System/report windows use `ui_kit::Window`; its low-level host builder is kit-private.
  Do not construct raw egui hosts, outer popup/window frames or shadows in feature modules.
  `cargo xtask ui-containers` recursively checks source/examples in all current and future crates
  and apps. It runs in the fast repository CI job and `cargo xtask ci`, with no caller allowances.
  See `docs/ui-container-audit.md` for the inventory and legitimate content/native exceptions.
- **One set of camera controls**: every page or canvas with a camera (viewport, simulation
  preview, toolpath inspection) calls `navigation::handle` with its `NavCaps`.
  New canvases must use `navigation::handle`; never hand-roll camera input (a test rejects it).
- **Application code is Rust, with two Python exceptions.** Units are millimetres and seconds
  internally; Z is up. Everything else in the app (engine, geometry, toolpaths, simulation, UI) is Rust.
  - **Post-processors and machine configuration are Python** (decided 2026-10-24, Phase 8).
    Each machine's dialect, limits, tool-change, coolant, safe positions and program
    header/footer is a user-editable Python module, and the post logic that turns CL data
    into G-code is Python. Heavy computation (arc fitting, limit and kinematic checks over
    long toolpaths, bulk formatting) is Rust, exposed to the Python post as helpers; Python stays
    the single place that decides what the machine receives. Python 3 is therefore a runtime
    dependency for posting only: the rest of the app must work without it, and a missing
    interpreter or a faulty post is reported as an error, never a crash. Posts run out of
    process: `crate::postproc` starts `python3 -I posts/run_post.py` and feeds it CL-data JSON
    (docs/posts.md); the shipped posts live in `posts/`.
  - **The Python API client** in `python/cosmiccam` is a separate, stdlib-only package that talks
    to the app over the control channel; the app never depends on it.
  - Local asset authoring, lint and review tools use Python 3.11+ as documented in
    `assets/AGENTS.md`.
  - The only other non-Rust application code is JavaScript the browser requires: the service
    worker `apps/CosmicCAM-web/sw.js` and small inline scripts in `apps/CosmicCAM-web/index.html`.
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests,
  assets, layers, ui-containers, `api --check`, Python tests, wasm). Commit after every feature arc that builds.
- **Publishing**: the repository is github.com/tangency-labs/CosmicCAM. Changes land through
  **pull requests** and the **merge queue**: push a branch and open a PR. The heavy CI jobs start
  when a maintainer adds `approved-for-ci` after reviewing the code. Approved PRs go into the queue
  with `gh pr merge N -R tangency-labs/CosmicCAM --squash --auto`; the queue runs the full suite
  on each group before it lands on `main`. Keep each PR to one topic and one issue, and never
  force-push `main`. Never commit `plan/`, build dirs (`target*/`) or generated reference data;
  check `git ls-files` before pushing.
- **Never break wasm**: everything below the apps must build for `wasm32-unknown-unknown`
  (`cargo xtask wasm`, part of `cargo xtask ci`).

## Showing and following through on changes

- Help reviewers see what is broken or changed. For observable CAM UI or toolpath behavior,
  issues and PRs must include actual descriptive screenshots or a short recording from a
  small reproduction. This includes simulation, toolpath generation, and post-processor changes whose
  effects can be shown in the app. Coding agents should run the app and capture this evidence
  themselves; do not ask a human for evidence the agent can produce. Show the actual failure
  or result, preferably before and after, with brief reproduction steps, expected/actual
  behavior and annotations that point out the relevant detail.
- Flowcharts and Mermaid diagrams cannot substitute for screenshots or recordings of visible
  behavior. Use diagrams for architectural reasoning when actual screenshots cannot illustrate
  the point. Changes with no useful observable behavior do not need screenshot or video evidence.
- Use a small public-safe example and check for private designs, paths, account details or
  other sensitive content before sharing. Attach media to the issue/PR or link to an accessible
  recording; do not commit generated screenshots or videos just to provide review evidence.
- Throughout the work and before handoff, check the relevant PR's merge status. Don't push to
  an open PR only to catch up with `main` (no merge of `main`, no Update branch, no rebase for
  that alone): every push restarts the whole CI run, and `main`'s own run catches what lands in
  between, to be fixed forward. When you push a change anyway, or GitHub reports a conflict,
  rebase onto the latest `main` (push your own branch with `--force-with-lease`); never merge
  `main` into a PR. Resolve conflicts without discarding others' work; never force-push `main`.
  Don't re-run CI on an unchanged commit: a real failure fails again. Re-run only a job that
  runner trouble cancelled, with "Re-run failed jobs". Report the PR state you checked and any
  conflicts resolved. If a PR remains open, keep monitoring it while the task is active and
  make its pending status clear at handoff; do not claim it merged without checking.

## Before you start: look at what is already open
- Work with an authenticated GitHub CLI (`gh auth status`) and `git`; the checks below need both.
- Pull the whole list, not one search: `gh pr list -R tangency-labs/CosmicCAM --state open --limit 500`
  (drafts included) and `gh issue list -R tangency-labs/CosmicCAM --state open --limit 500`. Look for
  work on the same issue, the same feature under another number, and PRs that change the files
  you are about to change (`gh pr list ... --json number,title,files`).
- If someone already has it, comment on their PR or issue instead of opening a second one. Two
  PRs for one change can't both land, and one of them is wasted work and CI.

## Claim = a draft PR
- Before working on issue #N, check nobody has it: `gh pr list -R tangency-labs/CosmicCAM --state open --search "#N"`.
- If none, branch from the latest `origin/main`, `git commit --allow-empty -m "claim: #N"`, push,
  and open a **draft** PR titled `<summary> (#N)` right away. **The lowest-numbered open PR for an
  issue wins**; if yours lost, close it. Opening the PR is the claim: the bot assigns you and
  labels the issue `in progress` (or says on your PR who holds it). Reference `#N` in the PR title
  so the bot can find it. Don't claim by editing files; comment `/claim` on the issue only to stake
  a claim before you have a branch.
- Push work in progress at least daily. A draft with no pushes for 72 hours may be taken over:
  comment on it first, then open your own claim (you may start from its branch).
- On a shared build machine, check the load before a big build (`cut -d' ' -f1 /proc/loadavg`)
  and wait while it is over 24; build with `nice`.

## Python API — every feature ships with it
CosmicCAM is fully scriptable from Python. A feature is not done until it is in the API and its guide.
- **Commands are the API.** Every feature is a command with a typed parameter schema
  (`params: &[Param]`, checked by the engine; unknown or mistyped parameters are rejected) and a
  `returns` description. UI-only state (panes, ribbon tab, window, display toggles) gets commands
  too, and new state a script would want to read gets a `*.get` command.
- **Regenerate** with `cargo xtask api`: it writes `python/cosmiccam/_commands.py` (one typed method
  per command) and `docs/api/python.md` (the API guide) from the registry and the hand-written
  `docs/api/_intro.md`. Never edit the generated files; add workflow sections and examples to
  `_intro.md`. CI fails when they are stale.
- **Test from Python**: extend `python/tests/` (stdlib `unittest`, run by `cargo xtask python`) and,
  for visible features, `python/examples/`. `cosmiccam.launch()` starts the app with the control
  channel open; `cc.screenshot(path)` captures evidence.
- **Control channel**: `CosmicCAM --control PORT`, token-authenticated JSON lines on 127.0.0.1
  (`docs/control-protocol.md`). New protocol methods are documented there and wrapped in
  `python/cosmiccam/client.py`.

## Job file format
A change that saves something new in a job file adds ONE file, `crates/doc/src/format/pending/<name>.rs`
(a `//!` note, and `pub fn step` if older job files must be rewritten). Never edit `FORMAT`,
`JSON_STEPS`, the history in `format.rs`, another change's file or a `v<N>/` folder: the build
works out the number (the next one while anything is pending), and the release folds pending
changes into `v<N>/` with `scripts/fold-format-pending.sh`. See CONTRIBUTING.md.

## Application releases
Development pushes are not application releases. Follow `docs/releasing.md` when asked to
prepare or ship a release: choose the version from the actual changes, update Cargo.toml and
Cargo.lock, pass the quality gates, advance `release` without force-pushing, and verify the
resulting draft and its artifacts. When shipping is authorized, publish that verified draft
and confirm its tag and downstream deployment. Do not auto-release every push to `main`, infer
version bumps solely from commit prefixes, overwrite published versions, or push release tags
before the draft is verified.

## Running and looking at the app
- `cargo run --release -p CosmicCAM -- --control PORT` (opens the demo project, `samples/demo.cosmic`,
  plus the control channel; `--empty` for a blank job, `--sample` for the simple sample part).
  Pick a free port. Without a display, run it under `Xvfb :99` with `DISPLAY=:99`.
- Drive it with JSON lines on `127.0.0.1:PORT` (see `docs/control-protocol.md`). The first
  request on a connection carries the launch's token as `"token"`: read it from
  `~/.local/share/CosmicCAM/control/PORT.token`, or start the app with
  `CosmicCAM_CONTROL_TOKEN=$(openssl rand -hex 32)` and send that.
- `{"id":1,"method":"engine.execute","params":{"command":"toolpath.adaptive","params":{"stepover":2.5}}}`
- `{"id":2,"method":"ui.start","params":{"command":"setup.wcs.create"}}` then `ui.click {x, y}`
- `{"id":3,"method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}` then read the PNG.
- **For UI work, look at the result** (screenshot, read the PNG). Without a window use
  `ui.render` or `CosmicCAM-cli snapshot`.
- Headless: `CosmicCAM-cli run script.json --out output.nc`, `CosmicCAM-cli eval job.CosmicCAM`,
  `CosmicCAM-cli snapshot script.json --out shot.png`, `CosmicCAM-cli commands`.
- Reference tests: `cargo xtask oracle` replays `tests/oracle/*/recipe.json` and compares with
  each case's `expected.json` (analytic or open-source CAM values; writes `docs/oracle.md`). Expected
  values never come from a commercial CAM product.
- Build with `CARGO_TARGET_DIR=<repo>/target`. Windows: `cargo xwin build --release --target x86_64-pc-windows-msvc -p CosmicCAM`.

## Roadmap
`ROADMAP.md` (committed) tracks status, milestones and estimates. Update it whenever a
milestone task lands. `docs/features.md` (CosmicCAM's commands by area, from the registry) and
`THIRD-PARTY-LICENSES.txt` are regenerated on `main` after each merge by the Generated files
workflow; never regenerate them in a PR.

## Lists many PRs add to
Keep them conflict-free (CONTRIBUTING.md, "Lists many changes add to"): message catalogs sorted
(`cargo xtask lists --fix`), `mod` lines in their sorted group, the language list English first
then by id, generated files left to `main`, and a job-format change as one new file.