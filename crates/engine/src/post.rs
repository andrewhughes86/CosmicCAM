//! A built-in generic G-code writer, until the Python post-processors and machine
//! configurations of Phase 8 replace it. It writes plain RS-274 milling code: a safety
//! block, then for each toolpath a tool change, spindle and coolant, the work offset of the
//! toolpath's WCS, tool length compensation and the moves as G0 / G1, then an end block.
//! Every line that moves an axis starts with its motion code; feeds are written only when
//! they change.
//!
//! Coordinates are in the toolpath's WCS (the machine's G54, G55, … for the first, second, …
//! WCS of the job; G54 for toolpaths in world coordinates), in inches or millimetres.

use std::fmt::Write as _;

use CosmicCAM_geom::Vec3;
use glam::DVec3;

use crate::error::{EngineError, EngineResult};
use crate::session::Session;
use crate::toolpath::{MoveKind, Toolpath};
use crate::units::DisplayUnits;

/// Most toolpaths in one program.
pub const MAX_PROGRAM_TOOLPATHS: usize = 512;

/// What to write.
pub struct Options {
    pub units: DisplayUnits,
    pub program_number: u32,
    pub line_numbers: bool,
}

/// The WCS frame a toolpath's moves are written in, and its work offset code.
struct Frame {
    origin: DVec3,
    x: DVec3,
    y: DVec3,
    z: DVec3,
    offset: String,
}

impl Frame {
    fn local(&self, p: Vec3) -> DVec3 {
        let d = p.as_dvec3() - self.origin;
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }
}

fn frame(s: &Session, tp: &Toolpath) -> EngineResult<Frame> {
    let Some(id) = &tp.wcs else {
        return Ok(Frame { origin: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z, offset: "G54".into() });
    };
    let index = s.job.wcs.iter().position(|w| &w.id == id).ok_or_else(|| {
        EngineError::InvalidParams(format!("{} is programmed in {id}, which no longer exists; regenerate it", tp.name))
    })?;
    let w = s.job.wcs.get(index).ok_or_else(|| EngineError::Internal("WCS lookup".into()))?;
    // G54..G59, then the extended G54.1 P1.. offsets.
    let offset = if index < 6 { format!("G{}", 54 + index) } else { format!("G54.1 P{}", index - 5) };
    Ok(Frame {
        origin: w.origin.as_dvec3(),
        x: w.x_axis.as_dvec3(),
        y: w.y_axis().as_dvec3(),
        z: w.z_axis.as_dvec3(),
        offset,
    })
}

/// Comment text the controller will accept: no parentheses, upper case, printable ASCII.
fn comment(text: &str) -> String {
    let t: String = text
        .chars()
        .map(|c| match c {
            '(' | ')' => ' ',
            'Ø' => 'D',
            c if c.is_ascii_graphic() || c == ' ' => c.to_ascii_uppercase(),
            _ => '?',
        })
        .take(70)
        .collect();
    format!("({})", t.trim())
}

struct Writer {
    out: String,
    line: u32,
    numbered: bool,
}

impl Writer {
    fn line(&mut self, text: &str) {
        if self.numbered && !text.starts_with('(') && text != "%" {
            self.line = self.line.saturating_add(5);
            let _ = writeln!(self.out, "N{} {text}", self.line);
        } else {
            let _ = writeln!(self.out, "{text}");
        }
    }
}

/// A drilling toolpath as canned cycles: one line per hole, each naming its cycle, returning
/// to the retract plane (G99) between holes, then G80 and back to clearance.
fn drill_cycle(
    w: &mut Writer,
    d: &crate::toolpath::DrillCycle,
    rpm: f64,
    n: &dyn Fn(f64) -> String,
    feed: &dyn Fn(f64) -> String,
) {
    let dwell_ms = (d.dwell * 1000.0).round() as u64;
    let (code, extra) = match d.cycle.as_str() {
        "spot" => ("G82", format!(" P{dwell_ms}")),
        "drill" if dwell_ms > 0 => ("G82", format!(" P{dwell_ms}")),
        "peck" => ("G83", format!(" Q{}", n(d.peck))),
        "chip_break" => ("G73", format!(" Q{}", n(d.peck))),
        "tap" => ("G84", String::new()),
        "bore" => ("G85", String::new()),
        _ => ("G81", String::new()),
    };
    // Tapping feeds at the pitch times the speed so the tap follows its thread.
    let f = if d.cycle == "tap" { feed(d.pitch * rpm) } else { feed(d.feed) };
    for (i, h) in d.holes.iter().enumerate() {
        let lead = if i == 0 { "G99 " } else { "" };
        w.line(&format!("{lead}{code} X{} Y{} Z{} R{}{extra} F{f}", n(h[0]), n(h[1]), n(h[3]), n(d.retract)));
    }
    w.line("G80");
    w.line(&format!("G0 Z{}", n(d.clearance)));
}

/// Writes the program for `toolpaths` (ids, in order; all when empty).
pub fn gcode(s: &Session, ids: &[String], opt: &Options) -> EngineResult<String> {
    let toolpaths: Vec<&Toolpath> = if ids.is_empty() {
        s.job.toolpaths.iter().collect()
    } else {
        ids.iter()
            .map(|id| {
                s.job
                    .toolpaths
                    .iter()
                    .find(|t| &t.id == id)
                    .ok_or_else(|| EngineError::InvalidParams(format!("no toolpath `{id}`")))
            })
            .collect::<EngineResult<_>>()?
    };
    if toolpaths.is_empty() {
        return Err(EngineError::InvalidParams(
            "there are no toolpaths to export; create one on the Toolpaths tab".into(),
        ));
    }
    if toolpaths.len() > MAX_PROGRAM_TOOLPATHS {
        return Err(EngineError::InvalidParams(format!("a program holds at most {MAX_PROGRAM_TOOLPATHS} toolpaths")));
    }
    let u = opt.units;
    let places = u.places();
    let n = |mm: f64| -> String {
        let v = u.from_mm(mm);
        let s = format!("{v:.places$}");
        // "-0.0000" reads as zero.
        if s.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { format!("{:.places$}", 0.0) } else { s }
    };
    let feed = |mm_per_min: f64| -> String {
        let fp = if u == DisplayUnits::Millimetres { 1 } else { 2 };
        format!("{:.fp$}", u.from_mm(mm_per_min))
    };

    let mut w = Writer { out: String::new(), line: 0, numbered: opt.line_numbers };
    w.line("%");
    w.line(&format!("O{:04}", opt.program_number.min(9999)));
    w.line(&comment(&format!("{} - generated by CosmicCAM", s.job.name)));
    w.line(&comment("generic G-code - no post-processor - check before running"));
    w.line(&comment(&format!("units: {}", u.label())));
    let mut listed: Vec<(String, u32)> = Vec::new();
    for tp in &toolpaths {
        if listed.iter().all(|(m, t)| !(m == &tp.machine && *t == tp.tool)) {
            listed.push((tp.machine.clone(), tp.tool));
            let desc = s
                .library
                .machine(&tp.machine)
                .and_then(|m| m.tool(tp.tool))
                .map_or("not in the library".to_owned(), |t| t.name.clone());
            w.line(&comment(&format!("T{} {desc}", tp.tool)));
        }
    }
    // Safety block: absolute, feed per minute, XY plane, no cutter or length compensation,
    // no canned cycle, then the units.
    w.line("G90 G94 G17 G40 G49 G80");
    w.line(if u == DisplayUnits::Inches { "G20" } else { "G21" });

    let mut current_tool: Option<(String, u32)> = None;
    for tp in &toolpaths {
        let m = s
            .library
            .machine(&tp.machine)
            .ok_or_else(|| EngineError::InvalidParams(format!("{} uses a machine that no longer exists", tp.name)))?;
        let tool = m.tool(tp.tool).ok_or_else(|| {
            EngineError::InvalidParams(format!("{} uses T{}, which is no longer in the library", tp.name, tp.tool))
        })?;
        let rpm = tp
            .params
            .get("spindle_rpm")
            .and_then(serde_json::Value::as_f64)
            .filter(|v| *v > 0.0)
            .unwrap_or(tool.spindle_rpm);
        if rpm <= 0.0 {
            return Err(EngineError::InvalidParams(format!(
                "{} has no spindle speed; set one on the tool (T{}) or the toolpath",
                tp.name, tp.tool
            )));
        }
        if rpm > m.max_spindle_rpm {
            return Err(EngineError::InvalidParams(format!("{}: {rpm} rpm is above {}'s maximum", tp.name, m.name)));
        }
        let f = frame(s, tp)?;
        let coolant = match tp.params.get("coolant").and_then(serde_json::Value::as_str).unwrap_or("flood") {
            "off" => None,
            "mist" | "air" => Some("M7"),
            _ => Some("M8"),
        };

        w.line(&comment(&format!("{} - {}", tp.name, tool.name)));
        let same_tool = current_tool.as_ref().is_some_and(|(mc, t)| mc == &tp.machine && *t == tp.tool);
        if !same_tool {
            if current_tool.is_some() {
                w.line("M9");
                w.line("M5");
            }
            w.line(&format!("T{} M6", tp.tool));
            current_tool = Some((tp.machine.clone(), tp.tool));
        }
        w.line(&format!("S{} M3", rpm.round()));
        w.line(&f.offset);
        // Over the start at clearance, then length compensation on the first Z move.
        let start = f.local(tp.start);
        w.line(&format!("G0 X{} Y{}", n(start.x), n(start.y)));
        w.line(&format!("G0 G43 Z{} H{}", n(start.z), tool.length_offset));
        if let Some(c) = coolant {
            w.line(c);
        }
        if let Some(d) = &tp.drill {
            drill_cycle(&mut w, d, rpm, &n, &feed);
            continue;
        }
        let mut at = start;
        let mut last_f: Option<String> = None;
        for mv in tp.moves.iter() {
            let p = f.local(mv.to);
            let axes: Vec<String> = [('X', p.x, at.x), ('Y', p.y, at.y), ('Z', p.z, at.z)]
                .into_iter()
                .filter(|(_, new, old)| n(*new) != n(*old))
                .map(|(axis, new, _)| format!("{axis}{}", n(new)))
                .collect();
            at = p;
            // A move that changes no axis writes nothing.
            if axes.is_empty() {
                continue;
            }
            // Every move names its motion (G0 or G1), even where the controller would keep it.
            let g = if mv.kind == MoveKind::Rapid { 0 } else { 1 };
            let mut words = vec![format!("G{g}")];
            words.extend(axes);
            if g == 1 {
                let fv = feed(f64::from(mv.feed));
                if last_f.as_deref() != Some(fv.as_str()) {
                    words.push(format!("F{fv}"));
                    last_f = Some(fv);
                }
            }
            w.line(&words.join(" "));
        }
    }
    // End: coolant and spindle off, Z home, cancel length compensation, end of program.
    w.line("M9");
    w.line("M5");
    w.line("G91 G28 Z0");
    w.line("G90 G49");
    w.line("M30");
    w.line("%");
    Ok(w.out)
}
