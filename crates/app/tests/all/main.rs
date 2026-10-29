mod assets_table;
mod assets_tree;
mod build;
mod build_stock;
mod buy_list;
mod characters;
mod costs;
mod history;
mod invention;
mod item_panel;
mod jobs;
mod layout_model;
mod names;
mod net_worth;
mod offline;
mod orders;
mod pages;
mod reserve;
mod setup;
mod setup_costs;
mod setup_invention;
mod setup_read;
mod setup_rules;
mod setup_structures;
mod shell;
mod table;
mod ui;

/// Frames [`egui_kittest::Harness::run`] may step to settle. The app's worker thread asks for a
/// repaint as each background job ends, and that request can land on the last of the four frames
/// `run` allows by default; a few more frames let it settle.
pub(crate) const SETTLE_STEPS: u64 = 16;
