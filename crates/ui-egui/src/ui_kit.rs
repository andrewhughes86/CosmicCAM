//! The one outer UI container. Feature modules declare a [`Surface`] and get their
//! panel or frame from here; they never build raw egui frames, panels or windows.

use egui::{CornerRadius, Frame, InnerResponse, Margin, Popup, Response, Stroke, Ui};

use crate::theme::Tokens;

/// Semantic role of a container; it decides fill, border and spacing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The contextual ribbon across the top.
    Ribbon,
    /// One labelled group of ribbon buttons.
    RibbonGroup,
    /// The status bar along the bottom.
    StatusBar,
    /// The dock area that hosts workspace panes.
    Workspace,
    /// Floating chrome over the viewport (hints, readouts).
    Overlay,
    /// Dropdown menus opened from buttons.
    Menu,
}

pub struct Container {
    surface: Surface,
}

impl Container {
    pub fn new(surface: Surface) -> Self {
        Self { surface }
    }

    pub fn frame(&self, t: &Tokens) -> Frame {
        match self.surface {
            // The ribbon body is one flat surface; groups are separated by rules, not boxes.
            Surface::Ribbon => {
                Frame::new().fill(t.panel_bg).inner_margin(Margin::ZERO).stroke(Stroke::new(1.0, t.border))
            }
            Surface::RibbonGroup => Frame::new().inner_margin(Margin::symmetric(4, 2)),
            Surface::StatusBar => {
                Frame::new().fill(t.raised_bg).inner_margin(Margin::symmetric(10, 3)).stroke(Stroke::new(1.0, t.border))
            }
            Surface::Workspace => Frame::new().fill(t.app_bg).inner_margin(Margin::same(4)),
            Surface::Overlay => Frame::new()
                .fill(t.panel_bg.gamma_multiply(0.92))
                .inner_margin(Margin::symmetric(8, 4))
                .corner_radius(CornerRadius::same(4))
                .stroke(Stroke::new(1.0, t.border)),
            Surface::Menu => Frame::new()
                .fill(t.panel_bg)
                .inner_margin(Margin::same(4))
                .corner_radius(CornerRadius::same(4))
                .stroke(Stroke::new(1.0, t.border)),
        }
    }

    pub fn show<R>(self, ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        self.frame(t).show(ui, add)
    }
}

/// A dropdown menu that opens below `button` when it is clicked.
pub struct Menu<'a> {
    button: &'a Response,
}

impl<'a> Menu<'a> {
    pub fn below(button: &'a Response) -> Self {
        Self { button }
    }

    pub fn show<R>(self, t: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
        Popup::menu(self.button).frame(Container::new(Surface::Menu).frame(t)).show(add)
    }
}

/// Edge-docked or central application panels, framed by their [`Surface`].
pub struct Panel {
    kind: PanelKind,
    surface: Surface,
}

enum PanelKind {
    Top(&'static str),
    Bottom(&'static str),
    Central,
}

impl Panel {
    pub fn top(id: &'static str, surface: Surface) -> Self {
        Self { kind: PanelKind::Top(id), surface }
    }

    pub fn bottom(id: &'static str, surface: Surface) -> Self {
        Self { kind: PanelKind::Bottom(id), surface }
    }

    pub fn central(surface: Surface) -> Self {
        Self { kind: PanelKind::Central, surface }
    }

    pub fn show<R>(self, ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let frame = Container::new(self.surface).frame(t);
        match self.kind {
            PanelKind::Top(id) => egui::Panel::top(id).resizable(false).frame(frame).show(ui, add),
            PanelKind::Bottom(id) => egui::Panel::bottom(id).resizable(false).frame(frame).show(ui, add),
            PanelKind::Central => egui::CentralPanel::default().frame(frame).show(ui, add),
        }
    }
}
