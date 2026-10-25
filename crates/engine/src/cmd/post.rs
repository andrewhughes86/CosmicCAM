//! G-code output from the built-in generic writer (see `crate::post`). Python
//! post-processors replace it in Phase 8.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::post::{MAX_PROGRAM_TOOLPATHS, Options, gcode};
use crate::session::Session;
use crate::units::DisplayUnits;

/// Longest program written.
const MAX_PROGRAM_BYTES: usize = 512 * 1024 * 1024;

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "post.gcode.export",
        label: "Export G-code",
        tooltip: "Write the toolpaths as a G-code program (.nc) with the generic writer",
        placement: Some(Placement::large(RibbonTab::Export, "Post")),
        icon: None,
        shortcut: None,
        params: &[
            Param::required("path", ParamKind::Str, "File to write, e.g. part.nc"),
            Param::optional(
                "toolpaths",
                ParamKind::StrList { max: MAX_PROGRAM_TOOLPATHS },
                "Toolpath ids in order (default: all)",
            ),
            Param::optional(
                "units",
                ParamKind::Choice(DisplayUnits::IDS),
                "Program units, in or mm (default: the units setting)",
            ),
            Param::optional("program_number", ParamKind::Integer { min: 1, max: 9999 }, "O number (default 1000)"),
            Param::optional("line_numbers", ParamKind::Bool, "Number the lines N5, N10, … (default false)"),
        ],
        returns: "{\"path\": path, \"lines\": count, \"toolpaths\": count}",
        enabled: always,
        run: |s, p| {
            let path = params::req_str(p, "path")?;
            let text = program(s, p)?;
            std::fs::write(path, &text)
                .map_err(|e| EngineError::InvalidParams(format!("could not write {path}: {e}")))?;
            Ok(json!({ "path": path, "lines": text.lines().count(), "toolpaths": toolpath_count(s, p)? }))
        },
    },
    CommandSpec {
        id: "post.gcode.get",
        label: "G-code text",
        tooltip: "The G-code program the export would write, as text",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional(
                "toolpaths",
                ParamKind::StrList { max: MAX_PROGRAM_TOOLPATHS },
                "Toolpath ids in order (default: all)",
            ),
            Param::optional(
                "units",
                ParamKind::Choice(DisplayUnits::IDS),
                "Program units, in or mm (default: the units setting)",
            ),
            Param::optional("program_number", ParamKind::Integer { min: 1, max: 9999 }, "O number (default 1000)"),
            Param::optional("line_numbers", ParamKind::Bool, "Number the lines (default false)"),
        ],
        returns: "{\"text\": program, \"lines\": count}",
        enabled: always,
        run: |s, p| {
            let text = program(s, p)?;
            Ok(json!({ "lines": text.lines().count(), "text": text }))
        },
    },
];

fn toolpath_count(s: &Session, p: &Value) -> EngineResult<usize> {
    let ids = params::str_list(p, "toolpaths")?;
    Ok(if ids.is_empty() { s.job.toolpaths.len() } else { ids.len() })
}

fn program(s: &Session, p: &Value) -> EngineResult<String> {
    let units = match params::opt_str(p, "units")? {
        Some(id) => {
            DisplayUnits::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown units `{id}`")))?
        }
        None => s.prefs.units,
    };
    let opt = Options {
        units,
        program_number: p.get("program_number").and_then(Value::as_u64).map_or(1000, |n| n.clamp(1, 9999) as u32),
        line_numbers: params::opt_bool(p, "line_numbers")?.unwrap_or(false),
    };
    let text = gcode(s, &params::str_list(p, "toolpaths")?, &opt)?;
    if text.len() > MAX_PROGRAM_BYTES {
        return Err(EngineError::InvalidParams("the program would be larger than 512 MiB".into()));
    }
    Ok(text)
}
