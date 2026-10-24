//! The command registry. Every user-visible action is a [`CommandSpec`]; the ribbon,
//! console, scripts, the Python API and agents all reach the same commands.
//!
//! Each command declares its parameters as a typed schema ([`Param`]). The engine checks
//! every call against it before running the command, and the Python API and its guide are
//! generated from it (`cargo xtask api`).

mod job;
mod model;
mod planned;
mod ui;
mod view;

use serde_json::{Map, Value, json};

use crate::error::{EngineError, EngineResult};
use crate::session::Session;

/// Ribbon tabs, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RibbonTab {
    Home,
    Setup,
    Tooling,
    Toolpaths,
    Simulation,
    Export,
}

impl RibbonTab {
    pub const ALL: [Self; 6] =
        [Self::Home, Self::Setup, Self::Tooling, Self::Toolpaths, Self::Simulation, Self::Export];
    pub const IDS: &'static [&'static str] = &["home", "setup", "tooling", "toolpaths", "simulation", "export"];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Setup => "Setup",
            Self::Tooling => "Tooling",
            Self::Toolpaths => "Toolpaths",
            Self::Simulation => "Simulation",
            Self::Export => "Export",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Setup => "setup",
            Self::Tooling => "tooling",
            Self::Toolpaths => "toolpaths",
            Self::Simulation => "simulation",
            Self::Export => "export",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
}

/// How a command's button is drawn in its ribbon group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonSize {
    /// Icon over a one- or two-line label.
    Large,
    /// Small icon beside its label; consecutive small buttons stack three high.
    Small,
    /// One tile in the group's scrollable gallery.
    Gallery,
}

/// Where a command's button sits on the ribbon.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub tab: RibbonTab,
    pub panel: &'static str,
    pub size: ButtonSize,
    /// Related command ids offered from the button's dropdown arrow (a split button).
    pub menu: &'static [&'static str],
}

impl Placement {
    pub const fn large(tab: RibbonTab, panel: &'static str) -> Self {
        Self { tab, panel, size: ButtonSize::Large, menu: &[] }
    }

    pub const fn small(tab: RibbonTab, panel: &'static str) -> Self {
        Self { tab, panel, size: ButtonSize::Small, menu: &[] }
    }

    pub const fn gallery(tab: RibbonTab, panel: &'static str) -> Self {
        Self { tab, panel, size: ButtonSize::Gallery, menu: &[] }
    }

    pub const fn with_menu(self, menu: &'static [&'static str]) -> Self {
        Self { menu, ..self }
    }
}

/// The type of one command parameter.
#[derive(Clone, Copy, Debug)]
pub enum ParamKind {
    Str,
    Bool,
    /// A finite number in `min..=max`.
    Number {
        min: f64,
        max: f64,
    },
    /// A whole number in `min..=max`.
    Integer {
        min: i64,
        max: i64,
    },
    /// One of a fixed set of strings.
    Choice(&'static [&'static str]),
    /// `[x, y, z]` in millimetres.
    Vec3,
}

impl ParamKind {
    /// Short type name used in error messages, the guide and `command.list`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Bool => "bool",
            Self::Number { .. } => "number",
            Self::Integer { .. } => "integer",
            Self::Choice(_) => "choice",
            Self::Vec3 => "vec3",
        }
    }
}

/// One named parameter of a command.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    pub doc: &'static str,
}

impl Param {
    pub const fn required(name: &'static str, kind: ParamKind, doc: &'static str) -> Self {
        Self { name, kind, required: true, doc }
    }

    pub const fn optional(name: &'static str, kind: ParamKind, doc: &'static str) -> Self {
        Self { name, kind, required: false, doc }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "name": self.name,
            "type": self.kind.type_name(),
            "required": self.required,
            "doc": self.doc,
        });
        match self.kind {
            ParamKind::Number { min, max } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
            }
            ParamKind::Integer { min, max } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
            }
            ParamKind::Choice(choices) => v["choices"] = json!(choices),
            _ => {}
        }
        v
    }
}

pub type RunFn = fn(&mut Session, &Value) -> EngineResult<Value>;
pub type EnabledFn = fn(&Session) -> bool;

pub struct CommandSpec {
    /// Dotted lower-case id: `<area>.<thing>[.<variant>]`.
    pub id: &'static str,
    pub label: &'static str,
    pub tooltip: &'static str,
    pub placement: Option<Placement>,
    /// Icon name under `assets/icons/ui/design/svg/`; none are assigned yet.
    pub icon: Option<&'static str>,
    pub shortcut: Option<&'static str>,
    /// Accepted parameters; anything else is rejected.
    pub params: &'static [Param],
    /// What a successful call returns, for the API guide.
    pub returns: &'static str,
    pub enabled: EnabledFn,
    pub run: RunFn,
}

impl CommandSpec {
    /// The command as `command.list` and the Python API describe it.
    pub fn to_json(&self, session: &Session) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "tooltip": self.tooltip,
            "shortcut": self.shortcut,
            "enabled": (self.enabled)(session),
            "params": self.params.iter().map(Param::to_json).collect::<Vec<_>>(),
            "returns": self.returns,
            "ribbon": self.placement.map(|p| json!({
                "tab": p.tab.id(),
                "panel": p.panel,
                "menu": p.menu,
            })),
        })
    }

    /// Checks `params` against the schema: an object (or null), no unknown keys,
    /// required keys present and every value of the declared type and range.
    pub fn validate(&self, params: &Value) -> EngineResult<()> {
        let empty = Map::new();
        let map = match params {
            Value::Null => &empty,
            Value::Object(m) => m,
            _ => return Err(EngineError::InvalidParams("parameters must be a JSON object".into())),
        };
        for key in map.keys() {
            if !self.params.iter().any(|p| p.name == key) {
                let known: Vec<&str> = self.params.iter().map(|p| p.name).collect();
                let key: String = key.chars().take(64).collect();
                return Err(EngineError::InvalidParams(if known.is_empty() {
                    format!("`{}` takes no parameters (got `{key}`)", self.id)
                } else {
                    format!("unknown parameter `{key}`; expected one of: {}", known.join(", "))
                }));
            }
        }
        for p in self.params {
            match map.get(p.name).filter(|v| !v.is_null()) {
                None if p.required => {
                    return Err(EngineError::InvalidParams(format!("`{}` is required", p.name)));
                }
                None => {}
                Some(v) => check_kind(p, v)?,
            }
        }
        Ok(())
    }
}

fn check_kind(p: &Param, v: &Value) -> EngineResult<()> {
    let bad = |what: String| Err(EngineError::InvalidParams(format!("`{}` {what}", p.name)));
    match p.kind {
        ParamKind::Str => match v.as_str() {
            Some(s) if s.len() <= crate::params::MAX_STR_LEN => Ok(()),
            Some(_) => bad("is too long".into()),
            None => bad("must be a string".into()),
        },
        ParamKind::Bool => {
            if v.is_boolean() {
                Ok(())
            } else {
                bad("must be true or false".into())
            }
        }
        ParamKind::Number { min, max } => match v.as_f64() {
            Some(n) if n.is_finite() && n >= min && n <= max => Ok(()),
            Some(_) => bad(format!("must be between {min} and {max}")),
            None => bad("must be a number".into()),
        },
        ParamKind::Integer { min, max } => match v.as_i64() {
            Some(n) if n >= min && n <= max => Ok(()),
            Some(_) => bad(format!("must be between {min} and {max}")),
            None => bad("must be a whole number".into()),
        },
        ParamKind::Choice(choices) => match v.as_str() {
            Some(s) if choices.contains(&s) => Ok(()),
            _ => bad(format!("must be one of: {}", choices.join(", "))),
        },
        ParamKind::Vec3 => match v.as_array() {
            Some(a)
                if a.len() == 3 && a.iter().all(|c| c.as_f64().is_some_and(|n| n.is_finite() && n.abs() <= 1.0e9)) =>
            {
                Ok(())
            }
            _ => bad("must be [x, y, z] with finite numbers".into()),
        },
    }
}

pub fn always(_: &Session) -> bool {
    true
}

pub fn never(_: &Session) -> bool {
    false
}

/// All registered commands, in ribbon order within each tab.
pub fn all() -> impl Iterator<Item = &'static CommandSpec> {
    let registered = job::COMMANDS
        .iter()
        .chain(model::COMMANDS.iter())
        .chain(view::COMMANDS.iter())
        .chain(ui::COMMANDS.iter())
        .chain(planned::COMMANDS.iter());
    #[cfg(test)]
    let registered = registered.chain(test_only::COMMANDS.iter());
    registered
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    all().find(|c| c.id == id)
}

/// Commands on `tab`, grouped by panel in first-seen order.
pub fn ribbon(tab: RibbonTab) -> Vec<(&'static str, Vec<&'static CommandSpec>)> {
    let mut panels: Vec<(&'static str, Vec<&'static CommandSpec>)> = Vec::new();
    for spec in all() {
        let Some(p) = spec.placement.filter(|p| p.tab == tab) else {
            continue;
        };
        match panels.iter_mut().find(|(name, _)| *name == p.panel) {
            Some((_, list)) => list.push(spec),
            None => panels.push((p.panel, vec![spec])),
        }
    }
    panels
}

#[cfg(test)]
pub(crate) mod test_only {
    use super::*;

    #[allow(clippy::panic)]
    pub(crate) static COMMANDS: &[CommandSpec] = &[CommandSpec {
        id: "test.panic",
        label: "Panic",
        tooltip: "",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "never returns",
        enabled: always,
        run: |s, _| {
            s.job.name = "half-edited".into();
            panic!("boom")
        },
    }];
}
