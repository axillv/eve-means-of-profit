//! Turns pasted stock text (inventory copies, multibuy lists, cargo lists) into
//! units per item. Lines that cannot be read are reported by line number.

use std::collections::BTreeMap;
use std::fmt;

use crate::industry::TypeId;

/// Thousands separators accepted in a quantity, besides plain digits.
const SEPARATORS: [char; 5] = [',', '.', ' ', '\u{a0}', '\u{202f}'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pasted {
    pub stock: BTreeMap<TypeId, u64>,
    pub problems: Vec<LineProblem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineProblem {
    /// 1-based, counting empty lines.
    pub line: usize,
    /// The trimmed line.
    pub text: String,
    pub kind: ProblemKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    UnknownItem,
    BadQuantity,
    Overflow,
}

impl fmt::Display for ProblemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::UnknownItem => "unknown item",
            Self::BadQuantity => "quantity not readable",
            Self::Overflow => "total too large",
        };
        f.write_str(text)
    }
}

/// Parses pasted text into stock. `resolve` maps a trimmed name to its type id.
pub fn parse_stock(text: &str, resolve: &dyn Fn(&str) -> Option<TypeId>) -> Pasted {
    let mut pasted = Pasted {
        stock: BTreeMap::new(),
        problems: Vec::new(),
    };
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let number = index + 1;
        match parse_line(line, resolve) {
            Ok((id, qty)) => add(&mut pasted, number, line, id, qty),
            Err(kind) => pasted.problems.push(LineProblem {
                line: number,
                text: line.to_string(),
                kind,
            }),
        }
    }
    pasted
}

fn add(pasted: &mut Pasted, number: usize, line: &str, id: TypeId, qty: u64) {
    let total = pasted.stock.get(&id).copied().unwrap_or(0);
    match total.checked_add(qty) {
        Some(sum) => {
            pasted.stock.insert(id, sum);
        }
        None => pasted.problems.push(LineProblem {
            line: number,
            text: line.to_string(),
            kind: ProblemKind::Overflow,
        }),
    }
}

fn parse_line(
    line: &str,
    resolve: &dyn Fn(&str) -> Option<TypeId>,
) -> Result<(TypeId, u64), ProblemKind> {
    if line.contains('\t') {
        parse_tab_line(line, resolve)
    } else {
        plain_line(line, resolve).ok_or(ProblemKind::UnknownItem)
    }
}

/// Fields split at tabs: name, then quantity (empty or absent means 1), then ignored fields.
fn parse_tab_line(
    line: &str,
    resolve: &dyn Fn(&str) -> Option<TypeId>,
) -> Result<(TypeId, u64), ProblemKind> {
    let mut fields = line.split('\t').map(str::trim);
    let name = fields.next().unwrap_or_default();
    let id = resolve(name).ok_or(ProblemKind::UnknownItem)?;
    let qty = match fields.next() {
        None | Some("") => 1,
        Some(text) => parse_quantity(text).ok_or(ProblemKind::BadQuantity)?,
    };
    Ok((id, qty))
}

/// The first form, in this order, whose name resolves and whose quantity reads wins.
fn plain_line(line: &str, resolve: &dyn Fn(&str) -> Option<TypeId>) -> Option<(TypeId, u64)> {
    let whole_name = resolve(line).map(|id| (id, 1));
    whole_name
        // <name> x<qty> or <name> x <qty>
        .or_else(|| {
            line.rsplit_once(" x")
                .and_then(|(name, qty)| pair(name, qty, resolve))
        })
        // <qty> x <name> or <qty>x <name>
        .or_else(|| {
            line.split_once('x')
                .and_then(|(qty, name)| pair(name, qty, resolve))
        })
        // <name> <qty>
        .or_else(|| {
            line.rsplit_once(' ')
                .and_then(|(name, qty)| pair(name, qty, resolve))
        })
}

fn pair(name: &str, qty: &str, resolve: &dyn Fn(&str) -> Option<TypeId>) -> Option<(TypeId, u64)> {
    Some((resolve(name.trim())?, parse_quantity(qty.trim())?))
}

/// Plain digits, or digits grouped in threes with one separator. Must be at least 1 and fit in `u64`.
fn parse_quantity(text: &str) -> Option<u64> {
    let groups: Vec<&str> = match text.chars().find(|c| !c.is_ascii_digit()) {
        None => vec![text],
        Some(sep) if SEPARATORS.contains(&sep) => {
            let groups: Vec<&str> = text.split(sep).collect();
            let (first, rest) = groups.split_first()?;
            let grouped = (1..=3).contains(&first.len()) && rest.iter().all(|g| g.len() == 3);
            if !grouped {
                return None;
            }
            groups
        }
        Some(_) => return None,
    };
    if !groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let value: u64 = groups.concat().parse().ok()?;
    (value >= 1).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(name: &str) -> Option<TypeId> {
        const NAMES: [(&str, TypeId); 7] = [
            ("Tritanium", 34),
            ("Rifter", 587),
            ("Hobgoblin II", 2456),
            ("Particle Accelerator Unit", 11688),
            ("R.A.M.- Robotics", 11481),
            ("Test Item 50", 900_050),
            ("Test Item", 900_051),
        ];
        NAMES
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, id)| *id)
    }

    fn parse(text: &str) -> Pasted {
        parse_stock(text, &resolver)
    }

    fn stock(entries: &[(TypeId, u64)]) -> BTreeMap<TypeId, u64> {
        entries.iter().copied().collect()
    }

    #[test]
    fn inventory_copy_lines_sum_per_item() {
        let text = "Tritanium\t1,000,000\tMineral\t\t\t10,000 m3\t4,250,000.00 ISK\n\
                    Rifter\t\tFrigate\tShip\t\t27,289 m3\n\
                    Particle Accelerator Unit\t4\tConstruction Components\n\
                    Tritanium\t2 500\tMineral";
        let pasted = parse(text);
        assert_eq!(
            pasted.stock,
            stock(&[(34, 1_002_500), (587, 1), (11688, 4)])
        );
        assert!(pasted.problems.is_empty());
    }

    #[test]
    fn separators_group_thousands() {
        assert_eq!(
            parse("Tritanium\t1.234.567").stock,
            stock(&[(34, 1_234_567)])
        );
        assert_eq!(parse("Tritanium\t1\u{a0}234").stock, stock(&[(34, 1_234)]));
        assert_eq!(
            parse("Tritanium\t1\u{202f}234").stock,
            stock(&[(34, 1_234)])
        );
        assert_eq!(parse("Tritanium\t1234567").stock, stock(&[(34, 1_234_567)]));
    }

    #[test]
    fn bad_quantities_are_reported_on_their_line() {
        for bad in ["1,23", "1,2345", "1,234.567", "0", "-5", "1.5", "abc"] {
            let text = format!("Tritanium\t{bad}");
            let pasted = parse(&text);
            assert!(pasted.stock.is_empty(), "{bad}");
            assert_eq!(
                pasted.problems,
                vec![LineProblem {
                    line: 1,
                    text: text.clone(),
                    kind: ProblemKind::BadQuantity,
                }],
                "{bad}"
            );
        }
    }

    #[test]
    fn other_characters_are_not_separators() {
        for bad in ["1-234", "1/234", "1_234"] {
            let pasted = parse(&format!("Tritanium\t{bad}"));
            assert!(pasted.stock.is_empty(), "{bad}");
            assert_eq!(pasted.problems[0].kind, ProblemKind::BadQuantity, "{bad}");
        }
    }

    #[test]
    fn own_multibuy_copy_ignores_name_case() {
        assert_eq!(parse("Hobgoblin II\t10").stock, stock(&[(2456, 10)]));
        assert_eq!(parse("hobgoblin ii\t10").stock, stock(&[(2456, 10)]));
    }

    #[test]
    fn plain_forms_without_a_tab() {
        assert_eq!(parse("Hobgoblin II x5").stock, stock(&[(2456, 5)]));
        assert_eq!(parse("Hobgoblin II x 5").stock, stock(&[(2456, 5)]));
        assert_eq!(parse("5 x Hobgoblin II").stock, stock(&[(2456, 5)]));
        assert_eq!(parse("5x Hobgoblin II").stock, stock(&[(2456, 5)]));
        assert_eq!(
            parse("R.A.M.- Robotics 1,200").stock,
            stock(&[(11481, 1_200)])
        );
        assert_eq!(parse("Rifter").stock, stock(&[(587, 1)]));
    }

    #[test]
    fn whole_name_is_tried_first() {
        assert_eq!(parse("Test Item 50").stock, stock(&[(900_050, 1)]));
        assert_eq!(parse("Test Item 50 3").stock, stock(&[(900_050, 3)]));
        assert_eq!(parse("Test Item 7").stock, stock(&[(900_051, 7)]));
    }

    #[test]
    fn problems_keep_their_line_numbers() {
        let text = "Tritanium\t100\n\n  \nNo Such Thing\t4\r\nRifter\t2\nfoo bar baz\n";
        let pasted = parse(text);
        assert_eq!(pasted.stock, stock(&[(34, 100), (587, 2)]));
        assert_eq!(
            pasted.problems,
            vec![
                LineProblem {
                    line: 4,
                    text: "No Such Thing\t4".to_string(),
                    kind: ProblemKind::UnknownItem,
                },
                LineProblem {
                    line: 6,
                    text: "foo bar baz".to_string(),
                    kind: ProblemKind::UnknownItem,
                },
            ]
        );
    }

    #[test]
    fn overflow_adds_nothing_and_is_reported() {
        let pasted = parse("Tritanium\t18446744073709551615\nTritanium\t1");
        assert_eq!(pasted.stock, stock(&[(34, u64::MAX)]));
        assert_eq!(
            pasted.problems,
            vec![LineProblem {
                line: 2,
                text: "Tritanium\t1".to_string(),
                kind: ProblemKind::Overflow,
            }]
        );
    }

    #[test]
    fn quantity_beyond_u64_is_bad_quantity() {
        let pasted = parse("Tritanium\t18446744073709551616");
        assert!(pasted.stock.is_empty());
        assert_eq!(pasted.problems[0].kind, ProblemKind::BadQuantity);
    }

    #[test]
    fn empty_and_blank_text_give_nothing() {
        for text in ["", "  \n\t\n \r\n"] {
            let pasted = parse(text);
            assert!(pasted.stock.is_empty());
            assert!(pasted.problems.is_empty());
        }
    }

    #[test]
    fn problem_kinds_display() {
        assert_eq!(ProblemKind::UnknownItem.to_string(), "unknown item");
        assert_eq!(
            ProblemKind::BadQuantity.to_string(),
            "quantity not readable"
        );
        assert_eq!(ProblemKind::Overflow.to_string(), "total too large");
    }
}
