//! What the toolpath strategies share: settings every strategy reads, heights, the part and
//! stock in the toolpath's frame, and an emitter that turns passes into moves with links and
//! entries (helix or plunge).

use std::sync::Arc;

use CosmicCAM_geom::faces::boundary_loops;
use CosmicCAM_geom::region::{self, Shape};
use CosmicCAM_geom::{FaceKind, Vec3};
use glam::{DVec2, DVec3};
use serde_json::{Map, Value, json};

use super::contour::{Builder, Frame};
use super::{MoveKind, Toolpath};
use crate::error::{EngineError, EngineResult};
use crate::library::Tool;
use crate::params;
use crate::session::{Entity, Session};

pub(super) fn bad(m: impl Into<String>) -> EngineError {
    EngineError::InvalidParams(m.into())
}

/// Most depth levels one toolpath steps through.
pub(super) const MAX_LEVELS: usize = 2_000;
/// Most rows or rings one level gets.
pub(super) const MAX_PASSES: usize = 20_000;

/// Settings every milling strategy reads, resolved to millimetres and mm/min.
pub(super) struct Common {
    pub name: String,
    pub tool: Tool,
    pub machine: String,
    pub frame: Frame,
    pub wcs: Option<String>,
    pub feed: f64,
    pub plunge_feed: f64,
    pub stepover: f64,
    pub stepdown: f64,
    pub tolerance: f64,
    pub stock_radial: f64,
    pub stock_axial: f64,
    pub climb: bool,
    /// Helix entry: ramp angle (radians) and radius; None to plunge.
    pub helix: Option<(f64, f64)>,
    pub rapid_links: bool,
}

impl Common {
    pub fn radius(&self) -> f64 {
        self.tool.diameter / 2.0
    }
}

/// Reads the shared settings. `stepover` and `stepdown` defaults are fractions of the tool
/// diameter, used when neither the parameters nor the tool give one.
pub(super) fn common(s: &Session, p: &Value, stepover_of_d: f64, stepdown_of_d: f64) -> EngineResult<Common> {
    let (machine, tool) = super::contour::pick_tool(s, p)?;
    let (frame, wcs) = super::contour::frame(s, p)?;
    let num = |key: &str, lo: f64, hi: f64| params::opt_f64(p, key, lo, hi);
    let pick = |v: Option<f64>, fallback: f64| v.filter(|f| *f > 0.0).unwrap_or(fallback);
    let d = tool.diameter;
    let feed = pick(num("feed", 0.0, 1.0e6)?, tool.feed);
    if feed <= 0.0 {
        return Err(bad(format!("T{} has no cutting feed; set `feed` or the tool's feeds", tool.number)));
    }
    let plunge_feed =
        pick(num("plunge_feed", 0.0, 1.0e6)?, if tool.plunge_feed > 0.0 { tool.plunge_feed } else { feed * 0.5 });
    for (name, v) in [("feed", feed), ("plunge_feed", plunge_feed)] {
        if v > machine.max_feed {
            return Err(bad(format!("`{name}` {v} is above {}'s maximum feed of {}", machine.name, machine.max_feed)));
        }
    }
    let stepover = pick(num("stepover", 0.0, 1.0e4)?, d * stepover_of_d);
    let stepdown =
        pick(num("stepdown", 0.0, 1.0e4)?, if tool.stepdown > 0.0 { tool.stepdown } else { d * stepdown_of_d });
    let helix = match params::opt_str(p, "entry")?.unwrap_or("helix") {
        "plunge" => None,
        _ => Some((
            pick(num("ramp_angle", 0.0, 90.0)?, 2.0).to_radians(),
            pick(num("helix_radius", 0.0, 1.0e4)?, d * 0.4),
        )),
    };
    Ok(Common {
        name: params::opt_str(p, "name")?.map(crate::library::clip).filter(|n| !n.is_empty()).unwrap_or_default(),
        tool: tool.clone(),
        machine: machine.id.clone(),
        frame,
        wcs,
        feed,
        plunge_feed,
        stepover,
        stepdown,
        tolerance: pick(num("tolerance", 0.0, 1.0)?, 0.01),
        stock_radial: num("stock_radial", -1.0e3, 1.0e3)?.unwrap_or(0.0),
        stock_axial: num("stock_axial", -1.0e3, 1.0e3)?.unwrap_or(0.0),
        climb: params::opt_str(p, "direction")?.unwrap_or("climb") == "climb",
        helix,
        rapid_links: params::opt_bool(p, "rapid_links")?.unwrap_or(true),
    })
}

/// A height's reference planes: what `selection` means for this toolpath (top, bottom).
pub(super) type Selection = Option<(f64, f64)>;

/// A height in WCS Z: its reference (`<prefix>_from`) plus its offset (`<prefix>_offset`).
pub(super) fn height(
    s: &Session,
    p: &Value,
    f: &Frame,
    selection: Selection,
    prefix: &str,
    default: (&str, f64),
) -> EngineResult<f64> {
    let reference = params::opt_str(p, &format!("{prefix}_from"))?.unwrap_or(default.0);
    let offset = params::opt_f64(p, &format!("{prefix}_offset"), -1.0e5, 1.0e5)?
        .unwrap_or_else(|| super::contour::default_offset(s, default.1));
    let base = match reference {
        "selection" => {
            let (top, bottom) = selection.ok_or_else(|| bad(format!("`{prefix}_from`: nothing is selected")))?;
            if prefix == "bottom" { bottom } else { top }
        }
        "model_top" | "model_bottom" => {
            let (lo, hi) = f.z_range(s.job.part_bounds()).ok_or_else(|| bad("there is no model for a model height"))?;
            if reference == "model_top" { hi } else { lo }
        }
        "stock_top" | "stock_bottom" => {
            let (lo, hi) = s
                .job
                .stock
                .and_then(|b| f.z_range(b))
                .ok_or_else(|| bad("there is no stock; create a setup or fit the stock first"))?;
            if reference == "stock_top" { hi } else { lo }
        }
        "origin" => 0.0,
        other => return Err(bad(format!("unknown height reference `{other}`"))),
    };
    if !base.is_finite() {
        return Err(bad(format!("`{prefix}_from`: no height to take")));
    }
    Ok(base + offset)
}

/// The five heights (clearance, retract, feed, top, bottom), checked to run top to bottom.
pub(super) fn heights(
    s: &Session,
    p: &Value,
    f: &Frame,
    selection: Selection,
    defaults: &[(&str, &str, f64); 5],
) -> EngineResult<[f64; 5]> {
    let mut h = [0.0f64; 5];
    for (slot, (prefix, reference, offset)) in h.iter_mut().zip(defaults) {
        *slot = height(s, p, f, selection, prefix, (reference, *offset))?;
    }
    let names = ["Clearance", "Retract", "Feed", "Top", "Bottom"];
    for i in 0..4 {
        if h[i] + 1.0e-9 < h[i + 1] {
            return Err(bad(format!(
                "{} height ({:.3}) must not be below the {} height ({:.3})",
                names[i],
                h[i],
                names[i + 1].to_lowercase(),
                h[i + 1]
            )));
        }
    }
    Ok(h)
}

/// Depth levels from just below `top` down to `bottom`, at most `step` apart (one level at
/// `bottom` when `multiple` is false or the step is not smaller than the depth).
pub(super) fn levels(top: f64, bottom: f64, step: f64, multiple: bool) -> Vec<f64> {
    if multiple && top - bottom > step + 1.0e-9 && step > 0.0 {
        let count = ((top - bottom) / step).ceil().clamp(1.0, MAX_LEVELS as f64) as usize;
        (1..=count).map(|k| top - (top - bottom) * k as f64 / count as f64).collect()
    } else {
        vec![bottom]
    }
}

/// The models' triangles in the toolpath frame.
pub(super) fn part_triangles(s: &Session, f: &Frame) -> Vec<[DVec3; 3]> {
    s.job
        .models
        .iter()
        .flat_map(|m| {
            let mesh = &m.mesh;
            mesh.triangles().iter().map(move |t| mesh.corners(*t).map(|p| f.local(p))).collect::<Vec<_>>()
        })
        .collect()
}

/// A world box's footprint in the frame's XY plane (its bounding rectangle there).
pub(super) fn footprint(f: &Frame, b: CosmicCAM_geom::Aabb) -> Option<(DVec2, DVec2)> {
    if b.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        let l = f.local(c).truncate();
        lo = lo.min(l);
        hi = hi.max(l);
    }
    Some((lo, hi))
}

/// The selected faces' outlines (with their holes) and closed edge chains, as shapes in the
/// frame's XY plane, and the selection's (top, bottom) heights.
pub(super) fn selected_shapes(
    s: &Session,
    p: &Value,
    f: &Frame,
    tolerance: f64,
    warnings: &mut Vec<String>,
) -> EngineResult<(Vec<Shape>, Selection)> {
    let refs = params::str_list(p, "geometry")?;
    let mut shapes: Vec<Shape> = Vec::new();
    let (mut top, mut bottom) = (f64::NEG_INFINITY, f64::INFINITY);
    let mut edge_refs = Vec::new();
    for r in &refs {
        match s.job.entity(r) {
            Some((m, Entity::Face(fi))) => {
                let loops: Vec<Vec<DVec2>> = boundary_loops(&m.mesh, &m.faces, fi)
                    .iter()
                    .map(|l| {
                        l.iter()
                            .map(|q| {
                                let local = f.local(*q);
                                top = top.max(local.z);
                                bottom = bottom.min(local.z);
                                local.truncate()
                            })
                            .collect()
                    })
                    .collect();
                if let Some(kind) = m.faces.faces.get(fi).map(|face| face.kind)
                    && !matches!(kind, FaceKind::Plane { .. })
                {
                    warnings.push(format!("{r} is not flat; its outline was projected onto the XY plane"));
                }
                match region::shape_from_loops(&loops) {
                    Some(sh) => shapes.push(sh),
                    None => return Err(bad(format!("`geometry`: {r} has no outline"))),
                }
            }
            Some((_, Entity::Edge(_))) => edge_refs.push(r.clone()),
            Some((_, Entity::Vertex(_))) => {
                return Err(bad(format!("`geometry`: {r} is a vertex; select faces or edges")));
            }
            None => return Err(bad(format!("`geometry`: no face or edge `{r}`"))),
        }
    }
    if !edge_refs.is_empty() {
        let chains = super::contour::chains(s, &edge_refs, f, tolerance, warnings)?;
        for c in chains {
            top = top.max(c.z);
            bottom = bottom.min(c.z);
            if c.closed {
                shapes.extend(region::shape_from_loops(&[c.pts]));
            } else {
                warnings.push("an open chain of edges was skipped; this toolpath needs closed boundaries".into());
            }
        }
    }
    let selection = (top.is_finite() && bottom.is_finite()).then_some((top, bottom));
    // Overlapping selections become one region.
    let merged = if shapes.len() > 1 { region::boolean(&shapes, &[], region::Op::Union) } else { shapes };
    Ok((merged, selection))
}

/// One pass to cut, in the frame.
pub(super) struct Pass {
    pub pts: Vec<DVec3>,
    pub closed: bool,
    /// The material is clear down to this height above the start, so the tool may rapid that
    /// far before feeding down.
    pub clear_above: f64,
    /// The start is in material: enter with a helix (or plunge) instead of feeding straight down.
    pub ramp: bool,
    /// Where the tool centre may go at depth: links to this pass at depth stay inside it.
    pub safe: Option<Arc<Vec<Shape>>>,
}

/// Turns passes into moves: rapids at clearance and retract height, feed links that stay
/// inside the safe region, entries by helix or plunge.
pub(super) struct Emitter {
    pub b: Builder,
    pub clearance: f64,
    pub retract: f64,
    /// Rapid stops this far above where the material is clear, then feeds down.
    pub gap: f64,
    started: bool,
}

impl Emitter {
    pub fn new(frame: Frame, clearance: f64, retract: f64, gap: f64) -> Self {
        Self {
            b: Builder { frame, moves: Vec::new(), at: DVec3::ZERO },
            clearance,
            retract,
            gap: gap.max(0.5),
            started: false,
        }
    }

    fn helix(&mut self, c: &Common, center: DVec2, from: f64, to: f64, safe: Option<&[Shape]>) -> EngineResult<bool> {
        let Some((angle, radius)) = c.helix else { return Ok(false) };
        let radius = radius.min(c.radius() * 0.95);
        if radius < 0.05 || angle <= 0.0 {
            return Ok(false);
        }
        let step = CosmicCAM_geom::offset::arc_step(radius, c.tolerance);
        let per_turn = std::f64::consts::TAU * radius * angle.tan();
        let turns = ((from - to) / per_turn.max(1.0e-6)).max(0.0);
        let segments = ((turns * std::f64::consts::TAU / step).ceil() as usize).clamp(8, 200_000);
        let total = std::f64::consts::TAU * turns.max(1.0e-3);
        let point = |a: f64| center + DVec2::new(a.cos(), a.sin()) * radius;
        if let Some(safe) = safe {
            let ring_ok = (0..16).all(|k| region::contains(safe, point(std::f64::consts::TAU * k as f64 / 16.0)));
            if !ring_ok {
                return Ok(false);
            }
        }
        self.b.push(MoveKind::Plunge, point(0.0).extend(from), c.plunge_feed)?;
        for k in 1..=segments {
            let t = k as f64 / segments as f64;
            self.b.push(MoveKind::Plunge, point(total * t).extend(from + (to - from) * t), c.plunge_feed)?;
        }
        // A full circle at depth clears the centre post, then back to the start point.
        for k in 1..=16 {
            self.b.push(MoveKind::Cut, point(total + std::f64::consts::TAU * k as f64 / 16.0).extend(to), c.feed)?;
        }
        self.b.push(MoveKind::Link, center.extend(to), c.feed)?;
        Ok(true)
    }

    /// Adds `pass`, linking from where the tool is.
    pub fn pass(&mut self, c: &Common, pass: &Pass) -> EngineResult<()> {
        let (Some(&first), Some(&last)) = (pass.pts.first(), pass.pts.last()) else { return Ok(()) };
        let start_xy = first.truncate();
        let safe = pass.safe.as_deref().map(Vec::as_slice);
        let at = self.b.at;
        let linked = self.started
            && safe.is_some_and(|sh| region::segment_inside(sh, at.truncate(), start_xy, c.radius() * 0.5))
            && (at.z - first.z).abs() < 1.0e-6;
        if linked {
            self.b.push(MoveKind::Link, first, c.feed)?;
        } else {
            if self.started {
                self.b.push(MoveKind::Rapid, at.truncate().extend(self.retract.max(at.z)), 0.0)?;
                if c.rapid_links {
                    self.b.push(MoveKind::Rapid, start_xy.extend(self.retract.max(at.z)), 0.0)?;
                } else {
                    self.b.push(MoveKind::Link, start_xy.extend(self.retract.max(at.z)), c.feed)?;
                }
            } else {
                self.b.at = start_xy.extend(self.clearance);
                self.b.moves.clear();
                self.b.push(MoveKind::Rapid, start_xy.extend(self.retract), 0.0)?;
                self.started = true;
            }
            let rapid_to = (pass.clear_above + self.gap).min(self.b.at.z).max(first.z);
            self.b.push(MoveKind::Rapid, start_xy.extend(rapid_to), 0.0)?;
            let entered = pass.ramp && self.helix(c, start_xy, rapid_to, first.z, safe)?;
            if !entered {
                self.b.push(MoveKind::Plunge, first, c.plunge_feed)?;
            }
        }
        for q in pass.pts.iter().skip(1) {
            self.b.push(MoveKind::Cut, *q, c.feed)?;
        }
        if pass.closed {
            self.b.push(MoveKind::Cut, first, c.feed)?;
        } else {
            let _ = last;
        }
        Ok(())
    }

    /// Ends at clearance height and makes the toolpath.
    pub fn finish(
        mut self,
        p: &Value,
        c: Common,
        id: String,
        strategy: &str,
        default_name: &str,
        mut warnings: Vec<String>,
    ) -> EngineResult<Toolpath> {
        if !self.started {
            return Err(bad("there is nothing to cut with these settings"));
        }
        let at = self.b.at;
        self.b.push(MoveKind::Rapid, at.truncate().extend(self.clearance), 0.0)?;
        let start = self.b.moves.first().map_or(Vec3::ZERO, |m| {
            let l = c.frame.local(m.to);
            c.frame.world(DVec3::new(l.x, l.y, self.clearance))
        });
        if c.tool.spindle_rpm <= 0.0 && p.get("spindle_rpm").and_then(Value::as_f64).unwrap_or(0.0) <= 0.0 {
            warnings.push(format!("T{} has no spindle speed set", c.tool.number));
        }
        warnings.sort();
        warnings.dedup();
        let name = if c.name.is_empty() { format!("{default_name} T{}", c.tool.number) } else { c.name.clone() };
        let mut stored = p.as_object().cloned().unwrap_or_else(Map::new);
        stored.insert("tool".into(), json!(c.tool.number));
        stored.remove("id");
        Ok(Toolpath {
            id,
            name,
            strategy: strategy.into(),
            machine: c.machine,
            tool: c.tool.number,
            wcs: c.wcs,
            params: Value::Object(stored),
            start,
            moves: Arc::new(self.b.moves),
            warnings,
            drill: None,
            visible: false,
        })
    }
}

/// Orders closed contours so each starts nearest where the last ended, rotated to begin
/// there, and runs them in the cutting direction.
pub(super) fn order_loops(mut loops: Vec<Vec<DVec2>>, mut from: DVec2) -> Vec<Vec<DVec2>> {
    let mut out = Vec::with_capacity(loops.len());
    while !loops.is_empty() {
        let mut best = (0usize, 0usize, f64::INFINITY);
        for (i, l) in loops.iter().enumerate() {
            for (k, q) in l.iter().enumerate() {
                let d = q.distance_squared(from);
                if d < best.2 {
                    best = (i, k, d);
                }
            }
        }
        let mut l = loops.swap_remove(best.0);
        let k = best.1.min(l.len().saturating_sub(1));
        l.rotate_left(k);
        from = l.first().copied().unwrap_or(from);
        out.push(l);
    }
    out
}

/// A shape's contours, each turned to run with climb milling (`climb`) or against it, for a
/// tool cutting the inside of the shape's outer contour and the outside of its holes.
pub(super) fn cutting_contours(shapes: &[Shape], climb: bool) -> Vec<Vec<DVec2>> {
    // Outer contours come counter-clockwise and holes clockwise: both climb on the inside.
    shapes
        .iter()
        .flatten()
        .map(|c| {
            let mut c = c.clone();
            if !climb {
                c.reverse();
            }
            c
        })
        .collect()
}
