//! Stock: the raw material the part is cut from. For now a box sized to the models; the
//! shaped stocks and per-face margins of Phase 5 build on it.

use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{Session, stock_json};

const MARGIN: ParamKind = ParamKind::Length { min: 0.0, max: 10_000.0 };
const MAX_MARGIN: f64 = 10_000.0;

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "setup.stock.fit",
        label: "Fit to model",
        tooltip: "Size the stock box to the models' bounding box, plus margins",
        placement: Some(Placement::small(RibbonTab::Setup, "Stock")),
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("margin_sides", MARGIN, "Extra material on the four sides, mm (default 0)"),
            Param::optional("margin_top", MARGIN, "Extra material on top, mm (default 0)"),
            Param::optional("margin_bottom", MARGIN, "Extra material underneath, mm (default 0)"),
        ],
        returns: "the stock {shape, min, max, size}",
        enabled: always,
        run: fit,
    },
    CommandSpec {
        id: "setup.stock.get",
        label: "Stock",
        tooltip: "The job's stock",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "the stock {shape, min, max, size}, or null when none is defined",
        enabled: always,
        run: |s, _| Ok(s.job.stock.as_ref().map_or(Value::Null, stock_json)),
    },
    CommandSpec {
        id: "setup.stock.in_process.get",
        label: "In-process stock",
        tooltip: "The stock as the toolpaths up to one leave it; the viewport shows it for the selected toolpath",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("id", ParamKind::Str, "Toolpath id: the stock after it, e.g. toolpath.3")],
        returns: "{id, removed_volume, remaining_volume, cell, mode, events}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "id")?;
            let sim = crate::sim::in_process(s, id)?;
            let v = sim.to_json();
            Ok(json!({
                "id": id,
                "removed_volume": v["removed_volume"],
                "remaining_volume": v["remaining_volume"],
                "cell": v["cell"],
                "mode": v["mode"],
                "events": v["events"],
            }))
        },
    },
];

fn fit(s: &mut Session, p: &Value) -> EngineResult<Value> {
    let bounds = s.job.models_bounds();
    if bounds.is_empty() {
        return Err(EngineError::InvalidParams("there is no model to fit the stock to; import a model first".into()));
    }
    let margin = |key: &str| params::opt_f64(p, key, 0.0, MAX_MARGIN).map(|m| m.unwrap_or(0.0) as f32);
    let (sides, top, bottom) = (margin("margin_sides")?, margin("margin_top")?, margin("margin_bottom")?);
    let mut stock = bounds;
    stock.min -= CosmicDMS_geom::Vec3::new(sides, sides, bottom);
    stock.max += CosmicDMS_geom::Vec3::new(sides, sides, top);
    if !(stock.min.is_finite() && stock.max.is_finite()) {
        return Err(EngineError::InvalidParams("the stock would be out of range".into()));
    }
    s.job.set_stock(Some(stock));
    Ok(json!(stock_json(&stock)))
}
