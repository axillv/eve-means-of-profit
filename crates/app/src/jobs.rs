//! The Industry jobs page: the jobs of the login characters. The page task fills the panel; [`show`]
//! draws its body.

use egui::Ui;

use crate::Shared;
use crate::dock;

#[derive(Debug, Default)]
pub struct JobsPage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobsPanel {
    IndustryJobs,
}

impl JobsPanel {
    pub fn title(self) -> &'static str {
        match self {
            Self::IndustryJobs => "Industry jobs",
        }
    }
}

/// Draws the body of the panel of this page.
pub fn show(_page: &mut JobsPage, _panel: JobsPanel, ui: &mut Ui, cx: &mut Shared) {
    dock::placeholder(ui, cx);
}
