//! Dockable workspace: panes can be dragged, stacked, split and resized.

use CosmicCAM_engine::{Pane, Session};
use egui::{Id, Ui, WidgetText};
use egui_dock::{DockState, NodeIndex, SurfaceIndex, TabViewer};
use egui_wgpu::RenderState;

use crate::Request;
use crate::panes::{self, ConsoleState};
use crate::theme::Tokens;
use crate::viewport::Viewport;

/// Dock tabs are the engine's panes, so commands can name them.
pub type Tab = Pane;

fn title(tab: Tab) -> &'static str {
    match tab {
        Pane::Viewport => "Viewport",
        Pane::Explorer => "Explorer",
        Pane::Inspector => "Inspector",
        Pane::Console => "Console",
        Pane::Library => "Tool library",
    }
}

/// Viewport in the middle, explorer over inspector on the left, console along the bottom. The
/// tool library is a window of its own, not a dock pane.
pub fn default_layout() -> DockState<Tab> {
    let mut state = DockState::new(vec![Pane::Viewport]);
    let tree = state.main_surface_mut();
    // Fractions size the left / upper side of each split.
    let [center, left] = tree.split_left(NodeIndex::root(), 0.2, vec![Pane::Explorer]);
    tree.split_below(left, 0.55, vec![Pane::Inspector]);
    tree.split_below(center, 0.78, vec![Pane::Console]);
    state
}

/// Panes currently in the dock, in `Pane::ALL` order.
pub fn open_panes(state: &DockState<Tab>) -> Vec<Pane> {
    Pane::ALL.into_iter().filter(|p| state.find_tab(p).is_some()).collect()
}

pub fn hide(state: &mut DockState<Tab>, pane: Pane) {
    if pane == Pane::Viewport {
        return;
    }
    if let Some(path) = state.find_tab(&pane) {
        state.remove_tab(path);
    }
}

/// Opens `pane` where it would be in the default layout, next to its neighbours.
pub fn show(state: &mut DockState<Tab>, pane: Pane) {
    if state.find_tab(&pane).is_some() {
        return;
    }
    let main_leaf = |state: &DockState<Tab>, p: Pane| {
        state.find_tab(&p).filter(|path| path.surface == SurfaceIndex::main()).map(|path| path.node)
    };
    // The library is a window the app opens, never a dock tab.
    if pane == Pane::Library {
        return;
    }
    let partner = match pane {
        Pane::Explorer => Some(Pane::Inspector),
        Pane::Inspector => Some(Pane::Explorer),
        _ => None,
    };
    if let Some(node) = partner.and_then(|p| main_leaf(state, p)) {
        let surface = state.main_surface_mut();
        if pane == Pane::Explorer {
            surface.split_above(node, 0.45, vec![pane]);
        } else {
            surface.split_below(node, 0.55, vec![pane]);
        }
        return;
    }
    match main_leaf(state, Pane::Viewport) {
        Some(node) => {
            let surface = state.main_surface_mut();
            match pane {
                Pane::Console => {
                    surface.split_below(node, 0.78, vec![pane]);
                }
                _ => {
                    surface.split_left(node, 0.2, vec![pane]);
                }
            }
        }
        None => state.push_to_first_leaf(pane),
    }
}

/// Opens `pane` if needed and makes it the active tab of its stack.
pub fn focus(state: &mut DockState<Tab>, pane: Pane) {
    show(state, pane);
    if let Some(path) = state.find_tab(&pane) {
        let _ = state.set_active_tab(path);
        state.set_focused_node_and_surface(path.node_path());
    }
}

pub struct Viewer<'a> {
    pub session: &'a mut Session,
    pub tokens: &'a Tokens,
    pub viewport: &'a mut Viewport,
    pub render_state: Option<&'a RenderState>,
    pub console: &'a mut ConsoleState,
    pub requests: &'a mut Vec<Request>,
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("dock-tab", tab.id()))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        title(*tab).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Pane::Viewport => self.viewport.show(ui, self.session, self.tokens, self.render_state, self.requests),
            Pane::Explorer => panes::explorer(ui, self.session, self.tokens, self.requests),
            Pane::Inspector => panes::inspector(ui, self.session, self.tokens),
            Pane::Console => panes::console(ui, self.session, self.tokens, self.console, self.requests),
            // Never docked; the app draws the library as a window.
            Pane::Library => {
                ui.label("The tool library opens in its own window.");
            }
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Pane::Viewport
    }

    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        // The viewport stays docked; its GPU target is sized to the dock.
        *tab != Pane::Viewport
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            // The explorer scrolls itself, so dragging a toolpath does not drag the view.
            Pane::Viewport | Pane::Console | Pane::Explorer => [false, false],
            Pane::Inspector | Pane::Library => [true, true],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_every_pane_once() {
        let state = default_layout();
        let tabs: Vec<Tab> = state.iter_all_tabs().map(|(_, t)| *t).collect();
        for tab in Pane::ALL {
            let want = usize::from(tab != Pane::Library);
            assert_eq!(tabs.iter().filter(|t| **t == tab).count(), want, "{tab:?}");
        }
    }

    #[test]
    fn panes_hide_show_and_focus() {
        let mut state = default_layout();
        for pane in [Pane::Explorer, Pane::Inspector, Pane::Console, Pane::Library] {
            hide(&mut state, pane);
        }
        assert_eq!(open_panes(&state), vec![Pane::Viewport]);
        hide(&mut state, Pane::Viewport);
        assert_eq!(open_panes(&state), vec![Pane::Viewport]);
        show(&mut state, Pane::Inspector);
        show(&mut state, Pane::Explorer);
        focus(&mut state, Pane::Console);
        focus(&mut state, Pane::Library);
        assert_eq!(open_panes(&state), vec![Pane::Viewport, Pane::Explorer, Pane::Inspector, Pane::Console]);
        show(&mut state, Pane::Console);
        assert_eq!(state.iter_all_tabs().count(), 4);
    }
}
