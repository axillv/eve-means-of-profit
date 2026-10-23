//! The Characters page: the login characters and jEve's data. The page task fills the panels;
//! [`show`] draws one panel's body.

use egui::Ui;

use crate::Shared;
use crate::dock;

#[derive(Debug, Default)]
pub struct CharactersPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharactersPanel {
    Characters,
    JeveData,
}

impl CharactersPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Characters => "Characters",
            Self::JeveData => "jEve data",
        }
    }
}

/// Draws the body of one panel of this page.
pub fn show(_page: &mut CharactersPage, _panel: CharactersPanel, ui: &mut Ui, cx: &mut Shared) {
    dock::placeholder(ui, cx);
}
