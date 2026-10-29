//! The Today page: the first page after start-up, with two columns of blocks in fixed places.
//! A block with work to show lists it, oldest first; every other empty block is one row that says
//! why it is empty and, where there is one, the button that fills it.

use egui::{
    Align, Button, Label, Layout, RichText, ScrollArea, Sense, TextWrapMode, Ui, UiBuilder,
    WidgetInfo, WidgetType, vec2,
};

use mop_core::money::Money;
use mop_market::fuzzwork::Side;

use crate::Shared;
use crate::build::prices::age_text;
use crate::jobs::rows::{self as job_rows, Entry as JobEntry};
use crate::orders::rows::{self as order_rows, Entry as OrderEntry, grouped};
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
/// The space between two parts of one row.
const PART_GAP: f32 = 8.0;

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

/// Draws the Today page. Returns the page a block's row or button asks for, when one was clicked.
pub fn show(ui: &mut Ui, cx: &mut Shared) -> Option<Page> {
    let style = cx.style;
    let now = (cx.clock)();
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
    let jobs = if no_characters {
        Vec::new()
    } else {
        job_rows::entries(cx, now)
    };
    let orders = if no_characters {
        Vec::new()
    } else {
        order_rows::entries(cx, now)
    };

    ui.heading("Today");
    ui.add_space(HEADING_GAP);

    let left_width = (ui.available_width() - RIGHT_WIDTH - COLUMN_GAP).max(0.0);
    let mut target = None;
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (column, width) in [(Column::Left, left_width), (Column::Right, RIGHT_WIDTH)] {
                    ui.allocate_ui_with_layout(
                        vec2(width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (block, empty) in Block::ALL.into_iter().zip(empty) {
                                if block.column() != column {
                                    continue;
                                }
                                let page = match block {
                                    Block::Jobs if !no_characters => {
                                        jobs_block(ui, &style, &jobs, now)
                                    }
                                    Block::Orders if !no_characters => {
                                        orders_block(ui, &style, &orders)
                                    }
                                    Block::NetWorth if !no_characters => net_worth_block(ui, cx),
                                    Block::Jobs
                                    | Block::Orders
                                    | Block::Plans
                                    | Block::Warnings
                                    | Block::NetWorth
                                    | Block::Characters => row(ui, &style, block, empty),
                                };
                                if let Some(page) = page {
                                    target = Some(page);
                                }
                                ui.add_space(ROW_GAP);
                            }
                        },
                    );
                    if column == Column::Left {
                        ui.add_space(COLUMN_GAP);
                    }
                }
            });
        });
    target
}

/// Draws a block's title line, with an optional muted count after the title.
fn heading(ui: &mut Ui, style: &Style, block: Block, count: &str) {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.label(RichText::new(block.title()).strong());
            ui.add_space(PART_GAP);
            ui.label(RichText::new(count).color(style.mute));
        },
    );
}

/// Draws the Net worth block for a login: its title, then the latest total and its change.
fn net_worth_block(ui: &mut Ui, cx: &mut Shared) -> Option<Page> {
    let style = cx.style;
    heading(ui, &style, Block::NetWorth, "");
    crate::net_worth::today::body(ui, cx);
    None
}

/// Draws the Jobs block: every job ready to deliver, oldest first. Returns `Page::Jobs` when a row
/// was clicked.
fn jobs_block(ui: &mut Ui, style: &Style, entries: &[JobEntry], now: i64) -> Option<Page> {
    let mut ready: Vec<&JobEntry> = entries.iter().filter(|entry| entry.ready).collect();
    ready.sort_by_key(|entry| entry.end);
    heading(
        ui,
        style,
        Block::Jobs,
        &format!("{} ready to deliver", count(ready.len())),
    );
    let mut target = None;
    for (index, entry) in ready.into_iter().enumerate() {
        if job_row(ui, style, entry, now, index) {
            target = Some(Page::Jobs);
        }
    }
    target
}

/// Draws one job ready to deliver. Returns whether the row was clicked.
fn job_row(ui: &mut Ui, style: &Style, entry: &JobEntry, now: i64, index: usize) -> bool {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    let hit = ui.interact(rect, ui.id().with(("today-job", index)), Sense::click());
    ui.scope_builder(row_layout(rect), |ui| {
        ui.add(Label::new(&entry.character).selectable(false));
        ui.add_space(PART_GAP);
        ui.add(
            Label::new(
                RichText::new(format!(
                    "Deliver {} × {}",
                    grouped(u64::from(entry.runs)),
                    entry.product,
                ))
                .strong(),
            )
            .selectable(false)
            .wrap_mode(TextWrapMode::Extend),
        );
        ui.add_space(PART_GAP);
        ui.add(
            Label::new(
                RichText::new(format!(
                    "{} · ready {}",
                    entry.location,
                    age_text(now.saturating_sub(entry.end)),
                ))
                .color(style.mute),
            )
            .selectable(false)
            .wrap_mode(TextWrapMode::Extend),
        );
    });
    hit.clicked()
}

/// Draws the Orders block: the open orders someone has beaten, the largest gap first. Returns
/// `Page::Orders` when a row was clicked; a click on a Copy button only copies.
fn orders_block(ui: &mut Ui, style: &Style, entries: &[OrderEntry]) -> Option<Page> {
    let open: Vec<&OrderEntry> = entries
        .iter()
        .filter(|entry| entry.expires_in.is_some_and(|seconds| seconds > 0))
        .collect();
    let mut beaten: Vec<&OrderEntry> = open
        .iter()
        .copied()
        .filter(|entry| entry.beaten_by.is_some())
        .collect();
    beaten.sort_by_key(|entry| std::cmp::Reverse(entry.beaten_by));
    heading(
        ui,
        style,
        Block::Orders,
        &format!(
            "{} beaten · {} open",
            count(beaten.len()),
            count(open.len()),
        ),
    );
    let mut target = None;
    for (index, entry) in beaten.into_iter().enumerate() {
        if order_row(ui, style, entry, index) {
            target = Some(Page::Orders);
        }
    }
    target
}

/// Draws one beaten order, with the price that beats it. Returns whether the row (not its button)
/// was clicked.
fn order_row(ui: &mut Ui, style: &Style, entry: &OrderEntry, index: usize) -> bool {
    let Some(best) = entry.forge_best else {
        return false;
    };
    let side = if entry.sell { Side::Sell } else { Side::Buy };
    let undercut = undercut(best, side);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    let hit = ui.interact(rect, ui.id().with(("today-order", index)), Sense::click());
    let mut copied = false;
    ui.scope_builder(row_layout(rect), |ui| {
        ui.add(Label::new(&entry.character).selectable(false));
        ui.add_space(PART_GAP);
        let side = match side {
            Side::Sell => "Sell",
            Side::Buy => "Buy",
        };
        ui.add(
            Label::new(RichText::new(format!("{side} {} beaten", entry.item)).strong())
                .selectable(false)
                .wrap_mode(TextWrapMode::Extend),
        );
        ui.add_space(PART_GAP);
        ui.add(
            Label::new(
                RichText::new(format!(
                    "{}, best {}",
                    entry.price.format_isk(),
                    best.format_isk(),
                ))
                .color(style.mute),
            )
            .selectable(false)
            .wrap_mode(TextWrapMode::Extend),
        );
        let Some(undercut) = undercut else {
            return;
        };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = format!("Copy {}", undercut.format_isk());
            let response = ui.add(Button::new(&label));
            let name = format!("{}: {label}", Block::Orders.title());
            response.widget_info(|| {
                WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), name.clone())
            });
            if response.clicked() {
                ui.ctx().copy_text(plain_isk(undercut));
                copied = true;
            }
        });
    });
    if copied { false } else { hit.clicked() }
}

/// The layout of one block's row: a fixed height and the page's current width.
fn row_layout(rect: egui::Rect) -> UiBuilder {
    UiBuilder::new()
        .max_rect(rect)
        .layout(Layout::left_to_right(Align::Center))
}

/// A count with thousands grouped, such as `1,234`.
fn count(value: usize) -> String {
    grouped(u64::try_from(value).unwrap_or(u64::MAX))
}

/// The price to type into EVE to beat `best` on `side`, to four significant digits: sells round
/// down, buys round up, and a price that lands exactly on `best` moves one step further.
fn undercut(best: Money, side: Side) -> Option<Money> {
    let units = best.ten_thousandths();
    if units <= 0 {
        return None;
    }
    let digits = units.to_string().len();
    let step = if units >= 10_000 {
        let exponent = u32::try_from(digits.checked_sub(4)?).ok()?;
        10i64.checked_pow(exponent)?
    } else {
        1
    };
    let mut cut = match side {
        Side::Sell => units.checked_div(step)?.checked_mul(step)?,
        Side::Buy => {
            let below = units.checked_div(step)?.checked_mul(step)?;
            if below == units {
                units
            } else {
                below.checked_add(step)?
            }
        }
    };
    if cut == units {
        cut = match side {
            Side::Sell => cut.checked_sub(step)?,
            Side::Buy => cut.checked_add(step)?,
        };
    }
    if cut <= 0 {
        return None;
    }
    Some(Money::from_ten_thousandths(cut))
}

/// An amount as the digits EVE's price field takes: no thousands separators, two decimals.
fn plain_isk(amount: Money) -> String {
    amount.format_isk().replace(',', "")
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
            ui.add_space(PART_GAP);
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

    #[test]
    fn the_undercut_is_four_significant_digits_under_the_best() {
        let isk = |text: &str| Money::parse_isk(text).expect("a price");

        for (best, wanted) in [
            ("799,490.55", "799,400.00"),
            ("328,979.79", "328,900.00"),
            ("61,200.00", "61,190.00"),
            ("431.60", "431.50"),
        ] {
            assert_eq!(
                undercut(isk(best), Side::Sell),
                Some(isk(wanted)),
                "sell {best}"
            );
        }
        for (best, wanted) in [("431.60", "431.70"), ("2,150.00", "2,151.00")] {
            assert_eq!(
                undercut(isk(best), Side::Buy),
                Some(isk(wanted)),
                "buy {best}"
            );
        }

        assert_eq!(undercut(Money::ZERO, Side::Sell), None);
        assert_eq!(undercut(Money::ZERO, Side::Buy), None);
        // One step lower would be zero.
        assert_eq!(undercut(isk("0.0001"), Side::Sell), None);
        // Rounding up the largest amount overflows.
        assert_eq!(
            undercut(Money::from_ten_thousandths(i64::MAX), Side::Buy),
            None
        );
    }

    #[test]
    fn a_price_for_eve_has_no_separators_and_two_decimals() {
        let isk = |text: &str| Money::parse_isk(text).expect("a price");
        assert_eq!(plain_isk(isk("61,190.00")), "61190.00");
        assert_eq!(plain_isk(isk("2,151.00")), "2151.00");
    }
}
