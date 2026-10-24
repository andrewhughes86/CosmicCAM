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
        for id in spec.placement.map(|p| p.menu).unwrap_or_default() {
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
