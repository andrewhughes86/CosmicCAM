//! Viewport camera, projection, theme, display and layout commands.

use std::f32::consts::FRAC_PI_2;

use CosmicCAM_render::camera::{MAX_DISTANCE, MIN_DISTANCE, wrap_angle};
use CosmicCAM_render::{Projection, ViewPreset};
use serde_json::{Value, json};

use super::{CommandSpec, Param, ParamKind, Placement, RibbonTab, always};
use crate::error::{EngineError, EngineResult};
use crate::params;
use crate::session::{Display, MAX_FPS_LIMIT, Session, Theme, camera_json};

const HOME_VIEW: &str = "View";
const HOME_DISPLAY: &str = "Display";

const fn large(panel: &'static str) -> Option<Placement> {
    Some(Placement::large(RibbonTab::Home, panel))
}

const fn small(panel: &'static str) -> Option<Placement> {
    Some(Placement::small(RibbonTab::Home, panel))
}

/// Every standard view, offered from the Iso split button.
const VIEW_MENU: &[&str] = &[
    "view.camera.iso",
    "view.camera.top",
    "view.camera.front",
    "view.camera.right",
    "view.camera.back",
    "view.camera.left",
    "view.camera.bottom",
];

/// Accepted by `view.camera.preset`; a test keeps it in step with `ViewPreset`.
pub(crate) const PRESET_IDS: &[&str] = &[
    "iso",
    "top",
    "bottom",
    "front",
    "back",
    "left",
    "right",
    "iso-front-right",
    "iso-front-left",
    "iso-back-right",
    "iso-back-left",
];

const DEGREES: ParamKind = ParamKind::Number { min: -1.0e6, max: 1.0e6 };
const MILLIMETRES: ParamKind = ParamKind::Number { min: -1.0e9, max: 1.0e9 };
const CAMERA_RETURNS: &str = "camera {target, distance, yaw, pitch, projection, eye}";

const DISPLAY_PARAMS: &[Param] = &[
    Param::optional("grid", ParamKind::Bool, "Ground grid"),
    Param::optional("wcs", ParamKind::Bool, "WCS triad in the scene"),
    Param::optional("view_cube", ParamKind::Bool, "View cube overlay"),
    Param::optional("orientation_triad", ParamKind::Bool, "Screen-corner orientation triad"),
    Param::optional("stock", ParamKind::Bool, "Stock outline"),
    Param::optional("edges", ParamKind::Bool, "Model outline edges"),
    Param::optional("readout", ParamKind::Bool, "Projection and frame-rate readout"),
];

macro_rules! preset_command {
    ($id:literal, $label:literal, $shortcut:expr, $preset:expr, $placement:expr) => {
        CommandSpec {
            id: $id,
            label: $label,
            tooltip: concat!("Look at the part from the ", $label, " view"),
            placement: $placement,
            icon: None,
            shortcut: $shortcut,
            params: &[],
            returns: "{\"preset\": id}",
            enabled: always,
            run: |s, _| set_preset(s, $preset),
        }
    };
}

pub(super) static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "view.fit",
        label: "Fit",
        tooltip: "Zoom to fit everything in the viewport",
        placement: large(HOME_VIEW),
        icon: None,
        shortcut: Some("F"),
        params: &[],
        returns: CAMERA_RETURNS,
        enabled: always,
        run: |s, _| {
            let bounds = s.view.scene_bounds;
            let aspect = s.view.aspect;
            s.view.camera.fit(&bounds, aspect);
            Ok(camera_json(&s.view.camera))
        },
    },
    preset_command!(
        "view.camera.iso",
        "Iso",
        Some("0"),
        ViewPreset::IsoFrontRight,
        Some(Placement::large(RibbonTab::Home, HOME_VIEW).with_menu(VIEW_MENU))
    ),
    preset_command!("view.camera.top", "Top", Some("7"), ViewPreset::Top, small(HOME_VIEW)),
    preset_command!("view.camera.front", "Front", Some("1"), ViewPreset::Front, small(HOME_VIEW)),
    preset_command!("view.camera.right", "Right", Some("3"), ViewPreset::Right, small(HOME_VIEW)),
    preset_command!("view.camera.bottom", "Bottom", None, ViewPreset::Bottom, None),
    preset_command!("view.camera.back", "Back", None, ViewPreset::Back, None),
    preset_command!("view.camera.left", "Left", None, ViewPreset::Left, None),
    CommandSpec {
        id: "view.camera.preset",
        label: "View preset",
        tooltip: "Orient the camera to a standard view",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("preset", ParamKind::Choice(PRESET_IDS), "Standard view")],
        returns: "{\"preset\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "preset")?;
            let preset =
                ViewPreset::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown preset `{id}`")))?;
            set_preset(s, preset)
        },
    },
    CommandSpec {
        id: "view.camera.get",
        label: "Camera",
        tooltip: "Where the camera is and how it looks",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: CAMERA_RETURNS,
        enabled: always,
        run: |s, _| Ok(camera_json(&s.view.camera)),
    },
    CommandSpec {
        id: "view.camera.set",
        label: "Set camera",
        tooltip: "Place the camera exactly; omitted values are kept",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("target", ParamKind::Vec3, "Point the camera orbits and looks at, mm"),
            Param::optional(
                "distance",
                ParamKind::Number { min: MIN_DISTANCE as f64, max: MAX_DISTANCE as f64 },
                "Distance from the target, mm",
            ),
            Param::optional("yaw", DEGREES, "Degrees around +Z from +X; -90 looks from the front"),
            Param::optional("pitch", ParamKind::Number { min: -90.0, max: 90.0 }, "Degrees above the XY plane"),
        ],
        returns: CAMERA_RETURNS,
        enabled: always,
        run: |s, p| {
            let c = &mut s.view.camera;
            if let Some(t) = params::opt_vec3(p, "target")? {
                c.target = t;
            }
            if let Some(d) = params::opt_f64(p, "distance", f64::from(MIN_DISTANCE), f64::from(MAX_DISTANCE))? {
                c.distance = d as f32;
            }
            if let Some(y) = params::opt_f64(p, "yaw", -1.0e6, 1.0e6)? {
                c.yaw = wrap_angle((y as f32).to_radians());
            }
            if let Some(pitch) = params::opt_f64(p, "pitch", -90.0, 90.0)? {
                c.pitch = (pitch as f32).to_radians().clamp(-FRAC_PI_2, FRAC_PI_2);
            }
            *c = c.sanitized();
            Ok(camera_json(c))
        },
    },
    CommandSpec {
        id: "view.camera.orbit",
        label: "Orbit",
        tooltip: "Turn the camera around its target",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("yaw", DEGREES, "Degrees to turn around +Z"),
            Param::optional("pitch", DEGREES, "Degrees to tilt up (clamped to straight up or down)"),
        ],
        returns: CAMERA_RETURNS,
        enabled: always,
        run: |s, p| {
            let c = &mut s.view.camera;
            let dy = params::opt_f64(p, "yaw", -1.0e6, 1.0e6)?.unwrap_or(0.0) as f32;
            let dp = params::opt_f64(p, "pitch", -1.0e6, 1.0e6)?.unwrap_or(0.0) as f32;
            c.yaw = wrap_angle(c.yaw + dy.to_radians());
            c.pitch = (c.pitch + dp.to_radians()).clamp(-FRAC_PI_2, FRAC_PI_2);
            *c = c.sanitized();
            Ok(camera_json(c))
        },
    },
    CommandSpec {
        id: "view.camera.pan",
        label: "Pan",
        tooltip: "Slide the camera and its target across the screen",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[
            Param::optional("right", MILLIMETRES, "Millimetres towards screen right"),
            Param::optional("up", MILLIMETRES, "Millimetres towards screen top"),
        ],
        returns: CAMERA_RETURNS,
        enabled: always,
        run: |s, p| {
            let c = &mut s.view.camera;
            let right = params::opt_f64(p, "right", -1.0e9, 1.0e9)?.unwrap_or(0.0) as f32;
            let up = params::opt_f64(p, "up", -1.0e9, 1.0e9)?.unwrap_or(0.0) as f32;
            let b = c.basis();
            c.target += b.right * right + b.up * up;
            *c = c.sanitized();
            Ok(camera_json(c))
        },
    },
    CommandSpec {
        id: "view.camera.zoom",
        label: "Zoom",
        tooltip: "Magnify the view about its centre",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required(
            "factor",
            ParamKind::Number { min: 0.001, max: 1000.0 },
            "Magnification; 2 doubles the size on screen, 0.5 halves it",
        )],
        returns: CAMERA_RETURNS,
        enabled: always,
        run: |s, p| {
            let factor = params::opt_f64(p, "factor", 0.001, 1000.0)?.unwrap_or(1.0) as f32;
            let c = &mut s.view.camera;
            c.distance = (c.distance / factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
            Ok(camera_json(c))
        },
    },
    CommandSpec {
        id: "view.projection.toggle",
        label: "Perspective",
        tooltip: "Switch between orthographic and perspective projection",
        placement: small(HOME_DISPLAY),
        icon: None,
        shortcut: Some("P"),
        params: &[],
        returns: "{\"projection\": id}",
        enabled: always,
        run: |s, _| {
            let next = match s.view.camera.projection {
                Projection::Orthographic => Projection::Perspective,
                Projection::Perspective => Projection::Orthographic,
            };
            set_projection(s, next)
        },
    },
    CommandSpec {
        id: "view.projection.set",
        label: "Projection",
        tooltip: "Set the viewport projection",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required(
            "projection",
            ParamKind::Choice(&["orthographic", "perspective"]),
            "Projection mode",
        )],
        returns: "{\"projection\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "projection")?;
            let projection = Projection::from_id(id)
                .ok_or_else(|| EngineError::InvalidParams(format!("unknown projection `{id}`")))?;
            set_projection(s, projection)
        },
    },
    CommandSpec {
        id: "view.theme.toggle",
        label: "Light theme",
        tooltip: "Switch between the dark and light themes",
        placement: small(HOME_DISPLAY),
        icon: None,
        shortcut: Some("Ctrl+Shift+T"),
        params: &[],
        returns: "{\"theme\": id}",
        enabled: always,
        run: |s, _| {
            let next = match s.prefs.theme {
                Theme::Dark => Theme::Light,
                Theme::Light => Theme::Dark,
            };
            set_theme(s, next)
        },
    },
    CommandSpec {
        id: "view.theme.set",
        label: "Theme",
        tooltip: "Set the colour theme",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required("theme", ParamKind::Choice(&["dark", "light"]), "Colour theme")],
        returns: "{\"theme\": id}",
        enabled: always,
        run: |s, p| {
            let id = params::req_str(p, "theme")?;
            let theme =
                Theme::from_id(id).ok_or_else(|| EngineError::InvalidParams(format!("unknown theme `{id}`")))?;
            set_theme(s, theme)
        },
    },
    CommandSpec {
        id: "view.display.set",
        label: "Display",
        tooltip: "Show or hide viewport elements; omitted elements are kept",
        placement: None,
        icon: None,
        shortcut: None,
        params: DISPLAY_PARAMS,
        returns: "display {grid, wcs, view_cube, orientation_triad, stock, edges, readout}",
        enabled: always,
        run: |s, p| {
            let mut display = s.prefs.display;
            for key in Display::KEYS {
                if let Some(on) = params::opt_bool(p, key)? {
                    display.set(key, on);
                }
            }
            s.prefs.display = display;
            Ok(display.to_json())
        },
    },
    CommandSpec {
        id: "view.display.get",
        label: "Display state",
        tooltip: "Which viewport elements are shown",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[],
        returns: "display {grid, wcs, view_cube, orientation_triad, stock, edges, readout}",
        enabled: always,
        run: |s, _| Ok(s.prefs.display.to_json()),
    },
    CommandSpec {
        id: "view.grid.toggle",
        label: "Grid",
        tooltip: "Show or hide the ground grid",
        placement: small(HOME_DISPLAY),
        icon: None,
        shortcut: Some("G"),
        params: &[],
        returns: "{\"grid\": bool}",
        enabled: always,
        run: |s, _| {
            s.prefs.display.grid = !s.prefs.display.grid;
            Ok(json!({ "grid": s.prefs.display.grid }))
        },
    },
    CommandSpec {
        id: "view.layout.reset",
        label: "Reset layout",
        tooltip: "Put the workspace panels back where they started",
        placement: small(HOME_DISPLAY),
        icon: None,
        shortcut: None,
        params: &[],
        returns: "{\"layout_generation\": n}",
        enabled: always,
        run: |s, _| {
            s.view.layout_generation = s.view.layout_generation.wrapping_add(1);
            Ok(json!({ "layout_generation": s.view.layout_generation }))
        },
    },
    CommandSpec {
        id: "view.fps_limit.set",
        label: "Frame-rate limit",
        tooltip: "Cap viewport frames per second while the view is moving",
        placement: None,
        icon: None,
        shortcut: None,
        params: &[Param::required(
            "fps",
            ParamKind::Integer { min: 1, max: MAX_FPS_LIMIT as i64 },
            "Frames per second",
        )],
        returns: "{\"fps\": n}",
        enabled: always,
        run: |s, p| {
            let fps = params::opt_f64(p, "fps", 1.0, f64::from(MAX_FPS_LIMIT))?
                .ok_or_else(|| EngineError::InvalidParams("`fps` is required".into()))?;
            s.prefs.fps_limit = fps.round() as u32;
            Ok(json!({ "fps": s.prefs.fps_limit }))
        },
    },
];

fn set_preset(s: &mut Session, preset: ViewPreset) -> EngineResult<Value> {
    s.view.camera.set_preset(preset);
    Ok(json!({ "preset": preset.id() }))
}

fn set_projection(s: &mut Session, projection: Projection) -> EngineResult<Value> {
    s.view.camera.projection = projection;
    Ok(json!({ "projection": projection.id() }))
}

fn set_theme(s: &mut Session, theme: Theme) -> EngineResult<Value> {
    s.prefs.theme = theme;
    Ok(json!({ "theme": theme.id() }))
}
