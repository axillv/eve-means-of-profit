//! The status line at the bottom of the window: the activity or idle text, then the notices.
//!
//! Activity (a background update) shows first. Otherwise the first notice shows, with
//! ` (1 of <n>)` when there are several. With no notice, the idle text shows.

use egui::Ui;

use crate::theme::Style;

const IDLE_TEXT: &str = "No market data yet";
/// Longer notices are cut to this many characters; the full text shows on hover.
const CUT_AT: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Notice,
    Error,
}

/// A button on a notice. Pressing it also removes the notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Downloads and imports build `build` in the background.
    Update { build: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
    pub action: Option<Action>,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            level: Level::Notice,
            text: text.into(),
            action: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            level: Level::Error,
            text: text.into(),
            action: None,
        }
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }
}

/// Draws the status line. Returns the action whose button was pressed, if any.
pub fn show(
    ui: &mut Ui,
    style: &Style,
    notices: &mut Vec<Notice>,
    activity: Option<&str>,
) -> Option<Action> {
    let mut action = None;
    let mut dismiss = false;
    egui::Panel::bottom("status").show(ui, |ui| {
        ui.horizontal(|ui| {
            if let Some(text) = activity {
                ui.label(text);
            } else if let Some(first) = notices.first() {
                let (label, color) = match first.level {
                    Level::Notice => ("NOTICE", style.mute),
                    Level::Error => ("ERROR", style.neg),
                };
                ui.colored_label(color, label);
                let count = if notices.len() > 1 {
                    format!(" (1 of {})", notices.len())
                } else {
                    String::new()
                };
                ui.label(format!("{}{count}", cut(&first.text)))
                    .on_hover_text(first.text.as_str());
                if let Some(pending) = first.action
                    && ui.button(button_text(pending)).clicked()
                {
                    action = Some(pending);
                }
                if ui.button("Dismiss").clicked() {
                    dismiss = true;
                }
            } else {
                ui.label(IDLE_TEXT);
            }
        });
    });
    if (dismiss || action.is_some()) && !notices.is_empty() {
        notices.remove(0);
    }
    action
}

fn button_text(action: Action) -> &'static str {
    match action {
        Action::Update { .. } => "Update",
    }
}

/// The text cut to [`CUT_AT`] characters, with `…` when it was cut.
fn cut(text: &str) -> String {
    if text.chars().count() <= CUT_AT {
        return text.to_owned();
    }
    let head: String = text.chars().take(CUT_AT).collect();
    format!("{head}…")
}
