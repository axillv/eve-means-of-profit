//! The Today page: the first page after start-up, with two columns of blocks in fixed places.
//! An empty block is one row that says why it is empty and, where there is one, the button that
//! fills it.

use egui::{
    Align, Button, Label, Layout, RichText, TextWrapMode, Ui, WidgetInfo, WidgetType, vec2,
};

use crate::Shared;
use crate::rail::Page;
use crate::theme::Style;

/// The height of one block's row.
const ROW_HEIGHT: f32 = 38.0;
/// The space between two rows of one column.
const ROW_GAP: f32 = 10.0;
/// The width of the right column.
const RIGHT_WIDTH: f32 = 392.0;
/// The space between the two columns.
const COLUMN_GAP: f32 = 12.0;
/// The space under the heading.
const HEADING_GAP: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Jobs,
    Plans,
    Orders,
    Warnings,
    NetWorth,
    Characters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Left,
    Right,
}

impl Block {
    pub const ALL: [Block; 6] = [
        Block::Jobs,
        Block::Plans,
        Block::Orders,
        Block::Warnings,
        Block::NetWorth,
        Block::Characters,
    ];

    /// The column the block sits in: the four to do blocks on the left, the two money blocks on
    /// the right.
    pub fn column(self) -> Column {
        match self {
            Self::Jobs | Self::Plans | Self::Orders | Self::Warnings => Column::Left,
            Self::NetWorth | Self::Characters => Column::Right,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Jobs => "Jobs",
            Self::Plans => "Plans",
            Self::Orders => "Orders",
            Self::Warnings => "Warnings",
            Self::NetWorth => "Net worth",
            Self::Characters => "Characters",
        }
    }

    /// What an empty block says, in one row.
    fn empty_copy(self) -> &'static str {
        match self {
            Self::Jobs => "Jobs to deliver and free slots show here once a character is added.",
            Self::Plans => "No plans yet. Each plan shows its next step here.",
            Self::Orders => "Market orders someone has beaten show here once a character is added.",
            Self::Warnings => "None. Price spikes and thin markets in your plans show here.",
            Self::NetWorth => "Starts with your first character.",
            Self::Characters => "None yet.",
        }
    }

    /// The button that fills an empty block: its visible label and the page it goes to. An empty
    /// warning list is good news, so it has none.
    fn filling(self) -> Option<(&'static str, Page)> {
        match self {
            Self::Jobs | Self::Orders | Self::NetWorth | Self::Characters => {
                Some(("Add character", Page::Characters))
            }
            Self::Plans => Some(("Pick a product", Page::Build)),
            Self::Warnings => None,
        }
    }
}

/// Draws the Today page. Returns the page a block's button asks for, when one was clicked.
pub fn show(ui: &mut Ui, cx: &mut Shared) -> Option<Page> {
    let style = cx.style;
    let no_characters = cx.session.characters().is_empty();
    let no_plans = cx.store.plans().is_ok_and(|plans| plans.is_empty());
    let empty = [
        no_characters,
        no_plans,
        no_characters,
        true,
        no_characters,
        no_characters,
    ];

    ui.heading("Today");
    ui.add_space(HEADING_GAP);

    let left_width = (ui.available_width() - RIGHT_WIDTH - COLUMN_GAP).max(0.0);
    let mut target = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (column, width) in [(Column::Left, left_width), (Column::Right, RIGHT_WIDTH)] {
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (block, empty) in Block::ALL.into_iter().zip(empty) {
                    if block.column() != column {
                        continue;
                    }
                    if let Some(page) = row(ui, &style, block, empty) {
                        target = Some(page);
                    }
                    ui.add_space(ROW_GAP);
                }
            });
            if column == Column::Left {
                ui.add_space(COLUMN_GAP);
            }
        }
    });
    target
}

/// Draws one block's row, and the button that fills it when it is empty. Returns the page the
/// button asks for, when it was clicked.
fn row(ui: &mut Ui, style: &Style, block: Block, empty: bool) -> Option<Page> {
    let width = ui.available_width();
    let mut target = None;
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(block.title()).strong());
            if !empty {
                return;
            }
            ui.add_space(8.0);
            ui.add(
                Label::new(RichText::new(block.empty_copy()).color(style.mute))
                    .wrap_mode(TextWrapMode::Extend),
            );
            let Some((label, page)) = block.filling() else {
                return;
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let response = ui.add(Button::new(label));
                let name = format!("{}: {label}", block.title());
                response.widget_info(|| {
                    WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), name.clone())
                });
                if response.clicked() {
                    target = Some(page);
                }
            });
        },
    );
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blocks_have_fixed_places_and_exact_empty_copy() {
        assert_eq!(
            Block::ALL,
            [
                Block::Jobs,
                Block::Plans,
                Block::Orders,
                Block::Warnings,
                Block::NetWorth,
                Block::Characters,
            ]
        );
        for block in Block::ALL {
            let column = match block {
                Block::Jobs | Block::Plans | Block::Orders | Block::Warnings => Column::Left,
                Block::NetWorth | Block::Characters => Column::Right,
            };
            assert_eq!(block.column(), column, "{block:?}");
        }

        let copy = [
            (
                Block::Jobs,
                "Jobs to deliver and free slots show here once a character is added.",
            ),
            (
                Block::Plans,
                "No plans yet. Each plan shows its next step here.",
            ),
            (
                Block::Orders,
                "Market orders someone has beaten show here once a character is added.",
            ),
            (
                Block::Warnings,
                "None. Price spikes and thin markets in your plans show here.",
            ),
            (Block::NetWorth, "Starts with your first character."),
            (Block::Characters, "None yet."),
        ];
        for (block, text) in copy {
            assert_eq!(block.empty_copy(), text, "{block:?}");
        }

        for (block, label) in [
            (Block::Jobs, "Add character"),
            (Block::Plans, "Pick a product"),
            (Block::Orders, "Add character"),
            (Block::NetWorth, "Add character"),
            (Block::Characters, "Add character"),
        ] {
            assert_eq!(
                block.filling(),
                Some((
                    label,
                    if block == Block::Plans {
                        Page::Build
                    } else {
                        Page::Characters
                    }
                )),
                "{block:?}"
            );
        }
        assert_eq!(Block::Warnings.filling(), None);
    }
}
