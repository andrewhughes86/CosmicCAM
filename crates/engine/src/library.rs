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

/// Tool shapes. Only flat end mills can be created so far; the rest are listed so the
/// ribbon, the API and saved files already know them.
pub const TOOL_TYPES: &[&str] =
    &["flat", "ball", "bull", "chamfer", "drill", "spot_drill", "face_mill", "tap", "thread_mill"];
/// Tool types that can be created now.
pub const SUPPORTED_TOOL_TYPES: &[&str] = &["flat"];
pub const TOOL_MATERIALS: &[&str] = &["carbide", "hss", "cobalt", "ceramic", "diamond"];
pub const MACHINE_KINDS: &[&str] = &["mill_3axis"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Tool number (T word); unique within its machine.
    pub number: u32,
    pub name: String,
    /// One of [`TOOL_TYPES`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Cutting diameter, mm.
    pub diameter: f64,
    /// Corner radius, mm (0 for a flat end mill).
    pub corner_radius: f64,
    pub flutes: u32,
    /// Length of cut, mm.
    pub flute_length: f64,
    /// Stick-out from the holder face to the tip, mm.
    pub overall_length: f64,
    pub shank_diameter: f64,
    /// One of [`TOOL_MATERIALS`].
    pub material: String,
    pub coating: String,
    /// Cutting data; 0 means not set.
    pub spindle_rpm: f64,
    /// Cutting feed, mm/min.
    pub feed: f64,
    /// Plunge feed, mm/min.
    pub plunge_feed: f64,
    /// Radial depth of cut, mm.
    pub stepover: f64,
    /// Axial depth of cut, mm.
    pub stepdown: f64,
    /// Length and diameter offset registers on the controller.
    pub length_offset: u32,
    pub diameter_offset: u32,
    pub notes: String,
}

impl Tool {
    pub fn to_json(&self, machine: &str) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(m) = v.as_object_mut() {
            m.insert("id".into(), json!(tool_ref(machine, self.number)));
            m.insert("machine".into(), json!(machine));
        }
        v
    }

    /// Checks every value; returns what is wrong with the first bad one.
    pub fn validate(&self) -> Result<(), String> {
        let positive = |name: &str, v: f64, max: f64| {
            if v.is_finite() && v > 0.0 && v <= max {
                Ok(())
            } else {
                Err(format!("`{name}` must be above 0 and at most {max}"))
            }
        };
        let non_negative = |name: &str, v: f64, max: f64| {
            if v.is_finite() && (0.0..=max).contains(&v) {
                Ok(())
            } else {
                Err(format!("`{name}` must be between 0 and {max}"))
            }
        };
        if !(1..=MAX_TOOL_NUMBER).contains(&self.number) {
            return Err(format!("`number` must be between 1 and {MAX_TOOL_NUMBER}"));
        }
        if !TOOL_TYPES.contains(&self.kind.as_str()) {
            return Err(format!("unknown tool type `{}`", self.kind));
        }
        if !TOOL_MATERIALS.contains(&self.material.as_str()) {
            return Err(format!("unknown material `{}`", self.material));
        }
        positive("diameter", self.diameter, 1000.0)?;
        non_negative("corner_radius", self.corner_radius, self.diameter / 2.0)?;
        if !(1..=32).contains(&self.flutes) {
            return Err("`flutes` must be between 1 and 32".into());
        }
        positive("flute_length", self.flute_length, 2000.0)?;
        positive("overall_length", self.overall_length, 2000.0)?;
        positive("shank_diameter", self.shank_diameter, 1000.0)?;
        if self.overall_length < self.flute_length {
            return Err("`overall_length` must be at least `flute_length`".into());
        }
        non_negative("spindle_rpm", self.spindle_rpm, 500_000.0)?;
        non_negative("feed", self.feed, 1.0e6)?;
        non_negative("plunge_feed", self.plunge_feed, 1.0e6)?;
        non_negative("stepover", self.stepover, self.diameter)?;
        non_negative("stepdown", self.stepdown, self.flute_length)?;
        if self.length_offset > MAX_TOOL_NUMBER || self.diameter_offset > MAX_TOOL_NUMBER {
            return Err(format!("offset registers must be at most {MAX_TOOL_NUMBER}"));
        }
        Ok(())
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
    /// The Python post-processor module, once posts exist (Phase 8).
    pub post: String,
    pub tools: Vec<Tool>,
}

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
            "tools": self.tools.len(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if !MACHINE_KINDS.contains(&self.kind.as_str()) {
            return Err(format!("unknown machine kind `{}`", self.kind));
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
    /// The machine new tools go to and the Tooling tab shows.
    pub active: Option<String>,
    pub(crate) next_machine: u64,
    /// Bumped on every change, so the app knows to save.
    pub revision: u64,
}

impl Default for MachineLibrary {
    /// One generic machine with an empty tool library.
    fn default() -> Self {
        Self {
            machines: vec![Machine::generic("machine.1".into())],
            active: Some("machine.1".into()),
            next_machine: 2,
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
                t.name = clip(&t.name);
                t.coating = clip(&t.coating);
                t.notes = clip(&t.notes);
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
        Ok((Self { machines, active, next_machine, revision: 0 }, dropped))
    }
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
            number: 1,
            name: "Ø6 flat".into(),
            kind: "flat".into(),
            diameter: 6.0,
            corner_radius: 0.0,
            flutes: 3,
            flute_length: 18.0,
            overall_length: 50.0,
            shank_diameter: 6.0,
            material: "carbide".into(),
            coating: String::new(),
            spindle_rpm: 12_000.0,
            feed: 1200.0,
            plunge_feed: 300.0,
            stepover: 2.0,
            stepdown: 6.0,
            length_offset: 1,
            diameter_offset: 1,
            notes: String::new(),
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
