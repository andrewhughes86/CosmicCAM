//! Cutting tools: every common milling and holemaking tool type with the geometry that defines
//! it, cutting data presets, and tool holders.
//!
//! - A [`Tool`] keeps the common geometry (cutting diameter, flutes, flute length, stick-out,
//!   shank) and the values only some types use (corner radius, point angle, tip diameter, pilot
//!   length, taper, neck and reach, thread pitch and hand). [`ToolType::fields`] says which.
//! - [`Tool::profile`] is the tool's outline as a solid of revolution, from the tip up: the
//!   preview, the simulation's drawing and the multi-axis collision check all use it, so what is
//!   shown is what is checked.
//! - [`Preset`]s keep cutting data per workpiece material; speeds and feeds follow from surface
//!   speed and chip load when those are given.
//! - [`Holder`]s are stacks of cones and cylinders above the holder face.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::library::{MAX_TOOL_NUMBER, clip, tool_ref};

/// Most presets one tool keeps.
pub const MAX_PRESETS: usize = 32;
/// Most holders a library keeps, and most segments one holder has.
pub const MAX_HOLDERS: usize = 500;
pub const MAX_HOLDER_SEGMENTS: usize = 12;

pub const TOOL_MATERIALS: &[&str] = &["carbide", "hss", "cobalt", "ceramic", "diamond", "cbn"];
pub const HANDS: &[&str] = &["right", "left"];
/// Workpiece materials presets are kept for.
pub const WORKPIECES: &[&str] = &[
    "aluminium",
    "steel",
    "stainless",
    "cast_iron",
    "titanium",
    "brass",
    "copper",
    "plastic",
    "wood",
    "composite",
    "other",
];

/// A tool type: its id, how people call it, and the type-specific values it uses.
pub struct ToolType {
    pub id: &'static str,
    pub label: &'static str,
    /// Of `corner_radius`, `tip_angle`, `tip_diameter`, `tip_length`, `taper_angle`,
    /// `neck_diameter`, `shoulder_length`, `thread_pitch`, `hand`.
    pub fields: &'static [&'static str],
}

const NECK: &[&str] = &["neck_diameter", "shoulder_length"];

/// Every tool type, in the order the library and menus show them.
pub const TYPES: &[ToolType] = &[
    ToolType { id: "flat", label: "Flat end mill", fields: NECK },
    ToolType { id: "ball", label: "Ball end mill", fields: NECK },
    ToolType {
        id: "bull",
        label: "Bull nose end mill",
        fields: &["corner_radius", "neck_diameter", "shoulder_length"],
    },
    ToolType {
        id: "tapered_ball",
        label: "Tapered ball mill",
        fields: &["taper_angle", "neck_diameter", "shoulder_length"],
    },
    ToolType { id: "lollipop", label: "Lollipop cutter", fields: NECK },
    ToolType { id: "chamfer", label: "Chamfer mill", fields: &["tip_angle", "tip_diameter"] },
    ToolType { id: "engraving", label: "Engraving tool", fields: &["tip_angle", "tip_diameter"] },
    ToolType {
        id: "dovetail",
        label: "Dovetail cutter",
        fields: &["taper_angle", "corner_radius", "neck_diameter", "shoulder_length"],
    },
    ToolType { id: "slot_mill", label: "Slot mill", fields: &["corner_radius", "neck_diameter", "shoulder_length"] },
    ToolType { id: "radius_mill", label: "Corner rounding mill", fields: &["corner_radius", "tip_diameter"] },
    ToolType { id: "face_mill", label: "Face mill", fields: &["corner_radius", "tip_angle"] },
    ToolType {
        id: "thread_mill",
        label: "Thread mill",
        fields: &["thread_pitch", "hand", "neck_diameter", "shoulder_length"],
    },
    ToolType { id: "drill", label: "Drill", fields: &["tip_angle"] },
    ToolType { id: "spot_drill", label: "Spot drill", fields: &["tip_angle"] },
    ToolType { id: "center_drill", label: "Centre drill", fields: &["tip_angle", "tip_diameter", "tip_length"] },
    ToolType { id: "reamer", label: "Reamer", fields: &[] },
    ToolType { id: "tap", label: "Tap", fields: &["thread_pitch", "hand"] },
    ToolType { id: "boring_bar", label: "Boring bar", fields: &["corner_radius", "neck_diameter", "shoulder_length"] },
    ToolType { id: "counterbore", label: "Counterbore", fields: &["tip_diameter", "tip_length"] },
    ToolType { id: "countersink", label: "Countersink", fields: &["tip_angle", "tip_diameter"] },
];

/// The type ids, for parameter choices.
pub const TOOL_TYPES: &[&str] = &[
    "flat",
    "ball",
    "bull",
    "tapered_ball",
    "lollipop",
    "chamfer",
    "engraving",
    "dovetail",
    "slot_mill",
    "radius_mill",
    "face_mill",
    "thread_mill",
    "drill",
    "spot_drill",
    "center_drill",
    "reamer",
    "tap",
    "boring_bar",
    "counterbore",
    "countersink",
];

pub fn tool_type(id: &str) -> Option<&'static ToolType> {
    TYPES.iter().find(|t| t.id == id)
}

/// Types whose end is a ball (3D finishing: the ball drop cutter applies).
pub const BALL_ENDED: &[&str] = &["ball", "tapered_ball", "lollipop"];
/// Types that cut a flat floor with their end (2D milling).
pub const FLAT_ENDED: &[&str] = &["flat", "bull", "face_mill", "slot_mill"];
/// Types that make holes along their axis (drilling cycles).
pub const HOLEMAKING: &[&str] =
    &["drill", "spot_drill", "center_drill", "reamer", "tap", "boring_bar", "counterbore", "countersink", "flat"];

/// Cutting data for one job: a workpiece material and how hard to push the tool in it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    /// One of [`WORKPIECES`].
    pub workpiece: String,
    /// Cutting speed at the tool's diameter, m/min (0: not given).
    pub surface_speed: f64,
    /// Chip load, mm per tooth (0: not given).
    pub feed_per_tooth: f64,
    pub spindle_rpm: f64,
    /// mm/min.
    pub feed: f64,
    pub plunge_feed: f64,
    pub ramp_feed: f64,
    pub stepover: f64,
    pub stepdown: f64,
    /// flood, mist, air, through_tool or off; empty for the toolpath's choice.
    pub coolant: String,
}

impl Preset {
    fn validate(&self, tool: &Tool) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a preset needs a name".into());
        }
        if !self.workpiece.is_empty() && !WORKPIECES.contains(&self.workpiece.as_str()) {
            return Err(format!("preset {}: unknown workpiece material `{}`", self.name, self.workpiece));
        }
        let ok = |v: f64, max: f64| v.is_finite() && (0.0..=max).contains(&v);
        let checks = [
            ("surface_speed", self.surface_speed, 10_000.0),
            ("feed_per_tooth", self.feed_per_tooth, 10.0),
            ("spindle_rpm", self.spindle_rpm, 500_000.0),
            ("feed", self.feed, 1.0e6),
            ("plunge_feed", self.plunge_feed, 1.0e6),
            ("ramp_feed", self.ramp_feed, 1.0e6),
            ("stepover", self.stepover, tool.diameter.max(1.0e-6)),
            ("stepdown", self.stepdown, tool.flute_length.max(1.0e-6)),
        ];
        for (name, v, max) in checks {
            if !ok(v, max) {
                return Err(format!("preset {}: `{name}` must be between 0 and {max}", self.name));
            }
        }
        if !["", "flood", "mist", "air", "through_tool", "off"].contains(&self.coolant.as_str()) {
            return Err(format!("preset {}: unknown coolant `{}`", self.name, self.coolant));
        }
        Ok(())
    }

    /// Fills spindle speed and feeds from surface speed and chip load where they are missing.
    pub fn derive(&mut self, diameter: f64, flutes: u32) {
        if self.spindle_rpm <= 0.0 && self.surface_speed > 0.0 && diameter > 0.0 {
            self.spindle_rpm = (self.surface_speed * 1000.0 / (std::f64::consts::PI * diameter)).round();
        }
        if self.feed <= 0.0 && self.feed_per_tooth > 0.0 && self.spindle_rpm > 0.0 {
            self.feed = (self.spindle_rpm * self.feed_per_tooth * f64::from(flutes.max(1))).round();
        }
        if self.plunge_feed <= 0.0 && self.feed > 0.0 {
            self.plunge_feed = (self.feed / 3.0).round();
        }
        if self.ramp_feed <= 0.0 && self.feed > 0.0 {
            self.ramp_feed = (self.feed / 2.0).round();
        }
    }
}

/// One stage of a holder: a cone (or cylinder) from `lower_diameter` at its bottom to
/// `upper_diameter` at its top, mm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HolderSegment {
    pub lower_diameter: f64,
    pub upper_diameter: f64,
    pub height: f64,
}

/// A tool holder: segments stacked upwards from the holder face (where the tool comes out).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    /// `holder.<n>`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub product_id: String,
    pub segments: Vec<HolderSegment>,
}

impl Holder {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a holder needs a name".into());
        }
        if self.segments.is_empty() || self.segments.len() > MAX_HOLDER_SEGMENTS {
            return Err(format!("a holder has 1 to {MAX_HOLDER_SEGMENTS} segments"));
        }
        for (i, s) in self.segments.iter().enumerate() {
            let ok = |v: f64| v.is_finite() && v > 0.0 && v <= 1000.0;
            if !(ok(s.lower_diameter) && ok(s.upper_diameter) && ok(s.height)) {
                return Err(format!(
                    "holder segment {}: diameters and height must be above 0 and at most 1000 mm",
                    i + 1
                ));
            }
        }
        Ok(())
    }

    /// Total height, mm.
    pub fn height(&self) -> f64 {
        self.segments.iter().map(|s| s.height).sum()
    }

    /// The outline as (radius, height above the holder face) points, bottom to top.
    pub fn profile(&self) -> Vec<[f64; 2]> {
        let mut out = Vec::with_capacity(self.segments.len() * 2 + 2);
        let mut h = 0.0;
        out.push([0.0, 0.0]);
        for s in &self.segments {
            out.push([s.lower_diameter / 2.0, h]);
            h += s.height;
            out.push([s.upper_diameter / 2.0, h]);
        }
        out.push([0.0, h]);
        out
    }

    pub fn to_json(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(m) = v.as_object_mut() {
            m.insert("height".into(), json!(self.height()));
        }
        v
    }
}

fn default_hand() -> String {
    "right".into()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Tool number (T word); unique within its machine.
    pub number: u32,
    pub name: String,
    /// One of [`TOOL_TYPES`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Cutting diameter, mm: the largest diameter the cutting edges reach (a ball's or tapered
    /// ball's ball diameter; a boring bar's bore diameter).
    pub diameter: f64,
    /// Corner radius, mm (bull nose, face mill and slot mill inserts, dovetail; a corner
    /// rounding mill's form radius; a ball's is half its diameter).
    pub corner_radius: f64,
    pub flutes: u32,
    /// Length of cut, mm.
    pub flute_length: f64,
    /// Stick-out from the holder face to the tip, mm.
    pub overall_length: f64,
    pub shank_diameter: f64,
    /// Included point angle (drills, spot and centre drills, chamfer mills, countersinks,
    /// engraving tools) or a face mill's cutting edge angle, degrees; 0 when it has none.
    #[serde(default)]
    pub tip_angle: f64,
    /// Flat at the tip (chamfer mills, engraving tools, countersinks) or the pilot diameter
    /// (centre drills, counterbores, corner rounding mills), mm.
    #[serde(default)]
    pub tip_diameter: f64,
    /// Pilot length (centre drills, counterbores), mm.
    #[serde(default)]
    pub tip_length: f64,
    /// Side angle of a tapered tool, or a dovetail's angle from the axis, degrees.
    #[serde(default)]
    pub taper_angle: f64,
    /// A reduced neck behind the flutes, mm (0: none).
    #[serde(default)]
    pub neck_diameter: f64,
    /// Reach: from the tip to where the shank starts, mm (0: the flute length).
    #[serde(default)]
    pub shoulder_length: f64,
    /// Thread pitch, mm (taps, thread mills).
    #[serde(default)]
    pub thread_pitch: f64,
    /// `right` or `left` hand.
    #[serde(default = "default_hand")]
    pub hand: String,
    #[serde(default)]
    pub coolant_through: bool,
    /// One of [`TOOL_MATERIALS`].
    pub material: String,
    pub coating: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub product_id: String,
    /// The holder it sits in (`holder.<n>`), if set.
    #[serde(default)]
    pub holder: Option<String>,
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
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// Length and diameter offset registers on the controller.
    pub length_offset: u32,
    pub diameter_offset: u32,
    pub notes: String,
}

/// A tool's outline: (radius, height above the tip) points from the tip, round the cutting end,
/// up the side and across the top, and where its flutes end.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub points: Vec<[f64; 2]>,
    pub flute_top: f64,
}

/// Points along a circular arc centred at `c` from angle `a0` to `a1` (radians), in (r, h).
fn arc(c: [f64; 2], radius: f64, a0: f64, a1: f64, out: &mut Vec<[f64; 2]>) {
    const STEPS: usize = 16;
    for k in 0..=STEPS {
        let a = a0 + (a1 - a0) * k as f64 / STEPS as f64;
        out.push([c[0] + radius * a.cos(), c[1] + radius * a.sin()]);
    }
}

fn tan_half(angle_deg: f64) -> f64 {
    (angle_deg.clamp(1.0, 179.0).to_radians() / 2.0).tan()
}

impl Tool {
    /// A tool of `kind` and `diameter` with typical geometry for it (millimetres).
    pub fn typical(number: u32, kind: &str, diameter: f64) -> Self {
        let d = diameter;
        let micron = |v: f64| (v * 1000.0).round() / 1000.0;
        let r = d / 2.0;
        let (mut tip_angle, mut tip_diameter, mut tip_length, mut taper_angle) = (0.0, 0.0, 0.0, 0.0);
        let (mut corner_radius, mut neck_diameter, mut thread_pitch) = (0.0, 0.0, 0.0);
        let (mut flutes, mut material) = (if d < 3.0 { 2 } else { 3 }, "carbide");
        let mut shank = d;
        let flute_length = match kind {
            "ball" => {
                corner_radius = r;
                3.0 * d
            }
            "bull" => {
                corner_radius = micron((d * 0.1).max(0.2).min(r));
                flutes = 4;
                3.0 * d
            }
            "tapered_ball" => {
                corner_radius = r;
                taper_angle = 3.0;
                shank = micron((d * 2.0).max(3.0));
                6.0 * d
            }
            "lollipop" => {
                corner_radius = r;
                neck_diameter = micron(d * 0.6);
                flutes = 4;
                d
            }
            "chamfer" | "engraving" | "countersink" => {
                tip_angle = if kind == "engraving" { 60.0 } else { 90.0 };
                tip_diameter = if kind == "engraving" { micron((d * 0.05).max(0.1)) } else { 0.0 };
                flutes = match kind {
                    "engraving" => 1,
                    "countersink" => 3,
                    _ => 4,
                };
                if kind == "countersink" {
                    material = "hss";
                }
                micron((r - tip_diameter / 2.0) / tan_half(tip_angle))
            }
            "dovetail" => {
                taper_angle = 45.0;
                corner_radius = micron((d * 0.02).max(0.1));
                neck_diameter = micron(d * 0.45);
                flutes = 6;
                micron((r - neck_diameter / 2.0) / 45_f64.to_radians().tan())
            }
            "slot_mill" => {
                neck_diameter = micron(d * 0.4);
                flutes = 6;
                micron((d * 0.15).max(1.0))
            }
            "radius_mill" => {
                corner_radius = micron(d / 4.0);
                tip_diameter = micron(d - 2.0 * corner_radius);
                flutes = 4;
                micron(corner_radius * 1.5)
            }
            "face_mill" => {
                corner_radius = (0.8_f64).min(r * 0.5);
                tip_angle = 90.0;
                flutes = ((d / 12.0).round() as u32).clamp(3, 16);
                shank = micron((d * 0.4).max(16.0).min(d));
                micron((d * 0.12).max(6.0))
            }
            "thread_mill" => {
                thread_pitch = micron((d * 0.15).max(0.25));
                flutes = 3;
                2.0 * d
            }
            "drill" => {
                tip_angle = if d >= 3.0 { 140.0 } else { 118.0 };
                flutes = 2;
                5.0 * d
            }
            "spot_drill" => {
                tip_angle = 90.0;
                flutes = 2;
                1.5 * d
            }
            "center_drill" => {
                tip_angle = 60.0;
                tip_diameter = micron(d * 0.4);
                tip_length = micron(tip_diameter * 1.2);
                flutes = 2;
                material = "hss";
                micron(tip_length + (r - tip_diameter / 2.0) / tan_half(tip_angle) + d * 0.2)
            }
            "reamer" => {
                flutes = 6;
                3.0 * d
            }
            "tap" => {
                // ISO coarse pitch, near enough for a starting value.
                thread_pitch = match d {
                    x if x <= 3.0 => 0.5,
                    x if x <= 4.0 => 0.7,
                    x if x <= 5.0 => 0.8,
                    x if x <= 6.0 => 1.0,
                    x if x <= 8.0 => 1.25,
                    x if x <= 10.0 => 1.5,
                    x if x <= 12.0 => 1.75,
                    x if x <= 16.0 => 2.0,
                    _ => 2.5,
                };
                flutes = 3;
                material = "hss";
                shank = micron(d * 0.8);
                2.5 * d
            }
            "boring_bar" => {
                corner_radius = (0.4_f64).min(r * 0.5);
                neck_diameter = micron(d * 0.75);
                flutes = 1;
                shank = micron(d * 0.75);
                micron((d * 0.2).max(1.0))
            }
            "counterbore" => {
                tip_diameter = micron(d * 0.55);
                tip_length = micron(tip_diameter);
                flutes = 3;
                material = "hss";
                2.0 * d
            }
            _ => 3.0 * d,
        };
        let flute_length = micron(flute_length.max(0.1));
        // A neck needs length: the reach goes past the flutes.
        let shoulder_length = if neck_diameter > 0.0 { micron(flute_length + (1.5 * d).max(5.0)) } else { 0.0 };
        // Insert cutters sit close to their arbor; solid tools stick out a couple of diameters.
        let stick_out = if kind == "face_mill" { (d * 0.4).max(30.0) } else { (2.0 * d).max(15.0) };
        let overall_length = micron(flute_length.max(shoulder_length) + stick_out);
        Self {
            number,
            name: String::new(),
            kind: kind.into(),
            diameter: d,
            corner_radius,
            flutes,
            flute_length,
            overall_length,
            shank_diameter: shank,
            tip_angle,
            tip_diameter,
            tip_length,
            taper_angle,
            neck_diameter,
            shoulder_length,
            thread_pitch,
            hand: "right".into(),
            coolant_through: false,
            material: material.into(),
            coating: String::new(),
            vendor: String::new(),
            product_id: String::new(),
            holder: None,
            spindle_rpm: 0.0,
            feed: 0.0,
            plunge_feed: 0.0,
            stepover: 0.0,
            stepdown: 0.0,
            presets: Vec::new(),
            length_offset: number,
            diameter_offset: number,
            notes: String::new(),
        }
    }

    /// The type's name, e.g. "Bull nose end mill".
    pub fn type_label(&self) -> &'static str {
        tool_type(&self.kind).map_or("Tool", |t| t.label)
    }

    /// True for tools whose end is a ball (ball, tapered ball, lollipop).
    pub fn ball_ended(&self) -> bool {
        BALL_ENDED.contains(&self.kind.as_str())
    }

    /// The radius of the ball at the end of a ball-ended tool, else of the cutting end.
    pub fn tip_radius(&self) -> f64 {
        self.diameter / 2.0
    }

    /// Where the shank starts, above the tip.
    pub fn reach(&self) -> f64 {
        if self.shoulder_length > 0.0 { self.shoulder_length } else { self.flute_length }
    }

    /// The tool's outline, from the tip up to the holder face (see [`Profile`]).
    pub fn profile(&self) -> Profile {
        let r = self.diameter / 2.0;
        let f = self.flute_length;
        let rc = self.corner_radius.clamp(0.0, r);
        let rt = self.tip_diameter / 2.0;
        let mut p: Vec<[f64; 2]> = vec![[0.0, 0.0]];
        // The cutting end, up to the top of the flutes; `top_r` is its radius there.
        let top_r = match self.kind.as_str() {
            "ball" => {
                arc([0.0, r], r, -std::f64::consts::FRAC_PI_2, 0.0, &mut p);
                p.push([r, f.max(r)]);
                r
            }
            "bull" | "face_mill" => {
                p.push([r - rc, 0.0]);
                arc([r - rc, rc], rc, -std::f64::consts::FRAC_PI_2, 0.0, &mut p);
                if self.kind == "face_mill" && self.tip_angle > 0.0 && self.tip_angle < 89.0 {
                    // A lead angle: the edge flares out to the body.
                    p.push([r + (f * 0.5) / self.tip_angle.to_radians().tan(), f * 0.5]);
                    p.push([r + (f * 0.5) / self.tip_angle.to_radians().tan(), f]);
                    return self.finish(p, f, r);
                }
                p.push([r, f]);
                r
            }
            "tapered_ball" => {
                let a = self.taper_angle.clamp(0.0, 30.0).to_radians();
                // The ball, up to where the taper leaves it at a tangent.
                arc([0.0, r], r, -std::f64::consts::FRAC_PI_2, -a, &mut p);
                let (rt0, ht0) = (r * a.cos(), r - r * a.sin());
                let top = rt0 + (f - ht0).max(0.0) * a.tan();
                p.push([top, f.max(ht0)]);
                top
            }
            "lollipop" => {
                let n = self.neck_diameter.clamp(0.0, self.diameter) / 2.0;
                // Round the sphere until it meets the neck.
                let meet = (n / r).clamp(-1.0, 1.0).acos();
                // Through the equator, where the ball is widest.
                arc([0.0, r], r, -std::f64::consts::FRAC_PI_2, 0.0, &mut p);
                arc([0.0, r], r, 0.0, meet, &mut p);
                n
            }
            "chamfer" | "engraving" | "countersink" | "drill" | "spot_drill" => {
                let flat = if matches!(self.kind.as_str(), "drill" | "spot_drill") { 0.0 } else { rt };
                if flat > 0.0 {
                    p.push([flat, 0.0]);
                }
                let cone = (r - flat) / tan_half(self.tip_angle);
                if f >= cone - 1.0e-3 {
                    // The whole cone (a rounding hair short counts as whole), then the side.
                    p.push([r, cone.min(f)]);
                    if f > cone {
                        p.push([r, f]);
                    }
                    r
                } else {
                    // Flutes shorter than the cone: the edge stops partway up it.
                    let top = flat + f * tan_half(self.tip_angle);
                    p.push([top, f]);
                    top
                }
            }
            "center_drill" => {
                let pilot_point = rt / tan_half(118.0);
                p.push([rt, pilot_point]);
                p.push([rt, self.tip_length.max(pilot_point)]);
                let cone = (r - rt) / tan_half(self.tip_angle);
                p.push([r, self.tip_length.max(pilot_point) + cone]);
                p.push([r, f.max(self.tip_length + cone)]);
                r
            }
            "counterbore" => {
                p.push([rt, 0.0]);
                p.push([rt, self.tip_length]);
                p.push([r, self.tip_length]);
                p.push([r, f]);
                r
            }
            "radius_mill" => {
                // The pilot, then the concave form radius out to the full diameter.
                p.push([rt, 0.0]);
                arc([rt + rc, 0.0], rc, std::f64::consts::PI, std::f64::consts::FRAC_PI_2, &mut p);
                p.push([r, rc]);
                p.push([r, f.max(rc)]);
                r
            }
            "dovetail" => {
                p.push([r - rc, 0.0]);
                arc([r - rc, rc], rc, -std::f64::consts::FRAC_PI_2, 0.0, &mut p);
                let top = (r - f * self.taper_angle.clamp(0.0, 80.0).to_radians().tan()).max(0.0);
                p.push([top, f]);
                top
            }
            "slot_mill" => {
                p.push([r - rc, 0.0]);
                if rc > 0.0 {
                    arc([r - rc, rc], rc, -std::f64::consts::FRAC_PI_2, 0.0, &mut p);
                    arc([r - rc, f - rc], rc, 0.0, std::f64::consts::FRAC_PI_2, &mut p);
                } else {
                    p.push([r, 0.0]);
                    p.push([r, f]);
                }
                p.push([r - rc, f]);
                r - rc
            }
            "tap" => {
                // A lead chamfer over two pitches, then the thread (its crests and roots).
                let pitch = self.thread_pitch.max(0.05);
                let depth = (pitch * 0.6).min(r * 0.3);
                p.push([r - depth, 0.0]);
                p.push([r, 2.0 * pitch]);
                let mut h = 2.0 * pitch;
                let mut crest = true;
                while h + pitch / 2.0 < f && p.len() < 400 {
                    h += pitch / 2.0;
                    p.push([if crest { r - depth } else { r }, h]);
                    crest = !crest;
                }
                p.push([r, f]);
                r
            }
            "thread_mill" => {
                let pitch = self.thread_pitch.max(0.05);
                let depth = (pitch * 0.6).min(r * 0.3);
                p.push([r - depth, 0.0]);
                let mut h = 0.0;
                let mut crest = true;
                while h + pitch / 2.0 < f && p.len() < 400 {
                    h += pitch / 2.0;
                    p.push([if crest { r } else { r - depth }, h]);
                    crest = !crest;
                }
                p.push([r - depth, f]);
                r - depth
            }
            "reamer" => {
                let lead = (r * 0.1).min(f * 0.2);
                p.push([r - lead, 0.0]);
                p.push([r, lead]);
                p.push([r, f]);
                r
            }
            "boring_bar" => {
                // The insert at the full bore radius, then the bar.
                p.push([r - rc, 0.0]);
                arc([r - rc, rc], rc, -std::f64::consts::FRAC_PI_2, 0.0, &mut p);
                p.push([r, f]);
                r
            }
            _ => {
                p.push([r, 0.0]);
                p.push([r, f]);
                r
            }
        };
        self.finish(p, f, top_r)
    }

    /// The neck and shank above the flutes, up to the holder face.
    fn finish(&self, mut p: Vec<[f64; 2]>, flute_top: f64, top_r: f64) -> Profile {
        let l = self.overall_length.max(flute_top);
        let shank = self.shank_diameter / 2.0;
        let reach = self.reach().clamp(flute_top, l);
        let neck = self.neck_diameter / 2.0;
        let at = p.last().map_or(flute_top, |q| q[1]);
        if neck > 0.0 && reach > at + 1.0e-9 {
            p.push([neck.min(top_r.max(neck)), at]);
            p.push([neck, reach]);
            p.push([shank, reach]);
        } else if neck > 0.0 {
            p.push([neck, at]);
            p.push([neck, reach.max(at)]);
            p.push([shank, reach.max(at)]);
        } else {
            if reach > at + 1.0e-9 {
                p.push([top_r, reach]);
            }
            p.push([shank, reach.max(at)]);
        }
        p.push([shank, l]);
        p.push([0.0, l]);
        Profile { points: p, flute_top }
    }

    pub fn to_json(&self, machine: &str) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(m) = v.as_object_mut() {
            m.insert("id".into(), json!(tool_ref(machine, self.number)));
            m.insert("machine".into(), json!(machine));
            m.insert("type_label".into(), json!(self.type_label()));
        }
        v
    }

    /// Caps free text after reading a tool from a file.
    pub fn clip_text(&mut self) {
        self.name = clip(&self.name);
        self.coating = clip(&self.coating);
        self.notes = clip(&self.notes);
        self.vendor = clip(&self.vendor);
        self.product_id = clip(&self.product_id);
        for p in &mut self.presets {
            p.name = clip(&p.name);
        }
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
        let Some(ty) = tool_type(&self.kind) else {
            return Err(format!("unknown tool type `{}`", self.kind));
        };
        if !TOOL_MATERIALS.contains(&self.material.as_str()) {
            return Err(format!("unknown material `{}`", self.material));
        }
        if !HANDS.contains(&self.hand.as_str()) {
            return Err("`hand` must be right or left".into());
        }
        positive("diameter", self.diameter, 1000.0)?;
        let r = self.diameter / 2.0;
        non_negative("corner_radius", self.corner_radius, r)?;
        if !(1..=64).contains(&self.flutes) {
            return Err("`flutes` must be between 1 and 64".into());
        }
        positive("flute_length", self.flute_length, 2000.0)?;
        positive("overall_length", self.overall_length, 2000.0)?;
        positive("shank_diameter", self.shank_diameter, 1000.0)?;
        if self.overall_length < self.flute_length {
            return Err("`overall_length` must be at least `flute_length`".into());
        }
        non_negative("tip_angle", self.tip_angle, 179.0)?;
        non_negative("tip_diameter", self.tip_diameter, self.diameter)?;
        non_negative("tip_length", self.tip_length, self.flute_length)?;
        non_negative("taper_angle", self.taper_angle, 80.0)?;
        non_negative("neck_diameter", self.neck_diameter, 1000.0)?;
        non_negative("shoulder_length", self.shoulder_length, self.overall_length)?;
        non_negative("thread_pitch", self.thread_pitch, 50.0)?;
        if self.shoulder_length > 0.0 && self.shoulder_length < self.flute_length {
            return Err("`shoulder_length` (reach) must be at least the flute length".into());
        }
        let uses = |f: &str| ty.fields.contains(&f);
        if uses("tip_angle") && (self.tip_angle < 1.0 || self.tip_angle.is_nan()) {
            return Err(format!("a {} needs its `tip_angle` (1 to 179°)", ty.label.to_lowercase()));
        }
        if uses("thread_pitch") && self.thread_pitch <= 0.0 {
            return Err(format!("a {} needs its `thread_pitch`", ty.label.to_lowercase()));
        }
        match self.kind.as_str() {
            "ball" | "tapered_ball" | "lollipop" if (self.corner_radius - r).abs() > 1.0e-6 => {
                return Err("a ball-ended tool's corner radius is half its diameter".into());
            }
            "lollipop" if !(self.neck_diameter > 0.0 && self.neck_diameter < self.diameter) => {
                return Err("a lollipop cutter's neck must be narrower than its ball".into());
            }
            "slot_mill" | "dovetail" if self.neck_diameter <= 0.0 || self.neck_diameter >= self.diameter => {
                return Err(format!("a {}'s neck must be narrower than its cutting diameter", ty.label.to_lowercase()));
            }
            "center_drill" | "counterbore" if !(self.tip_diameter > 0.0 && self.tip_diameter < self.diameter) => {
                return Err("the pilot (`tip_diameter`) must be narrower than the body".into());
            }
            "center_drill" | "counterbore" if self.tip_length <= 0.0 => {
                return Err("the pilot needs a length (`tip_length`)".into());
            }
            "radius_mill" if self.tip_diameter + 2.0 * self.corner_radius > self.diameter + 1.0e-6 => {
                return Err("a corner rounding mill's pilot plus twice its form radius must fit its diameter".into());
            }
            "radius_mill" if self.corner_radius <= 0.0 => {
                return Err("a corner rounding mill needs its form radius (`corner_radius`)".into());
            }
            "bull" if self.corner_radius <= 0.0 => {
                return Err("a bull nose end mill needs a corner radius".into());
            }
            "dovetail" if self.taper_angle <= 0.0 => {
                return Err("a dovetail cutter needs its angle (`taper_angle`)".into());
            }
            _ => {}
        }
        non_negative("spindle_rpm", self.spindle_rpm, 500_000.0)?;
        non_negative("feed", self.feed, 1.0e6)?;
        non_negative("plunge_feed", self.plunge_feed, 1.0e6)?;
        non_negative("stepover", self.stepover, self.diameter)?;
        non_negative("stepdown", self.stepdown, self.flute_length)?;
        if self.length_offset > MAX_TOOL_NUMBER || self.diameter_offset > MAX_TOOL_NUMBER {
            return Err(format!("offset registers must be at most {MAX_TOOL_NUMBER}"));
        }
        if self.presets.len() > MAX_PRESETS {
            return Err(format!("a tool keeps at most {MAX_PRESETS} presets"));
        }
        for (i, p) in self.presets.iter().enumerate() {
            p.validate(self)?;
            if self.presets.iter().take(i).any(|o| o.name == p.name) {
                return Err(format!("two presets are called {}", p.name));
            }
        }
        if let Some(h) = &self.holder
            && (h.is_empty() || h.len() > 64)
        {
            return Err("`holder` must name a holder".into());
        }
        Ok(())
    }

    /// The cutting data to use: the preset called `name`, else the tool's own.
    pub fn cutting(&self, preset: Option<&str>) -> Result<Preset, String> {
        match preset.filter(|p| !p.is_empty()) {
            Some(name) => self
                .presets
                .iter()
                .find(|p| p.name == name)
                .cloned()
                .ok_or_else(|| format!("T{} has no preset `{name}`", self.number)),
            None => Ok(Preset {
                name: String::new(),
                spindle_rpm: self.spindle_rpm,
                feed: self.feed,
                plunge_feed: self.plunge_feed,
                stepover: self.stepover,
                stepdown: self.stepdown,
                ..Preset::default()
            }),
        }
    }
}

/// The built-in sample library: two tools of each type with typical geometry and cutting data,
/// presets for aluminium and steel on the milling tools, and a few holders they sit in. Holder
/// ids are `holder.1`.. here; importing renumbers them.
pub fn sample_library() -> (Vec<Tool>, Vec<Holder>) {
    let seg =
        |lower: f64, upper: f64, height: f64| HolderSegment { lower_diameter: lower, upper_diameter: upper, height };
    let flange = seg(63.5, 63.5, 16.0);
    let holder = |n: u32, name: &str, segments: Vec<HolderSegment>| Holder {
        id: format!("holder.{n}"),
        name: name.into(),
        vendor: "Sample".into(),
        product_id: String::new(),
        segments,
    };
    let holders = vec![
        holder(1, "ER32 collet chuck, CAT40", vec![seg(50.0, 50.0, 22.0), seg(45.0, 45.0, 30.0), flange]),
        holder(2, "Shrink fit 6 mm slim, CAT40", vec![seg(16.0, 27.0, 60.0), seg(32.0, 32.0, 20.0), flange]),
        holder(3, "Shrink fit 12 mm, CAT40", vec![seg(24.0, 32.0, 50.0), seg(44.0, 44.0, 25.0), flange]),
        holder(4, "Side-lock holder 20 mm, CAT40", vec![seg(42.0, 42.0, 50.0), flange]),
        holder(5, "Shell mill arbor 22 mm, CAT40", vec![seg(40.0, 40.0, 30.0), flange]),
        holder(6, "Tapping chuck ER20, CAT40", vec![seg(34.0, 34.0, 30.0), seg(40.0, 40.0, 40.0), flange]),
    ];
    let preset = |name: &str, workpiece: &str, vc: f64, fz: f64, d: f64, z: u32, ae: f64, ap: f64| {
        let mut p = Preset {
            name: name.into(),
            workpiece: workpiece.into(),
            surface_speed: vc,
            feed_per_tooth: fz,
            stepover: ae,
            stepdown: ap,
            ..Preset::default()
        };
        p.derive(d, z);
        p
    };
    let mut tools = Vec::new();
    let mut add = |number: u32, kind: &str, d: f64, name: &str, holder: u32, edit: &dyn Fn(&mut Tool)| {
        let mut t = Tool::typical(number, kind, d);
        t.name = name.into();
        t.vendor = "Sample".into();
        t.product_id = format!("S-{number:03}");
        t.holder = Some(format!("holder.{holder}"));
        edit(&mut t);
        // Cutting data: an aluminium and a steel preset for milling tools, and the aluminium one
        // as the tool's own (within a 20000 rpm spindle).
        let milling = !HOLEMAKING.contains(&t.kind.as_str()) || t.kind == "flat";
        if milling && t.presets.is_empty() {
            let (ae, ap) = (t.diameter * 0.4, t.flute_length.min(t.diameter));
            t.presets.push(preset(
                "Aluminium",
                "aluminium",
                300.0,
                (t.diameter * 0.008).clamp(0.01, 0.15),
                t.diameter,
                t.flutes,
                ae,
                ap,
            ));
            t.presets.push(preset(
                "Steel",
                "steel",
                110.0,
                (t.diameter * 0.005).clamp(0.005, 0.1),
                t.diameter,
                t.flutes,
                ae * 0.5,
                ap * 0.5,
            ));
        }
        if let Some(p) = t.presets.first().cloned() {
            t.spindle_rpm = p.spindle_rpm.min(20_000.0);
            t.feed = (p.feed * t.spindle_rpm / p.spindle_rpm.max(1.0)).round().min(10_000.0);
            t.plunge_feed = (t.feed / 3.0).round();
            t.stepover = p.stepover.min(t.diameter);
            t.stepdown = p.stepdown.min(t.flute_length);
        }
        for p in &mut t.presets {
            p.spindle_rpm = p.spindle_rpm.min(20_000.0);
            p.feed = p.feed.min(10_000.0);
            p.plunge_feed = p.plunge_feed.min(10_000.0);
            p.ramp_feed = p.ramp_feed.min(10_000.0);
        }
        tools.push(t);
    };
    let hole = |rpm: f64, feed: f64| {
        move |t: &mut Tool| {
            t.spindle_rpm = rpm;
            t.feed = feed;
            t.plunge_feed = feed;
        }
    };
    add(1, "flat", 12.0, "Ø12 3FL end mill", 3, &|t| {
        t.flute_length = 26.0;
        t.overall_length = 75.0;
        t.coating = "AlTiN".into();
    });
    add(2, "flat", 6.0, "Ø6 3FL end mill, necked", 2, &|t| {
        t.flute_length = 13.0;
        t.neck_diameter = 5.6;
        t.shoulder_length = 21.0;
        t.overall_length = 57.0;
        t.coating = "AlTiN".into();
    });
    add(3, "ball", 6.0, "Ø6 2FL ball end mill", 2, &|t| {
        t.flutes = 2;
        t.flute_length = 12.0;
        t.overall_length = 60.0;
    });
    add(4, "ball", 3.0, "Ø3 2FL ball, long reach", 2, &|t| {
        t.flutes = 2;
        t.flute_length = 4.5;
        t.neck_diameter = 2.8;
        t.shoulder_length = 20.0;
        t.shank_diameter = 6.0;
        t.overall_length = 50.0;
    });
    add(5, "bull", 10.0, "Ø10 R1 bull nose", 3, &|t| {
        t.corner_radius = 1.0;
        t.flute_length = 22.0;
        t.overall_length = 72.0;
        t.coating = "TiAlN".into();
    });
    add(6, "bull", 16.0, "Ø16 R2 bull nose", 4, &|t| {
        t.corner_radius = 2.0;
        t.flute_length = 32.0;
        t.overall_length = 92.0;
    });
    add(7, "tapered_ball", 2.0, "R1 3° tapered ball", 2, &|t| {
        t.taper_angle = 3.0;
        t.flute_length = 25.0;
        t.shank_diameter = 6.0;
        t.overall_length = 60.0;
    });
    add(8, "tapered_ball", 4.0, "R2 1.5° tapered ball", 2, &|t| {
        t.taper_angle = 1.5;
        t.flute_length = 30.0;
        t.shank_diameter = 6.0;
        t.overall_length = 75.0;
    });
    add(9, "lollipop", 6.0, "Ø6 lollipop cutter", 2, &|t| {
        t.neck_diameter = 3.5;
        t.shoulder_length = 20.0;
        t.overall_length = 60.0;
    });
    add(10, "lollipop", 10.0, "Ø10 lollipop cutter", 3, &|t| {
        t.neck_diameter = 6.0;
        t.shoulder_length = 30.0;
        t.shank_diameter = 10.0;
        t.overall_length = 75.0;
    });
    add(11, "chamfer", 10.0, "90° chamfer mill Ø10", 3, &|t| {
        t.tip_diameter = 0.5;
        t.flute_length = (5.0 - 0.25) / 45_f64.to_radians().tan();
    });
    add(12, "chamfer", 12.0, "60° chamfer mill Ø12", 3, &|t| {
        t.tip_angle = 60.0;
        t.tip_diameter = 1.0;
        t.flute_length = (6.0 - 0.5) / 30_f64.to_radians().tan();
        t.overall_length = t.flute_length + 40.0;
    });
    add(13, "engraving", 3.175, "30° engraver, 0.1 mm tip", 2, &|t| {
        t.tip_angle = 30.0;
        t.tip_diameter = 0.1;
        t.flute_length = (1.5875 - 0.05) / 15_f64.to_radians().tan();
        t.overall_length = 40.0;
    });
    add(14, "engraving", 6.0, "60° engraver, 0.2 mm tip", 2, &|t| {
        t.tip_diameter = 0.2;
        t.flute_length = (3.0 - 0.1) / 30_f64.to_radians().tan();
    });
    add(15, "dovetail", 12.0, "Ø12 45° dovetail", 3, &|t| {
        t.neck_diameter = 5.5;
        t.flute_length = 3.0;
    });
    add(16, "dovetail", 20.0, "Ø20 30° dovetail", 4, &|t| {
        t.taper_angle = 30.0;
        t.neck_diameter = 10.0;
        t.flute_length = 6.0;
        t.shank_diameter = 20.0;
    });
    add(17, "slot_mill", 25.0, "Ø25 × 3 slot mill", 4, &|t| {
        t.flute_length = 3.0;
        t.neck_diameter = 10.0;
        t.shoulder_length = 12.0;
        t.shank_diameter = 20.0;
        t.overall_length = 70.0;
    });
    add(18, "slot_mill", 40.0, "Ø40 × 5 slot mill", 4, &|t| {
        t.flute_length = 5.0;
        t.neck_diameter = 16.0;
        t.shoulder_length = 20.0;
        t.shank_diameter = 20.0;
        t.overall_length = 80.0;
    });
    add(19, "radius_mill", 10.0, "R2 corner rounding Ø10", 3, &|t| {
        t.corner_radius = 2.0;
        t.tip_diameter = 6.0;
        t.flute_length = 4.0;
    });
    add(20, "radius_mill", 16.0, "R3 corner rounding Ø16", 4, &|t| {
        t.corner_radius = 3.0;
        t.tip_diameter = 10.0;
        t.flute_length = 6.0;
        t.shank_diameter = 12.0;
    });
    add(21, "face_mill", 50.0, "Ø50 90° face mill, 5 inserts", 5, &|t| {
        t.flutes = 5;
        t.shank_diameter = 22.0;
        t.coolant_through = true;
    });
    add(22, "face_mill", 63.0, "Ø63 45° face mill, 6 inserts", 5, &|t| {
        t.flutes = 6;
        t.tip_angle = 45.0;
        t.shank_diameter = 22.0;
        t.coolant_through = true;
    });
    add(23, "thread_mill", 8.0, "M10×1.5 thread mill", 3, &|t| {
        t.thread_pitch = 1.5;
        t.flute_length = 16.0;
    });
    add(24, "thread_mill", 4.8, "M6×1 thread mill", 2, &|t| {
        t.thread_pitch = 1.0;
        t.flute_length = 10.0;
        t.shank_diameter = 6.0;
    });
    add(25, "drill", 5.0, "Ø5 carbide drill 140°", 1, &|t| {
        hole(5_000.0, 600.0)(t);
        t.coolant_through = true;
    });
    add(26, "drill", 8.5, "Ø8.5 drill (M10 tap drill)", 1, &|t| hole(3_000.0, 450.0)(t));
    add(27, "spot_drill", 6.0, "Ø6 90° spot drill", 1, &|t| hole(4_000.0, 300.0)(t));
    add(28, "spot_drill", 10.0, "Ø10 120° spot drill", 1, &|t| {
        t.tip_angle = 120.0;
        hole(3_000.0, 300.0)(t);
    });
    add(29, "center_drill", 6.3, "A2.5 centre drill", 1, &|t| {
        t.tip_diameter = 2.5;
        t.tip_length = 3.1;
        hole(2_000.0, 120.0)(t);
    });
    add(30, "center_drill", 10.0, "A4 centre drill", 1, &|t| {
        t.tip_diameter = 4.0;
        t.tip_length = 5.0;
        t.flute_length = 12.0;
        hole(1_500.0, 100.0)(t);
    });
    add(31, "reamer", 6.0, "Ø6 H7 reamer", 1, &|t| hole(1_500.0, 200.0)(t));
    add(32, "reamer", 10.0, "Ø10 H7 reamer", 4, &|t| hole(900.0, 180.0)(t));
    add(33, "tap", 6.0, "M6×1 tap", 6, &|t| hole(800.0, 800.0)(t));
    add(34, "tap", 10.0, "M10×1.5 tap", 6, &|t| hole(500.0, 750.0)(t));
    add(35, "boring_bar", 16.0, "Ø16 boring bar", 4, &|t| {
        t.neck_diameter = 12.0;
        t.shank_diameter = 16.0;
        t.shoulder_length = 40.0;
        t.overall_length = 80.0;
        hole(2_500.0, 250.0)(t);
    });
    add(36, "boring_bar", 25.0, "Ø25 boring bar", 4, &|t| {
        t.neck_diameter = 20.0;
        t.shank_diameter = 20.0;
        t.shoulder_length = 60.0;
        t.overall_length = 100.0;
        hole(1_800.0, 200.0)(t);
    });
    add(37, "counterbore", 11.0, "M6 counterbore", 1, &|t| {
        t.tip_diameter = 6.4;
        t.tip_length = 6.0;
        hole(900.0, 90.0)(t);
    });
    add(38, "counterbore", 15.0, "M8 counterbore", 4, &|t| {
        t.tip_diameter = 8.4;
        t.tip_length = 8.0;
        hole(700.0, 80.0)(t);
    });
    add(39, "countersink", 12.4, "90° countersink Ø12.4", 1, &|t| hole(800.0, 100.0)(t));
    add(40, "countersink", 16.0, "82° countersink Ø16", 4, &|t| {
        t.tip_angle = 82.0;
        t.flute_length = 8.0 / 41_f64.to_radians().tan();
        hole(600.0, 80.0)(t);
    });
    (tools, holders)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_has_a_valid_typical_tool_and_a_closed_profile() {
        for ty in TYPES {
            assert!(TOOL_TYPES.contains(&ty.id), "{}", ty.id);
            for d in [1.0, 6.0, 20.0, 63.0] {
                let t = Tool::typical(1, ty.id, d);
                assert!(t.validate().is_ok(), "{} Ø{d}: {:?}", ty.id, t.validate());
                let p = t.profile();
                let (first, last) = (p.points.first().copied(), p.points.last().copied());
                assert_eq!(first, Some([0.0, 0.0]), "{}", ty.id);
                assert_eq!(last.map(|q| q[0]), Some(0.0), "{}", ty.id);
                assert!(p.points.iter().all(|q| q[0] >= -1.0e-9 && q[0].is_finite() && q[1].is_finite()), "{}", ty.id);
                // Heights never go back down, and the outline reaches the holder face.
                assert!(p.points.windows(2).all(|w| w[1][1] >= w[0][1] - 1.0e-9), "{} {:?}", ty.id, p.points);
                assert!((last.map_or(0.0, |q| q[1]) - t.overall_length).abs() < 1.0e-9, "{}", ty.id);
                // The widest point of the cutting end is the cutting diameter (face mills with a
                // lead angle reach beyond it).
                let widest = p.points.iter().filter(|q| q[1] <= p.flute_top + 1.0e-9).map(|q| q[0]).fold(0.0, f64::max);
                if !matches!(ty.id, "face_mill" | "tapered_ball") {
                    assert!((widest - d / 2.0).abs() < 1.0e-6, "{} Ø{d}: widest {widest}", ty.id);
                }
            }
        }
    }

    #[test]
    fn profiles_follow_the_geometry() {
        // A 90° chamfer mill with a 1 mm flat: the cone rises 45°.
        let mut c = Tool::typical(1, "chamfer", 10.0);
        c.tip_diameter = 1.0;
        let p = c.profile().points;
        assert!(
            p.contains(&[0.5, 0.0]) && p.iter().any(|q| (q[0] - 5.0).abs() < 1.0e-9 && (q[1] - 4.5).abs() < 1.0e-9),
            "{p:?}"
        );
        // A 118° drill: the point is r / tan(59°) long.
        let mut d = Tool::typical(1, "drill", 10.0);
        d.tip_angle = 118.0;
        let h = 5.0 / 59_f64.to_radians().tan();
        assert!(d.profile().points.iter().any(|q| (q[0] - 5.0).abs() < 1.0e-9 && (q[1] - h).abs() < 1.0e-9));
        // A bull nose's corner ends at the corner radius up the side.
        let mut b = Tool::typical(1, "bull", 10.0);
        b.corner_radius = 1.0;
        assert!(b.profile().points.iter().any(|q| (q[0] - 5.0).abs() < 1.0e-9 && (q[1] - 1.0).abs() < 1.0e-9));
        // A necked flat end mill: narrower between the flutes and the reach.
        let mut n = Tool::typical(1, "flat", 6.0);
        n.neck_diameter = 5.0;
        n.shoulder_length = 30.0;
        n.overall_length = 50.0;
        let p = n.profile().points;
        assert!(p.contains(&[2.5, 30.0]) && p.contains(&[3.0, 30.0]), "{p:?}");
    }

    #[test]
    fn presets_work_out_speeds_and_feeds() {
        let mut p = Preset { name: "Al".into(), surface_speed: 300.0, feed_per_tooth: 0.05, ..Preset::default() };
        p.derive(10.0, 3);
        assert_eq!(p.spindle_rpm, (300_000.0 / (std::f64::consts::PI * 10.0)).round());
        assert_eq!(p.feed, (p.spindle_rpm * 0.15).round());
        assert!(p.plunge_feed > 0.0 && p.ramp_feed > 0.0);
        let mut t = Tool::typical(1, "flat", 10.0);
        t.presets.push(p);
        assert!(t.validate().is_ok());
        assert_eq!(t.cutting(Some("Al")).map(|c| c.feed > 0.0), Ok(true));
        assert!(t.cutting(Some("Steel")).is_err());
    }

    #[test]
    fn the_sample_library_has_two_valid_tools_of_each_type() {
        let (tools, holders) = sample_library();
        for h in &holders {
            assert!(h.validate().is_ok(), "{}", h.name);
        }
        for ty in TYPES {
            assert_eq!(tools.iter().filter(|t| t.kind == ty.id).count(), 2, "{}", ty.id);
        }
        for t in &tools {
            assert!(t.validate().is_ok(), "T{} {}: {:?}", t.number, t.name, t.validate());
            assert!(
                t.spindle_rpm > 0.0 && t.feed > 0.0 && t.spindle_rpm <= 20_000.0 && t.feed <= 10_000.0,
                "T{}",
                t.number
            );
            assert!(t.holder.as_ref().is_some_and(|h| holders.iter().any(|o| &o.id == h)), "T{}", t.number);
        }
        let numbers: std::collections::BTreeSet<u32> = tools.iter().map(|t| t.number).collect();
        assert_eq!(numbers.len(), tools.len());
    }

    #[test]
    fn bad_geometry_is_refused() {
        let mut t = Tool::typical(1, "lollipop", 6.0);
        t.neck_diameter = 7.0;
        assert!(t.validate().is_err());
        let mut t = Tool::typical(1, "drill", 6.0);
        t.tip_angle = 0.0;
        assert!(t.validate().is_err());
        let mut t = Tool::typical(1, "radius_mill", 10.0);
        t.corner_radius = 4.0;
        assert!(t.validate().is_err());
        let mut t = Tool::typical(1, "flat", 6.0);
        t.shoulder_length = 5.0;
        assert!(t.validate().is_err());
    }
}
