//! The Settings window: the interface size and the market density (both saved at once), and the
//! data folder.

use std::path::Path;

use egui::{Context, Window};
use mop_store::{Density, InterfaceSize};

use crate::Shared;

/// Shows the window while `open` is set. Choosing an interface size or a density saves it at once.
pub fn show(ctx: &Context, open: &mut bool, shared: &mut Shared, data_dir: &Path) {
    Window::new("Settings")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Interface size");
            let mut size = zoomed_size(ctx.zoom_factor());
            ui.horizontal(|ui| {
                for choice in InterfaceSize::ALL {
                    let picked = ui
                        .radio_value(&mut size, choice, size_label(choice))
                        .changed();
                    if picked {
                        ui.ctx()
                            .set_zoom_factor(f32::from(choice.percent()) / 100.0);
                        shared.change_settings(|settings| settings.interface_size = choice);
                    }
                }
            });
            ui.add_space(8.0);
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

/// The interface size whose zoom is `zoom`: the saved size, which the app starts with and Ctrl +
/// and Ctrl − set.
fn zoomed_size(zoom: f32) -> InterfaceSize {
    InterfaceSize::ALL
        .into_iter()
        .find(|size| (f32::from(size.percent()) / 100.0 - zoom).abs() < 0.001)
        .unwrap_or(InterfaceSize::P100)
}

fn size_label(size: InterfaceSize) -> String {
    format!("{}%", size.percent())
}

fn density_label(density: Density) -> &'static str {
    match density {
        Density::Sparse => "Roomy",
        Density::Normal => "Normal",
        Density::Dense => "Dense",
    }
}
