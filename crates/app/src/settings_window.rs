//! The Settings window: the market density (saved at once) and the data folder.

use std::path::Path;

use egui::{Context, Window};
use profit_store::Density;

use crate::Shared;

/// Shows the window while `open` is set. Choosing a density saves it at once.
pub fn show(ctx: &Context, open: &mut bool, shared: &mut Shared, data_dir: &Path) {
    Window::new("Settings")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Density");
            let mut density = shared.density;
            for choice in [Density::Sparse, Density::Normal, Density::Dense] {
                let picked = ui
                    .radio_value(&mut density, choice, density_label(choice))
                    .changed();
                if picked && shared.change_settings(|settings| settings.density = choice) {
                    shared.density = choice;
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(format!("Data folder: {}", data_dir.display()));
                if ui.button("Copy path").clicked() {
                    ui.ctx().copy_text(data_dir.display().to_string());
                }
            });
        });
}

fn density_label(density: Density) -> &'static str {
    match density {
        Density::Sparse => "Sparse",
        Density::Normal => "Normal",
        Density::Dense => "Dense",
    }
}
