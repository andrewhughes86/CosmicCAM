//! Posting: G-code from the machine's post-processor (a Python post, or the generic writer in
//! `crate::post`), choosing posts, checking limits and writing CL data.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::post::{MAX_PROGRAM_TOOLPATHS, Options, gcode};
use crate::postproc::{self, ClOptions, GENERIC};
use crate::session::Session;
use crate::units::DisplayUnits;

/// Longest program written.
const MAX_PROGRAM_BYTES: usize = 512 * 1024 * 1024;

macro_rules! program_params {
    ($($extra:expr),* $(,)?) => {
        &[
            $($extra,)*
            Param::optional("post", ParamKind::Str, "Post-processor id (default: the machine's, else generic); see post.processor.list"),
            Param::optional(
                "toolpaths",
                ParamKind::StrList { max: MAX_PROGRAM_TOOLPATHS },
                "Toolpath ids in order (default: all)",
            ),
            Param::optional(
                "units",
                ParamKind::Choice(DisplayUnits::IDS),
                "Program units, in or mm (default: the units setting; a post may fix its own)",
            ),
            Param::optional("program_number", ParamKind::Integer { min: 1, max: 99_999 }, "O number (default 1000)"),
            Param::optional("line_numbers", ParamKind::Bool, "Number the lines (default: the post's choice)"),
            Param::optional(
                "arc_tolerance",
                ParamKind::Length { min: 0.0, max: 1.0 },
                "Fit arcs (G2/G3) within this, mm; 0 writes lines only (default 0.01)",
            ),
        ]
    };
}

/// A posted program.
struct Program {
    text: String,
    post: String,
    warnings: Vec<String>,
    extension: String,
    toolpaths: usize,
}

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "post.processor.select",
        label: "Post-processor",
        tooltip: "Choose the post-processor (controller dialect) the machine's programs are written with",
        placement: Some(Placement::large(RibbonTab::Export, "Post")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required(
                "post",
                ParamKind::Str,
                "Post-processor id, e.g. fanuc, haas, linuxcnc, grbl, mach, generic",
            ),
            Param::optional("machine", ParamKind::Str, "Machine id (default: the active machine)"),
        ],
        returns: "{\"machine\": id, \"post\": id}",
        enabled: always,
        run: |s, p| {
            let post = params::req_str(p, "post")?.trim().to_owned();
            if !postproc::available().iter().any(|e| e.id == post) {
                return Err(EngineError::InvalidParams(format!("there is no post `{post}`; see post.processor.list")));
            }
            let machine = match params::opt_str(p, "machine")? {
                Some(id) => id.to_owned(),
                None => s.library.active.clone().ok_or_else(|| {
                    EngineError::InvalidParams("there is no machine; create one on the Machine tab".into())
                })?,
            };
            let m = s
                .library
                .machines
                .iter_mut()
                .find(|m| m.id == machine)
                .ok_or_else(|| EngineError::InvalidParams(format!("no machine `{machine}`")))?;
            let new = if post == GENERIC { String::new() } else { post.clone() };
            // Another post's settings mean nothing to this one.
            if m.post != new {
                m.post_properties.clear();
            }
            m.post = new;
            s.library.changed();
            Ok(json!({ "machine": machine, "post": post }))
        },
    },
    CommandSpec {
        id: "post.gcode.export",
        label: "Export G-code",
        tooltip: "Post the toolpaths with the machine's post-processor and write the program",
        placement: Some(Placement::large(RibbonTab::Export, "Post")),
        icon: None,
        shortcut: None,
        params: program_params!(Param::required("path", ParamKind::Str, "File to write, e.g. part.nc")),
        returns: "{\"path\": path, \"lines\": count, \"toolpaths\": count, \"post\": id, \"extension\": ext, \"warnings\": [...]}",
        enabled: always,
        run: |s, p| {
            let path = params::req_str(p, "path")?;
            let prog = program(s, p)?;
            std::fs::write(path, &prog.text)
                .map_err(|e| EngineError::InvalidParams(format!("could not write {path}: {e}")))?;
            Ok(json!({
                "path": path,
                "lines": prog.text.lines().count(),
                "toolpaths": prog.toolpaths,
                "post": prog.post,
                "extension": prog.extension,
                "warnings": prog.warnings,
            }))
        },
    },
    CommandSpec {
        id: "post.gcode.get",
        label: "G-code text",
        tooltip: "The program the export would write, as text",
        placement: None,
        icon: None,
        shortcut: None,
        params: program_params!(),
        returns: "{\"text\": program, \"lines\": count, \"post\": id, \"extension\": ext, \"warnings\": [...]}",
        enabled: always,
        run: |s, p| {
            let prog = program(s, p)?;
            Ok(json!({
                "lines": prog.text.lines().count(),
                "text": prog.text,
                "post": prog.post,
                "extension": prog.extension,
                "warnings": prog.warnings,
            }))
        },
    },
    CommandSpec {
        id: "post.processor.list",
        label: "Post-processors",
        tooltip: "The post-processors that can be chosen: generic, the shipped ones and your own",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of {id, description, builtin, path}",
        enabled: always,
        run: |_, _| {
            Ok(json!(
                postproc::available()
                    .into_iter()
                    .map(|e| json!({
                        "id": e.id,
                        "description": e.description,
                        "builtin": e.builtin,
                        "path": e.path.map(|p| p.display().to_string()),
                    }))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "post.properties.get",
        label: "Post settings",
        tooltip: "The settings the machine's post offers, with the values chosen for the machine",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("machine", ParamKind::Str, "Machine id (default: the active machine)"),
            Param::optional("post", ParamKind::Str, "Another post's settings (default: the machine's post)"),
        ],
        returns: "{\"post\": id, \"properties\": [{name, title, description, group, kind, default, choices, value}]}",
        enabled: always,
        run: |s, p| {
            let machine = machine_id(s, p)?;
            let m = s
                .library
                .machine(&machine)
                .ok_or_else(|| EngineError::InvalidParams(format!("no machine `{machine}`")))?;
            let post = postproc::resolve(s, params::opt_str(p, "post")?, &machine);
            let mut props = postproc::properties(&post)?;
            for prop in &mut props {
                let name = prop["name"].as_str().unwrap_or_default().to_owned();
                let value = m.post_properties.get(&name).map_or(prop["default"].clone(), |v| json!(v));
                prop["value"] = value;
            }
            Ok(json!({ "post": post, "machine": machine, "properties": props }))
        },
    },
    CommandSpec {
        id: "post.properties.set",
        label: "Set post setting",
        tooltip: "Choose a value for one of the machine's post settings (empty restores the post's default)",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::required("name", ParamKind::Str, "Setting name, e.g. blend_finish"),
            Param::required(
                "value",
                ParamKind::Str,
                "Its value as text (true/false, numbers, a choice); empty for the default",
            ),
            Param::optional("machine", ParamKind::Str, "Machine id (default: the active machine)"),
        ],
        returns: "{\"machine\": id, \"properties\": {name: value}}",
        enabled: always,
        run: |s, p| {
            let machine = machine_id(s, p)?;
            let name = params::req_str(p, "name")?.trim().to_owned();
            let value = params::req_str(p, "value")?.trim().to_owned();
            if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(EngineError::InvalidParams(format!("`{name}` is not a setting name")));
            }
            if value.chars().count() > 2048 {
                return Err(EngineError::InvalidParams("the value is longer than 2048 characters".into()));
            }
            let m = s
                .library
                .machines
                .iter_mut()
                .find(|m| m.id == machine)
                .ok_or_else(|| EngineError::InvalidParams(format!("no machine `{machine}`")))?;
            if value.is_empty() {
                m.post_properties.remove(&name);
            } else {
                if !m.post_properties.contains_key(&name)
                    && m.post_properties.len() >= crate::library::MAX_POST_PROPERTIES
                {
                    return Err(EngineError::InvalidParams(
                        "this machine keeps as many post settings as it can".into(),
                    ));
                }
                m.post_properties.insert(name, value);
            }
            let props = json!(m.post_properties);
            s.library.changed();
            Ok(json!({ "machine": machine, "properties": props }))
        },
    },
    CommandSpec {
        id: "post.processor.folder",
        label: "Posts folder",
        tooltip: "Where post-processors live: copy one from builtin/ into this folder to change it",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{\"path\": folder, \"builtin\": folder}",
        enabled: always,
        run: |_, _| {
            let dir = postproc::posts_dir()
                .ok_or_else(|| EngineError::InvalidParams("there is no folder for posts (no home folder)".into()))?;
            let builtin = postproc::install_builtin(&dir)?;
            Ok(json!({ "path": dir.display().to_string(), "builtin": builtin.display().to_string() }))
        },
    },
    CommandSpec {
        id: "post.limits.check",
        label: "Check limits",
        tooltip: "Check the toolpaths against the machine's travel, spindle speed and feed limits",
        placement: Some(Placement::small(RibbonTab::Export, "Post")),
        icon: None,
        shortcut: None,
        params: &[Param::optional(
            "toolpaths",
            ParamKind::StrList { max: MAX_PROGRAM_TOOLPATHS },
            "Toolpath ids (default: all)",
        )],
        returns: "{\"ok\": bool, \"problems\": [text]}",
        enabled: always,
        run: |s, p| {
            let ids = params::str_list(p, "toolpaths")?;
            let tps = postproc::chosen(s, &ids)?;
            let cl = postproc::cl_data(s, &tps, &cl_options(s, p)?)?;
            let problems = cl["warnings"].clone();
            Ok(json!({ "ok": problems.as_array().is_none_or(Vec::is_empty), "problems": problems }))
        },
    },
    CommandSpec {
        id: "toolpath.cl.export",
        label: "CL data",
        tooltip: "Write machine-neutral cutter location data (JSON) for a post-processor or another program",
        placement: Some(Placement::large(RibbonTab::Toolpaths, "Output")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required("path", ParamKind::Str, "File to write, e.g. part.cl.json"),
            Param::optional(
                "toolpaths",
                ParamKind::StrList { max: MAX_PROGRAM_TOOLPATHS },
                "Toolpath ids in order (default: all)",
            ),
            Param::optional("units", ParamKind::Choice(DisplayUnits::IDS), "Units the program should be written in"),
            Param::optional(
                "arc_tolerance",
                ParamKind::Length { min: 0.0, max: 1.0 },
                "Fit arcs within this, mm; 0 for lines only (default 0.01)",
            ),
        ],
        returns: "{\"path\": path, \"operations\": count, \"moves\": count}",
        enabled: always,
        run: |s, p| {
            let path = params::req_str(p, "path")?;
            let ids = params::str_list(p, "toolpaths")?;
            let tps = postproc::chosen(s, &ids)?;
            let cl = postproc::cl_data(s, &tps, &cl_options(s, p)?)?;
            let text = serde_json::to_string_pretty(&cl).map_err(|e| EngineError::Internal(e.to_string()))?;
            std::fs::write(path, &text)
                .map_err(|e| EngineError::InvalidParams(format!("could not write {path}: {e}")))?;
            let moves: usize = cl["operations"]
                .as_array()
                .map_or(0, |ops| ops.iter().map(|o| o["moves"].as_array().map_or(0, Vec::len)).sum());
            Ok(json!({ "path": path, "operations": tps.len(), "moves": moves }))
        },
    },
];

fn units(s: &Session, p: &Value) -> EngineResult<DisplayUnits> {
    match params::opt_str(p, "units")? {
        Some(id) => {
            DisplayUnits::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown units `{id}`")))
        }
        None => Ok(s.prefs.units),
    }
}

fn cl_options(s: &Session, p: &Value) -> EngineResult<ClOptions> {
    let tolerance = params::opt_f64(p, "arc_tolerance", 0.0, 1.0)?.unwrap_or(0.01);
    Ok(ClOptions {
        units: units(s, p)?.id(),
        program_number: p.get("program_number").and_then(Value::as_u64).map_or(1000, |n| n.clamp(1, 99_999) as u32),
        line_numbers: params::opt_bool(p, "line_numbers")?.unwrap_or(false),
        arc_tolerance: (tolerance > 0.0).then_some(tolerance),
    })
}

fn program(s: &Session, p: &Value) -> EngineResult<Program> {
    let ids = params::str_list(p, "toolpaths")?;
    let tps = postproc::chosen(s, &ids)?;
    let machine = tps.first().map(|t| t.machine.clone()).unwrap_or_default();
    let post = postproc::resolve(s, params::opt_str(p, "post")?, &machine);
    let prog = if post == GENERIC {
        let opt = Options {
            units: units(s, p)?,
            program_number: p.get("program_number").and_then(Value::as_u64).map_or(1000, |n| n.clamp(1, 9999) as u32),
            line_numbers: params::opt_bool(p, "line_numbers")?.unwrap_or(false),
        };
        Program {
            text: gcode(s, &ids, &opt)?,
            post,
            warnings: Vec::new(),
            extension: "nc".into(),
            toolpaths: tps.len(),
        }
    } else {
        let cl = postproc::cl_data(s, &tps, &cl_options(s, p)?)?;
        let out = postproc::run(&post, &cl)?;
        Program { text: out.gcode, post, warnings: out.warnings, extension: out.extension, toolpaths: tps.len() }
    };
    if prog.text.len() > MAX_PROGRAM_BYTES {
        return Err(EngineError::InvalidParams("the program would be larger than 512 MiB".into()));
    }
    Ok(prog)
}

fn machine_id(s: &Session, p: &Value) -> EngineResult<String> {
    match params::opt_str(p, "machine")? {
        Some(id) => Ok(id.to_owned()),
        None => s
            .library
            .active
            .clone()
            .ok_or_else(|| EngineError::InvalidParams("there is no machine; create one on the Machine tab".into())),
    }
}
