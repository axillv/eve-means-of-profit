//! The Assets page: where the owned items sit, and the item picked. The page task fills the panels;
//! [`show`] draws one panel's body.

use egui::Ui;

use crate::Shared;
use crate::empty::{self, Fill};
use crate::item_panel;
use crate::rail::Page;

mod name_dialog;
mod table;
mod tree;
mod value;
pub mod worth;

#[derive(Debug, Default)]
pub struct AssetsPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetsPanel {
    Places,
    Assets,
    Item,
}

impl AssetsPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Places => "Places",
            Self::Assets => "Assets",
            Self::Item => "Item",
        }
    }
}

/// Draws the body of one panel of this page.
pub fn show(_page: &mut AssetsPage, panel: AssetsPanel, ui: &mut Ui, cx: &mut Shared) {
    if cx.session.loaded()
        && matches!(panel, AssetsPanel::Places | AssetsPanel::Assets)
        && let Some((why, what, fill)) = empty_state(cx)
    {
        empty::show(ui, &cx.style, why, what, fill);
        return;
    }
    match panel {
        AssetsPanel::Item => item_panel::show(ui, cx),
        AssetsPanel::Places => tree::show(ui, cx),
        AssetsPanel::Assets => table::show(ui, cx),
    }
}

/// The empty state both the Places and Assets panels draw, or `None` when they can show their
/// tables. The two panels read the same stock and game data, so they always agree.
fn empty_state(cx: &Shared) -> Option<(&'static str, &'static str, Option<Fill>)> {
    let stock = cx.session.stock_characters();
    if stock.is_empty() {
        if cx.session.characters().is_empty() && cx.session.jeve().is_none() {
            Some((
                "No characters yet.",
                "The app shows what you own, and where it sits, for each character you add.",
                Some(Fill {
                    label: "Add character",
                    page: Some(Page::Characters),
                }),
            ))
        } else {
            Some((
                "No assets yet.",
                "Refresh a character on the Characters page to fetch what you own.",
                Some(Fill {
                    label: "Refresh a character",
                    page: Some(Page::Characters),
                }),
            ))
        }
    } else if cx.sde.is_none() {
        Some((
            "The game data is not in yet.",
            "It downloads at start; the places and items show when it is in.",
            None,
        ))
    } else {
        None
    }
}
