//! The Assets page: where the owned items sit, and the item picked. The page task fills the panels;
//! [`show`] draws one panel's body.

use egui::Ui;

use crate::Shared;
use crate::item_panel;

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
    match panel {
        AssetsPanel::Item => item_panel::show(ui, cx),
        AssetsPanel::Places => tree::show(ui, cx),
        AssetsPanel::Assets => table::show(ui, cx),
    }
}
