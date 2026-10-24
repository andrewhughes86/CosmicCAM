//! Job, selection, introspection and console commands.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, all, always, find};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{OutlineNode, Session};

const SINCE: ParamKind = ParamKind::Integer { min: 0, max: i64::MAX };
const LIMIT: ParamKind = ParamKind::Integer { min: 1, max: crate::session::MAX_LOG as i64 };

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "job.new",
        label: "New job",
        tooltip: "Start an empty job",
        placement: Some(Placement::large(RibbonTab::Home, "Job")),
        icon: None,
        shortcut: Some("Ctrl+N"),
        params: &[Param::optional("sample", ParamKind::Bool, "Load the built-in sample part")],
        returns: "{\"job\": name}",
        enabled: always,
        run: |s, p| {
            s.reset_job(params::opt_bool(p, "sample")?.unwrap_or(false));
            Ok(json!({ "job": s.job.name }))
        },
    },
    CommandSpec {
        id: "job.outline.get",
        label: "Job outline",
        tooltip: "The job tree shown in the explorer",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of nodes {id, label, properties: [{name, value}], children}",
        enabled: always,
        run: |s, _| Ok(json!(s.outline().iter().map(OutlineNode::to_json).collect::<Vec<_>>())),
    },
    CommandSpec {
        id: "selection.set",
        label: "Select",
        tooltip: "Select an item in the job outline",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::optional("id", ParamKind::Str, "Outline node id; omit or null to clear the selection")],
        returns: "{\"selection\": id or null}",
        enabled: always,
        run: |s, p| {
            match params::opt_str(p, "id")? {
                None => s.view.selection = None,
                Some(id) => {
                    if s.outline_node(id).is_none() {
                        return Err(EngineError::InvalidParams(format!("no outline item `{id}`")));
                    }
                    s.view.selection = Some(id.to_owned());
                }
            }
            Ok(json!({ "selection": s.view.selection }))
        },
    },
    CommandSpec {
        id: "selection.get",
        label: "Selection",
        tooltip: "The selected outline item",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "the selected node {id, label, properties, children}, or null",
        enabled: always,
        run: |s, _| {
            Ok(s.view.selection.as_deref().and_then(|id| s.outline_node(id)).map_or(Value::Null, |n| n.to_json()))
        },
    },
    CommandSpec {
        id: "session.state.get",
        label: "Session state",
        tooltip: "Job, camera, display, theme, selection, ribbon and pane state in one object",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{job, theme, fps_limit, display, camera, selection, scene_bounds, ribbon_tab, open_panes, outline, log_seq}",
        enabled: always,
        run: |s, _| Ok(s.state_json()),
    },
    CommandSpec {
        id: "command.list",
        label: "List commands",
        tooltip: "Every registered command with its parameters",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::optional("enabled_only", ParamKind::Bool, "Only commands that can run right now")],
        returns: "list of {id, label, tooltip, shortcut, enabled, params, returns, ribbon}",
        enabled: always,
        run: |s, p| {
            let enabled_only = params::opt_bool(p, "enabled_only")?.unwrap_or(false);
            Ok(json!(
                all()
                    .filter(|c| !c.id.starts_with("test."))
                    .filter(|c| !enabled_only || (c.enabled)(s))
                    .map(|c| c.to_json(s))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "command.describe",
        label: "Describe command",
        tooltip: "One command's parameters and state",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Command id, e.g. view.camera.preset")],
        returns: "{id, label, tooltip, shortcut, enabled, params, returns, ribbon}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            find(id).map(|c| c.to_json(s)).ok_or_else(|| EngineError::InvalidParams(format!("unknown command `{id}`")))
        },
    },
    CommandSpec {
        id: "console.clear",
        label: "Clear console",
        tooltip: "Clear the command log",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "null",
        enabled: always,
        run: |s, _| {
            s.log_mut().clear();
            Ok(Value::Null)
        },
    },
    CommandSpec {
        id: "console.log.get",
        label: "Command log",
        tooltip: "Commands run so far and their outcomes",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("since", SINCE, "Only entries with seq greater than this"),
            Param::optional("limit", LIMIT, "Most recent entries to return (default 100)"),
        ],
        returns: "list of {seq, command, params, ok, result | error}",
        enabled: always,
        run: |s, p| log_get(s, p),
    },
];

fn log_get(s: &Session, p: &Value) -> EngineResult<Value> {
    let since = p.get("since").and_then(Value::as_u64).unwrap_or(0);
    let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
    let entries: Vec<Value> = s
        .log()
        .iter()
        .filter(|e| e.seq > since)
        .map(|e| match &e.outcome {
            Ok(v) => json!({ "seq": e.seq, "command": e.command, "params": e.params, "ok": true, "result": v }),
            Err(err) => json!({
                "seq": e.seq,
                "command": e.command,
                "params": e.params,
                "ok": false,
                "error": crate::error::error_json(err),
            }),
        })
        .collect();
    let skip = entries.len().saturating_sub(limit);
    Ok(json!(entries.into_iter().skip(skip).collect::<Vec<_>>()))
}
