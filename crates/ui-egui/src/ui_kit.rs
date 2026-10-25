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
    /// Floating dialogs that gather a command's input.
    Dialog,
    /// A settings panel docked at the side of the window (the toolpath editor).
    SidePanel,
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
            Surface::SidePanel => {
                Frame::new().fill(t.panel_bg).inner_margin(Margin::same(10)).stroke(Stroke::new(1.0, t.border))
            }
            Surface::Dialog => Frame::new()
                .fill(t.panel_bg)
                .inner_margin(Margin::same(12))
                .corner_radius(CornerRadius::same(6))
                .stroke(Stroke::new(1.0, t.border))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 16,
                    spread: 0,
                    color: t.app_bg.gamma_multiply(0.6),
                }),
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

    /// The same menu, opened by right-clicking `button` instead.
    pub fn context<R>(self, t: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
        Popup::context_menu(self.button).frame(Container::new(Surface::Menu).frame(t)).show(add)
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
    /// Resizable, at the right of the window, starting this wide.
    Right(&'static str, f32),
    Central,
}

impl Panel {
    pub fn top(id: &'static str, surface: Surface) -> Self {
        Self { kind: PanelKind::Top(id), surface }
    }

    pub fn bottom(id: &'static str, surface: Surface) -> Self {
        Self { kind: PanelKind::Bottom(id), surface }
    }

    pub fn right(id: &'static str, surface: Surface, width: f32) -> Self {
        Self { kind: PanelKind::Right(id, width), surface }
    }

    pub fn central(surface: Surface) -> Self {
        Self { kind: PanelKind::Central, surface }
    }

    pub fn show<R>(self, ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let frame = Container::new(self.surface).frame(t);
        match self.kind {
            PanelKind::Top(id) => egui::Panel::top(id).resizable(false).frame(frame).show(ui, add),
            PanelKind::Bottom(id) => egui::Panel::bottom(id).resizable(false).frame(frame).show(ui, add),
            PanelKind::Right(id, width) => {
                egui::Panel::right(id).resizable(true).default_size(width).min_size(280.0).frame(frame).show(ui, add)
            }
            PanelKind::Central => egui::CentralPanel::default().frame(frame).show(ui, add),
        }
    }
}

/// A floating, titled dialog window framed as [`Surface::Dialog`].
pub struct Window {
    title: String,
    /// Tool panels open at the upper left, clear of the part; dialogs open centred.
    tool: bool,
    /// Resizable windows (libraries, browsers) and their starting size.
    size: Option<egui::Vec2>,
}

impl Window {
    pub fn dialog(title: impl Into<String>) -> Self {
        Self { title: title.into(), tool: false, size: None }
    }

    /// A centred, resizable window that stays open beside the work (the tool library).
    pub fn browser(title: impl Into<String>, size: egui::Vec2) -> Self {
        Self { title: title.into(), tool: false, size: Some(size) }
    }

    /// A panel that stays open while the user works in the viewport.
    pub fn tool(title: impl Into<String>) -> Self {
        Self { title: title.into(), tool: true, size: None }
    }

    /// Shows the window while `open` is true; the title bar's close button clears it.
    pub fn show<R>(
        self,
        ctx: &egui::Context,
        t: &Tokens,
        open: &mut bool,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        egui::Window::new(self.title)
            .open(open)
            .collapsible(false)
            .resizable(self.size.is_some())
            .max_size(ctx.content_rect().size() * 0.95)
            .default_size(self.size.unwrap_or(egui::vec2(320.0, 200.0)))
            .pivot(if self.tool { egui::Align2::LEFT_TOP } else { egui::Align2::CENTER_CENTER })
            .default_pos(if self.tool {
                ctx.content_rect().left_top() + egui::vec2(280.0, 160.0)
            } else {
                ctx.content_rect().center()
            })
            .frame(Container::new(Surface::Dialog).frame(t))
            .show(ctx, add)
            .and_then(|r| r.inner)
    }
}
