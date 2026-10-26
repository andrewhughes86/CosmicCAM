//! Machines and their tool libraries.
//!
//! Every machine owns its own tool library, so tool numbers, holders and cutting data match
//! the machine that runs them. The library lives on the session, outside any one job: a new
//! job keeps it, and the app saves it after every change. Controller dialect, post-processing
//! and full kinematic limits are Python machine configurations (Phase 8); a machine here
//! names its post module once those exist.
//!
//! Saved libraries are untrusted input: [`MachineLibrary::from_json`] checks every value and
//! drops what does not fit rather than failing on it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Format tag written into saved libraries.
pub const LIBRARY_FORMAT: &str = "CosmicCAM-library/1";
/// Most machines one library holds.
pub const MAX_MACHINES: usize = 64;
/// Most tools one machine's library holds.
pub const MAX_TOOLS: usize = 2000;
/// Highest tool number.
pub const MAX_TOOL_NUMBER: u32 = 9999;
/// Longest name or note kept.
pub const MAX_TEXT_CHARS: usize = 128;

pub use crate::tooling::{Holder, HolderSegment, Preset, TOOL_MATERIALS, TOOL_TYPES, Tool};
pub const MACHINE_KINDS: &[&str] = &["mill_3axis", "mill_4axis", "mill_5axis"];
/// Which units carry a 4- or 5-axis machine's rotary axes.
pub const LAYOUTS: &[&str] = &["table_table", "head_table", "head_head"];
/// Rotary axis names; A turns about X, B about Y, C about Z.
pub const ROTARY_NAMES: &[&str] = &["A", "B", "C"];
/// Widest rotary travel either way, degrees (a continuous axis is given a large travel).
pub const MAX_ROTARY_TRAVEL: f64 = 100_000.0;

/// One rotary axis of a 4- or 5-axis machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RotaryAxis {
    /// `A`, `B` or `C`: it turns about X, Y or Z.
    pub name: String,
    /// It turns the other way from the ISO convention.
    #[serde(default)]
    pub reversed: bool,
    /// Travel, degrees.
    pub min: f64,
    pub max: f64,
}

impl RotaryAxis {
    fn new(name: &str, min: f64, max: f64) -> Self {
        Self { name: name.into(), reversed: false, min, max }
    }

    /// The direction it turns about, with the sign of its convention.
    pub fn direction(&self) -> glam::DVec3 {
        let d = match self.name.as_str() {
            "A" => glam::DVec3::X,
            "B" => glam::DVec3::Y,
            _ => glam::DVec3::Z,
        };
        if self.reversed { -d } else { d }
    }

    fn joint(&self) -> CosmicCAM_geom::kinematics::Joint {
        CosmicCAM_geom::kinematics::Joint {
            axis: self.direction(),
            min: self.min.to_radians(),
            max: self.max.to_radians(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if !ROTARY_NAMES.contains(&self.name.as_str()) {
            return Err(format!("rotary axis `{}` must be A, B or C", self.name));
        }
        let ok = |v: f64| v.is_finite() && v.abs() <= MAX_ROTARY_TRAVEL;
        if !(ok(self.min) && ok(self.max) && self.min < self.max) {
            return Err(format!(
                "{} travel must run from a lower to a higher angle within ±{MAX_ROTARY_TRAVEL}°",
                self.name
            ));
        }
        Ok(())
    }

    pub fn to_json(&self) -> Value {
        json!({ "name": self.name, "reversed": self.reversed, "min": self.min, "max": self.max })
    }
}

/// How a 4- or 5-axis machine's rotary axes are arranged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kinematics {
    /// One of [`LAYOUTS`].
    pub layout: String,
    /// The axis nearer the machine's base (for head-table machines, the table's axis).
    pub primary: RotaryAxis,
    /// The axis it carries (for head-table machines, the head's axis); none on a 4-axis machine.
    pub secondary: Option<RotaryAxis>,
    /// Where the primary axis passes, relative to the work origin, mm (tables; used when the
    /// control does not follow the tool tip itself).
    #[serde(default)]
    pub primary_centre: [f64; 3],
    /// Where the secondary axis passes, relative to the work origin, mm (table-table).
    #[serde(default)]
    pub secondary_centre: [f64; 3],
    /// From where the head turns to the spindle's gauge line, mm (heads).
    #[serde(default)]
    pub pivot_length: f64,
}

impl Kinematics {
    /// A machine of `kind` with typical axes: a trunnion (A tilting ±120°, C continuous) for
    /// 5-axis, a rotary A for 4-axis.
    pub fn typical(kind: &str) -> Option<Self> {
        let secondary = match kind {
            "mill_5axis" => Some(RotaryAxis::new("C", -MAX_ROTARY_TRAVEL, MAX_ROTARY_TRAVEL)),
            "mill_4axis" => None,
            _ => return None,
        };
        let primary = if secondary.is_some() {
            RotaryAxis::new("A", -120.0, 120.0)
        } else {
            RotaryAxis::new("A", -MAX_ROTARY_TRAVEL, MAX_ROTARY_TRAVEL)
        };
        Some(Self {
            layout: "table_table".into(),
            primary,
            secondary,
            primary_centre: [0.0; 3],
            secondary_centre: [0.0; 3],
            pivot_length: 0.0,
        })
    }

    pub fn geom(&self) -> CosmicCAM_geom::kinematics::Kinematics {
        use CosmicCAM_geom::kinematics::Layout;
        let v = |a: [f64; 3]| glam::DVec3::from_array(a);
        CosmicCAM_geom::kinematics::Kinematics {
            layout: match self.layout.as_str() {
                "head_head" => Layout::HeadHead,
                "head_table" => Layout::HeadTable,
                _ => Layout::TableTable,
            },
            primary: self.primary.joint(),
            secondary: self.secondary.as_ref().map(RotaryAxis::joint),
            primary_centre: v(self.primary_centre),
            secondary_centre: v(self.secondary_centre),
            pivot_length: self.pivot_length,
        }
    }

    /// The rotary axis names, primary first.
    pub fn names(&self) -> Vec<String> {
        std::iter::once(&self.primary).chain(self.secondary.as_ref()).map(|a| a.name.clone()).collect()
    }

    fn validate(&self, kind: &str) -> Result<(), String> {
        if !LAYOUTS.contains(&self.layout.as_str()) {
            return Err(format!("unknown layout `{}`", self.layout));
        }
        self.primary.validate()?;
        if let Some(s) = &self.secondary {
            s.validate()?;
            if s.name == self.primary.name {
                return Err("the two rotary axes must turn about different directions".into());
            }
            if self.layout == "head_table" && s.name == "C" {
                return Err("a head-table machine tilts its head about A or B".into());
            }
        }
        match (kind, &self.secondary) {
            ("mill_5axis", None) => return Err("a 5-axis machine needs two rotary axes".into()),
            ("mill_4axis", Some(_)) => return Err("a 4-axis machine has one rotary axis".into()),
            _ => {}
        }
        let ok = |v: &[f64; 3]| v.iter().all(|x| x.is_finite() && x.abs() <= 100_000.0);
        if !ok(&self.primary_centre) || !ok(&self.secondary_centre) {
            return Err("rotary centres must be within 100000 mm".into());
        }
        if !(self.pivot_length.is_finite() && (0.0..=10_000.0).contains(&self.pivot_length)) {
            return Err("`pivot_length` must be between 0 and 10000 mm".into());
        }
        Ok(())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "layout": self.layout,
            "primary": self.primary.to_json(),
            "secondary": self.secondary.as_ref().map(RotaryAxis::to_json),
            "primary_centre": self.primary_centre,
            "secondary_centre": self.secondary_centre,
            "pivot_length": self.pivot_length,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    /// `machine.<n>`.
    pub id: String,
    pub name: String,
    pub description: String,
    /// One of [`MACHINE_KINDS`].
    pub kind: String,
    /// Travel along X, Y and Z, mm.
    pub travel: [f64; 3],
    pub max_spindle_rpm: f64,
    /// Fastest cutting feed, mm/min.
    pub max_feed: f64,
    /// Tool changer pockets; 0 for manual tool changes.
    pub tool_capacity: u32,
    /// The post-processor its programs are written with (empty for the generic one).
    pub post: String,
    /// The user's choices for the post's settings, by name (text, as the post reads them).
    #[serde(default)]
    pub post_properties: std::collections::BTreeMap<String, String>,
    /// The rotary axes of a 4- or 5-axis machine; none on a 3-axis one.
    #[serde(default)]
    pub kinematics: Option<Kinematics>,
    pub tools: Vec<Tool>,
}

/// Most post settings one machine keeps.
pub const MAX_POST_PROPERTIES: usize = 200;

impl Machine {
    pub fn to_json(&self, active: bool) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "kind": self.kind,
            "active": active,
            "travel": self.travel,
            "max_spindle_rpm": self.max_spindle_rpm,
            "max_feed": self.max_feed,
            "tool_capacity": self.tool_capacity,
            "post": self.post,
            "kinematics": self.kinematics.as_ref().map(Kinematics::to_json),
            "tools": self.tools.len(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.post_properties.len() > MAX_POST_PROPERTIES
            || self.post_properties.iter().any(|(k, v)| k.len() > 64 || v.chars().count() > 2048)
        {
            return Err("post settings: too many, or a name or value too long".into());
        }
        if !MACHINE_KINDS.contains(&self.kind.as_str()) {
            return Err(format!("unknown machine kind `{}`", self.kind));
        }
        match (&self.kinematics, self.kind.as_str()) {
            (Some(_), "mill_3axis") => return Err("a 3-axis mill has no rotary axes".into()),
            (None, "mill_4axis" | "mill_5axis") => return Err(format!("a {} needs its rotary axes", self.kind)),
            (Some(k), kind) => k.validate(kind)?,
            (None, _) => {}
        }
        if self.name.trim().is_empty() {
            return Err("`name` must not be empty".into());
        }
        if !self.travel.iter().all(|t| t.is_finite() && *t > 0.0 && *t <= 100_000.0) {
            return Err("travel must be above 0 and at most 100000 mm on each axis".into());
        }
        if !(self.max_spindle_rpm.is_finite() && self.max_spindle_rpm > 0.0 && self.max_spindle_rpm <= 500_000.0) {
            return Err("`max_spindle_rpm` must be above 0 and at most 500000".into());
        }
        if !(self.max_feed.is_finite() && self.max_feed > 0.0 && self.max_feed <= 1.0e6) {
            return Err("`max_feed` must be above 0 and at most 1000000 mm/min".into());
        }
        if self.tool_capacity > MAX_TOOL_NUMBER {
            return Err(format!("`tool_capacity` must be at most {MAX_TOOL_NUMBER}"));
        }
        Ok(())
    }

    /// A tool must also fit the machine: spindle speed and feeds within its limits.
    pub fn check_tool(&self, tool: &Tool) -> Result<(), String> {
        tool.validate()?;
        if tool.spindle_rpm > self.max_spindle_rpm {
            return Err(format!(
                "`spindle_rpm` {} is above {}'s maximum of {}",
                tool.spindle_rpm, self.name, self.max_spindle_rpm
            ));
        }
        for (name, feed) in [("feed", tool.feed), ("plunge_feed", tool.plunge_feed)] {
            if feed > self.max_feed {
                return Err(format!("`{name}` {feed} is above {}'s maximum feed of {}", self.name, self.max_feed));
            }
        }
        Ok(())
    }

    pub fn tool(&self, number: u32) -> Option<&Tool> {
        self.tools.iter().find(|t| t.number == number)
    }

    /// The lowest unused tool number.
    pub fn next_tool_number(&self) -> u32 {
        (1..=MAX_TOOL_NUMBER).find(|n| self.tool(*n).is_none()).unwrap_or(MAX_TOOL_NUMBER)
    }

    /// A generic machine to start from.
    pub fn generic(id: String) -> Self {
        Self {
            id,
            name: "Generic 3-axis mill".into(),
            description: String::new(),
            kind: "mill_3axis".into(),
            travel: [500.0, 400.0, 400.0],
            max_spindle_rpm: 24_000.0,
            max_feed: 10_000.0,
            tool_capacity: 0,
            post: String::new(),
            post_properties: std::collections::BTreeMap::new(),
            kinematics: None,
            tools: Vec::new(),
        }
    }
}

/// `<machine id>/tool.<number>`, as selections and the API name tools.
pub fn tool_ref(machine: &str, number: u32) -> String {
    format!("{machine}/tool.{number}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct MachineLibrary {
    pub machines: Vec<Machine>,
    /// Tool holders, shared by every machine's tools.
    pub holders: Vec<Holder>,
    pub(crate) next_holder: u64,
    /// The machine new tools go to and the Tooling tab shows.
    pub active: Option<String>,
    pub(crate) next_machine: u64,
    /// Saved toolpath settings to start new toolpaths from.
    pub templates: Vec<Template>,
    /// Bumped on every change, so the app knows to save.
    pub revision: u64,
}

/// Most toolpath templates kept.
pub const MAX_TEMPLATES: usize = 500;

/// A toolpath's settings saved under a name: the strategy's command and its parameters
/// (without geometry), to start new toolpaths from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub command: String,
    pub params: Value,
}

impl Default for MachineLibrary {
    /// One generic machine with an empty tool library.
    fn default() -> Self {
        Self {
            machines: vec![Machine::generic("machine.1".into())],
            holders: Vec::new(),
            next_holder: 1,
            active: Some("machine.1".into()),
            next_machine: 2,
            templates: Vec::new(),
            revision: 0,
        }
    }
}

impl MachineLibrary {
    pub fn machine(&self, id: &str) -> Option<&Machine> {
        self.machines.iter().find(|m| m.id == id)
    }

    pub(crate) fn machine_mut(&mut self, id: &str) -> Option<&mut Machine> {
        self.machines.iter_mut().find(|m| m.id == id)
    }

    pub fn holder(&self, id: &str) -> Option<&Holder> {
        self.holders.iter().find(|h| h.id == id)
    }

    /// The holder `tool` sits in, if any.
    pub fn holder_of(&self, tool: &Tool) -> Option<&Holder> {
        tool.holder.as_deref().and_then(|id| self.holder(id))
    }

    pub fn active_machine(&self) -> Option<&Machine> {
        self.active.as_deref().and_then(|id| self.machine(id))
    }

    pub(crate) fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// The library as it is saved.
    pub fn to_json(&self) -> Value {
        json!({
            "format": LIBRARY_FORMAT,
            "active": self.active,
            "next_machine": self.next_machine,
            "machines": self.machines,
            "holders": self.holders,
            "next_holder": self.next_holder,
            "templates": self.templates,
        })
    }

    /// Reads a saved library. Machines and tools that fail their checks are dropped and
    /// reported; a file that is not a library at all is an error.
    pub fn from_json(v: &Value) -> Result<(Self, Vec<String>), String> {
        if v.get("format").and_then(Value::as_str) != Some(LIBRARY_FORMAT) {
            return Err(format!("not a CosmicCAM library (expected format {LIBRARY_FORMAT})"));
        }
        let mut dropped = Vec::new();
        let mut machines: Vec<Machine> = Vec::new();
        for raw in v.get("machines").and_then(Value::as_array).into_iter().flatten().take(MAX_MACHINES) {
            let Ok(mut m) = serde_json::from_value::<Machine>(raw.clone()) else {
                dropped.push("a machine that could not be read".into());
                continue;
            };
            m.name = clip(&m.name);
            m.description = clip(&m.description);
            m.post = clip(&m.post);
            if let Err(e) = m.validate() {
                dropped.push(format!("machine {}: {e}", clip(&m.id)));
                continue;
            }
            if machines.iter().any(|o| o.id == m.id) || !valid_machine_id(&m.id) {
                dropped.push(format!("machine {}: duplicate or malformed id", clip(&m.id)));
                continue;
            }
            let mut tools: Vec<Tool> = Vec::new();
            for mut t in std::mem::take(&mut m.tools).into_iter().take(MAX_TOOLS) {
                t.clip_text();
                match t.validate() {
                    Ok(()) if tools.iter().all(|o| o.number != t.number) => tools.push(t),
                    Ok(()) => dropped.push(format!("{}: duplicate tool number T{}", m.name, t.number)),
                    Err(e) => dropped.push(format!("{} T{}: {e}", m.name, t.number)),
                }
            }
            m.tools = tools;
            machines.push(m);
        }
        if machines.is_empty() {
            return Err("the library has no usable machine".into());
        }
        let active = v
            .get("active")
            .and_then(Value::as_str)
            .filter(|id| machines.iter().any(|m| m.id == *id))
            .map(str::to_owned)
            .or_else(|| machines.first().map(|m| m.id.clone()));
        let highest =
            machines.iter().filter_map(|m| m.id.strip_prefix("machine.")?.parse::<u64>().ok()).max().unwrap_or(0);
        let next_machine = v.get("next_machine").and_then(Value::as_u64).unwrap_or(0).max(highest.saturating_add(1));
        let mut holders: Vec<Holder> = Vec::new();
        for raw in v.get("holders").and_then(Value::as_array).into_iter().flatten().take(crate::tooling::MAX_HOLDERS) {
            let Ok(mut h) = serde_json::from_value::<Holder>(raw.clone()) else {
                dropped.push("a holder that could not be read".into());
                continue;
            };
            h.name = clip(&h.name);
            h.vendor = clip(&h.vendor);
            h.product_id = clip(&h.product_id);
            match h.validate() {
                Ok(()) if holders.iter().all(|o| o.id != h.id) && valid_holder_id(&h.id) => holders.push(h),
                Ok(()) => dropped.push(format!("holder {}: duplicate or malformed id", clip(&h.id))),
                Err(e) => dropped.push(format!("holder {}: {e}", h.name)),
            }
        }
        // Tools keep only holders that exist.
        for m in &mut machines {
            for t in &mut m.tools {
                if t.holder.as_ref().is_some_and(|id| holders.iter().all(|h| &h.id != id)) {
                    t.holder = None;
                }
            }
        }
        let highest_holder =
            holders.iter().filter_map(|h| h.id.strip_prefix("holder.")?.parse::<u64>().ok()).max().unwrap_or(0);
        let next_holder =
            v.get("next_holder").and_then(Value::as_u64).unwrap_or(0).max(highest_holder.saturating_add(1));
        let mut templates: Vec<Template> = Vec::new();
        for raw in v.get("templates").and_then(Value::as_array).into_iter().flatten().take(MAX_TEMPLATES) {
            match serde_json::from_value::<Template>(raw.clone()) {
                Ok(mut t)
                    if t.command.starts_with("toolpath.")
                        && crate::cmd::find(&t.command).is_some()
                        && t.params.is_object()
                        && t.params.to_string().len() <= 64 * 1024 =>
                {
                    t.name = clip(&t.name);
                    if !t.name.is_empty() && templates.iter().all(|o| o.name != t.name) {
                        templates.push(t);
                    }
                }
                _ => dropped.push("a toolpath template that could not be read".into()),
            }
        }
        Ok((Self { machines, holders, next_holder, active, next_machine, templates, revision: 0 }, dropped))
    }
}

pub(crate) fn valid_holder_id(id: &str) -> bool {
    id.strip_prefix("holder.").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

fn valid_machine_id(id: &str) -> bool {
    id.strip_prefix("machine.").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Trims and caps free text.
pub fn clip(s: &str) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(MAX_TEXT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libraries_round_trip_and_bad_entries_are_dropped() {
        let mut lib = MachineLibrary::default();
        let mut tool = Tool {
            name: "Ø6 flat".into(),
            spindle_rpm: 12_000.0,
            feed: 1200.0,
            plunge_feed: 300.0,
            stepover: 2.0,
            stepdown: 6.0,
            ..Tool::typical(1, "flat", 6.0)
        };
        if let Some(m) = lib.machines.first_mut() {
            m.tools.push(tool.clone());
        }
        let (back, dropped) =
            MachineLibrary::from_json(&lib.to_json()).unwrap_or_else(|e| (MachineLibrary::default(), vec![e]));
        assert!(dropped.is_empty(), "{dropped:?}");
        assert_eq!(back.machines, lib.machines);

        // A tool with a NaN diameter and a duplicate number are dropped; the rest stays.
        let mut v = lib.to_json();
        tool.number = 2;
        let mut bad = serde_json::to_value(&tool).unwrap_or(Value::Null);
        bad["diameter"] = json!(-1);
        v["machines"][0]["tools"].as_array_mut().into_iter().for_each(|t| {
            t.push(bad.clone());
            t.push(serde_json::to_value(Tool { number: 1, ..tool.clone() }).unwrap_or(Value::Null));
        });
        let (back, dropped) = MachineLibrary::from_json(&v).unwrap_or_else(|e| (MachineLibrary::default(), vec![e]));
        assert_eq!(back.machines.first().map(|m| m.tools.len()), Some(1));
        assert_eq!(dropped.len(), 2, "{dropped:?}");

        assert!(MachineLibrary::from_json(&json!({"format": "other"})).is_err());
        assert!(MachineLibrary::from_json(&json!({"format": LIBRARY_FORMAT, "machines": []})).is_err());
        assert!(MachineLibrary::from_json(&json!(42)).is_err());
    }
}
