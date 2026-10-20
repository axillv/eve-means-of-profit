//! The text fields of the Setup forms. Each one is parsed on every change, and its error shows under
//! it; a form's Save stays disabled while any of its fields has one.

use egui::{Response, TextEdit, Ui};
use profit_core::decimal::ParseNumberError;
use profit_core::money::{Money, Rate};
use profit_core::volume::Volume;

use crate::theme::Style;

/// The most jumps a service may be given by hand.
const MAX_JUMPS: u32 = 1_000;

/// The text for a jumps value out of range, built from the limit so the two cannot drift apart.
fn jumps_error() -> String {
    format!(
        "Jumps: must be a whole number from 0 to {}",
        thousands(MAX_JUMPS)
    )
}

/// A whole number with a comma before each group of three digits: `1000` is `1,000`.
fn thousands(number: u32) -> String {
    let digits = number.to_string();
    let first_group = digits.len() % 3;
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && index % 3 == first_group {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Draws a labelled text field, and its error in the negative colour under it when there is one.
pub(super) fn text_field(
    ui: &mut Ui,
    style: &Style,
    label: &str,
    text: &mut String,
    hint: &str,
    error: Option<&str>,
) -> Response {
    let response = ui
        .horizontal(|ui| {
            let caption = ui.label(label);
            ui.add(
                TextEdit::singleline(text)
                    .hint_text(hint)
                    .desired_width(220.0),
            )
            .labelled_by(caption.id)
        })
        .inner;
    if let Some(error) = error {
        ui.colored_label(style.neg, error);
    }
    response
}

/// An optional amount of ISK: empty is `None`.
pub(super) fn isk(label: &str, text: &str) -> Result<Option<Money>, String> {
    optional(label, text, Money::parse_isk)
}

/// An optional percentage such as `0.5`: empty is `None`.
pub(super) fn percent(label: &str, text: &str) -> Result<Option<Rate>, String> {
    optional(label, text, Rate::parse_percent)
}

/// An optional volume in m³, with the grouping of [`Money::parse_isk`] (`360,000.5`): empty is `None`.
pub(super) fn m3(label: &str, text: &str) -> Result<Option<Volume>, String> {
    optional(label, text, |text| Volume::parse_decimal(&ungroup(text)?))
}

/// An optional whole number of jumps from 0 to 1,000: empty is `None`.
pub(super) fn jumps(text: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    // `parse` would take a leading `+`; only plain digits are a whole number here.
    text.bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| text.parse::<u32>().ok())
        .flatten()
        .filter(|jumps| *jumps <= MAX_JUMPS)
        .map(Some)
        .ok_or_else(jumps_error)
}

fn optional<T>(
    label: &str,
    text: &str,
    parse: impl FnOnce(&str) -> Result<T, ParseNumberError>,
) -> Result<Option<T>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    parse(text)
        .map(Some)
        .map_err(|error| format!("{label}: {error}"))
}

/// Checks the thousands grouping of the whole part, then drops the commas.
fn ungroup(text: &str) -> Result<String, ParseNumberError> {
    if !text.contains(',') {
        return Ok(text.to_owned());
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.contains(',') {
        return Err(ParseNumberError::Invalid);
    }
    let mut groups = whole.split(',');
    let first = groups.next().unwrap_or_default();
    let digits = |group: &str| group.bytes().all(|byte| byte.is_ascii_digit());
    let first_ok = (1..=3).contains(&first.len()) && digits(first);
    let rest_ok = groups.all(|group| group.len() == 3 && digits(group));
    if first_ok && rest_ok {
        Ok(text.replace(',', ""))
    } else {
        Err(ParseNumberError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use profit_core::decimal::ParseNumberError;

    use super::ungroup;

    #[test]
    fn a_comma_after_the_decimal_point_is_invalid() {
        assert_eq!(ungroup("1,000.5,5"), Err(ParseNumberError::Invalid));
        assert_eq!(ungroup("1,000.5").unwrap(), "1000.5");
    }

    #[test]
    fn the_jumps_message_names_the_limit_it_checks() {
        assert_eq!(
            super::jumps_error(),
            "Jumps: must be a whole number from 0 to 1,000"
        );
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(10_000), "10,000");
    }
}
