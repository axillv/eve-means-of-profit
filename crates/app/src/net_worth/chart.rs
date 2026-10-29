//! What the Net worth chart shows, without drawing it: the stacked layers of each day, where the
//! line breaks, which days are partial, the axis ticks and the hover text.

use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;

use mop_core::money::Money;
use mop_core::net_worth::{CharacterKey, Part};
use mop_esi::characters::Character;

use crate::build::prices::age_text;
use crate::net_worth::data::ValuedPoint;
use crate::orders::rows::utc_text;

/// Neighbouring points further apart than this many seconds are not joined by the line.
pub const GAP_SECS: i64 = 259_200;
/// A character carried for at least this many seconds is left out of the chart's line.
pub const LEAVE_OUT_AFTER: i64 = 604_800;

/// One layer of the stacked chart: a part of net worth, or the PLEX vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Part(Part),
    Plex,
}

impl Layer {
    /// The layer's name in the hover text and the legend.
    pub fn label(self) -> &'static str {
        match self {
            Self::Part(part) => super::part_label(part),
            Self::Plex => "PLEX vault",
        }
    }
}

/// One day of the chart: its total, the stack tops and whether any member is stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dot {
    pub at: i64,
    pub total: Money,
    pub tops: Vec<(Layer, Money)>,
    pub partial: bool,
}

/// Everything the chart draws, worked out from the valued points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub dots: Vec<Dot>,
    /// Runs of dots joined by the line; a gap starts a new run.
    pub segments: Vec<Range<usize>>,
    pub layers: Vec<Layer>,
    pub top: Money,
    pub first_at: i64,
    pub last_at: i64,
}

/// What could go wrong while working out the chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartError {
    Overflow,
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("net worth is too large to add up"),
        }
    }
}

impl std::error::Error for ChartError {}

/// Works out the chart's dots, segments and layers.
pub fn build(
    points: &[ValuedPoint],
    shown: &BTreeSet<Part>,
    show_plex: bool,
) -> Result<Model, ChartError> {
    let layers = layers(shown, show_plex);
    let mut dots = Vec::with_capacity(points.len());
    let mut top: Option<Money> = None;
    for point in points {
        let mut running = Money::ZERO;
        let mut tops = Vec::with_capacity(layers.len());
        for layer in &layers {
            running = running
                .checked_add(layer_value(*layer, point))
                .ok_or(ChartError::Overflow)?;
            tops.push((*layer, running));
        }
        let total = tops.last().map_or(Money::ZERO, |(_, total)| *total);
        top = Some(top.map_or(total, |best| best.max(total)));
        dots.push(Dot {
            at: point.value.at,
            total,
            tops,
            partial: !point.value.stale.is_empty(),
        });
    }
    let segments = segments(&dots);
    let first_at = dots.first().map_or(0, |dot| dot.at);
    let last_at = dots.last().map_or(0, |dot| dot.at);
    Ok(Model {
        dots,
        segments,
        layers,
        top: top.unwrap_or(Money::ZERO),
        first_at,
        last_at,
    })
}

/// The layers in drawing order: shown parts in their own order, then the vault.
fn layers(shown: &BTreeSet<Part>, show_plex: bool) -> Vec<Layer> {
    let mut layers: Vec<Layer> = Part::ALL
        .into_iter()
        .filter(|part| shown.contains(part))
        .map(Layer::Part)
        .collect();
    if show_plex {
        layers.push(Layer::Plex);
    }
    layers
}

/// One layer's own amount at a point.
fn layer_value(layer: Layer, point: &ValuedPoint) -> Money {
    match layer {
        Layer::Part(part) => point.value.by_part.get(&part).copied().unwrap_or_default(),
        Layer::Plex => point.plex_value,
    }
}

/// Runs of dots whose neighbours are no further apart than [`GAP_SECS`].
fn segments(dots: &[Dot]) -> Vec<Range<usize>> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut previous: Option<i64> = None;
    for (index, dot) in dots.iter().enumerate() {
        if previous.is_some_and(|before| dot.at.saturating_sub(before) > GAP_SECS) {
            segments.push(start..index);
            start = index;
        }
        previous = Some(dot.at);
    }
    if !dots.is_empty() {
        segments.push(start..dots.len());
    }
    segments
}

/// Five axis ticks from zero to `top`, each with its short label.
pub fn axis_ticks(top: Money) -> Vec<(Money, String)> {
    if top.ten_thousandths() == 0 {
        let zero = Money::ZERO.format_isk_short();
        return vec![(Money::ZERO, zero)];
    }
    let scaled = i128::from(top.ten_thousandths());
    (0_i128..=4)
        .map(|step| {
            let rounded = round_div_4(scaled * step);
            // Rounding never leaves `top`'s own range for steps 0 to 4.
            let count = i64::try_from(rounded).unwrap_or(top.ten_thousandths());
            let amount = Money::from_ten_thousandths(count);
            (amount, amount.format_isk_short())
        })
        .collect()
}

/// `value / 4`, rounded to the nearest whole, halves away from zero.
fn round_div_4(value: i128) -> i128 {
    let quotient = value / 4;
    let remainder = value % 4;
    if remainder.unsigned_abs() >= 2 {
        quotient + value.signum()
    } else {
        quotient
    }
}

/// The hover text for one point, as lines joined by `\n`.
pub fn hover_text(
    point: &ValuedPoint,
    shown: &BTreeSet<Part>,
    show_plex: bool,
    basis: &str,
    name: &dyn Fn(&CharacterKey) -> String,
) -> String {
    let mut lines = vec![
        utc_text(point.value.at),
        format!("Total {} ISK", point.total.format_isk()),
    ];
    for layer in layers(shown, show_plex) {
        lines.push(format!(
            "{} {}",
            layer.label(),
            layer_value(layer, point).format_isk()
        ));
    }
    lines.push(basis.to_owned());
    if !point.value.stale.is_empty() {
        let carried: Vec<String> = point
            .value
            .stale
            .iter()
            .map(|(key, age)| format!("{} carried from {} before", name(key), age_text(*age)))
            .collect();
        lines.push(format!("Partial: {}", carried.join(", ")));
    }
    lines.join("\n")
}

/// The stale members carried for at least [`LEAVE_OUT_AFTER`], in key order.
pub fn carried_names(latest: &ValuedPoint) -> Vec<(CharacterKey, i64)> {
    let mut carried: Vec<(CharacterKey, i64)> = latest
        .value
        .stale
        .iter()
        .filter(|(_, age)| *age >= LEAVE_OUT_AFTER)
        .cloned()
        .collect();
    carried.sort();
    carried
}

/// A character key's name, using the login list when the key holds an id.
pub fn character_name(key: &CharacterKey, login: &[Character]) -> String {
    match key {
        CharacterKey::Id(id) => login
            .iter()
            .find(|character| character.id == *id)
            .map_or_else(
                || format!("Character {id}"),
                |character| character.name.clone(),
            ),
        CharacterKey::Name(name) => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mop_core::net_worth::PointValue;
    use mop_esi::characters::Login;

    use super::*;
    use crate::net_worth::data::pricer::PriceSource;

    const A: i64 = 999_395_200;
    const B: i64 = 999_481_600;
    const C: i64 = 1_000_000_000;

    fn money(text: &str) -> Money {
        Money::parse_isk(text).unwrap()
    }

    fn point(
        at: i64,
        parts: &[(Part, &str)],
        plex: &str,
        total: &str,
        stale: Vec<(CharacterKey, i64)>,
    ) -> ValuedPoint {
        let by_part = Part::ALL
            .into_iter()
            .map(|part| {
                let amount = parts
                    .iter()
                    .find(|(shown, _)| *shown == part)
                    .map_or(Money::ZERO, |(_, text)| money(text));
                (part, amount)
            })
            .collect();
        let value = PointValue {
            at,
            by_part,
            by_character: BTreeMap::new(),
            total: money(total),
            unpriced: BTreeMap::new(),
            stale,
        };
        ValuedPoint {
            value,
            plex_units: 0,
            plex_value: money(plex),
            total: money(total),
            unpriced: BTreeMap::new(),
            source: PriceSource::Copy { published_at: None },
            members: BTreeMap::new(),
        }
    }

    fn shown(parts: &[Part]) -> BTreeSet<Part> {
        parts.iter().copied().collect()
    }

    fn abc() -> Vec<ValuedPoint> {
        vec![
            point(
                A,
                &[
                    (Part::Wallet, "1,000,000.00"),
                    (Part::Assets, "9,600,000.00"),
                ],
                "2,600,000,000.00",
                "2,610,600,000.00",
                Vec::new(),
            ),
            point(
                B,
                &[
                    (Part::Wallet, "1,000,000.00"),
                    (Part::Assets, "9,600,000.00"),
                ],
                "2,600,000,000.00",
                "2,610,600,000.00",
                Vec::new(),
            ),
            point(
                C,
                &[
                    (Part::Wallet, "1,234,567.89"),
                    (Part::Assets, "9,792,000.00"),
                ],
                "0",
                "11,026,567.89",
                Vec::new(),
            ),
        ]
    }

    #[test]
    fn each_layer_has_its_label() {
        let labels = [
            "Wallet",
            "Assets",
            "Sell orders",
            "Buy escrow",
            "Industry jobs",
            "Contract collateral",
            "Contracts",
            "Implants",
        ];
        for (part, label) in Part::ALL.into_iter().zip(labels) {
            assert_eq!(Layer::Part(part).label(), label);
        }
        assert_eq!(Layer::Plex.label(), "PLEX vault");
    }

    #[test]
    fn the_stack_breaks_where_the_days_are_far_apart() {
        let model = build(&abc(), &shown(&[Part::Wallet, Part::Assets]), true).unwrap();
        assert_eq!(
            model.layers,
            vec![
                Layer::Part(Part::Wallet),
                Layer::Part(Part::Assets),
                Layer::Plex,
            ]
        );
        assert_eq!(model.top, money("2,610,600,000.00"));
        assert_eq!(model.first_at, A);
        assert_eq!(model.last_at, C);
        assert_eq!(model.segments, vec![0..2, 2..3]);
        let first = model.dots.first().unwrap();
        assert_eq!(
            first.tops,
            vec![
                (Layer::Part(Part::Wallet), money("1,000,000.00")),
                (Layer::Part(Part::Assets), money("10,600,000.00")),
                (Layer::Plex, money("2,610,600,000.00")),
            ]
        );
        assert_eq!(first.total, money("2,610,600,000.00"));
        let last = model.dots.last().unwrap();
        assert_eq!(
            last.tops,
            vec![
                (Layer::Part(Part::Wallet), money("1,234,567.89")),
                (Layer::Part(Part::Assets), money("11,026,567.89")),
                (Layer::Plex, money("11,026,567.89")),
            ]
        );
        assert_eq!(last.total, money("11,026,567.89"));
    }

    #[test]
    fn without_the_vault_there_is_no_plex_layer() {
        let model = build(&abc()[..1], &shown(&[Part::Wallet, Part::Assets]), false).unwrap();
        assert_eq!(
            model.layers,
            vec![Layer::Part(Part::Wallet), Layer::Part(Part::Assets)]
        );
        assert_eq!(model.dots.first().unwrap().total, money("10,600,000.00"));
        assert_eq!(model.top, money("10,600,000.00"));
    }

    #[test]
    fn a_gap_of_exactly_the_limit_joins_and_one_second_more_does_not() {
        let first = point(0, &[(Part::Wallet, "1.00")], "0", "1.00", Vec::new());
        let joined = point(GAP_SECS, &[(Part::Wallet, "2.00")], "0", "2.00", Vec::new());
        let split = point(
            GAP_SECS + 1,
            &[(Part::Wallet, "3.00")],
            "0",
            "3.00",
            Vec::new(),
        );
        let parts = shown(&[Part::Wallet]);
        assert_eq!(
            build(&[first.clone(), joined], &parts, false)
                .unwrap()
                .segments,
            vec![0..2]
        );
        assert_eq!(
            build(&[first, split], &parts, false).unwrap().segments,
            vec![0..1, 1..2]
        );
    }

    #[test]
    fn stale_members_mark_a_day_partial_and_old_ones_are_carried() {
        let partial = point(
            0,
            &[(Part::Wallet, "1.00")],
            "0",
            "1.00",
            vec![(CharacterKey::Id(7), 90_000)],
        );
        assert!(
            build(&[partial], &shown(&[Part::Wallet]), false)
                .unwrap()
                .dots
                .first()
                .unwrap()
                .partial
        );
        let fresh = point(0, &[(Part::Wallet, "1.00")], "0", "1.00", Vec::new());
        assert!(
            !build(&[fresh], &shown(&[Part::Wallet]), false)
                .unwrap()
                .dots
                .first()
                .unwrap()
                .partial
        );

        let latest = point(
            0,
            &[],
            "0",
            "0",
            vec![
                (CharacterKey::Id(1), 604_799),
                (CharacterKey::Id(2), 604_800),
                (CharacterKey::Name("Old".to_owned()), 700_000),
            ],
        );
        assert_eq!(
            carried_names(&latest),
            vec![
                (CharacterKey::Id(2), 604_800),
                (CharacterKey::Name("Old".to_owned()), 700_000),
            ]
        );
    }

    #[test]
    fn a_character_name_comes_from_the_login_or_the_key() {
        let login = vec![Character {
            id: 2_112_000_001,
            name: "Main".to_owned(),
            scopes: BTreeSet::new(),
            added_at: 0,
            login: Login::Ok,
        }];
        assert_eq!(
            character_name(&CharacterKey::Id(2_112_000_001), &login),
            "Main"
        );
        assert_eq!(character_name(&CharacterKey::Id(5), &login), "Character 5");
        assert_eq!(
            character_name(&CharacterKey::Name("TEST-NAME".to_owned()), &login),
            "TEST-NAME"
        );
    }

    #[test]
    fn the_axis_ticks_split_the_top_into_quarters() {
        let ticks = axis_ticks(money("18,308,667.89"));
        let expected = [
            (Money::ZERO, "0.00"),
            (money("4,577,166.9725"), "4.58 M"),
            (money("9,154,333.945"), "9.15 M"),
            (money("13,731,500.9175"), "13.73 M"),
            (money("18,308,667.89"), "18.31 M"),
        ];
        assert_eq!(ticks.len(), expected.len());
        for (tick, (amount, label)) in ticks.iter().zip(expected) {
            assert_eq!(tick.0, amount);
            assert_eq!(tick.1, label);
        }
        assert_eq!(
            axis_ticks(Money::ZERO),
            vec![(Money::ZERO, "0.00".to_owned())]
        );
    }

    fn hover_parts() -> BTreeSet<Part> {
        shown(&[
            Part::Wallet,
            Part::Assets,
            Part::SellOrders,
            Part::BuyEscrow,
            Part::IndustryJobs,
        ])
    }

    fn hover_point(plex: &str, total: &str) -> ValuedPoint {
        point(
            C,
            &[
                (Part::Wallet, "1,234,567.89"),
                (Part::Assets, "9,795,890.00"),
                (Part::SellOrders, "6,120,000.00"),
                (Part::BuyEscrow, "550,100.00"),
                (Part::IndustryJobs, "612,000.00"),
            ],
            plex,
            total,
            vec![(CharacterKey::Id(7), 90_000)],
        )
    }

    #[test]
    fn the_hover_text_lists_the_time_total_parts_basis_and_partial() {
        let named = |_: &CharacterKey| "Alt".to_owned();
        let basis = "Forge sell, 5% price · 9 Sep 01:30";
        let lines = [
            "2001-09-09 01:46",
            "Total 18,312,557.89 ISK",
            "Wallet 1,234,567.89",
            "Assets 9,795,890.00",
            "Sell orders 6,120,000.00",
            "Buy escrow 550,100.00",
            "Industry jobs 612,000.00",
            basis,
            "Partial: Alt carried from 1d before",
        ];
        assert_eq!(
            hover_text(
                &hover_point("0", "18,312,557.89"),
                &hover_parts(),
                false,
                basis,
                &named
            ),
            lines.join("\n")
        );
    }

    #[test]
    fn the_hover_text_adds_the_vault_after_the_parts() {
        let named = |_: &CharacterKey| "Alt".to_owned();
        let basis = "Forge sell, 5% price · 9 Sep 01:30";
        let lines = [
            "2001-09-09 01:46",
            "Total 2,618,312,557.89 ISK",
            "Wallet 1,234,567.89",
            "Assets 9,795,890.00",
            "Sell orders 6,120,000.00",
            "Buy escrow 550,100.00",
            "Industry jobs 612,000.00",
            "PLEX vault 2,600,000,000.00",
            basis,
            "Partial: Alt carried from 1d before",
        ];
        assert_eq!(
            hover_text(
                &hover_point("2,600,000,000.00", "2,618,312,557.89"),
                &hover_parts(),
                true,
                basis,
                &named,
            ),
            lines.join("\n")
        );
    }

    #[test]
    fn adding_two_large_parts_overflows() {
        let huge = Money::from_ten_thousandths(i64::MAX);
        let mut overflowing = point(0, &[], "0", "0", Vec::new());
        overflowing.value.by_part.insert(Part::Wallet, huge);
        overflowing.value.by_part.insert(Part::Assets, huge);
        assert_eq!(
            build(&[overflowing], &shown(&[Part::Wallet, Part::Assets]), false),
            Err(ChartError::Overflow)
        );
    }
}
