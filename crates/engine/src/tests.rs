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
    let spec = cmd::find("toolpath.pocket");
    assert!(spec.is_some_and(|c| !s.is_enabled(c)));
    assert_eq!(s.execute("toolpath.pocket", &Value::Null), Err(EngineError::Disabled("toolpath.pocket".into())));
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
    assert_eq!(panels, ["Job", "Setup", "Display"]);
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
