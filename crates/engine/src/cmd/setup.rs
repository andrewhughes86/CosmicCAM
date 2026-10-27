//! Setups: how the part is held on a machine, the WCS it is programmed in and the stock it is
//! cut from. The active setup's stock is the job's stock and its WCS the active one.

use CosmicDMS_geom::{Aabb, Vec3};
use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{MAX_SETUPS, Session, Setup, stock_json};

/// How a stock is sized.
pub const STOCK_MODES: &[&str] = &["fit", "size", "box", "none"];
/// Where a fixed-size stock sits on the models.
pub const STOCK_ALIGNS: &[&str] = &["top", "center", "bottom"];

const MARGIN: ParamKind = ParamKind::Length { min: 0.0, max: 10_000.0 };
const ROUND: ParamKind = ParamKind::Length { min: 0.0, max: 1_000.0 };
/// Largest stock dimension or coordinate, mm.
const MAX_STOCK: f32 = 1.0e6;
/// Longest setup name.
const MAX_NAME: usize = 128;

macro_rules! stock_params {
    () => {
        [
            Param::optional(
                "stock",
                ParamKind::Choice(STOCK_MODES),
                "How the stock is sized: fit (the models' box plus margins), size (a fixed size placed on the \
                 models), box (two corners) or none (default fit)",
            ),
            Param::optional("stock_margin_sides", MARGIN, "Fit: extra material on the four sides, mm (default 0)"),
            Param::optional("stock_margin_top", MARGIN, "Fit and size: extra material on top, mm (default 0)"),
            Param::optional("stock_margin_bottom", MARGIN, "Fit: extra material underneath, mm (default 0)"),
            Param::optional("stock_size", ParamKind::Vec3, "Size: the stock's X, Y and Z size, mm"),
            Param::optional(
                "stock_align",
                ParamKind::Choice(STOCK_ALIGNS),
                "Size: where the stock sits on the models, centred in X and Y: its top at the model top plus the \
                 top margin, its centre on theirs, or its bottom on theirs (default top)",
            ),
            Param::optional("stock_min", ParamKind::Vec3, "Box: the low corner, world mm"),
            Param::optional("stock_max", ParamKind::Vec3, "Box: the high corner, world mm"),
            Param::optional(
                "stock_round",
                ROUND,
                "Fit and size: round each dimension up to a multiple of this, mm (default 0, no rounding)",
            ),
        ]
    };
}

const CREATE_PARAMS: &[Param] = &{
    let stock = stock_params!();
    let mut all = [Param::required("name", ParamKind::Str, ""); 13];
    all[0] = Param::required("name", ParamKind::Str, "Setup name");
    all[1] = Param::optional("machine", ParamKind::Str, "Machine it runs on, e.g. machine.1 (default: the active one)");
    all[2] = Param::optional(
        "wcs",
        ParamKind::Str,
        "WCS it is programmed in, e.g. wcs.1, or `origin` for the world origin (default: the active WCS)",
    );
    all[3] = Param::optional("activate", ParamKind::Bool, "Make it the active setup (default true)");
    let mut i = 0;
    while i < stock.len() {
        all[4 + i] = stock[i];
        i += 1;
    }
    all
};

const EDIT_PARAMS: &[Param] = &{
    let stock = stock_params!();
    let mut all = [Param::required("id", ParamKind::Str, ""); 13];
    all[0] = Param::required("id", ParamKind::Str, "Setup id, e.g. setup.1");
    all[1] = Param::optional("name", ParamKind::Str, "New name");
    all[2] = Param::optional("machine", ParamKind::Str, "Machine it runs on");
    all[3] = Param::optional("wcs", ParamKind::Str, "WCS it is programmed in, or `origin`");
    let mut i = 0;
    while i < stock.len() {
        all[4 + i] = stock[i];
        i += 1;
    }
    all
};

const STOCK_PARAMS: &[Param] = &stock_params!();

const SETUP_RETURNS: &str = "the setup {id, name, active, machine, wcs, stock}";

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "setup.create",
        label: "Create setup",
        tooltip: "Start a setup: its name, machine, WCS and stock",
        placement: Some(Placement::large(RibbonTab::Setup, "Setup")),
        icon: None,
        shortcut: None,
        params: CREATE_PARAMS,
        returns: SETUP_RETURNS,
        enabled: always,
        run: create,
    },
    CommandSpec {
        id: "setup.edit",
        label: "Edit setup",
        tooltip: "Change a setup's name, machine, WCS or stock",
        placement: None,
        icon: None,
        shortcut: None,
        params: EDIT_PARAMS,
        returns: SETUP_RETURNS,
        enabled: always,
        run: edit,
    },
    CommandSpec {
        id: "setup.activate",
        label: "Activate setup",
        tooltip: "Work in this setup: its stock, WCS and machine become the current ones",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Setup id, e.g. setup.1")],
        returns: SETUP_RETURNS,
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            activate(s, &id)?;
            setup_json(s, &id)
        },
    },
    CommandSpec {
        id: "setup.remove",
        label: "Remove setup",
        tooltip: "Delete a setup",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Setup id, e.g. setup.1")],
        returns: "{\"removed\": id, \"active_setup\": id or null}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?.to_owned();
            let before = s.job.setups.len();
            s.job.setups.retain(|st| st.id != id);
            if s.job.setups.len() == before {
                return Err(missing(&id));
            }
            if s.job.active_setup.as_deref() == Some(id.as_str()) {
                s.job.active_setup = None;
                match s.job.setups.first().map(|st| st.id.clone()) {
                    Some(next) => activate(s, &next)?,
                    None => s.job.set_stock(None),
                }
            }
            if s.view.selection.as_deref().is_some_and(|sel| sel == id || sel.starts_with(&format!("{id}/"))) {
                s.view.selection = None;
            }
            s.job.setup_changed();
            Ok(json!({ "removed": id, "active_setup": s.job.active_setup }))
        },
    },
    CommandSpec {
        id: "setup.list",
        label: "Setups",
        tooltip: "The job's setups",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "list of setups {id, name, active, machine, wcs, stock}",
        enabled: always,
        run: |s, _| {
            let active = s.job.active_setup.as_deref();
            Ok(json!(s.job.setups.iter().map(|st| st.to_json(active == Some(st.id.as_str()))).collect::<Vec<_>>()))
        },
    },
    CommandSpec {
        id: "setup.get",
        label: "Setup",
        tooltip: "One setup",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Setup id, e.g. setup.1")],
        returns: SETUP_RETURNS,
        enabled: always,
        run: |s, p| setup_json(s, params::req_str(p, "id")?),
    },
    CommandSpec {
        id: "setup.stock.box",
        label: "Box stock",
        tooltip: "Rectangular block stock for the active setup: fitted to the models, a fixed size, or two corners",
        placement: Some(Placement::large(RibbonTab::Setup, "Stock")),
        icon: None,
        shortcut: None,
        params: STOCK_PARAMS,
        returns: "the stock {shape, min, max, size}, or null for none",
        enabled: always,
        run: |s, p| {
            let stock = build_stock(s, p, "fit")?;
            s.job.set_stock(stock);
            Ok(stock.as_ref().map_or(Value::Null, stock_json))
        },
    },
];

fn missing(id: &str) -> EngineError {
    EngineError::InvalidParams(format!("no setup `{id}`"))
}

fn setup_json(s: &Session, id: &str) -> EngineResult<Value> {
    let st = s.job.setup_by_id(id).ok_or_else(|| missing(id))?;
    Ok(st.to_json(s.job.active_setup.as_deref() == Some(id)))
}

fn name(p: &Value, key: &str) -> EngineResult<Option<String>> {
    match params::opt_str(p, key)? {
        None => Ok(None),
        Some(n) if n.trim().is_empty() => Err(EngineError::InvalidParams("give the setup a name".into())),
        Some(n) => Ok(Some(n.trim().chars().take(MAX_NAME).collect())),
    }
}

fn machine(s: &Session, p: &Value) -> EngineResult<Option<String>> {
    match params::opt_str(p, "machine")? {
        None => Ok(None),
        Some(id) => match s.library.machine(id) {
            Some(m) => Ok(Some(m.id.clone())),
            None => Err(EngineError::InvalidParams(format!("no machine `{id}`"))),
        },
    }
}

/// The `wcs` parameter: Some(None) for the origin, Some(Some(id)) for a WCS.
fn wcs(s: &Session, p: &Value) -> EngineResult<Option<Option<String>>> {
    match params::opt_str(p, "wcs")? {
        None => Ok(None),
        Some("origin") => Ok(Some(None)),
        Some(id) => match s.job.wcs_by_id(id) {
            Some(w) => Ok(Some(Some(w.id.clone()))),
            None => Err(EngineError::InvalidParams(format!("no WCS `{id}`"))),
        },
    }
}

fn create(s: &mut Session, p: &Value) -> EngineResult<Value> {
    if s.job.setups.len() >= MAX_SETUPS {
        return Err(EngineError::InvalidParams(format!("a job holds at most {MAX_SETUPS} setups")));
    }
    let name = name(p, "name")?.ok_or_else(|| EngineError::InvalidParams("give the setup a name".into()))?;
    let machine = match machine(s, p)? {
        Some(m) => m,
        None => {
            s.library.active_machine().or_else(|| s.library.machines.first()).map(|m| m.id.clone()).ok_or_else(
                || EngineError::InvalidParams("there is no machine; create one on the Machine tab".into()),
            )?
        }
    };
    let wcs = wcs(s, p)?.unwrap_or_else(|| s.job.active_wcs.clone());
    let default_mode = if s.job.models_bounds().is_empty() { "none" } else { "fit" };
    let stock = build_stock(s, p, default_mode)?;
    let id = format!("setup.{}", s.job.next_setup);
    s.job.next_setup = s.job.next_setup.saturating_add(1);
    s.job.setups.push(Setup { id: id.clone(), name, machine, wcs, stock });
    if params::opt_bool(p, "activate")?.unwrap_or(true) || s.job.active_setup.is_none() {
        activate(s, &id)?;
    }
    s.job.setup_changed();
    setup_json(s, &id)
}

fn edit(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let id = params::req_str(p, "id")?.to_owned();
    let index = s.job.setups.iter().position(|st| st.id == id).ok_or_else(|| missing(&id))?;
    let name = name(p, "name")?;
    let machine = machine(s, p)?;
    let wcs = wcs(s, p)?;
    let stock = if p.get("stock").is_some() || STOCK_PARAMS.iter().any(|q| p.get(q.name).is_some()) {
        Some(build_stock(s, p, "fit")?)
    } else {
        None
    };
    let st = s.job.setups.get_mut(index).ok_or_else(|| missing(&id))?;
    if let Some(n) = name {
        st.name = n;
    }
    if let Some(m) = machine {
        st.machine = m;
    }
    if let Some(w) = wcs {
        st.wcs = w;
    }
    if let Some(stock) = stock {
        st.stock = stock;
    }
    if s.job.active_setup.as_deref() == Some(id.as_str()) {
        activate(s, &id)?;
    }
    s.job.setup_changed();
    setup_json(s, &id)
}

/// Makes `id` the active setup: its stock becomes the job's, its WCS and machine the active ones.
pub(crate) fn activate(s: &mut Session, id: &str) -> EngineResult<()> {
    let st = s.job.setup_by_id(id).ok_or_else(|| missing(id))?.clone();
    s.job.active_setup = Some(st.id.clone());
    s.job.stock = st.stock;
    s.job.active_wcs = st.wcs.clone();
    if s.library.machine(&st.machine).is_some() && s.library.active.as_deref() != Some(st.machine.as_str()) {
        s.library.active = Some(st.machine.clone());
        s.library.changed();
    }
    s.job.setup_changed();
    Ok(())
}

fn finite_point(v: Vec3) -> bool {
    v.is_finite() && v.abs().max_element() <= MAX_STOCK
}

/// Rounds `b`'s size up to multiples of `step`: X and Y grow evenly on both sides, Z downward
/// so the top stays where it is.
fn round_up(b: Aabb, step: f32) -> Aabb {
    if step <= 0.0 {
        return b;
    }
    let size = b.size();
    let want = Vec3::new((size.x / step).ceil() * step, (size.y / step).ceil() * step, (size.z / step).ceil() * step);
    let grow = (want - size).max(Vec3::ZERO);
    Aabb {
        min: Vec3::new(b.min.x - grow.x / 2.0, b.min.y - grow.y / 2.0, b.min.z - grow.z),
        max: Vec3::new(b.max.x + grow.x / 2.0, b.max.y + grow.y / 2.0, b.max.z),
    }
}

/// The stock the `stock*` parameters describe; `default_mode` when `stock` is not given.
pub(crate) fn build_stock(s: &Session, p: &Value, default_mode: &str) -> EngineResult<Option<Aabb>> {
    let bad = |m: &str| EngineError::InvalidParams(m.to_owned());
    let mode = params::opt_str(p, "stock")?.unwrap_or(default_mode);
    let margin = |key: &str| params::opt_f64(p, key, 0.0, 10_000.0).map(|m| m.unwrap_or(0.0) as f32);
    let round = params::opt_f64(p, "stock_round", 0.0, 1_000.0)?.unwrap_or(0.0) as f32;
    let models = s.job.models_bounds();
    let stock = match mode {
        "none" => return Ok(None),
        "fit" => {
            if models.is_empty() {
                return Err(bad("there is no model to fit the stock to; import one or give the stock a size"));
            }
            let (sides, top, bottom) =
                (margin("stock_margin_sides")?, margin("stock_margin_top")?, margin("stock_margin_bottom")?);
            round_up(
                Aabb {
                    min: models.min - Vec3::new(sides, sides, bottom),
                    max: models.max + Vec3::new(sides, sides, top),
                },
                round,
            )
        }
        "size" => {
            let size = params::opt_vec3(p, "stock_size")?.ok_or_else(|| bad("give the stock's size (stock_size)"))?;
            if !(size.min_element() > 0.0 && size.max_element() <= MAX_STOCK) {
                return Err(bad("each stock dimension must be more than 0"));
            }
            let size = round_up(Aabb { min: Vec3::ZERO, max: size }, round).size();
            let (centre, top, bottom) =
                if models.is_empty() { (Vec3::ZERO, 0.0, 0.0) } else { (models.center(), models.max.z, models.min.z) };
            let z0 = match params::opt_str(p, "stock_align")?.unwrap_or("top") {
                "center" => centre.z - size.z / 2.0,
                "bottom" => bottom,
                _ => top + margin("stock_margin_top")? - size.z,
            };
            let min = Vec3::new(centre.x - size.x / 2.0, centre.y - size.y / 2.0, z0);
            Aabb { min, max: min + size }
        }
        _ => {
            let (a, b) = (
                params::opt_vec3(p, "stock_min")?.ok_or_else(|| bad("give the stock's low corner (stock_min)"))?,
                params::opt_vec3(p, "stock_max")?.ok_or_else(|| bad("give the stock's high corner (stock_max)"))?,
            );
            let b = Aabb { min: a.min(b), max: a.max(b) };
            if b.size().min_element() <= 0.0 {
                return Err(bad("the stock corners must differ in X, Y and Z"));
            }
            b
        }
    };
    if !(finite_point(stock.min) && finite_point(stock.max)) {
        return Err(bad("the stock would be out of range"));
    }
    Ok(Some(stock))
}
