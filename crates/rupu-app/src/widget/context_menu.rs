//! Minimal right-click context menu — a floating overlay anchored at the
//! mouse position with a vertical list of selectable items. Used by the
//! sidebar's workflow / agent right-click handlers.
//!
//! State (`ContextMenuState`) lives on `WorkspaceWindow`; the window
//! decides what items to show based on which row was right-clicked.
//! `render` returns an overlay element that should be embedded as the
//! last child of the window's root div so it paints over everything else.

use std::sync::Arc;

use gpui::{
    anchored, deferred, div, prelude::*, px, AnyElement, App, Edges, IntoElement, MouseButton,
    Pixels, Point, SharedString, Window,
};

use crate::palette;

/// Callback type for a single menu item's action.
#[allow(clippy::type_complexity)]
pub type SelectCb = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync + 'static>;

/// One row in a context menu.
#[derive(Clone)]
pub struct ContextMenuItem {
    pub label: SharedString,
    pub on_select: SelectCb,
}

impl std::fmt::Debug for ContextMenuItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextMenuItem")
            .field("label", &self.label)
            .field("on_select", &"<fn>")
            .finish()
    }
}

/// State driving the context-menu overlay. `None` on `WorkspaceWindow` means
/// no menu is open; `Some` means the overlay renders.
#[derive(Clone, Debug)]
pub struct ContextMenuState {
    pub position: Point<Pixels>,
    pub items: Vec<ContextMenuItem>,
}

/// Callback the overlay invokes to dismiss itself (item selection or
/// click-outside both call this).
pub type DismissCb = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync + 'static>;

/// Render the menu as an absolutely-positioned overlay. Returns an
/// `AnyElement` that should be embedded as the last child of the window's
/// root so it paints over everything else.
pub fn render(state: &ContextMenuState, on_dismiss: DismissCb) -> AnyElement {
    let dismiss_for_backdrop = on_dismiss.clone();
    // Backdrop: invisible fullscreen layer that catches a mouse-down anywhere
    // outside the menu and dismisses. Sits behind the menu in z-order.
    let backdrop = div()
        .absolute()
        .inset_0()
        .on_mouse_down(MouseButton::Left, move |_ev, w, cx| {
            dismiss_for_backdrop(w, cx)
        })
        .on_mouse_down(MouseButton::Right, {
            let cb = on_dismiss.clone();
            move |_ev, w, cx| cb(w, cx)
        });

    let mut list = div()
        .min_w(px(180.0))
        .bg(palette::BG_SIDEBAR)
        .border_1()
        .border_color(palette::BORDER)
        .rounded(px(4.0))
        .py(px(4.0))
        .flex()
        .flex_col();

    for (idx, item) in state.items.iter().enumerate() {
        let cb_select = item.on_select.clone();
        let cb_dismiss_after = on_dismiss.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("ctxmenu-item-{idx}")))
                .px(px(10.0))
                .py(px(4.0))
                .text_sm()
                .text_color(palette::TEXT_PRIMARY)
                .cursor_pointer()
                .hover(|s| s.bg(palette::BG_ROW_HOVER))
                .child(item.label.clone())
                .on_mouse_down(MouseButton::Left, move |_ev, w, cx| {
                    cb_select(w, cx);
                    cb_dismiss_after(w, cx);
                }),
        );
    }

    let menu = anchored()
        .position(state.position)
        .snap_to_window_with_margin(Edges {
            top: px(4.0),
            right: px(4.0),
            bottom: px(4.0),
            left: px(4.0),
        })
        .child(list);

    deferred(div().absolute().inset_0().child(backdrop).child(menu))
        .with_priority(1)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_menu_state_clone_preserves_items() {
        let state = ContextMenuState {
            position: Point::new(px(10.0), px(20.0)),
            items: vec![ContextMenuItem {
                label: "Run".into(),
                on_select: Arc::new(|_w, _cx| {}),
            }],
        };
        let cloned = state.clone();
        assert_eq!(cloned.items.len(), 1);
        assert_eq!(cloned.items[0].label.as_ref(), "Run");
        assert_eq!(cloned.position.x, px(10.0));
    }

    #[test]
    fn context_menu_state_with_two_items() {
        let mut item_a_count = 0;
        let mut item_b_count = 0;
        let _ = (&mut item_a_count, &mut item_b_count);
        let state = ContextMenuState {
            position: Point::new(px(50.0), px(60.0)),
            items: vec![
                ContextMenuItem {
                    label: "Run\u{2026}".into(),
                    on_select: Arc::new(|_w, _cx| {}),
                },
                ContextMenuItem {
                    label: "Reveal in Finder".into(),
                    on_select: Arc::new(|_w, _cx| {}),
                },
            ],
        };
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].label.as_ref(), "Run\u{2026}");
        assert_eq!(state.items[1].label.as_ref(), "Reveal in Finder");
    }
}
