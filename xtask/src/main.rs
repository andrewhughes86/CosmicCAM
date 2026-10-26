//! Repository checks: `cargo xtask <ci|layers|ui-containers|assets|wasm>`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod api;
mod demo;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::Value;

const USAGE: &str = "usage: cargo xtask <command>

  ci              fmt, clippy, tests, assets, layers, ui-containers and wasm
  layers          check crate layering (L0 geom -> ... -> L5 ui-egui -> apps)
  ui-containers   check that only ui_kit builds outer egui containers
  assets          check that every asset file has a row in ATTRIBUTION.md
  wasm            build everything below the apps for wasm32-unknown-unknown
  api [--check]   regenerate the Python API and its guide from the command registry
  python          run the Python API tests
  demo            rebuild the demo part and project (samples/) and the sample tool library file";

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "ci" => ci(),
        "layers" => layers(),
        "ui-containers" => ui_containers(),
        "assets" => assets(),
        "wasm" => wasm(),
        "api" => api::run(std::env::args().any(|a| a == "--check")),
        "demo" => demo::run_demo(),
        "python" => python_tests(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask {task}: {e}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

fn cargo(args: &[&str]) -> Result<(), String> {
    println!("==> cargo {}", args.join(" "));
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(args)
        .current_dir(root())
        .status()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("`cargo {}` failed", args.join(" "))) }
}

fn ci() -> Result<(), String> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])?;
    cargo(&["test", "--workspace"])?;
    assets()?;
    layers()?;
    ui_containers()?;
    api::run(true)?;
    python_tests()?;
    wasm()?;
    println!("==> all checks passed");
    Ok(())
}

/// Workspace crate -> layer. Every workspace crate must be listed.
const LAYERS: &[(&str, u8)] = &[
    ("CosmicCAM-geom", 0),
    ("CosmicCAM-stock", 1),
    ("CosmicCAM-kernel", 1),
    ("CosmicCAM-render", 1),
    ("CosmicCAM-job", 2),
    ("CosmicCAM-io", 3),
    ("CosmicCAM-engine", 4),
    ("CosmicCAM-ui-egui", 5),
    ("CosmicCAM", 6),
    ("CosmicCAM-cli", 6),
    ("CosmicCAM-web", 6),
];

/// UI and windowing crates only L5 and the apps may use.
const UI_ONLY: &[&str] = &["egui", "eframe", "egui-wgpu", "egui_dock", "egui-winit", "winit", "wgpu", "rfd"];

fn layers() -> Result<(), String> {
    println!("==> layers");
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root())
        .output()
        .map_err(|e| format!("could not run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err("cargo metadata failed".into());
    }
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let layer_of = |name: &str| LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l);
    let mut problems = Vec::new();
    for pkg in meta["packages"].as_array().into_iter().flatten() {
        let name = pkg["name"].as_str().unwrap_or_default();
        if name == "xtask" {
            continue;
        }
        let Some(layer) = layer_of(name) else {
            problems.push(format!("{name} has no layer; add it to LAYERS in xtask"));
            continue;
        };
        for dep in pkg["dependencies"].as_array().into_iter().flatten() {
            if dep["kind"].as_str() == Some("dev") {
                continue;
            }
            let dep_name = dep["name"].as_str().unwrap_or_default();
            if let Some(dep_layer) = layer_of(dep_name) {
                if dep_layer >= layer {
                    problems.push(format!("{name} (L{layer}) depends on {dep_name} (L{dep_layer})"));
                }
            } else if layer < 5 && UI_ONLY.contains(&dep_name) {
                problems.push(format!("{name} (L{layer}) depends on UI crate {dep_name}"));
            }
        }
    }
    report(problems)
}

/// Outer containers that only `ui_kit` may construct.
const RAW_CONTAINERS: &[&str] = &[
    "egui::Frame",
    "egui::Window",
    "egui::Area",
    "egui::Panel",
    "egui::CentralPanel",
    "egui::TopBottomPanel",
    "egui::SidePanel",
    "egui::Modal",
    "Frame::new(",
    "Frame::none(",
    "Frame::default(",
    "Window::new(",
    "Area::new(",
];

fn ui_containers() -> Result<(), String> {
    println!("==> ui-containers");
    let mut problems = Vec::new();
    for dir in ["crates", "apps"] {
        for file in rust_files(&root().join(dir)) {
            if file.file_name().is_some_and(|n| n == "ui_kit.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                let imports_raw = code.trim_start().starts_with("use egui::{")
                    && code
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|ident| ["Frame", "Window", "Area", "Panel", "CentralPanel", "Modal"].contains(&ident));
                if imports_raw || RAW_CONTAINERS.iter().any(|p| contains_token(code, p)) {
                    problems.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
                }
            }
        }
    }
    report(problems)
}

fn assets() -> Result<(), String> {
    println!("==> assets");
    let attribution =
        std::fs::read_to_string(root().join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let mut problems = Vec::new();
    let mut stack = vec![root().join("assets")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
            if matches!(ext, "md" | "py" | "txt") {
                continue;
            }
            let rel = path.strip_prefix(root()).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if !attribution.contains(&rel) {
                problems.push(format!("{rel} has no row in ATTRIBUTION.md"));
            }
        }
    }
    report(problems)
}

fn wasm() -> Result<(), String> {
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false);
    if !installed {
        return Err("wasm32-unknown-unknown is not installed: rustup target add wasm32-unknown-unknown".into());
    }
    let mut args = vec!["check", "--target", "wasm32-unknown-unknown"];
    for (name, layer) in LAYERS {
        if *layer < 5 && root().join(crate_dir(name)).exists() {
            args.extend(["-p", name]);
        }
    }
    cargo(&args)?;
    // The browser app (and the UI crate under it), linted as it is built for the web.
    cargo(&["clippy", "--target", "wasm32-unknown-unknown", "-p", "CosmicCAM-web", "--", "-D", "warnings"])
}

/// Runs the Python client's unit tests with the standard library only.
fn python_tests() -> Result<(), String> {
    println!("==> python");
    let python = ["python3", "python"]
        .into_iter()
        .find(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
        .ok_or("python 3 was not found; it is needed for the Python API tests")?;
    let status = Command::new(python)
        .args(["-I", "-m", "unittest", "discover", "-s", "python/tests", "-t", "python"])
        .current_dir(root())
        .status()
        .map_err(|e| format!("could not run {python}: {e}"))?;
    if status.success() { Ok(()) } else { Err("Python API tests failed".into()) }
}

fn crate_dir(package: &str) -> String {
    format!("crates/{}", package.trim_start_matches("CosmicCAM-"))
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("target")) {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out
}

/// True if `pattern` occurs in `code` not glued to a preceding identifier character.
fn contains_token(code: &str, pattern: &str) -> bool {
    code.match_indices(pattern)
        .any(|(i, _)| !code[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'))
}

fn report(problems: Vec<String>) -> Result<(), String> {
    if problems.is_empty() {
        return Ok(());
    }
    for p in &problems {
        eprintln!("  {p}");
    }
    Err(format!("{} problem(s)", problems.len()))
}
