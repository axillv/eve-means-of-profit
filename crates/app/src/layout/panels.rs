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
];

/// The panels of the pages shipped so far.
pub fn panels() -> &'static [PanelInfo] {
    PANELS
}
