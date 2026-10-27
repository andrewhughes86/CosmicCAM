//! The command registry. Every user-visible action is a [`CommandSpec`]; the ribbon,
//! console, scripts, the Python API and agents all reach the same commands.
//!
//! Each command declares its parameters as a typed schema ([`Param`]). The engine checks
//! every call against it before running the command, and the Python API and its guide are
//! generated from it (`cargo xtask api`).

mod cad;
mod job;
mod machine;
mod model;
mod planned;
mod post;
mod settings;
mod setup;
mod sim;
mod stock;
mod strategies;
pub(crate) mod tool;
mod toolpath;
mod ui;
mod view;
mod wcs;
pub(crate) mod wcs_define;

pub use tool::preview as tool_preview;
pub use view::VIEWPORT_TOOLS;
pub use wcs_define::{BOX_POINTS as WCS_BOX_POINTS, ORIENTATIONS as WCS_ORIENTATIONS, ORIGINS as WCS_ORIGINS};

use serde_json::{Map, Value, json};

use crate::error::{EngineError, EngineResult};
use crate::session::Session;

/// What the application is being used for; each mode has its own ribbon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Modelling parts.
    Cad,
    /// Toolpaths, simulation and G-code.
    Cam,
    /// 2D drawings of parts (coming).
    Drawings,
    /// 3D printing (coming).
    Slicer,
}

impl Mode {
    pub const ALL: [Self; 4] = [Self::Cad, Self::Cam, Self::Drawings, Self::Slicer];
    pub const IDS: &'static [&'static str] = &["cad", "cam", "drawings", "slicer"];

    pub fn id(self) -> &'static str {
        match self {
            Self::Cad => "cad",
            Self::Cam => "cam",
            Self::Drawings => "drawings",
            Self::Slicer => "slicer",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cad => "CAD",
            Self::Cam => "CAM",
            Self::Drawings => "DRAWINGS",
            Self::Slicer => "SLICER",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// Whether the mode can be used yet.
    pub fn available(self) -> bool {
        matches!(self, Self::Cad | Self::Cam)
    }

    /// The mode's ribbon tabs, in display order.
    pub fn tabs(self) -> Vec<RibbonTab> {
        RibbonTab::ALL.into_iter().filter(|t| t.mode() == self).collect()
    }
}

/// Ribbon tabs, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RibbonTab {
    Home,
    Setup,
    Tooling,
    Toolpaths,
    Simulation,
    Export,
    Machine,
    Features,
    Sketch,
    Evaluate,
}

impl RibbonTab {
    pub const ALL: [Self; 10] = [
        Self::Home,
        Self::Setup,
        Self::Machine,
        Self::Tooling,
        Self::Toolpaths,
        Self::Simulation,
        Self::Export,
        Self::Features,
        Self::Sketch,
        Self::Evaluate,
    ];
    pub const IDS: &'static [&'static str] =
        &["home", "setup", "machine", "tooling", "toolpaths", "simulation", "export", "features", "sketch", "evaluate"];

    /// The mode whose ribbon it belongs to.
    pub fn mode(self) -> Mode {
        match self {
            Self::Features | Self::Sketch | Self::Evaluate => Mode::Cad,
            _ => Mode::Cam,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Setup => "Setup",
            Self::Tooling => "Tooling",
            Self::Toolpaths => "Toolpaths",
            Self::Simulation => "Simulation",
            Self::Export => "Export",
            Self::Machine => "Machine",
            Self::Features => "Features",
            Self::Sketch => "Sketch",
            Self::Evaluate => "Evaluate",
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
            Self::Machine => "machine",
            Self::Features => "features",
            Self::Sketch => "sketch",
            Self::Evaluate => "evaluate",
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
    /// A length in millimetres, `min..=max`. Dialogs show and accept it in the user's units.
    Length {
        min: f64,
        max: f64,
    },
    /// A feed in millimetres per minute, `min..=max`. Dialogs show it in the user's units.
    Rate {
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
    /// A list of up to `max` strings (e.g. geometry references).
    StrList {
        max: usize,
    },
    /// A list of up to `max` points, each `[x, y, z]` in millimetres.
    PointList {
        max: usize,
    },
    /// A list of up to `max` JSON objects, checked by the command (e.g. sketch entities).
    Objects {
        max: usize,
    },
}

impl ParamKind {
    /// Short type name used in error messages, the guide and `command.list`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Bool => "bool",
            Self::Number { .. } | Self::Length { .. } | Self::Rate { .. } => "number",
            Self::Integer { .. } => "integer",
            Self::Choice(_) => "choice",
            Self::Vec3 => "vec3",
            Self::StrList { .. } => "list",
            Self::PointList { .. } => "points",
            Self::Objects { .. } => "objects",
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
            ParamKind::Length { min, max } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
                v["unit"] = json!("mm");
            }
            ParamKind::Rate { min, max } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
                v["unit"] = json!("mm/min");
            }
            ParamKind::Integer { min, max } => {
                v["min"] = json!(min);
                v["max"] = json!(max);
            }
            ParamKind::Choice(choices) => v["choices"] = json!(choices),
            ParamKind::StrList { max } | ParamKind::PointList { max } | ParamKind::Objects { max } => {
                v["max_items"] = json!(max)
            }
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
        ParamKind::Number { min, max } | ParamKind::Length { min, max } | ParamKind::Rate { min, max } => {
            match v.as_f64() {
                Some(n) if n.is_finite() && n >= min && n <= max => Ok(()),
                Some(_) => bad(format!("must be between {min} and {max}")),
                None => bad("must be a number".into()),
            }
        }
        ParamKind::Integer { min, max } => match v.as_i64() {
            Some(n) if n >= min && n <= max => Ok(()),
            Some(_) => bad(format!("must be between {min} and {max}")),
            None => bad("must be a whole number".into()),
        },
        ParamKind::Choice(choices) => match v.as_str() {
            Some(s) if choices.contains(&s) => Ok(()),
            _ => bad(format!("must be one of: {}", choices.join(", "))),
        },
        ParamKind::StrList { max } => match v.as_array() {
            Some(a)
                if a.len() <= max
                    && a.iter().all(|s| s.as_str().is_some_and(|s| s.len() <= crate::params::MAX_STR_LEN)) =>
            {
                Ok(())
            }
            Some(a) if a.len() > max => bad(format!("has more than {max} items")),
            _ => bad("must be a list of strings".into()),
        },
        ParamKind::Vec3 => match v.as_array() {
            Some(a)
                if a.len() == 3 && a.iter().all(|c| c.as_f64().is_some_and(|n| n.is_finite() && n.abs() <= 1.0e9)) =>
            {
                Ok(())
            }
            _ => bad("must be [x, y, z] with finite numbers".into()),
        },
        ParamKind::PointList { max } => match v.as_array() {
            Some(a) if a.len() > max => bad(format!("has more than {max} points")),
            Some(a)
                if a.iter().all(|p| {
                    p.as_array().is_some_and(|c| {
                        c.len() == 3 && c.iter().all(|n| n.as_f64().is_some_and(|n| n.is_finite() && n.abs() <= 1.0e9))
                    })
                }) =>
            {
                Ok(())
            }
            _ => bad("must be a list of [x, y, z] points with finite numbers".into()),
        },
        ParamKind::Objects { max } => match v.as_array() {
            Some(a) if a.len() > max => bad(format!("has more than {max} items")),
            Some(a) if a.iter().all(Value::is_object) => Ok(()),
            _ => bad("must be a list of objects".into()),
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
        .chain(wcs::COMMANDS.iter())
        .chain(view::COMMANDS.iter())
        .chain(ui::COMMANDS.iter())
        // Working toolpath strategies come before the planned ones so they lead the gallery.
        .chain(toolpath::COMMANDS.iter())
        .chain(strategies::COMMANDS.iter())
        .chain(setup::COMMANDS.iter())
        .chain(planned::COMMANDS.iter())
        .chain(stock::COMMANDS.iter())
        .chain(tool::COMMANDS.iter())
        .chain(machine::COMMANDS.iter())
        .chain(settings::COMMANDS.iter())
        .chain(sim::COMMANDS.iter())
        .chain(post::COMMANDS.iter())
        .chain(cad::COMMANDS.iter());
    #[cfg(test)]
    let registered = registered.chain(test_only::COMMANDS.iter());
    registered
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    all().find(|c| c.id == id)
}

/// Commands that also appear on a second ribbon tab, as shortcuts to their home panel. The Home
/// tab gathers the commands that get a job going, in the order a job uses them: set it up, pick
/// the machine and tools, make the common toolpaths, then check and post them.
pub static EXTRA_PLACEMENTS: &[(&str, Placement)] = &[
    ("model.import", Placement::large(RibbonTab::Home, "Setup")),
    ("setup.create", Placement::large(RibbonTab::Home, "Setup")),
    ("setup.wcs.create", Placement::large(RibbonTab::Home, "Setup").with_menu(wcs::CREATE_MENU)),
    ("setup.stock.box", Placement::small(RibbonTab::Home, "Setup")),
    ("setup.stock.fit", Placement::small(RibbonTab::Home, "Setup")),
    ("machine.library.open", Placement::large(RibbonTab::Home, "Machine & tools")),
    ("tool.library.open", Placement::large(RibbonTab::Home, "Machine & tools")),
    ("toolpath.face", Placement::gallery(RibbonTab::Home, "Toolpaths")),
    ("toolpath.contour", Placement::gallery(RibbonTab::Home, "Toolpaths")),
    ("toolpath.pocket", Placement::gallery(RibbonTab::Home, "Toolpaths")),
    ("toolpath.drill", Placement::gallery(RibbonTab::Home, "Toolpaths")),
    ("toolpath.adaptive", Placement::gallery(RibbonTab::Home, "Toolpaths")),
    ("sim.play", Placement::large(RibbonTab::Home, "Verify & post")),
    ("post.gcode.export", Placement::large(RibbonTab::Home, "Verify & post")),
];

/// Panel order on tabs where first-seen order is not the order wanted. Panels not listed
/// follow in first-seen order.
const PANEL_ORDER: &[(RibbonTab, &[&str])] = &[
    (RibbonTab::Home, &["Job", "Setup", "Machine & tools", "Toolpaths", "Verify & post", "Preferences"]),
    (RibbonTab::Setup, &["Setup", "Model", "Coordinates", "Stock"]),
    (RibbonTab::Toolpaths, &["2D", "Drilling", "3D", "Multi-axis", "Output"]),
    (RibbonTab::Features, &["Features", "Part"]),
    (RibbonTab::Sketch, &["Sketch", "Draw"]),
];

/// Ribbon buttons in the order a job uses them, within their panels.
const BUTTON_ORDER: &[&str] = &[
    "toolpath.face",
    "toolpath.contour",
    "toolpath.pocket",
    "toolpath.slot",
    "toolpath.chamfer",
    "toolpath.trace",
    "toolpath.drill",
    "toolpath.bore",
    "toolpath.thread",
    "toolpath.adaptive",
    "toolpath.rough",
    "toolpath.flat",
    "toolpath.waterline",
    "toolpath.raster",
    "toolpath.scallop",
    "toolpath.steep_shallow",
    "toolpath.pencil",
    "toolpath.spiral",
    "toolpath.radial",
];

/// Every ribbon button: each command's own placement, then the extra ones.
pub fn placements() -> impl Iterator<Item = (&'static CommandSpec, Placement)> {
    all()
        .filter_map(|s| Some((s, s.placement?)))
        .chain(EXTRA_PLACEMENTS.iter().filter_map(|(id, p)| Some((find(id)?, *p))))
}

/// Commands on `tab`, grouped by panel.
pub fn ribbon(tab: RibbonTab) -> Vec<(&'static str, Vec<(&'static CommandSpec, Placement)>)> {
    let mut panels: Vec<(&'static str, Vec<(&'static CommandSpec, Placement)>)> = Vec::new();
    for (spec, p) in placements().filter(|(_, p)| p.tab == tab) {
        match panels.iter_mut().find(|(name, _)| *name == p.panel) {
            Some((_, list)) => list.push((spec, p)),
            None => panels.push((p.panel, vec![(spec, p)])),
        }
    }
    if let Some((_, order)) = PANEL_ORDER.iter().find(|(t, _)| *t == tab) {
        panels.sort_by_key(|(name, _)| order.iter().position(|o| o == name).unwrap_or(order.len()));
    }
    // Buttons in their workflow order where one is given; the rest as registered.
    for (_, list) in &mut panels {
        list.sort_by_key(|(spec, _)| BUTTON_ORDER.iter().position(|id| *id == spec.id).unwrap_or(BUTTON_ORDER.len()));
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
