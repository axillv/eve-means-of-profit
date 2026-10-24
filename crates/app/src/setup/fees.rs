//! The Fees and skills panel: the skills and standings of the character who sells, which set the
//! sales tax and the broker fee paid at Jita 4-4, and a broker fee typed by hand.

use egui::{ComboBox, Id, RichText, Ui};
use mop_core::fees::{Standing, npc_broker_rate, sales_tax_rate};
use mop_core::money::Rate;
use mop_store::FeeSettings;

use super::fields::{percent, text_field};
use crate::Shared;
use crate::status::Notice;
use crate::theme::Style;

const MUTE_LINE: &str =
    "Type the skills and standings of the character who sells. Fees are for Jita 4-4.";
const MIN_BROKER_FEE: &str = "Minimum 100.00 ISK per order.";
/// The highest level of a fee skill.
const MAX_SKILL_LEVEL: u8 = 5;

/// What the panel keeps between frames: the text of each field, and what the store holds.
#[derive(Debug, Clone, Default)]
struct Panel {
    /// The panel has tried to read the store. A failed read is not tried again each frame.
    tried: bool,
    /// The fields hold what the store holds. Nothing is saved until the read has worked.
    loaded: bool,
    saved: FeeSettings,
    /// The settings the last save failed for, so that its notice is not repeated each frame.
    failed: Option<FeeSettings>,
    accounting: u8,
    broker_relations: u8,
    faction: String,
    corporation: String,
    override_text: String,
}

impl Panel {
    fn from_settings(settings: FeeSettings) -> Self {
        Self {
            tried: true,
            loaded: true,
            saved: settings,
            failed: None,
            accounting: settings.accounting,
            broker_relations: settings.broker_relations,
            faction: standing_text(settings.faction_standing),
            corporation: standing_text(settings.corporation_standing),
            override_text: settings
                .broker_override
                .map(Rate::to_percent_string)
                .unwrap_or_default(),
        }
    }
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup fees panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    if !panel.tried {
        load(&mut panel, cx);
    }
    draw(&mut panel, ui, cx);
    ui.data_mut(|data| data.insert_temp(id, panel));
}

/// Reads the store once. A read that fails shows a notice once, and the fields stay unsaved.
fn load(panel: &mut Panel, cx: &mut Shared) {
    panel.tried = true;
    match cx.store.fee_settings() {
        Ok(settings) => *panel = Panel::from_settings(settings),
        Err(error) => cx
            .notices
            .push(Notice::error(format!("Could not load the fees: {error}"))),
    }
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    let style = cx.style;
    ui.label(RichText::new(MUTE_LINE).color(style.mute));
    skill_field(ui, "Accounting", &mut panel.accounting);
    skill_field(ui, "Broker Relations", &mut panel.broker_relations);
    let faction = standing_input(ui, &style, "Caldari State standing", &mut panel.faction);
    let corporation = standing_input(ui, &style, "Caldari Navy standing", &mut panel.corporation);
    let overridden = percent_input(ui, &style, &mut panel.override_text);

    if let (Ok(faction), Ok(corporation), Ok(broker_override)) =
        (faction.clone(), corporation.clone(), overridden.clone())
    {
        let settings = FeeSettings {
            accounting: panel.accounting,
            broker_relations: panel.broker_relations,
            faction_standing: faction,
            corporation_standing: corporation,
            broker_override,
        };
        if panel.loaded && settings != panel.saved && panel.failed.as_ref() != Some(&settings) {
            save(panel, cx, settings);
        }
    }
    results(ui, &style, panel, &faction, &corporation, &overridden);
}

/// Saves the settings the fields hold. A store error shows as a notice once, and the fields stay as
/// typed; the same settings are not saved again until a field changes.
fn save(panel: &mut Panel, cx: &mut Shared, settings: FeeSettings) {
    match cx.store.save_fee_settings(&settings) {
        Ok(()) => {
            panel.saved = settings;
            panel.failed = None;
        }
        Err(error) => {
            panel.failed = Some(settings);
            cx.notices
                .push(Notice::error(format!("Could not save the fees: {error}")));
        }
    }
}

/// The two result lines: the sales tax from Accounting, and the broker fee from the standings
/// and Broker Relations, or the override typed by hand.
fn results(
    ui: &mut Ui,
    style: &Style,
    panel: &Panel,
    faction: &Result<Standing, String>,
    corporation: &Result<Standing, String>,
    overridden: &Result<Option<Rate>, String>,
) {
    match sales_tax_rate(panel.accounting) {
        Ok(rate) => {
            ui.label(format!("Sales tax: {}", percent_text(rate)));
        }
        Err(error) => {
            ui.colored_label(style.neg, error.to_string());
        }
    }
    // While a field is invalid the broker line keeps the last saved fee, and each field's error shows under that field.
    let valid = match (faction, corporation, overridden) {
        (Ok(faction), Ok(corporation), Ok(over)) => Some((*faction, *corporation, *over)),
        _ => None,
    };
    let (broker_relations, faction_standing, corporation_standing, broker_override) = match valid {
        Some((faction, corporation, over)) => (panel.broker_relations, faction, corporation, over),
        None => (
            panel.saved.broker_relations,
            panel.saved.faction_standing,
            panel.saved.corporation_standing,
            panel.saved.broker_override,
        ),
    };
    let broker = match broker_override {
        Some(rate) => Ok(format!("Broker fee: {} (typed)", percent_text(rate))),
        None => npc_broker_rate(broker_relations, faction_standing, corporation_standing)
            .map(|rate| format!("Broker fee: {}", percent_text(rate))),
    };
    match broker {
        Ok(line) => {
            ui.label(line).on_hover_ui(|ui| {
                ui.label(RichText::new(MIN_BROKER_FEE).color(style.mute));
            });
        }
        Err(error) => {
            ui.colored_label(style.neg, error.to_string());
        }
    }
}

/// A dropdown of a fee skill, from 0 to 5.
fn skill_field(ui: &mut Ui, label: &str, level: &mut u8) {
    ui.horizontal(|ui| {
        ui.label(label);
        ComboBox::from_id_salt(label)
            .selected_text(level.to_string())
            .show_ui(ui, |ui| {
                for choice in 0..=MAX_SKILL_LEVEL {
                    ui.selectable_value(&mut *level, choice, choice.to_string());
                }
            });
    });
}

/// A standing field. Returns the standing it holds, or the error to show under it.
fn standing_input(
    ui: &mut Ui,
    style: &Style,
    label: &str,
    text: &mut String,
) -> Result<Standing, String> {
    text_field(ui, style, label, text, "", None);
    let parsed = Standing::parse(text.trim()).map_err(|error| format!("{label}: {error}"));
    if let Err(error) = &parsed {
        ui.colored_label(style.neg, error.as_str());
    }
    parsed
}

/// The optional broker fee typed by hand: empty is no override.
fn percent_input(ui: &mut Ui, style: &Style, text: &mut String) -> Result<Option<Rate>, String> {
    let label = "Broker fee override %";
    text_field(ui, style, label, text, "Empty: use the skills", None);
    let parsed = percent(label, text);
    if let Err(error) = &parsed {
        ui.colored_label(style.neg, error.as_str());
    }
    parsed
}

/// A percentage as the panels show it: at least 2 and at most 4 decimals, trailing zeros past the
/// second dropped, then `%`. `2,500` ppm is `0.25%`, `13,681` is `1.3681%`.
pub(super) fn percent_text(rate: Rate) -> String {
    let ppm = rate.ppm();
    let whole = ppm / 10_000;
    let fraction = format!("{:04}", ppm % 10_000);
    let kept = fraction.trim_end_matches('0').len().max(2);
    let digits = fraction.get(..kept).unwrap_or(&fraction);
    format!("{whole}.{digits}%")
}

/// A standing as `Standing` writes it: two decimals, `5.23`, `-1.25`, `0.00`.
pub(super) fn standing_text(standing: Standing) -> String {
    let hundredths = standing.hundredths();
    let sign = if hundredths < 0 { "-" } else { "" };
    let magnitude = hundredths.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

#[cfg(test)]
mod tests {
    use mop_core::fees::Standing;
    use mop_core::money::Rate;

    use super::{percent_text, standing_text};

    #[test]
    fn percentages_keep_two_to_four_decimals() {
        assert_eq!(percent_text(Rate::from_ppm(2_500)), "0.25%");
        assert_eq!(percent_text(Rate::from_ppm(10_000)), "1.00%");
        assert_eq!(percent_text(Rate::from_ppm(13_681)), "1.3681%");
        assert_eq!(percent_text(Rate::from_ppm(33_750)), "3.375%");
        assert_eq!(percent_text(Rate::from_ppm(0)), "0.00%");
    }

    #[test]
    fn standings_show_two_decimals() {
        let text = |hundredths| standing_text(Standing::from_hundredths(hundredths).unwrap());
        assert_eq!(text(523), "5.23");
        assert_eq!(text(-125), "-1.25");
        assert_eq!(text(0), "0.00");
    }
}
