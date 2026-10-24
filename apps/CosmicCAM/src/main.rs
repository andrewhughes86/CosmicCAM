//! CosmicCAM desktop application.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

use std::path::PathBuf;
use std::process::ExitCode;

use CosmicCAM_ui_egui::{Automation, LaunchOptions, Session, parse_console_line};

const USAGE: &str =
    "usage: CosmicCAM [--sample] [--run \"<command> [json]\"]... [--tab <name>] [--screenshot <file.png>] [--control <port>]

  --sample              open with the built-in sample part
  --run <line>          run a command after start-up, e.g. --run 'view.theme.set {\"theme\":\"light\"}'
  --tab <name>          open on this ribbon tab (Home, Setup, Tooling, ...)
  --screenshot <file>   save a PNG of the window once the steps have run, then quit
  --control <port>      open the control channel for the Python API on 127.0.0.1:<port> (0 = any free port);
                        the token is read from CosmicCAM_CONTROL_TOKEN or written to
                        <data dir>/CosmicCAM/control/<port>.token";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut sample = false;
    let mut steps = Vec::new();
    let mut screenshot = None;
    let mut tab = None;
    let mut control_port = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sample" => sample = true,
            "--run" => match args.next().map(|line| parse_console_line(&line)) {
                Some(Ok(Some(step))) => steps.push(step),
                Some(Ok(None)) => {}
                Some(Err(e)) => return usage_error(&format!("--run: {e}")),
                None => return usage_error("--run needs a command"),
            },
            "--tab" => match args.next() {
                Some(name) => tab = Some(name),
                None => return usage_error("--tab needs a ribbon tab name"),
            },
            "--control" => match args.next().map(|p| p.parse::<u16>()) {
                Some(Ok(port)) => control_port = Some(port),
                _ => return usage_error("--control needs a port number (0-65535)"),
            },
            "--screenshot" => match args.next() {
                Some(path) => screenshot = Some(PathBuf::from(path)),
                None => return usage_error("--screenshot needs a file path"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return usage_error(&format!("unknown argument: {other}")),
        }
    }
    let session = if sample { Session::with_sample() } else { Session::new() };
    match CosmicCAM_ui_egui::run(
        session,
        LaunchOptions { automation: automation(steps, screenshot, tab), control_port },
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("CosmicCAM could not start: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error(message: &str) -> ExitCode {
    eprintln!("{message}\n\n{USAGE}");
    ExitCode::from(2)
}

fn automation(steps: Vec<CosmicCAM_ui_egui::Request>, screenshot: Option<PathBuf>, tab: Option<String>) -> Automation {
    let mut automation = Automation::new(steps, screenshot);
    automation.ribbon_tab = tab;
    automation
}
