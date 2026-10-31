//! The first-run screens: checking, downloading and importing the game data, the welcome that
//! offers trying without a login or importing from jEve, and the two ways a start can stop short
//! (cancelled, or failed).

use egui::{ProgressBar, RichText, Ui};

use crate::theme::Style;

/// Where the start-up is. [`Startup::Welcome`] offers the first choice after a first download;
/// [`Startup::Ready`] means the pages can be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    Checking,
    Downloading {
        build: u64,
        done: u64,
        total: Option<u64>,
    },
    Importing {
        build: u64,
    },
    Cancelled,
    Failed(String),
    Welcome,
    Ready,
}

/// What the player asked for on the first-run screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Cancel,
    TryAgain,
    TryWithoutLogin,
    ImportFromJeve,
}

/// The line under a start that is still getting the game data.
const WAITING_HINT: &str =
    "When the game data is ready, try without logging in or import from jEve.";

/// Draws the screen for a start-up that is not ready yet, and reports a button press.
pub fn show(ui: &mut Ui, style: &Style, startup: &Startup) -> Option<Reply> {
    let mut reply = None;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        match startup {
            Startup::Checking => {
                ui.label("Checking for game data…");
            }
            Startup::Downloading { build, done, total } => {
                ui.heading("Getting the game data");
                ui.label(format!("Build {build} · {}", progress_text(*done, *total)));
                ui.add(ProgressBar::new(fraction(*done, *total)).desired_width(320.0));
                if ui.button("Cancel").clicked() {
                    reply = Some(Reply::Cancel);
                }
            }
            Startup::Importing { .. } => {
                ui.label("Importing game data…");
            }
            Startup::Cancelled => {
                ui.label("Download cancelled.");
                if ui.button("Try again").clicked() {
                    reply = Some(Reply::TryAgain);
                }
            }
            Startup::Failed(text) => {
                ui.label(RichText::new(text).color(style.neg));
                if ui.button("Try again").clicked() {
                    reply = Some(Reply::TryAgain);
                }
            }
            Startup::Welcome => {
                ui.heading("Welcome to EVE Means of Profit");
                ui.label("No EVE login is needed.");
                ui.add_space(6.0);
                if ui.button("Try without logging in").clicked() {
                    reply = Some(Reply::TryWithoutLogin);
                }
                ui.label(muted(style, "Product, quantity, paste what you own, buy list."));
                ui.add_space(6.0);
                if ui.button("Import from jEve").clicked() {
                    reply = Some(Reply::ImportFromJeve);
                }
                ui.label(muted(
                    style,
                    "jEve's characters now; import its history on Net worth and its stockpiles on Stockpiles.",
                ));
                ui.add_space(6.0);
                ui.label(
                    "Next: log in on Characters for live data, paste your stock on Build, and pick your facility in Settings.",
                );
            }
            Startup::Ready => {}
        }
        if matches!(
            startup,
            Startup::Checking | Startup::Downloading { .. } | Startup::Importing { .. }
        ) {
            ui.add_space(6.0);
            ui.label(muted(style, WAITING_HINT));
        }
    });
    reply
}

/// Words that explain a button without being part of it.
fn muted(style: &Style, text: &str) -> RichText {
    RichText::new(text).color(style.mute)
}

/// "<done> of <total> MB", or "<done> MB" while the total is not known.
pub fn progress_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!("{} of {} MB", megabytes(done), megabytes(total)),
        None => format!("{} MB", megabytes(done)),
    }
}

/// Bytes as megabytes (1,000,000 bytes), to one decimal, rounded half up.
fn megabytes(bytes: u64) -> String {
    let tenths = (u128::from(bytes) + 50_000) / 100_000;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// How far the download has got, from 0 to 1. Zero while the total is not known.
fn fraction(done: u64, total: Option<u64>) -> f32 {
    let Some(total) = total.filter(|total| *total > 0) else {
        return 0.0;
    };
    let permille =
        u16::try_from(u128::from(done.min(total)) * 1000 / u128::from(total)).unwrap_or(1000);
    f32::from(permille) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_total_gives_no_progress() {
        assert_eq!(fraction(0, Some(0)), 0.0);
        assert_eq!(fraction(5, Some(0)), 0.0);
    }
}
