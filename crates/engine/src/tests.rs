use std::collections::HashSet;

use CosmicCAM_render::{Projection, ViewPreset};
use serde_json::{Value, json};

use crate::cmd::{self, RibbonTab};
use crate::{EngineError, Session, Theme};

fn id_is_well_formed(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    (2..=4).contains(&parts.len())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
                && p.starts_with(|c: char| c.is_ascii_lowercase())
        })
}

#[test]
fn command_ids_are_unique_and_well_formed() {
    let mut seen = HashSet::new();
    for spec in cmd::all() {
        assert!(id_is_well_formed(spec.id), "bad id {}", spec.id);
        assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
        assert!(!spec.label.is_empty(), "{} has no label", spec.id);
    }
}

#[test]
fn every_ribbon_tab_has_primary_actions() {
    for tab in RibbonTab::ALL {
        let panels = cmd::ribbon(tab);
        assert!(!panels.is_empty(), "{tab:?} is empty");
        assert!(panels.iter().all(|(_, cmds)| !cmds.is_empty()));
    }
}

#[test]
fn planned_commands_are_disabled_and_say_so() {
    let mut s = Session::new();
    let spec = cmd::find("setup.stock.cylinder");
    assert!(spec.is_some_and(|c| !s.is_enabled(c)));
    assert_eq!(
        s.execute("setup.stock.cylinder", &Value::Null),
        Err(EngineError::Disabled("setup.stock.cylinder".into()))
    );
    let run = spec.map(|c| (c.run)(&mut s, &Value::Null));
    assert!(matches!(run, Some(Err(EngineError::NotSupported(_)))));
}

#[test]
fn theme_commands() {
    let mut s = Session::new();
    assert_eq!(s.prefs.theme, Theme::Dark);
    assert!(s.execute("view.theme.toggle", &Value::Null).is_ok());
    assert_eq!(s.prefs.theme, Theme::Light);
    assert!(s.execute("view.theme.set", &json!({"theme": "dark"})).is_ok());
    assert_eq!(s.prefs.theme, Theme::Dark);
    assert!(matches!(s.execute("view.theme.set", &json!({"theme": "neon"})), Err(EngineError::InvalidParams(_))));
    assert_eq!(s.prefs.theme, Theme::Dark);
}

#[test]
fn camera_commands() {
    let mut s = Session::with_sample();
    assert!(s.execute("view.camera.preset", &json!({"preset": "top"})).is_ok());
    let (yaw, pitch) = ViewPreset::Top.angles();
    assert_eq!((s.view.camera.yaw, s.view.camera.pitch), (yaw, pitch));
    assert!(s.execute("view.projection.set", &json!({"projection": "perspective"})).is_ok());
    assert_eq!(s.view.camera.projection, Projection::Perspective);
    assert!(s.execute("view.projection.toggle", &Value::Null).is_ok());
    assert_eq!(s.view.camera.projection, Projection::Orthographic);
}

#[test]
fn a_panicking_command_restores_the_session() {
    let mut s = Session::with_sample();
    let before = s.job.name.clone();
    let r = s.execute("test.panic", &Value::Null);
    assert!(matches!(r, Err(EngineError::Internal(ref m)) if m.contains("boom")), "{r:?}");
    assert_eq!(s.job.name, before);
    assert_eq!(s.log().len(), 1);
}

#[test]
fn hostile_params_never_panic() {
    let hostile = [
        Value::Null,
        json!(42),
        json!("text"),
        json!([1, 2, 3]),
        json!({"preset": 7, "theme": null, "projection": ["x"], "fps": 1e308, "id": {}}),
        json!({"preset": "x".repeat(100_000), "fps": -1, "sample": "yes"}),
        json!({"fps": f64::MAX, "id": "no-such-node"}),
        json!({"fps": 0.000001, "id": null, "sample": true}),
        json!({"path": "/dev/zero", "units": "furlong", "name": 5}),
        json!({"path": "/", "units": "mm"}),
        json!({"path": "\u{0}.stl", "id": "model.1"}),
        json!({"path": "x".repeat(4096), "name": ""}),
    ];
    for spec in cmd::all().filter(|c| c.id != "test.panic") {
        for params in &hostile {
            let mut s = Session::with_sample();
            let r = s.execute(spec.id, params);
            assert!(!matches!(r, Err(EngineError::Internal(_))), "{} panicked on {params}", spec.id);
            assert!(s.view.camera.view_proj(s.view.aspect, 100.0).is_finite());
        }
    }
    let mut s = Session::new();
    assert!(
        matches!(s.execute("\u{0}x".repeat(10_000).as_str(), &Value::Null), Err(EngineError::UnknownCommand(m)) if m.len() <= 128)
    );
}

#[test]
fn fps_limit_is_bounded() {
    let mut s = Session::new();
    assert!(s.execute("view.fps_limit.set", &json!({"fps": 30})).is_ok());
    assert_eq!(s.prefs.fps_limit, 30);
    assert!(s.execute("view.fps_limit.set", &json!({"fps": 10_000})).is_err());
    assert!(s.execute("view.fps_limit.set", &json!({"fps": "fast"})).is_err());
    assert_eq!(s.prefs.fps_limit, 30);
}

#[test]
fn selection_must_name_an_outline_node() {
    let mut s = Session::with_sample();
    assert!(s.execute("selection.set", &json!({"id": "model.sample"})).is_ok());
    assert_eq!(s.view.selection.as_deref(), Some("model.sample"));
    assert!(s.execute("selection.set", &json!({"id": "nope"})).is_err());
    assert_eq!(s.view.selection.as_deref(), Some("model.sample"));
    assert!(s.execute("selection.set", &json!({})).is_ok());
    assert_eq!(s.view.selection, None);
}

#[test]
fn job_new_keeps_preferences_and_log() {
    let mut s = Session::with_sample();
    assert!(s.execute("view.theme.set", &json!({"theme": "light"})).is_ok());
    assert!(s.execute("job.new", &Value::Null).is_ok());
    assert!(!s.job.sample_loaded);
    assert_eq!(s.prefs.theme, Theme::Light);
    assert_eq!(s.log().len(), 2);
    assert!(s.execute("console.clear", &Value::Null).is_ok());
    assert_eq!(s.log().len(), 1);
}

#[test]
fn log_is_capped() {
    let mut s = Session::new();
    for _ in 0..crate::session::MAX_LOG + 50 {
        let _ = s.execute("view.fit", &Value::Null);
    }
    assert_eq!(s.log().len(), crate::session::MAX_LOG);
}

#[test]
fn ribbon_menus_name_registered_commands() {
    for spec in cmd::all() {
        for id in cmd::placements().filter(|(s, _)| s.id == spec.id).flat_map(|(_, p)| p.menu.iter()) {
            assert!(cmd::find(id).is_some(), "{} menu names unknown command {id}", spec.id);
        }
    }
}

#[test]
fn every_parameter_is_documented() {
    for spec in cmd::all() {
        assert!(!spec.returns.is_empty(), "{} has no returns doc", spec.id);
        for p in spec.params {
            assert!(!p.doc.is_empty(), "{}.{} has no doc", spec.id, p.name);
            assert!(p.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}.{}", spec.id, p.name);
        }
    }
}

#[test]
fn unknown_and_mistyped_parameters_are_rejected() {
    let mut s = Session::new();
    let r = s.execute("view.theme.set", &json!({"theme": "light", "colour": "red"}));
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("colour")), "{r:?}");
    let r = s.execute("view.fit", &json!({"now": true}));
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("takes no parameters")), "{r:?}");
    assert!(s.execute("view.camera.set", &json!({"target": [1, 2]})).is_err());
    assert!(s.execute("view.camera.set", &json!({"pitch": 91})).is_err());
    assert!(s.execute("view.fps_limit.set", &json!({"fps": 2.5})).is_err());
    assert!(s.execute("view.camera.preset", &json!({})).is_err());
}

#[test]
fn preset_choices_match_the_render_presets() {
    let spec = cmd::find("view.camera.preset");
    let Some(crate::ParamKind::Choice(choices)) = spec.and_then(|c| c.params.first()).map(|p| p.kind) else {
        unreachable_test();
        return;
    };
    for id in choices {
        assert!(ViewPreset::from_id(id).is_some(), "{id}");
    }
    for p in ViewPreset::ALL {
        assert!(choices.contains(&p.id()), "{}", p.id());
    }
}

#[allow(clippy::panic)]
fn unreachable_test() {
    panic!("view.camera.preset has no choice parameter");
}

#[test]
fn camera_api_round_trips() {
    let mut s = Session::new();
    let r = s.execute("view.camera.set", &json!({"target": [10, 20, 5], "distance": 300, "yaw": -90, "pitch": 0}));
    assert!(r.is_ok(), "{r:?}");
    let cam = s.execute("view.camera.get", &Value::Null).unwrap_or_default();
    assert_eq!(cam["target"], json!([10.0, 20.0, 5.0]));
    assert!((cam["yaw"].as_f64().unwrap_or_default() + 90.0).abs() < 1e-3);
    // Front view: screen right is +X, so panning right moves the target along +X.
    assert!(s.execute("view.camera.pan", &json!({"right": 5, "up": 2})).is_ok());
    assert!((s.view.camera.target.x - 15.0).abs() < 1e-3 && (s.view.camera.target.z - 7.0).abs() < 1e-3);
    assert!(s.execute("view.camera.zoom", &json!({"factor": 2})).is_ok());
    assert!((s.view.camera.distance - 150.0).abs() < 1e-3);
    assert!(s.execute("view.camera.orbit", &json!({"pitch": 500})).is_ok());
    assert!((s.view.camera.pitch.to_degrees() - 90.0).abs() < 1e-3);
}

#[test]
fn display_and_ui_requests() {
    let mut s = Session::new();
    assert!(s.execute("view.display.set", &json!({"grid": false, "view_cube": false})).is_ok());
    assert!(!s.prefs.display.grid && !s.prefs.display.view_cube && s.prefs.display.wcs);
    assert!(s.execute("view.grid.toggle", &Value::Null).is_ok());
    assert!(s.prefs.display.grid);

    assert!(s.execute("ui.ribbon.tab.set", &json!({"tab": "toolpaths"})).is_ok());
    assert_eq!(s.ui.ribbon_tab, RibbonTab::Toolpaths);
    assert!(s.execute("ui.pane.hide", &json!({"pane": "viewport"})).is_err());
    assert!(s.execute("ui.pane.hide", &json!({"pane": "console"})).is_ok());
    assert!(s.execute("ui.window.size.set", &json!({"width": 1280, "height": 800})).is_ok());
    assert!(s.execute("app.quit", &Value::Null).is_ok());
    assert_eq!(
        s.take_ui_requests(),
        vec![
            crate::UiRequest::HidePane(crate::Pane::Console),
            crate::UiRequest::WindowSize { width: 1280.0, height: 800.0 },
            crate::UiRequest::Quit,
        ]
    );
    assert!(s.take_ui_requests().is_empty());
}

#[test]
fn introspection_commands() {
    let mut s = Session::with_sample();
    let list = s.execute("command.list", &Value::Null).unwrap_or_default();
    let ids: Vec<&str> = list.as_array().into_iter().flatten().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"view.camera.set") && !ids.contains(&"test.panic"));
    let d = s.execute("command.describe", &json!({"id": "view.camera.zoom"})).unwrap_or_default();
    assert_eq!(d["params"][0]["name"], "factor");
    let state = s.execute("session.state.get", &Value::Null).unwrap_or_default();
    assert_eq!(state["job"]["name"], "Sample");
    assert_eq!(state["ribbon_tab"], "home");

    let since = s.log_seq();
    assert!(s.execute("view.theme.set", &json!({"theme": "light"})).is_ok());
    assert!(s.execute("view.theme.set", &json!({"theme": "nope"})).is_err());
    let log = s.execute("console.log.get", &json!({"since": since})).unwrap_or_default();
    let entries = log.as_array().cloned().unwrap_or_default();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1]["error"]["kind"], "invalid_params");
}

/// A unique scratch file holding `contents`; removed when the guard drops.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str, contents: &[u8]) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cosmiccam-engine-{}-{n}", std::process::id()));
        let path = dir.join(name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&path, contents);
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap_or_default()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

const PYRAMID_OBJ: &[u8] =
    b"v 0 0 0\nv 40 0 0\nv 40 30 0\nv 0 30 0\nv 20 15 25\nf 1 4 3 2\nf 1 2 5\nf 2 3 5\nf 3 4 5\nf 4 1 5\n";

#[test]
fn import_adds_a_model_to_the_job() {
    let file = TempFile::new("Pyramid.obj", PYRAMID_OBJ);
    let mut s = Session::with_sample();
    let revision = s.job.revision;
    let r = s.execute("model.import", &json!({"path": file.path()})).unwrap_or_default();
    assert_eq!(r["id"], "model.1");
    assert_eq!(r["name"], "Pyramid");
    assert_eq!(r["format"], "obj");
    assert_eq!(r["triangles"], 6);
    assert_eq!(r["bounds"]["max"], json!([40.0, 30.0, 25.0]));
    assert_ne!(s.job.revision, revision);
    // The import replaces the placeholder part.
    assert!(!s.job.sample_loaded);
    assert!(s.outline_node("model.sample").is_none());
    let node = s.outline_node("model.1");
    assert!(node.is_some_and(|n| n.properties.iter().any(|(k, v)| k == "Triangles" && v == "6")));
    assert!(s.execute("selection.set", &json!({"id": "model.1"})).is_ok());
    let list = s.execute("model.list", &Value::Null).unwrap_or_default();
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    let state = s.execute("session.state.get", &Value::Null).unwrap_or_default();
    assert_eq!(state["job"]["models"][0]["id"], "model.1");
}

#[test]
fn import_scales_by_units_and_takes_a_name() {
    let file = TempFile::new("pyramid.OBJ", PYRAMID_OBJ);
    let mut s = Session::new();
    let r = s.execute("model.import", &json!({"path": file.path(), "units": "in", "name": "  Bracket  "}));
    let r = r.unwrap_or_default();
    assert_eq!(r["name"], "Bracket");
    assert_eq!(r["units"], "in");
    assert!((r["bounds"]["max"][0].as_f64().unwrap_or_default() - 1016.0).abs() < 1e-3);
}

#[test]
fn ids_are_not_reused_after_removal() {
    let file = TempFile::new("p.obj", PYRAMID_OBJ);
    let mut s = Session::new();
    let import = |s: &mut Session| s.execute("model.import", &json!({"path": file.path()})).unwrap_or_default();
    assert_eq!(import(&mut s)["id"], "model.1");
    assert!(s.execute("selection.set", &json!({"id": "model.1"})).is_ok());
    assert!(s.execute("model.remove", &json!({"id": "model.1"})).is_ok());
    assert_eq!(s.view.selection, None);
    assert!(s.job.models.is_empty());
    assert_eq!(import(&mut s)["id"], "model.2");
    assert!(matches!(s.execute("model.remove", &json!({"id": "model.1"})), Err(EngineError::InvalidParams(_))));
}

#[test]
fn failed_imports_leave_the_job_alone() {
    let bad = TempFile::new("broken.obj", b"v 0 0 0\nf 1 2 3\n");
    let step = TempFile::new("part.step", b"ISO-10303-21;");
    let mut s = Session::with_sample();
    let revision = s.job.revision;
    for path in [bad.path(), step.path(), "/no/such/file.stl"] {
        let r = s.execute("model.import", &json!({"path": path}));
        assert!(matches!(r, Err(EngineError::Import(_))), "{path}: {r:?}");
    }
    let r = s.execute("model.import", &json!({"path": bad.path()}));
    assert!(matches!(r, Err(ref e) if e.kind() == "import" && e.to_string().contains("line 2")), "{r:?}");
    assert!(s.job.sample_loaded && s.job.models.len() == 1);
    assert_eq!(s.job.revision, revision);
    assert!(s.execute("model.import", &Value::Null).is_err());
}

#[test]
fn job_new_clears_models_and_changes_the_revision() {
    let file = TempFile::new("p.obj", PYRAMID_OBJ);
    let mut s = Session::new();
    assert!(s.execute("model.import", &json!({"path": file.path()})).is_ok());
    let revision = s.job.revision;
    assert!(s.execute("job.new", &Value::Null).is_ok());
    assert!(s.job.models.is_empty());
    assert_ne!(s.job.revision, revision);
}

fn vec3(v: &Value) -> [f64; 3] {
    let c = |i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
    [c(0), c(1), c(2)]
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

#[test]
fn wcs_create_defaults_to_world_and_becomes_active() {
    let mut s = Session::new();
    let w = s.execute("setup.wcs.create", &Value::Null).unwrap_or_default();
    assert_eq!(w["id"], "wcs.1");
    assert_eq!(w["name"], "WCS 1");
    assert_eq!(w["active"], true);
    assert!(close(vec3(&w["origin"]), [0.0, 0.0, 0.0]));
    assert!(close(vec3(&w["z_axis"]), [0.0, 0.0, 1.0]) && close(vec3(&w["y_axis"]), [0.0, 1.0, 0.0]));
    let second = s.execute("setup.wcs.create", &json!({"name": "Op 2", "activate": false})).unwrap_or_default();
    assert_eq!(second["active"], false);
    assert_eq!(s.job.active_wcs.as_deref(), Some("wcs.1"));
    assert!(s.outline_node("wcs.1").is_some_and(|n| n.label == "WCS 1 (active)"));
    assert!(s.execute("selection.set", &json!({"id": "wcs.2"})).is_ok());
    let state = s.execute("session.state.get", &Value::Null).unwrap_or_default();
    assert_eq!(state["job"]["active_wcs"], "wcs.1");
    assert_eq!(state["job"]["wcs"].as_array().map(Vec::len), Some(2));
}

#[test]
fn wcs_activate_and_remove() {
    let mut s = Session::new();
    for _ in 0..3 {
        assert!(s.execute("setup.wcs.create", &Value::Null).is_ok());
    }
    assert_eq!(s.job.active_wcs.as_deref(), Some("wcs.3"));
    assert!(s.execute("setup.wcs.activate", &json!({"id": "wcs.2"})).is_ok());
    assert!(s.execute("setup.wcs.activate", &json!({"id": "wcs.9"})).is_err());
    let revision = s.job.setup_revision;
    let r = s.execute("setup.wcs.remove", &json!({"id": "wcs.2"})).unwrap_or_default();
    assert_eq!(r["active_wcs"], "wcs.1");
    assert_ne!(s.job.setup_revision, revision);
    assert_eq!(s.execute("setup.wcs.get", &Value::Null).unwrap_or_default()["id"], "wcs.1");
    assert_eq!(s.execute("setup.wcs.list", &Value::Null).unwrap_or_default().as_array().map(Vec::len), Some(2));
    assert!(s.execute("job.new", &Value::Null).is_ok());
    assert!(s.job.wcs.is_empty() && s.job.active_wcs.is_none());
    assert_eq!(s.execute("setup.wcs.get", &Value::Null).unwrap_or_default(), Value::Null);
}

#[test]
fn import_and_wcs_buttons_are_also_on_home() {
    let home: Vec<&str> = cmd::ribbon(RibbonTab::Home).iter().flat_map(|(_, c)| c.iter().map(|(s, _)| s.id)).collect();
    for id in ["model.import", "setup.wcs.create", "setup.wcs.align", "setup.wcs.offset"] {
        assert!(home.contains(&id), "{id} missing from Home");
    }
    assert!(!home.iter().any(|id| id.starts_with("view.camera") || *id == "view.fit"));
    let panels: Vec<&str> = cmd::ribbon(RibbonTab::Home).iter().map(|(p, _)| *p).collect();
    assert_eq!(panels, ["Job", "Setup", "Preferences"]);
    for (id, menu) in cmd::VIEWPORT_TOOLS {
        assert!(cmd::find(id).is_some(), "{id}");
        assert!(menu.iter().all(|m| cmd::find(m).is_some()));
    }
}

/// Faces of `model` of `kind` matching `keep`, as the API lists them.
fn faces_where(s: &mut Session, model: &str, kind: &str, keep: impl Fn(&Value) -> bool) -> Vec<String> {
    let list = s.execute("model.faces.list", &json!({"id": model, "kind": kind})).unwrap_or_default();
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|f| keep(f))
        .filter_map(|f| f["face"].as_str().map(str::to_owned))
        .collect()
}

fn one_face(s: &mut Session, kind: &str, keep: impl Fn(&Value) -> bool) -> String {
    let found = faces_where(s, "model.sample", kind, keep);
    assert_eq!(found.len(), 1, "{kind}: {found:?}");
    found.into_iter().next().unwrap_or_default()
}

fn plane_at(normal: [f64; 3], point_axis: usize, value: f64) -> impl Fn(&Value) -> bool {
    move |f: &Value| close(vec3(&f["normal"]), normal) && (vec3(&f["point"])[point_axis] - value).abs() < 1e-3
}

#[test]
fn the_sample_part_is_a_model_with_recognised_faces() {
    let mut s = Session::with_sample();
    let m = s.execute("model.list", &Value::Null).unwrap_or_default();
    assert_eq!(m[0]["id"], "model.sample");
    assert_eq!(m[0]["format"], "builtin");
    let cylinders = faces_where(&mut s, "model.sample", "cylinder", |_| true);
    assert_eq!(cylinders.len(), 1, "{cylinders:?}");
    let c = s.execute("model.face.get", &json!({"face": cylinders[0]})).unwrap_or_default();
    assert!((c["radius"].as_f64().unwrap_or_default() - 12.0).abs() < 1e-3, "{c}");
    assert_eq!(c["hole"], false);
    assert!(s.execute("model.face.get", &json!({"face": "model.sample/face.99999"})).is_err());
    assert!(s.execute("model.face.get", &json!({"face": "garbage"})).is_err());
    assert!(s.outline_node("model.sample").is_some_and(|n| n.properties.iter().any(|(k, _)| k == "Faces")));
}

#[test]
fn picking_finds_the_face_under_a_ray() {
    let mut s = Session::with_sample();
    let hit = s.execute("model.pick", &json!({"origin": [28, 0, 100], "direction": [0, 0, -1]})).unwrap_or_default();
    assert_eq!(hit["kind"], "plane");
    assert!(close(vec3(&hit["point"]), [28.0, 0.0, 42.0]), "{hit}");
    assert!((hit["distance"].as_f64().unwrap_or_default() - 58.0).abs() < 1e-3);
    let side = s.execute("model.pick", &json!({"origin": [100, 0, 30], "direction": [-1, 0, 0]})).unwrap_or_default();
    assert_eq!(side["kind"], "cylinder", "{side}");
    let miss = s.execute("model.pick", &json!({"origin": [500, 500, 500], "direction": [0, 0, 1]}));
    assert_eq!(miss.unwrap_or_default(), Value::Null);
    assert!(s.execute("model.pick", &json!({"origin": [0, 0, 0], "direction": [0, 0, 0]})).is_err());
}

/// A 100 × 60 × 10 plate with two Ø10 bores through it at (20, 30) and (80, 30).
fn plate_with_two_holes() -> String {
    use std::f64::consts::TAU;
    let mut v: Vec<[f64; 3]> = Vec::new();
    let mut f: Vec<[usize; 3]> = Vec::new();
    let quad = |v: &mut Vec<[f64; 3]>, f: &mut Vec<[usize; 3]>, q: [[f64; 3]; 4]| {
        let i = v.len() + 1;
        v.extend(q);
        f.push([i, i + 1, i + 2]);
        f.push([i, i + 2, i + 3]);
    };
    quad(&mut v, &mut f, [[0., 0., 0.], [0., 60., 0.], [100., 60., 0.], [100., 0., 0.]]);
    quad(&mut v, &mut f, [[0., 0., 10.], [100., 0., 10.], [100., 60., 10.], [0., 60., 10.]]);
    quad(&mut v, &mut f, [[0., 0., 0.], [100., 0., 0.], [100., 0., 10.], [0., 0., 10.]]);
    quad(&mut v, &mut f, [[100., 60., 0.], [0., 60., 0.], [0., 60., 10.], [100., 60., 10.]]);
    for cx in [20.0, 80.0] {
        let n = 36;
        let ring = |k: usize, z: f64| {
            let a = TAU * k as f64 / n as f64;
            [cx + 5.0 * a.cos(), 30.0 + 5.0 * a.sin(), z]
        };
        // Wound to face the axis: a bore.
        for k in 0..n {
            quad(&mut v, &mut f, [ring(k, 0.0), ring(k, 10.0), ring(k + 1, 10.0), ring(k + 1, 0.0)]);
        }
    }
    let mut out = String::new();
    for p in v {
        out.push_str(&format!("v {} {} {}\n", p[0], p[1], p[2]));
    }
    for t in f {
        out.push_str(&format!("f {} {} {}\n", t[0], t[1], t[2]));
    }
    out
}

/// Straight edges of the sample part running along `axis` (0 = X, 1 = Y, 2 = Z) through
/// the given fixed coordinates on the other two axes.
fn sample_line(s: &mut Session, axis: usize, fixed: [(usize, f64); 2]) -> String {
    let list = s.execute("model.edges.list", &json!({"id": "model.sample", "kind": "line"})).unwrap_or_default();
    let found: Vec<String> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| vec3(&e["direction"])[axis].abs() > 0.999)
        .filter(|e| fixed.iter().all(|(i, v)| (vec3(&e["start"])[*i] - v).abs() < 1e-3))
        .filter_map(|e| e["edge"].as_str().map(str::to_owned))
        .collect();
    assert!(!found.is_empty(), "no edge along {axis} at {fixed:?}");
    found.into_iter().next().unwrap_or_default()
}

#[test]
fn the_sample_part_has_straight_and_circular_edges() {
    let mut s = Session::with_sample();
    let circles = s.execute("model.edges.list", &json!({"id": "model.sample", "kind": "circle"})).unwrap_or_default();
    let circles = circles.as_array().cloned().unwrap_or_default();
    assert!(!circles.is_empty());
    assert!(circles.iter().all(|c| (c["radius"].as_f64().unwrap_or_default() - 12.0).abs() < 1e-3), "{circles:?}");
    let e = sample_line(&mut s, 0, [(1, -20.0), (2, 35.0)]);
    let got = s.execute("model.edge.get", &json!({"edge": e})).unwrap_or_default();
    assert_eq!(got["kind"], "line");
    assert!((got["length"].as_f64().unwrap_or_default() - 45.0).abs() < 1e-3, "{got}");
    // A ray just beside that edge, from above, picks it.
    let hit = s
        .execute("model.edge.pick", &json!({"origin": [-20, -20.2, 100], "direction": [0, 0, -1], "radius": 0.5}))
        .unwrap_or_default();
    assert_eq!(hit["edge"], json!(e), "{hit}");
    assert!(s.execute("model.edge.get", &json!({"edge": "model.sample/edge.99999"})).is_err());
}

fn wcs(s: &mut Session, params: Value) -> Value {
    let r = s.execute("setup.wcs.create", &params);
    assert!(r.is_ok(), "{params} -> {r:?}");
    r.unwrap_or_default()
}

fn wcs_err(s: &mut Session, params: Value) -> String {
    match s.execute("setup.wcs.create", &params) {
        Err(EngineError::InvalidParams(m)) => m,
        other => {
            assert!(other.is_err(), "{params} should fail, got {other:?}");
            String::new()
        }
    }
}

/// The sample's block (x -40..5, y -20..20, z 20..35) and boss (Ø24 at (28, 0), z 20..42).
struct SampleRefs {
    top: String,
    left: String,
    front: String,
    deck: String,
    boss: String,
    top_front_edge: String,
    top_left_edge: String,
}

fn sample_refs(s: &mut Session) -> SampleRefs {
    SampleRefs {
        top: one_face(s, "plane", plane_at([0.0, 0.0, 1.0], 2, 35.0)),
        left: one_face(s, "plane", plane_at([-1.0, 0.0, 0.0], 0, -40.0)),
        front: one_face(s, "plane", plane_at([0.0, -1.0, 0.0], 1, -20.0)),
        deck: one_face(s, "plane", plane_at([0.0, 0.0, 1.0], 2, 20.0)),
        boss: one_face(s, "cylinder", |_| true),
        top_front_edge: sample_line(s, 0, [(1, -20.0), (2, 35.0)]),
        top_left_edge: sample_line(s, 1, [(0, -40.0), (2, 35.0)]),
    }
}

#[test]
fn wcs_origin_modes() {
    let mut s = Session::with_sample();
    let r = sample_refs(&mut s);
    let origin = |w: &Value| vec3(&w["origin"]);
    assert!(close(origin(&wcs(&mut s, json!({"origin": "model"}))), [0.0, 0.0, 0.0]));
    // Box points on the sample's bounds (-50..50, -30..30, 0..42).
    for (point, want) in [
        ("top_center", [0.0, 0.0, 42.0]),
        ("bottom_front_left", [-50.0, -30.0, 0.0]),
        ("center", [0.0, 0.0, 21.0]),
        ("middle_back_right", [50.0, 30.0, 21.0]),
        ("top_front", [0.0, -30.0, 42.0]),
    ] {
        let w = wcs(&mut s, json!({"origin": "box", "origin_box": point}));
        assert!(close(origin(&w), want), "{point}: {w}");
    }
    assert!(close(origin(&wcs(&mut s, json!({"origin": "box"}))), [0.0, 0.0, 42.0]), "default top_center");
    assert!(close(origin(&wcs(&mut s, json!({"origin": "point", "origin_point": [1, 2, 3]}))), [1.0, 2.0, 3.0]));
    let w = wcs(&mut s, json!({"origin": "point", "origin_geometry": r.boss}));
    assert!(close(origin(&w), [28.0, 0.0, 42.0]), "top of the boss axis: {w}");
    let w = wcs(&mut s, json!({"origin": "intersection", "origin_a": r.front, "origin_b": r.top, "origin_c": r.left}));
    assert!(close(origin(&w), [-40.0, -20.0, 35.0]), "three planes in any order: {w}");
    let w = wcs(&mut s, json!({"origin": "intersection", "origin_a": r.deck, "origin_b": r.boss}));
    assert!(close(origin(&w), [28.0, 0.0, 20.0]), "axis meets plane: {w}");
    let w = wcs(&mut s, json!({"origin": "intersection", "origin_a": r.top_front_edge, "origin_b": r.top_left_edge}));
    assert!(close(origin(&w), [-40.0, -20.0, 35.0]), "two edges: {w}");

    assert!(
        wcs_err(&mut s, json!({"origin": "intersection", "origin_a": r.top, "origin_b": r.left})).contains("third")
    );
    assert!(wcs_err(&mut s, json!({"origin": "point"})).contains("origin_point"));
    assert!(wcs_err(&mut s, json!({"origin": "model", "origin_box": "center"})).contains("not used"));
    assert!(
        wcs_err(&mut s, json!({"origin": "point", "origin_geometry": "model.sample/face.9999"})).contains("no face")
    );
    let mut empty = Session::new();
    assert!(wcs_err(&mut empty, json!({"origin": "box"})).contains("import a model"));
}

#[test]
fn wcs_orientation_from_selected_z_and_x() {
    let mut s = Session::with_sample();
    let r = sample_refs(&mut s);
    let corner = json!({"origin": "intersection", "origin_a": r.top, "origin_b": r.left, "origin_c": r.front});
    let with = |extra: Value| {
        let mut p = corner.clone();
        if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(extra.clone());
        }
        p
    };
    // Z from the top face, X along the top-front edge (away from the corner).
    let w = wcs(&mut s, with(json!({"orientation": "z_x", "orientation_z": r.top, "orientation_x": r.top_front_edge})));
    assert!(close(vec3(&w["origin"]), [-40.0, -20.0, 35.0]));
    assert!(close(vec3(&w["x_axis"]), [1.0, 0.0, 0.0]) && close(vec3(&w["z_axis"]), [0.0, 0.0, 1.0]), "{w}");
    // X from a plane uses its outward normal; flips reverse each selected axis.
    let w = wcs(
        &mut s,
        with(json!({"orientation": "z_x", "orientation_z": r.top, "orientation_x": r.front, "flip_x": true})),
    );
    assert!(close(vec3(&w["x_axis"]), [0.0, 1.0, 0.0]), "{w}");
    let w = wcs(
        &mut s,
        with(json!({"orientation": "z_x", "orientation_z": r.top, "orientation_x": r.top_left_edge, "flip_z": true})),
    );
    assert!(close(vec3(&w["z_axis"]), [0.0, 0.0, -1.0]) && close(vec3(&w["x_axis"]), [0.0, 1.0, 0.0]), "{w}");
    // Away from a corner the edge takes the sense of world X, whichever end it starts from.
    let w = wcs(
        &mut s,
        json!({"origin": "box", "origin_box": "top_center", "orientation": "z_x", "orientation_z": r.top, "orientation_x": r.top_front_edge}),
    );
    assert!(close(vec3(&w["x_axis"]), [1.0, 0.0, 0.0]), "{w}");
    // Z from the boss axis.
    let w = wcs(&mut s, json!({"orientation": "z_x", "orientation_z": r.boss, "orientation_x": r.top_front_edge}));
    assert!(close(vec3(&w["z_axis"]), [0.0, 0.0, 1.0]), "{w}");
    // Z and Y; X and Y.
    let w = wcs(&mut s, with(json!({"orientation": "z_y", "orientation_z": r.top, "orientation_y": r.top_left_edge})));
    assert!(close(vec3(&w["y_axis"]), [0.0, 1.0, 0.0]) && close(vec3(&w["x_axis"]), [1.0, 0.0, 0.0]), "{w}");
    let w = wcs(
        &mut s,
        with(json!({"orientation": "x_y", "orientation_x": r.top_front_edge, "orientation_y": r.top_left_edge})),
    );
    assert!(close(vec3(&w["z_axis"]), [0.0, 0.0, 1.0]), "{w}");
    // Vectors.
    let w = wcs(
        &mut s,
        json!({"orientation": "vectors", "orientation_z_vector": [0, 0, -2], "orientation_x_vector": [1, 0, 0.5]}),
    );
    assert!(close(vec3(&w["z_axis"]), [0.0, 0.0, -1.0]) && close(vec3(&w["y_axis"]), [0.0, -1.0, 0.0]), "{w}");

    assert!(
        wcs_err(&mut s, json!({"orientation": "z_x", "orientation_z": r.top, "orientation_x": r.deck}))
            .contains("parallel")
    );
    assert!(wcs_err(&mut s, json!({"orientation": "z_x", "orientation_z": r.top})).contains("orientation_x"));
    assert!(wcs_err(&mut s, json!({"orientation": "model", "orientation_x": r.top})).contains("not used"));
    assert!(
        wcs_err(
            &mut s,
            json!({"orientation": "z_x", "orientation_z": r.top, "orientation_x": r.front, "flip_y": true})
        )
        .contains("flip_y")
    );
    let w = wcs(&mut s, json!({"flip_z": true}));
    assert!(close(vec3(&w["z_axis"]), [0.0, 0.0, -1.0]), "model axes flip too: {w}");
}

#[test]
fn wcs_on_two_dowel_holes() {
    let file = TempFile::new("plate.obj", plate_with_two_holes().as_bytes());
    let mut s = Session::new();
    assert!(s.execute("model.import", &json!({"path": file.path()})).is_ok());
    let holes = faces_where(&mut s, "model.1", "cylinder", |f| f["hole"] == true);
    let x_of = |s: &mut Session, f: &str| {
        vec3(&s.execute("model.face.get", &json!({"face": f})).unwrap_or_default()["base"])[0]
    };
    let (a, b) = if x_of(&mut s, &holes[0]) < 50.0 {
        (holes[0].clone(), holes[1].clone())
    } else {
        (holes[1].clone(), holes[0].clone())
    };
    let top = faces_where(&mut s, "model.1", "plane", plane_at([0.0, 0.0, 1.0], 2, 10.0));
    // Origin on the first hole at the top face, Z from the face, X towards the second hole.
    let w = wcs(
        &mut s,
        json!({
            "origin": "intersection", "origin_a": a, "origin_b": top[0],
            "orientation": "z_x", "orientation_z": top[0], "orientation_x": b,
        }),
    );
    assert!(close(vec3(&w["origin"]), [20.0, 30.0, 10.0]), "{w}");
    assert!(close(vec3(&w["x_axis"]), [1.0, 0.0, 0.0]) && close(vec3(&w["z_axis"]), [0.0, 0.0, 1.0]), "{w}");
}

#[test]
fn wcs_align_changes_only_the_groups_given() {
    let mut s = Session::with_sample();
    let r = sample_refs(&mut s);
    assert!(s.execute("setup.wcs.align", &json!({})).is_err(), "no WCS yet");
    wcs(&mut s, json!({"origin": "box", "origin_box": "top_center"}));
    let w = s
        .execute(
            "setup.wcs.align",
            &json!({"orientation": "z_x", "orientation_z": r.front, "orientation_x": r.top_front_edge}),
        )
        .unwrap_or_default();
    assert!(close(vec3(&w["origin"]), [0.0, 0.0, 42.0]), "origin kept: {w}");
    assert!(close(vec3(&w["z_axis"]), [0.0, -1.0, 0.0]), "{w}");
    let w = s.execute("setup.wcs.align", &json!({"origin": "point", "origin_point": [5, 5, 5]})).unwrap_or_default();
    assert!(
        close(vec3(&w["origin"]), [5.0, 5.0, 5.0]) && close(vec3(&w["z_axis"]), [0.0, -1.0, 0.0]),
        "axes kept: {w}"
    );
    let w = s.execute("setup.wcs.align", &json!({"flip_z": true})).unwrap_or_default();
    assert!(close(vec3(&w["z_axis"]), [0.0, 1.0, 0.0]), "{w}");
    assert!(s.execute("setup.wcs.align", &json!({"origin_box": "center"})).is_err(), "box point without origin = box");
}

#[test]
fn wcs_offset_rename_and_rotate() {
    let mut s = Session::new();
    assert!(s.execute("setup.wcs.offset", &json!({"delta": [1, 0, 0]})).is_err(), "no WCS yet");
    wcs(&mut s, json!({"origin": "point", "origin_point": [10, 0, 0]}));
    let w = s.execute("setup.wcs.rotate", &json!({"axis": "z", "degrees": 90})).unwrap_or_default();
    assert!(close(vec3(&w["x_axis"]), [0.0, 1.0, 0.0]), "{w}");
    let w = s.execute("setup.wcs.offset", &json!({"delta": [5, 0, 0]})).unwrap_or_default();
    assert!(close(vec3(&w["origin"]), [10.0, 5.0, 0.0]), "offsets follow the WCS axes: {w}");
    let w = s.execute("setup.wcs.offset", &json!({"delta": [5, 0, 1], "frame": "world"})).unwrap_or_default();
    assert!(close(vec3(&w["origin"]), [15.0, 5.0, 1.0]), "{w}");
    let w = s.execute("setup.wcs.rename", &json!({"name": "  Vice  "})).unwrap_or_default();
    assert_eq!(w["name"], "Vice");
    assert!(s.execute("setup.wcs.offset", &json!({"delta": [1.0e9, 0, 0], "frame": "world"})).is_ok());
    assert!(s.execute("setup.wcs.offset", &json!({"delta": [1.0e9, 0, 0], "frame": "world"})).is_err());
}

#[test]
fn wcs_quick_placements_need_a_part() {
    let mut s = Session::new();
    assert!(s.execute("setup.wcs.top_center", &Value::Null).is_err());
    let mut s = Session::with_sample();
    let w = s.execute("setup.wcs.top_center", &Value::Null).unwrap_or_default();
    assert!(close(vec3(&w["origin"]), [0.0, 0.0, 42.0]), "{w}");
    let w = s.execute("setup.wcs.bottom_corner", &json!({"corner": "back_right"})).unwrap_or_default();
    assert!(close(vec3(&w["origin"]), [50.0, 30.0, 0.0]) && w["id"] == "wcs.1", "{w}");
}

#[test]
fn stock_fits_the_models_with_margins() {
    let mut s = Session::new();
    assert!(s.execute("setup.stock.fit", &Value::Null).is_err(), "nothing to fit");
    assert_eq!(s.execute("setup.stock.get", &Value::Null).unwrap_or_default(), Value::Null);
    let file = TempFile::new("p.obj", PYRAMID_OBJ);
    assert!(s.execute("model.import", &json!({"path": file.path()})).is_ok());
    let st = s
        .execute("setup.stock.fit", &json!({"margin_sides": 2, "margin_top": 1, "margin_bottom": 0.5}))
        .unwrap_or_default();
    assert!(close(vec3(&st["min"]), [-2.0, -2.0, -0.5]) && close(vec3(&st["max"]), [42.0, 32.0, 26.0]), "{st}");
    assert!(close(vec3(&st["size"]), [44.0, 34.0, 26.5]));
    assert!(s.outline_node("stock").is_some_and(|n| n.properties.iter().any(|(k, _)| k == "Size")));
    let state = s.execute("session.state.get", &Value::Null).unwrap_or_default();
    assert_eq!(state["job"]["stock"]["shape"], "box");
    assert!(s.execute("setup.stock.fit", &json!({"margin_top": -1})).is_err());
    // A WCS on the stock's top centre.
    let w = wcs(&mut s, json!({"origin": "stock", "origin_box": "top_center"}));
    assert!(close(vec3(&w["origin"]), [20.0, 15.0, 26.0]), "{w}");
}

#[test]
fn the_sample_has_stock_and_an_import_replaces_it() {
    let mut s = Session::with_sample();
    let st = s.execute("setup.stock.get", &Value::Null).unwrap_or_default();
    assert!(close(vec3(&st["max"]), [53.0, 33.0, 45.0]), "{st}");
    let file = TempFile::new("p.obj", PYRAMID_OBJ);
    assert!(s.execute("model.import", &json!({"path": file.path()})).is_ok());
    assert_eq!(s.execute("setup.stock.get", &Value::Null).unwrap_or_default(), Value::Null);
    assert!(wcs_err(&mut s, json!({"origin": "stock"})).contains("setup.stock.fit"));
}

#[test]
fn faces_edges_and_vertices_can_be_selected_and_inspected() {
    let mut s = Session::with_sample();
    let r = sample_refs(&mut s);
    assert!(s.execute("selection.set", &json!({"id": r.boss})).is_ok());
    let node = s.execute("selection.get", &Value::Null).unwrap_or_default();
    let props = node["properties"].as_array().cloned().unwrap_or_default();
    let prop = |name: &str| props.iter().find(|p| p["name"] == name).map(|p| p["value"].clone()).unwrap_or_default();
    assert_eq!(prop("Type"), "Boss");
    assert_eq!(prop("Diameter"), "0.9449 in", "inches by default");
    assert!(s.execute("selection.set", &json!({"id": r.top_front_edge})).is_ok());
    let node = s.execute("selection.get", &Value::Null).unwrap_or_default();
    assert!(node["label"].as_str().is_some_and(|l| l.contains("line")), "{node}");

    let vertices = s.execute("model.vertices.list", &json!({"id": "model.sample"})).unwrap_or_default();
    let corner = vertices
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| close(vec3(&v["position"]), [-40.0, -20.0, 35.0]))
        .and_then(|v| v["vertex"].as_str().map(str::to_owned))
        .unwrap_or_default();
    assert!(s.execute("selection.set", &json!({"id": corner})).is_ok(), "{corner}");
    let node = s.execute("selection.get", &Value::Null).unwrap_or_default();
    assert!(node["label"].as_str().is_some_and(|l| l.starts_with("Vertex")), "{node}");
    // Picking a vertex just beside the corner, from above.
    let hit = s
        .execute("model.vertex.pick", &json!({"origin": [-40.2, -20.2, 100], "direction": [0, 0, -1], "radius": 0.5}))
        .unwrap_or_default();
    assert_eq!(hit["vertex"], json!(corner), "{hit}");
    // A vertex is an origin point but has no direction.
    let w = wcs(&mut s, json!({"origin": "point", "origin_geometry": corner}));
    assert!(close(vec3(&w["origin"]), [-40.0, -20.0, 35.0]));
    assert!(
        wcs_err(&mut s, json!({"orientation": "z_x", "orientation_z": corner, "orientation_x": r.top_front_edge}))
            .contains("no direction")
    );
    assert!(s.execute("selection.set", &json!({"id": "model.sample/vertex.99999"})).is_err());
    assert!(s.execute("selection.set", &json!({})).is_ok());
    assert_eq!(s.view.selection, None);
}

#[test]
fn staged_uploads_import_by_name() {
    let mut s = Session::new();
    let path = s.stage_upload("Pyramid.obj", PYRAMID_OBJ.to_vec());
    assert_eq!(path, "upload://Pyramid.obj");
    let r = s.execute("model.import", &json!({"path": path})).unwrap_or_default();
    assert_eq!(r["name"], "Pyramid");
    assert_eq!(r["triangles"], 6);
    // The staged bytes are used up by the import.
    assert!(matches!(s.execute("model.import", &json!({"path": path})), Err(EngineError::Import(_))));
    let bad = s.stage_upload("part.step", b"ISO-10303-21;".to_vec());
    assert!(matches!(s.execute("model.import", &json!({"path": bad})), Err(EngineError::Import(_))));
}

#[test]
fn a_new_session_has_one_generic_machine() {
    let mut s = Session::new();
    let machines = s.execute("machine.list", &Value::Null).unwrap_or_default();
    assert_eq!(machines.as_array().map(Vec::len), Some(1));
    assert_eq!(machines[0]["active"], true);
    assert_eq!(machines[0]["tools"], 0);
}

#[test]
fn flat_end_mills_go_into_the_active_machines_library() {
    let mut s = Session::new();
    let t = s.execute("tool.create.flat", &json!({"diameter": 6})).unwrap_or_default();
    assert_eq!(t["number"], 1);
    assert_eq!(t["name"], "Ø0.2362 in flat end mill", "named in the user's units");
    assert_eq!(t["type"], "flat");
    assert_eq!(t["flutes"], 3);
    assert_eq!(t["flute_length"], 18.0);
    assert_eq!(t["length_offset"], 1);
    assert_eq!(s.view.selection.as_deref(), Some("machine.1/tool.1"));
    let t2 = s
        .execute(
            "tool.create",
            &json!({"diameter": 3.175, "flutes": 2, "spindle_rpm": 18000, "feed": 900, "name": "1/8in upcut", "number": 12}),
        )
        .unwrap_or_default();
    assert_eq!(t2["number"], 12);
    // The lowest free number is used next.
    assert_eq!(s.execute("tool.create.flat", &json!({"diameter": 10})).unwrap_or_default()["number"], 2);
    let list = s.execute("tool.list", &Value::Null).unwrap_or_default();
    let numbers: Vec<u64> = list.as_array().into_iter().flatten().filter_map(|t| t["number"].as_u64()).collect();
    assert_eq!(numbers, [1, 2, 12]);
    // The inspector shows a selected tool.
    assert!(s.execute("selection.set", &json!({"id": "machine.1/tool.12"})).is_ok());
    let node = s.execute("selection.get", &Value::Null).unwrap_or_default();
    assert_eq!(node["label"], "T12 1/8in upcut");

    // Refused: duplicate numbers, other tool types, impossible geometry, limits over the machine's.
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "number": 1})).is_err());
    assert!(matches!(
        s.execute("tool.create", &json!({"diameter": 6, "type": "ball"})),
        Err(EngineError::NotSupported(_))
    ));
    assert!(matches!(s.execute("tool.create.ball", &Value::Null), Err(EngineError::Disabled(_))));
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "flute_length": 30, "overall_length": 20})).is_err());
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "spindle_rpm": 90000})).is_err(), "over 24000 rpm");
    assert!(s.execute("tool.create.flat", &json!({})).is_err(), "diameter is required");
    assert_eq!(s.execute("tool.list", &Value::Null).unwrap_or_default().as_array().map(Vec::len), Some(3));
}

#[test]
fn tools_can_be_edited_renumbered_copied_and_deleted() {
    let mut s = Session::new();
    assert!(s.execute("tool.create.flat", &json!({"diameter": 8})).is_ok());
    let t = s
        .execute("tool.feeds", &json!({"number": 1, "spindle_rpm": 9000, "feed": 1500, "stepdown": 8}))
        .unwrap_or_default();
    assert_eq!(t["spindle_rpm"], 9000.0);
    assert!(s.execute("tool.feeds", &json!({"number": 1, "stepdown": 99})).is_err(), "deeper than the flutes");
    let t = s.execute("tool.edit", &json!({"number": 1, "new_number": 5, "name": "Roughing 8"})).unwrap_or_default();
    assert_eq!(t["number"], 5);
    assert_eq!(s.view.selection.as_deref(), Some("machine.1/tool.5"), "selection follows the renumber");
    let copy = s.execute("tool.duplicate", &json!({"number": 5})).unwrap_or_default();
    assert_eq!(copy["number"], 1);
    assert_eq!(copy["name"], "Roughing 8");
    assert!(s.execute("tool.remove", &json!({"number": 5})).is_ok());
    assert!(s.execute("tool.get", &json!({"number": 5})).is_err());
    assert!(s.execute("tool.get", &json!({"number": 1})).is_ok());
}

#[test]
fn each_machine_has_its_own_library() {
    let mut s = Session::new();
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6})).is_ok());
    let router = s
        .execute("machine.create", &json!({"name": "Router", "max_spindle_rpm": 30000, "travel_x": 1200}))
        .unwrap_or_default();
    assert_eq!(router["id"], "machine.2");
    assert_eq!(router["active"], true);
    assert_eq!(s.execute("tool.list", &Value::Null).unwrap_or_default(), json!([]), "a new machine starts empty");
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "spindle_rpm": 28000})).is_ok());
    assert_eq!(
        s.execute("tool.list", &json!({"machine": "machine.1"})).unwrap_or_default().as_array().map(Vec::len),
        Some(1)
    );
    // Lowering the router's limit below a tool's speed is refused.
    assert!(s.execute("machine.edit", &json!({"max_spindle_rpm": 20000})).is_err());
    assert!(s.execute("machine.edit", &json!({"name": "Big router", "tool_capacity": 8})).is_ok());
    let copy = s.execute("machine.duplicate", &json!({"id": "machine.2"})).unwrap_or_default();
    assert_eq!(copy["name"], "Big router copy");
    assert_eq!(copy["tools"], 1);
    assert!(s.execute("machine.activate", &json!({"id": "machine.1"})).is_ok());
    assert!(s.execute("machine.remove", &json!({"id": "machine.3"})).is_ok());
    assert!(s.execute("machine.remove", &json!({"id": "machine.2"})).is_ok());
    assert!(s.execute("machine.remove", &json!({"id": "machine.1"})).is_err(), "the last machine stays");
    // The library survives a new job.
    assert!(s.execute("job.new", &Value::Null).is_ok());
    assert_eq!(s.execute("tool.list", &Value::Null).unwrap_or_default().as_array().map(Vec::len), Some(1));
    assert!(s.execute("machine.library.open", &Value::Null).is_ok());
    assert!(s.take_ui_requests().contains(&crate::UiRequest::ShowPane(crate::Pane::Library)));
}

#[test]
fn libraries_save_load_import_and_export() {
    let mut s = Session::new();
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "feed": 1000})).is_ok());
    assert!(s.execute("tool.create.flat", &json!({"diameter": 10})).is_ok());
    let saved = s.library_json();
    let mut other = Session::new();
    assert_eq!(other.load_library(&saved), Ok(Vec::new()));
    assert_eq!(other.execute("tool.list", &Value::Null).unwrap_or_default().as_array().map(Vec::len), Some(2));
    assert!(other.load_library(&json!({"nope": 1})).is_err());

    let dir = TempFile::new("tools.json", b"");
    assert!(s.execute("tool.library.export", &json!({"path": dir.path()})).is_ok());
    assert!(s.execute("machine.create", &json!({"name": "Second"})).is_ok());
    let r = s.execute("tool.library.import", &json!({"path": dir.path()})).unwrap_or_default();
    assert_eq!(r["imported"], 2);
    assert!(s.execute("tool.library.import", &json!({"path": dir.path()})).is_err(), "numbers already taken");
    assert!(s.execute("tool.library.import", &json!({"path": dir.path(), "replace": true})).is_ok());
    let garbage = TempFile::new("bad.json", b"{\"format\": \"CosmicCAM-tools/1\", \"tools\": [{\"number\": 1}]}");
    assert!(s.execute("tool.library.import", &json!({"path": garbage.path()})).is_err());
    assert!(s.execute("tool.library.import", &json!({"path": "/dev/zero"})).is_err());
}

#[test]
fn units_change_what_is_shown_but_not_the_api() {
    let mut s = Session::with_sample();
    assert_eq!(s.execute("settings.get", &Value::Null).unwrap_or_default()["units"], "in");
    let r = sample_refs(&mut s);
    let diameter = |s: &mut Session| {
        assert!(s.execute("selection.set", &json!({"id": r.boss})).is_ok());
        let node = s.execute("selection.get", &Value::Null).unwrap_or_default();
        node["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["name"] == "Diameter")
            .map(|p| p["value"].clone())
    };
    assert_eq!(diameter(&mut s), Some(json!("0.9449 in")));
    assert!(s.execute("settings.units.set", &json!({"units": "mm"})).is_ok());
    assert_eq!(diameter(&mut s), Some(json!("24.000 mm")));
    assert!(
        s.outline_node("job").is_some_and(|n| n.properties.iter().any(|(k, v)| k == "Units" && v == "Millimetres"))
    );
    // Commands and their results stay in millimetres either way.
    assert!(s.execute("settings.units.set", &json!({"units": "in"})).is_ok());
    let t = s.execute("tool.create.flat", &json!({"diameter": 6})).unwrap_or_default();
    assert_eq!(t["diameter"], 6.0);
    assert!(s.execute("settings.units.set", &json!({"units": "furlong"})).is_err());
    assert_eq!(s.execute("session.state.get", &Value::Null).unwrap_or_default()["units"], "in");
}

#[test]
fn the_machine_tab_sits_between_setup_and_tooling() {
    assert_eq!(&RibbonTab::IDS[..4], &["home", "setup", "machine", "tooling"]);
}

/// A sample session with a Ø6 flat end mill (T1) in the library.
fn sample_with_tool() -> Session {
    let mut s = Session::with_sample();
    let t =
        s.execute("tool.create.flat", &json!({"diameter": 6, "feed": 1000, "plunge_feed": 300, "spindle_rpm": 10000}));
    assert!(t.is_ok(), "{t:?}");
    s
}

/// The sample base's bottom face (z = 0, the 100 × 60 outline).
fn base_bottom(s: &mut Session) -> String {
    one_face(s, "plane", plane_at([0.0, 0.0, -1.0], 2, 0.0))
}

fn moves(s: &mut Session, id: &str) -> Vec<Value> {
    let tp = s.execute("toolpath.get", &json!({"id": id, "moves": true})).unwrap_or_default();
    tp["move_list"].as_array().cloned().unwrap_or_default()
}

fn cut_points(moves: &[Value]) -> Vec<[f64; 3]> {
    moves.iter().filter(|m| m["kind"] == "cut").map(|m| vec3(&m["to"])).collect()
}

fn signed_area(pts: &[[f64; 3]]) -> f64 {
    (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

/// Distance from (x, y) to the sample base outline (-50..50, -30..30), outside positive.
fn outside_distance(p: [f64; 3]) -> f64 {
    let dx = (p[0].abs() - 50.0).max(0.0);
    let dy = (p[1].abs() - 30.0).max(0.0);
    dx.hypot(dy)
}

#[test]
fn contour_runs_outside_the_outline_at_depth_climbing_clockwise() {
    let mut s = sample_with_tool();
    let bottom = base_bottom(&mut s);
    let tp = s.execute("toolpath.contour", &json!({"geometry": [bottom], "side": "outside"})).unwrap_or_default();
    assert_eq!(tp["id"], "toolpath.1", "{tp}");
    assert_eq!(tp["tool"], 1);
    let m = moves(&mut s, "toolpath.1");
    let cuts = cut_points(&m);
    assert!(cuts.len() > 4);
    assert!(cuts.iter().all(|p| p[2].abs() < 1e-4), "one depth, at the selected outline");
    assert!(cuts.iter().all(|p| (outside_distance(*p) - 3.0).abs() < 0.05), "offset by the tool radius");
    assert!(signed_area(&cuts) < 0.0, "climb outside runs clockwise");
    // Starts and ends at clearance: model top 42 + 0.5 in, the inch default.
    assert_eq!(m.first().map(|x| x["kind"].clone()), Some(json!("rapid")));
    assert!((vec3(&m.last().map(|x| x["to"].clone()).unwrap_or_default())[2] - 54.7).abs() < 1e-4);
    // The outline shows the toolpath; conventional reverses the direction.
    assert!(s.outline_node("toolpath.1").is_some());
    let conv = s
        .execute("toolpath.contour", &json!({"id": "toolpath.1", "geometry": [bottom], "direction": "conventional"}))
        .unwrap_or_default();
    assert_eq!(conv["id"], "toolpath.1", "regenerated in place");
    assert!(signed_area(&cut_points(&moves(&mut s, "toolpath.1"))) > 0.0);
    assert_eq!(s.execute("toolpath.list", &Value::Null).unwrap_or_default().as_array().map(Vec::len), Some(1));
}

#[test]
fn contour_depths_radial_passes_and_stock() {
    let mut s = sample_with_tool();
    let bottom = base_bottom(&mut s);
    let tp = s
        .execute(
            "toolpath.contour",
            &json!({
                "geometry": [bottom], "multiple_depths": true, "max_stepdown": 10,
                "radial_passes": 2, "radial_stepover": 2, "stock_radial": 0.5, "stock_axial": 0.25,
            }),
        )
        .unwrap_or_default();
    let cuts = cut_points(&moves(&mut s, tp["id"].as_str().unwrap_or_default()));
    let mut zs: Vec<i64> = cuts.iter().map(|p| (p[2] * 1000.0).round() as i64).collect();
    zs.dedup();
    // 42 → 0.25 in five steps of 8.35.
    assert_eq!(zs, [33_650, 25_300, 16_950, 8_600, 250], "{zs:?}");
    let mut dists: Vec<i64> = cuts.iter().map(|p| (outside_distance(*p) * 10.0).round() as i64).collect();
    dists.sort_unstable();
    dists.dedup();
    assert_eq!(dists, [35, 55], "radius + stock, plus one stepover");
}

#[test]
fn contour_inside_open_edges_leads_ramps_and_feed_optimization() {
    let mut s = sample_with_tool();
    let r = sample_refs(&mut s);
    // Inside the boss's ... a boss is solid; cut inside the deck outline instead.
    let deck = r.deck.clone();
    let tp = s
        .execute("toolpath.contour", &json!({"geometry": [deck], "side": "inside", "bottom_offset": -1}))
        .unwrap_or_default();
    let cuts = cut_points(&moves(&mut s, tp["id"].as_str().unwrap_or_default()));
    assert!(cuts.iter().all(|p| (p[0].abs() - 47.0).abs() < 0.05 || (p[1].abs() - 27.0).abs() < 0.05), "3 mm inside");
    assert!(signed_area(&cuts) > 0.0, "climb inside runs anticlockwise");
    assert!(cuts.iter().all(|p| (p[2] - 19.0).abs() < 1e-4), "bottom offset goes deeper");

    // An open edge: the block's top-front edge, cut on its left.
    let tp =
        s.execute("toolpath.contour", &json!({"geometry": [r.top_front_edge], "lead_in": "line", "ramp": "plunge"}));
    assert!(tp.is_ok(), "{tp:?}");
    let m = moves(&mut s, tp.unwrap_or_default()["id"].as_str().unwrap_or_default());
    assert!(m.iter().any(|x| x["kind"] == "plunge"));
    assert!(m.iter().any(|x| x["kind"] == "lead"));

    // Feed optimization slows the corners of the rectangle.
    let bottom = base_bottom(&mut s);
    let tp = s
        .execute(
            "toolpath.contour",
            &json!({"geometry": [bottom], "lead_in": "none", "feed_optimization": true, "optimization_feed": 40}),
        )
        .unwrap_or_default();
    let m = moves(&mut s, tp["id"].as_str().unwrap_or_default());
    let feeds: Vec<f64> = m.iter().filter(|x| x["kind"] == "cut").filter_map(|x| x["feed"].as_f64()).collect();
    assert!(feeds.contains(&400.0) && feeds.contains(&1000.0), "{feeds:?}");
}

#[test]
fn contour_refuses_what_it_cannot_cut() {
    let mut s = Session::with_sample();
    let bottom = base_bottom(&mut s);
    let r = s.execute("toolpath.contour", &json!({"geometry": [bottom]}));
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("empty")), "{r:?}");
    let mut s = sample_with_tool();
    let refs = sample_refs(&mut s);
    assert!(s.execute("toolpath.contour", &Value::Null).is_err(), "no geometry");
    let big = s.execute("tool.create.flat", &json!({"diameter": 30, "feed": 500})).unwrap_or_default();
    let boss_top = one_face(&mut s, "plane", plane_at([0.0, 0.0, 1.0], 2, 42.0));
    let r = s.execute("toolpath.contour", &json!({"geometry": [boss_top], "side": "inside", "tool": big["number"]}));
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("does not fit")), "{r:?}");
    let r = s.execute("toolpath.contour", &json!({"geometry": [refs.deck], "clearance_offset": -100}));
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("must not be below")), "{r:?}");
    let corner =
        s.execute("model.vertices.list", &json!({"id": "model.sample"})).unwrap_or_default()[0]["vertex"].clone();
    assert!(s.execute("toolpath.contour", &json!({"geometry": [corner]})).is_err(), "a vertex is no contour");
    assert!(
        s.execute("toolpath.contour", &json!({"geometry": [refs.deck], "feed": 999999})).is_err(),
        "over the machine's feed"
    );
    assert!(s.execute("toolpath.remove", &json!({"id": "toolpath.9"})).is_err());
}

fn gcode(s: &mut Session, p: Value) -> String {
    let r = s.execute("post.gcode.get", &p);
    assert!(r.is_ok(), "{r:?}");
    r.unwrap_or_default()["text"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn gcode_has_a_safety_block_tool_change_and_the_moves() {
    let mut s = sample_with_tool();
    assert!(s.execute("post.gcode.get", &Value::Null).is_err(), "no toolpaths yet");
    let bottom = base_bottom(&mut s);
    assert!(s.execute("toolpath.contour", &json!({"geometry": [bottom], "lead_in": "none"})).is_ok());
    let text = gcode(&mut s, json!({}));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.first(), Some(&"%"));
    assert_eq!(lines.get(1), Some(&"O1000"));
    for want in ["G90 G94 G17 G40 G49 G80", "G20", "T1 M6", "S10000 M3", "G54", "M8", "M9", "M5", "G91 G28 Z0", "M30"] {
        assert!(lines.contains(&want), "missing {want}:\n{text}");
    }
    assert_eq!(lines.last(), Some(&"%"));
    // Length compensation on the first Z move: clearance, model top 42 mm + 0.5 in = 2.1535 in.
    assert!(lines.iter().any(|l| l.starts_with("G0 G43 Z2.1535 H1")), "{text}");
    // Every line that moves an axis names its motion.
    for l in lines.iter().filter(|l| !l.starts_with('(') && l.split(' ').any(|w| w.starts_with(['X', 'Y', 'Z']))) {
        assert!(
            l.starts_with("G0 ") || l.starts_with("G1 ") || l.starts_with("G91 G28"),
            "no motion code: {l}\n{text}"
        );
    }
    // Cutting feed 1000 mm/min = 39.37 in/min, written once until it changes.
    assert!(lines.iter().any(|l| l.starts_with("G1 Z0.0000 F11.81")), "plunge feed: {text}");
    assert!(lines.iter().any(|l| l.ends_with("F39.37")), "{text}");
    assert_eq!(text.matches("F39.37").count(), 1, "modal feed");
    // The contour sits 3 mm outside the base: X reaches ±53 mm = 2.0866 in.
    assert!(text.contains("X2.0866") || text.contains("X-2.0866"), "{text}");
    // Nothing is written without an axis, and no "-0".
    assert!(!lines.iter().any(|l| *l == "G1" || *l == "G0"));
    assert!(!text.contains("-0.0000"));

    // Millimetres, line numbers and the program number.
    let mm = gcode(&mut s, json!({"units": "mm", "line_numbers": true, "program_number": 42}));
    assert!(mm.contains("O0042") && mm.contains("G21") && mm.contains("F1000.0"), "{mm}");
    assert!(mm.lines().any(|l| l.starts_with("N5 ")), "{mm}");
    assert!(mm.contains("X53.000") || mm.contains("X-53.000"), "{mm}");
}

#[test]
fn gcode_is_written_in_the_toolpaths_wcs() {
    let mut s = sample_with_tool();
    assert!(s.execute("setup.wcs.create", &json!({"name": "Vice", "activate": false})).is_ok());
    let w2 =
        s.execute("setup.wcs.create", &json!({"origin": "point", "origin_point": [10, 20, 0]})).unwrap_or_default();
    assert_eq!(w2["id"], "wcs.2");
    let bottom = base_bottom(&mut s);
    assert!(s.execute("toolpath.contour", &json!({"geometry": [bottom], "lead_in": "none"})).is_ok());
    let text = gcode(&mut s, json!({"units": "mm"}));
    assert!(text.lines().any(|l| l == "G55"), "second WCS is G55:\n{text}");
    // World X 53 is X 43 from an origin at X 10.
    assert!(text.contains("X43.000"), "{text}");
    assert!(!text.contains("X53.000"));
}

#[test]
fn gcode_refuses_a_toolpath_without_spindle_speed() {
    let mut s = Session::with_sample();
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "feed": 800})).is_ok());
    let bottom = base_bottom(&mut s);
    assert!(s.execute("toolpath.contour", &json!({"geometry": [bottom]})).is_ok());
    let r = s.execute("post.gcode.get", &Value::Null);
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("spindle speed")), "{r:?}");
    // A speed on the toolpath is enough.
    let id = s.execute("toolpath.list", &Value::Null).unwrap_or_default()[0]["id"].clone();
    assert!(s.execute("toolpath.contour", &json!({"id": id, "geometry": [bottom], "spindle_rpm": 9000})).is_ok());
    assert!(gcode(&mut s, json!({})).contains("S9000 M3"));
    let file = TempFile::new("part.nc", b"");
    let r = s.execute("post.gcode.export", &json!({"path": file.path()})).unwrap_or_default();
    assert!(r["lines"].as_u64().unwrap_or(0) > 10);
    assert!(std::fs::read_to_string(file.path()).unwrap_or_default().ends_with("%\n"));
}

#[test]
fn a_saved_job_opens_in_a_new_tab_as_it_was() {
    let mut s = Session::with_sample();
    assert!(s.execute("tool.create.flat", &json!({"diameter": 6, "feed": 800, "spindle_rpm": 9000})).is_ok());
    assert!(s.execute("setup.wcs.create", &json!({})).is_ok());
    let bottom = base_bottom(&mut s);
    assert!(s.execute("toolpath.contour", &json!({"geometry": [bottom]})).is_ok());
    assert!(s.job.is_modified());
    // A save without a path needs one the first time.
    assert!(s.execute("job.save", &json!({})).is_err());
    let file = TempFile::new("part.cosmic", b"");
    let stem = file.path().trim_end_matches(".cosmic").to_owned();
    let r = s.execute("job.save", &json!({"path": stem})).unwrap_or_default();
    assert_eq!(r["path"], file.path());
    assert!(!s.job.is_modified());
    assert_eq!(s.job.name, "part");
    let before = gcode(&mut s, json!({}));

    // Closed and opened again (into the empty document the close leaves), it comes back with
    // the same models, WCS and toolpaths.
    assert!(s.execute("job.tab.close", &json!({})).is_ok());
    assert!(s.job.models.is_empty());
    assert!(s.execute("job.open", &json!({"path": file.path()})).is_ok());
    assert_eq!(s.tabs().len(), 1);
    assert_eq!(s.job.models.len(), 1);
    assert_eq!(s.job.wcs.len(), 1);
    assert_eq!(s.job.toolpaths.len(), 1);
    assert!(!s.job.is_modified());
    assert_eq!(gcode(&mut s, json!({})), before);
    // A new document opens a second tab; opening the file again shows its tab instead.
    assert!(s.execute("job.new", &Value::Null).is_ok());
    let tabs = s.execute("job.tab.list", &Value::Null).unwrap_or_default();
    assert_eq!(tabs["tabs"].as_array().map(Vec::len), Some(2), "{tabs}");
    assert_eq!(tabs["active"], 1);
    assert!(s.execute("job.open", &json!({"path": file.path()})).is_ok());
    assert_eq!(s.tabs().len(), 2);
    assert_eq!(s.active_tab(), 0);
}

#[test]
fn tabs_switch_close_and_guard_unsaved_work() {
    let mut s = Session::with_sample();
    assert!(s.execute("job.new", &Value::Null).is_ok());
    assert_eq!(s.tabs().len(), 2);
    assert!(s.job.models.is_empty());
    // A blank document is replaced, not joined, by the next new one.
    assert!(s.execute("job.new", &Value::Null).is_ok());
    assert_eq!(s.tabs().len(), 2);
    let revision = s.job.revision;
    assert!(s.execute("job.tab.activate", &json!({"index": 0})).is_ok());
    assert_eq!(s.job.name, "Sample");
    assert_ne!(s.job.revision, revision);
    // Unsaved changes stop a close unless they are discarded.
    assert!(s.execute("setup.stock.fit", &json!({})).is_ok());
    assert!(s.tabs()[0].modified);
    assert!(s.execute("job.tab.close", &json!({"index": 0})).is_err());
    assert_eq!(s.tabs().len(), 2);
    assert!(s.execute("job.tab.close", &json!({"index": 0, "discard": true})).is_ok());
    assert_eq!(s.tabs().len(), 1);
    assert_eq!(s.job.name, "Untitled");
    // Closing the last tab leaves an empty document.
    assert!(s.execute("job.tab.close", &json!({})).is_ok());
    assert_eq!(s.tabs().len(), 1);
    assert!(s.execute("job.tab.activate", &json!({"index": 5})).is_err());
    // Views and selections are not changes to the job.
    assert!(s.execute("view.fit", &Value::Null).is_ok());
    assert!(!s.job.is_modified());
}

#[test]
fn hostile_job_files_are_refused() {
    let mut s = Session::with_sample();
    let good = jobfile_text(&s);
    let cases: Vec<String> = vec![
        String::new(),
        "{".into(),
        "{}".into(),
        r#"{"format":"CosmicCAM/99","job":{}}"#.into(),
        r#"{"format":"CosmicCAM/1"}"#.into(),
        good.replace("\"positions\":\"", "\"positions\":\"*"),
        good.replace("\"triangles\":\"", "\"triangles\":\"AAAA"),
        r#"{"format":"CosmicCAM/1","job":{"name":"x","stock":{"min":[1,1,1],"max":[0,0,0]}}}"#.into(),
        r#"{"format":"CosmicCAM/1","job":{"name":"x","wcs":[{"id":"w","name":"w","origin":[0,0,0],"x_axis":[1,0,0],"z_axis":[1,0,0]}]}}"#.into(),
        r#"{"format":"CosmicCAM/1","job":{"name":"x","models":[{"id":"m","name":"m","positions":"AAAAAAAAAAAAAAAA","triangles":"AQAAAAIAAAADAAAA"}]}}"#.into(),
        r#"{"format":"CosmicCAM/1","job":{"name":"x","toolpaths":[{"id":"t","name":"t","strategy":"s","machine":"m","tool":1,"start":[0,0,0],"moves":"Bw=="}]}}"#.into(),
    ];
    for (i, text) in cases.iter().enumerate() {
        let file = TempFile::new(&format!("bad{i}.cosmic"), text.as_bytes());
        let r = s.execute("job.open", &json!({"path": file.path()}));
        assert!(r.is_err(), "case {i} opened: {r:?}");
        assert_eq!(s.tabs().len(), 1);
    }
    let file = TempFile::new("good.cosmic", good.as_bytes());
    assert!(s.execute("job.open", &json!({"path": file.path()})).is_ok());
    assert!(s.execute("job.open", &json!({"path": "/no/such/file.cosmic"})).is_err());
}

fn jobfile_text(s: &Session) -> String {
    crate::jobfile::to_string(&s.job, &s.view.camera, &s.library)
}

#[test]
fn contours_start_nearest_the_entry_point() {
    let mut s = sample_with_tool();
    let bottom = base_bottom(&mut s);
    let start_of = |s: &mut Session, extra: Value| -> Option<[f32; 3]> {
        let mut p = json!({"geometry": [bottom], "lead_in": "none"});
        if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(extra.clone());
        }
        let r = s.execute("toolpath.contour", &p).ok()?;
        let id = r["id"].as_str()?.to_owned();
        s.job.toolpaths.iter().find(|t| t.id == id).map(|t| t.start.to_array())
    };
    // A point picked off the right-hand side starts the contour on that side, level with it.
    let right = start_of(&mut s, json!({"entry_from": "selection", "entry_point": [120, 10, 0]})).unwrap_or_default();
    assert!((right[0] - 53.0).abs() < 1.0e-3 && (right[1] - 10.0).abs() < 1.0e-3, "{right:?}");
    let left = start_of(&mut s, json!({"entry_from": "selection", "entry_point": [-120, -5, 0]})).unwrap_or_default();
    assert!((left[0] + 53.0).abs() < 1.0e-3 && (left[1] + 5.0).abs() < 1.0e-3, "{left:?}");
    // Selection needs the point.
    let r = s.execute("toolpath.contour", &json!({"geometry": [bottom], "entry_from": "selection"}));
    assert!(matches!(r, Err(EngineError::InvalidParams(ref m)) if m.contains("entry point")), "{r:?}");
    // Origin and WCS entries are accepted.
    assert!(start_of(&mut s, json!({"entry_from": "origin"})).is_some());
    assert!(start_of(&mut s, json!({"entry_from": "wcs"})).is_some());
}

#[test]
fn setups_hold_a_machine_a_wcs_and_their_stock() {
    let mut s = Session::with_sample();
    assert!(s.execute("setup.create", &json!({})).is_err(), "a name is required");
    assert!(s.execute("setup.create", &json!({"name": "  "})).is_err());
    assert!(s.execute("setup.create", &json!({"name": "Op1", "wcs": "wcs.9"})).is_err());
    assert!(s.execute("setup.wcs.create", &json!({"name": "Vice"})).is_ok());
    // Fitted with margins and rounded up to whole 10 mm.
    let op1 = s
        .execute(
            "setup.create",
            &json!({"name": "Op1", "wcs": "wcs.1", "stock_margin_sides": 2, "stock_margin_top": 1, "stock_round": 10}),
        )
        .unwrap_or_default();
    assert_eq!(op1["id"], "setup.1");
    assert_eq!(op1["active"], true);
    assert_eq!(op1["machine"], "machine.1");
    assert_eq!(op1["wcs"], "wcs.1");
    assert_eq!(op1["stock"]["size"], json!([110.0, 70.0, 50.0]));
    // The top stays at the model top plus the margin; rounding adds underneath.
    assert_eq!(op1["stock"]["max"][2], 43.0);
    assert_eq!(s.job.stock.map(|b| b.size().to_array()), Some([110.0, 70.0, 50.0]));

    // A fixed size on the origin, then corners.
    let op2 = s
        .execute("setup.create", &json!({"name": "Op2", "wcs": "origin", "stock": "size", "stock_size": [120, 80, 50]}))
        .unwrap_or_default();
    assert_eq!(op2["wcs"], Value::Null);
    assert_eq!(op2["stock"]["max"][2], 42.0);
    assert_eq!(op2["stock"]["min"][0], -60.0);
    assert_eq!(s.job.active_wcs, None);
    let r = s.execute("setup.stock.box", &json!({"stock": "box", "stock_min": [-1, -1, -1], "stock_max": [1, 1, 1]}));
    assert_eq!(r.unwrap_or_default()["size"], json!([2.0, 2.0, 2.0]));
    assert!(s.execute("setup.stock.box", &json!({"stock": "size", "stock_size": [0, 1, 1]})).is_err());
    assert!(
        s.execute("setup.stock.box", &json!({"stock": "box", "stock_min": [0, 0, 0], "stock_max": [1, 1, 0]})).is_err()
    );

    // Switching setups brings back each one's stock and WCS.
    assert!(s.execute("setup.activate", &json!({"id": "setup.1"})).is_ok());
    assert_eq!(s.job.active_wcs.as_deref(), Some("wcs.1"));
    assert_eq!(s.job.stock.map(|b| b.size().x), Some(110.0));
    let group = s.outline_node("setups").map(|g| g.children).unwrap_or_default();
    let names: Vec<String> = group.iter().map(|n| n.label.clone()).collect();
    assert_eq!(names, ["Op1 (active)", "Op2", "Vice (active)"]);
    assert!(s.outline_node("setup.1/stock").is_some());

    // They are saved with the job.
    let file = TempFile::new("setups.cosmic", b"");
    assert!(s.execute("job.save", &json!({"path": file.path()})).is_ok());
    assert!(s.execute("job.tab.close", &json!({})).is_ok());
    assert!(s.execute("job.open", &json!({"path": file.path()})).is_ok());
    assert_eq!(s.job.setups.len(), 2);
    assert_eq!(s.job.active_setup.as_deref(), Some("setup.1"));

    // Removing a WCS sends its setups to the origin; removing the active setup activates the next.
    assert!(s.execute("setup.wcs.remove", &json!({"id": "wcs.1"})).is_ok());
    assert_eq!(s.job.setup_by_id("setup.1").and_then(|st| st.wcs.clone()), None);
    assert!(s.execute("setup.remove", &json!({"id": "setup.1"})).is_ok());
    assert_eq!(s.job.active_setup.as_deref(), Some("setup.2"));
    assert_eq!(s.job.stock.map(|b| b.size().x), Some(2.0));
}

#[test]
fn a_tighter_tolerance_puts_more_points_on_curves() {
    let mut s = sample_with_tool();
    // The boss's cylinder face: its rim is followed as a true circle.
    let faces = s.execute("model.faces.list", &json!({"id": "model.sample"})).unwrap_or_default();
    let boss = faces
        .as_array()
        .and_then(|f| f.iter().find(|f| f["kind"] == "cylinder"))
        .map(|f| f["face"].clone())
        .unwrap_or_default();
    let moves = |s: &mut Session, tolerance: f64| -> usize {
        let r = s.execute("toolpath.contour", &json!({"geometry": [boss], "tolerance": tolerance, "lead_in": "none"}));
        r.ok().and_then(|v| v["moves"].as_u64()).unwrap_or(0) as usize
    };
    let (coarse, fine) = (moves(&mut s, 0.1), moves(&mut s, 0.001));
    assert!(coarse > 8 && fine > coarse * 4, "coarse {coarse}, fine {fine}");
}

/// The boss's top rim: the circular edge at z = 42.
fn boss_rim(s: &mut Session) -> String {
    let edges = s.execute("model.edges.list", &json!({"id": "model.sample"})).unwrap_or_default();
    edges
        .as_array()
        .and_then(|e| e.iter().find(|e| e["kind"] == "circle" && (vec3(&e["center"])[2] - 42.0).abs() < 1e-3))
        .and_then(|e| e["edge"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn new_toolpath(s: &mut Session, command: &str, params: Value) -> (Value, Vec<Value>) {
    let r = s.execute(command, &params);
    assert!(r.is_ok(), "{command}: {r:?}");
    let tp = r.unwrap_or_default();
    let id = tp["id"].as_str().unwrap_or_default().to_owned();
    (tp, moves(s, &id))
}

#[test]
fn facing_runs_rows_across_the_stock_down_to_the_model_top() {
    let mut s = sample_with_tool();
    let (tp, mv) =
        new_toolpath(&mut s, "toolpath.face", json!({"stepover": 4, "multiple_depths": true, "stepdown": 1}));
    assert_eq!(tp["strategy"], "face");
    let cuts = cut_points(&mv);
    // The stock top is 45 and the model top 42: three levels, the last at 42.
    let mut zs: Vec<i64> = cuts.iter().map(|c| (c[2] * 1000.0).round() as i64).collect();
    zs.sort_unstable();
    zs.dedup();
    assert_eq!(zs, [42_000, 43_000, 44_000]);
    // Rows run past the stock's sides (±53 in X) by more than the tool radius.
    let xs = cuts.iter().map(|c| c[0]);
    let (lo, hi) = xs.fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
    assert!(lo < -56.0 && hi > 56.0, "{lo} {hi}");
    let ys = cuts.iter().map(|c| c[1]).fold((f64::MAX, f64::MIN), |(a, b), y| (a.min(y), b.max(y)));
    assert!(ys.0 <= -33.0 + 1e-6 && ys.1 >= 33.0 - 1e-6, "{ys:?}");
}

#[test]
fn pockets_clear_inside_the_outline_down_to_depth() {
    let mut s = sample_with_tool();
    let top = one_face(&mut s, "plane", plane_at([0.0, 0.0, 1.0], 2, 35.0));
    let (tp, mv) = new_toolpath(
        &mut s,
        "toolpath.pocket",
        json!({"geometry": [top], "top_from": "selection", "bottom_offset": -5, "stepdown": 2.5, "stepover": 2.5}),
    );
    assert_eq!(tp["strategy"], "pocket2d");
    let cuts = cut_points(&mv);
    assert!(!cuts.is_empty());
    // Every cut keeps the 3 mm tool inside the block's top (-40..5, -20..20).
    for c in &cuts {
        assert!(c[0] >= -37.0 - 1e-3 && c[0] <= 2.0 + 1e-3 && c[1].abs() <= 17.0 + 1e-3, "{c:?}");
        assert!(c[2] >= 30.0 - 1e-6 && c[2] <= 35.0, "{c:?}");
    }
    assert!(cuts.iter().any(|c| (c[2] - 30.0).abs() < 1e-6), "reaches the bottom");
    // The last ring of the last level runs along the wall, one radius in.
    assert!(cuts.iter().any(|c| (c[0] + 37.0).abs() < 1e-3));
    // Too big a tool is refused.
    assert!(s.execute("toolpath.pocket", &json!({"geometry": [top], "stock_radial": 25})).is_err());
}

#[test]
fn drilling_writes_canned_cycles() {
    let mut s = sample_with_tool();
    let rim = boss_rim(&mut s);
    let r = s.execute("toolpath.drill", &json!({"geometry": [rim]}));
    assert!(r.is_err(), "a rim alone has no depth: {r:?}");
    let (tp, mv) = new_toolpath(
        &mut s,
        "toolpath.drill",
        json!({"geometry": [rim], "bottom_offset": -10, "retract_offset": 2, "clearance_offset": 15, "cycle": "peck", "peck_depth": 4}),
    );
    assert_eq!(tp["strategy"], "drill");
    // Three pecks down to 32 at the boss centre.
    let plunges: Vec<[f64; 3]> = mv.iter().filter(|m| m["kind"] == "plunge").map(|m| vec3(&m["to"])).collect();
    assert_eq!(plunges.len(), 3);
    assert!(plunges.iter().all(|p| (p[0] - 28.0).abs() < 1e-3 && p[1].abs() < 1e-3));
    assert!((plunges[2][2] - 32.0).abs() < 1e-3);
    let text = gcode(&mut s, json!({"units": "mm"}));
    assert!(text.contains("G99 G83 X28.000 Y0.000 Z32.000 R44.000 Q4.000 F300.0"), "{text}");
    assert!(text.lines().any(|l| l == "G80"), "{text}");
    // Tapping needs a pitch, and feeds at pitch × rpm.
    assert!(s.execute("toolpath.drill", &json!({"geometry": [rim], "bottom_offset": -10, "cycle": "tap"})).is_err());
    let id = tp["id"].clone();
    assert!(
        s.execute(
            "toolpath.drill",
            &json!({"id": id, "geometry": [rim], "bottom_offset": -10, "retract_offset": 2, "cycle": "tap", "pitch": 1.0})
        )
            .is_ok()
    );
    assert!(gcode(&mut s, json!({"units": "mm"})).contains("G99 G84 X28.000 Y0.000 Z32.000 R44.000 F10000.0"));
}

#[test]
fn roughing_clears_around_the_part_without_touching_it() {
    let mut s = sample_with_tool();
    let (tp, mv) = new_toolpath(&mut s, "toolpath.rough", json!({"stepdown": 5, "stepover": 3, "stock_radial": 0.5}));
    assert_eq!(tp["strategy"], "rough3d");
    let cuts = cut_points(&mv);
    assert!(cuts.len() > 20);
    for c in &cuts {
        let (x, y, z) = (c[0], c[1], c[2]);
        // Below the boss top, the tool stays clear of the boss (r 12 + tool 3 + stock 0.5).
        if z < 42.0 - 1e-6 {
            let d = ((x - 28.0).powi(2) + y * y).sqrt();
            assert!(d >= 15.5 - 0.05, "into the boss at {c:?}");
        }
        // Below the base top, nothing runs over the base.
        if z < 20.0 - 1e-6 {
            let d = (x.abs() - 50.0).max(0.0).hypot((y.abs() - 30.0).max(0.0));
            assert!(d >= 3.5 - 0.05, "into the base at {c:?}");
        }
        assert!(z >= -1e-6);
    }
}

#[test]
fn waterline_and_raster_follow_the_part() {
    let mut s = sample_with_tool();
    let (tp, mv) = new_toolpath(&mut s, "toolpath.waterline", json!({"stepdown": 4, "top_offset": -2}));
    assert_eq!(tp["strategy"], "waterline");
    let cuts = cut_points(&mv);
    // Around the boss above the block (z > 35), passes sit one radius off its wall.
    let ring: Vec<&[f64; 3]> =
        cuts.iter().filter(|c| c[2] > 35.5 && ((c[0] - 28.0).powi(2) + c[1] * c[1]).sqrt() < 20.0).collect();
    assert!(!ring.is_empty());
    for c in ring {
        let d = ((c[0] - 28.0).powi(2) + c[1] * c[1]).sqrt();
        assert!((d - 15.0).abs() < 0.1, "{d}");
    }

    let (tp, mv) = new_toolpath(&mut s, "toolpath.raster", json!({"stepover": 5}));
    assert_eq!(tp["strategy"], "raster");
    let cuts = cut_points(&mv);
    assert!(cuts.len() > 20);
    // Over the boss the tool rides its top; off the part it runs at the bottom height.
    assert!(cuts.iter().any(|c| (c[0] - 28.0).abs() < 16.0 && c[1].abs() < 4.0 && (c[2] - 42.0).abs() < 1e-3));
    assert!(cuts.iter().all(|c| c[2] >= -1e-6 && c[2] <= 42.0 + 1e-6));
}

#[test]
fn the_demo_project_opens_ready_to_post() {
    let mut s = Session::new();
    let before = s.library.active_machine().map_or(0, |m| m.tools.len());
    assert!(s.execute("job.demo.open", &Value::Null).is_ok());
    assert_eq!(s.job.name, "Demo project");
    assert_eq!(s.job.path, None, "saving the demo asks where");
    assert!(!s.job.is_modified());
    assert_eq!(s.job.models.len(), 1);
    assert_eq!(s.job.wcs.len(), 1);
    assert_eq!(s.job.setups.len(), 1);
    assert!(s.job.stock.is_some());
    assert!(s.job.toolpaths.len() >= 4);
    // Its tools join the library, so the whole program posts.
    assert!(s.library.active_machine().map_or(0, |m| m.tools.len()) >= before + 2);
    let text = gcode(&mut s, json!({}));
    assert!(text.lines().count() > 1000 && text.ends_with("%\n"));
    // Opening it again shows a second copy only if the first was kept; a blank tab is replaced.
    assert_eq!(s.tabs().len(), 1);
}

#[test]
fn job_files_bring_their_tools() {
    let mut s = sample_with_tool();
    let bottom = base_bottom(&mut s);
    assert!(s.execute("toolpath.contour", &json!({"geometry": [bottom]})).is_ok());
    let file = TempFile::new("tools.cosmic", b"");
    assert!(s.execute("job.save", &json!({"path": file.path()})).is_ok());
    // A session whose library has no tools opens the job and gains T1, and posts it.
    let mut other = Session::new();
    assert!(other.library.active_machine().is_some_and(|m| m.tools.is_empty()));
    assert!(other.execute("job.open", &json!({"path": file.path()})).is_ok());
    assert!(other.library.active_machine().is_some_and(|m| m.tool(1).is_some()));
    assert!(gcode(&mut other, json!({})).contains("T1 M6"));
    // A library that already has a T1 keeps its own.
    let mut third = Session::new();
    assert!(third.execute("tool.create.flat", &json!({"diameter": 12, "feed": 500, "spindle_rpm": 8000})).is_ok());
    assert!(third.execute("job.open", &json!({"path": file.path()})).is_ok());
    assert_eq!(third.library.active_machine().and_then(|m| m.tool(1)).map(|t| t.diameter), Some(12.0));
}
