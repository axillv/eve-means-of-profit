//! The first-run screen: checking, downloading and importing the game data, and the two ways it
//! can stop short (cancelled, or failed).

use egui::{ProgressBar, RichText, Ui};

use crate::theme::Style;

/// Where the start-up is. [`Startup::Ready`] means the pages can be shown.
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
    Ready,
}

/// What the player asked for on the first-run screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Cancel,
    TryAgain,
}

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
            Startup::Ready => {}
        }
    });
    reply
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
