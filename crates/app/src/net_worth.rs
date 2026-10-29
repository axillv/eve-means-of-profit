//! The Net worth page: what the owned items and the wallets are worth, and how that changes. The
//! page task fills the panels; [`show`] draws one panel's body.

use egui::Ui;

use crate::Shared;
use crate::dock;

pub mod data;
pub mod record;
pub mod summary;

#[derive(Debug, Default)]
pub struct NetWorthPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetWorthPanel {
    NetWorth,
    ByCharacter,
    RightNow,
}

impl NetWorthPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::NetWorth => "Net worth",
            Self::ByCharacter => "By character",
            Self::RightNow => "Right now",
        }
    }
}

/// Draws the body of one panel of this page.
pub fn show(_page: &mut NetWorthPage, _panel: NetWorthPanel, ui: &mut Ui, cx: &mut Shared) {
    dock::placeholder(ui, cx);
}
