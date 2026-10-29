//! The panels this version knows. Later pages add theirs here.

use crate::layout::model::{Follows, PanelInfo};

const PANELS: &[PanelInfo] = &[
    PanelInfo {
        key: "build.plans",
        name: "Plans",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "build.pasted_stock",
        name: "Pasted stock",
        follows: Follows::Plan,
    },
    PanelInfo {
        key: "build.plan",
        name: "Plan",
        follows: Follows::Plan,
    },
    PanelInfo {
        key: "build.steps",
        name: "Steps",
        follows: Follows::Plan,
    },
    PanelInfo {
        key: "build.buy_list",
        name: "Buy list",
        follows: Follows::Plan,
    },
    PanelInfo {
        key: "setup.facilities",
        name: "Facilities",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.freight_services",
        name: "Freight services",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.price_rule",
        name: "Price rule",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "item",
        name: "Item",
        follows: Follows::Item,
    },
    PanelInfo {
        key: "assets.places",
        name: "Places",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "assets.assets",
        name: "Assets",
        follows: Follows::Place,
    },
    PanelInfo {
        key: "net_worth.net_worth",
        name: "Net worth",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "net_worth.by_character",
        name: "By character",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "net_worth.right_now",
        name: "Right now",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "orders.orders",
        name: "Orders",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "jobs.industry_jobs",
        name: "Industry jobs",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "characters.characters",
        name: "Characters",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "characters.jeve_data",
        name: "jEve data",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.price_history",
        name: "Price history",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.structures",
        name: "Structures",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.fees",
        name: "Fees and skills",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.invention",
        name: "Invention skills",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "setup.display",
        name: "Display",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "stockpiles.stockpiles",
        name: "Stockpiles",
        follows: Follows::Nothing,
    },
    PanelInfo {
        key: "stockpiles.contents",
        name: "Contents",
        follows: Follows::Stockpile,
    },
];

/// The panels of the pages shipped so far.
pub fn panels() -> &'static [PanelInfo] {
    PANELS
}
