//! The Invention skills panel: the skill levels of the character who invents, and how many
//! attempts to buy. Every T2 plan reads both.

use std::collections::BTreeMap;

use egui::{ComboBox, Id, RichText, Ui};
use mop_core::industry::TypeId;
use mop_core::invention::AttemptRule;
use mop_core::money::Rate;
use mop_esi::skills::CharacterSkills;
use mop_sde::invention::InventionSkill;

use crate::Shared;
use crate::status::Notice;
use crate::theme::Style;

const MUTE_LINE: &str = "Skills of the character who invents. A skill not set is assumed 0.";
const NO_DATA: &str = "Load the game data to set invention skills.";
const RULE_HOVER: &str = "Expected buys enough attempts for the copies on average. \"90% sure\" buys enough that 9 times in 10 you get every copy.";

/// What the panel keeps between frames: the store was read once, and the choices it holds.
#[derive(Debug, Clone)]
struct Panel {
    /// The panel has tried to read the store. A failed read is not tried again each frame.
    tried: bool,
    rule: AttemptRule,
    /// The skill levels that were saved, by T2 product id.
    skills: BTreeMap<TypeId, u8>,
    /// The character the read dropdown shows.
    character: Option<u64>,
    /// The read in flight, if one is.
    reading: Option<super::Reading>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            tried: false,
            rule: AttemptRule::Expected,
            skills: BTreeMap::new(),
            character: None,
            reading: None,
        }
    }
}

pub(super) fn show(ui: &mut Ui, cx: &mut Shared) {
    let id = Id::new("setup invention panel");
    let mut panel = ui
        .data(|data| data.get_temp::<Panel>(id))
        .unwrap_or_default();
    if !panel.tried {
        load(&mut panel, cx);
    }
    if let Some(answer) = super::take_read(&mut panel.reading, cx)
        && save_read(&mut panel, cx, &answer.skills)
    {
        cx.notices.push(Notice::info(format!(
            "Read invention skills from {}.",
            answer.name
        )));
    }
    draw(&mut panel, ui, cx);
    ui.data_mut(|data| data.insert_temp(id, panel));
}

/// Sets every invention skill the panel lists to the character's level, 0 when it was not sent,
/// and saves it. One save error stops the rest. Returns whether every skill was saved.
fn save_read(panel: &mut Panel, cx: &mut Shared, read: &CharacterSkills) -> bool {
    let listed = cx.sde.as_ref().map(|sde| {
        sde.invention_skills()
            .map(|skills| {
                skills
                    .into_iter()
                    .map(|skill| skill.type_id)
                    .collect::<Vec<_>>()
            })
            .map_err(|error| error.to_string())
    });
    let listed = match listed {
        Some(Ok(listed)) => listed,
        Some(Err(error)) => {
            cx.notices.push(Notice::error(format!(
                "Could not save the invention skills: {error}"
            )));
            return false;
        }
        None => return false,
    };
    for type_id in listed {
        let level = read.levels.get(&type_id).copied().unwrap_or(0);
        if let Err(error) = cx.store.set_invention_skill(type_id, level) {
            cx.notices.push(Notice::error(format!(
                "Could not save the invention skills: {error}"
            )));
            return false;
        }
        panel.skills.insert(type_id, level);
    }
    true
}

/// Reads the store once. A read that fails shows a notice once; the panel keeps the defaults.
fn load(panel: &mut Panel, cx: &mut Shared) {
    panel.tried = true;
    match cx.store.invention_skills() {
        Ok(skills) => panel.skills = skills,
        Err(error) => cx.notices.push(Notice::error(format!(
            "Could not load the invention skills: {error}"
        ))),
    }
    match cx.store.attempt_rule() {
        Ok(rule) => panel.rule = rule,
        Err(error) => cx.notices.push(Notice::error(format!(
            "Could not load the invention skills: {error}"
        ))),
    }
}

fn draw(panel: &mut Panel, ui: &mut Ui, cx: &mut Shared) {
    let style = cx.style;

    let skills = match cx.sde.as_ref() {
        Some(sde) => match sde.invention_skills() {
            Ok(skills) => skills,
            Err(error) => {
                ui.colored_label(style.neg, error.to_string());
                return;
            }
        },
        None => {
            ui.label(RichText::new(NO_DATA).color(style.mute));
            return;
        }
    };

    super::read_row(
        &mut panel.character,
        &mut panel.reading,
        ui,
        cx,
        "invention read character",
    );
    ui.label(RichText::new(MUTE_LINE).color(style.mute));
    rule_field(ui, &style, panel, cx);

    let (science, encryption) = groups(&skills);
    for (heading, group) in [("Science", science), ("Encryption methods", encryption)] {
        ui.label(RichText::new(heading).strong());
        for skill in group {
            skill_row(ui, panel, cx, skill);
        }
    }
}

/// The attempts dropdown. A change saves at once.
fn rule_field(ui: &mut Ui, style: &Style, panel: &mut Panel, cx: &mut Shared) {
    ui.horizontal(|ui| {
        ui.label("Attempts to buy");
        let mut choice = panel.rule;
        ComboBox::from_id_salt("invention attempts")
            .selected_text(rule_label(choice))
            .show_ui(ui, |ui| {
                for rule in rule_choices() {
                    ui.selectable_value(&mut choice, rule, rule_label(rule));
                }
            })
            .response
            .on_hover_ui(|ui| {
                ui.label(RichText::new(RULE_HOVER).color(style.mute));
            });
        if choice != panel.rule {
            panel.rule = choice;
            if let Err(error) = cx.store.set_attempt_rule(choice) {
                cx.notices.push(Notice::error(format!(
                    "Could not save the invention skills: {error}"
                )));
            }
        }
    });
}

/// One skill row: its name and a dropdown of its level. A change saves at once.
fn skill_row(ui: &mut Ui, panel: &mut Panel, cx: &mut Shared, skill: &InventionSkill) {
    ui.horizontal(|ui| {
        ui.label(skill.name.as_str());
        let before = panel.skills.get(&skill.type_id).copied();
        let mut choice = before;
        ComboBox::from_id_salt(("invention skill", skill.type_id))
            .selected_text(level_label(choice))
            .show_ui(ui, |ui| {
                for level in level_choices() {
                    ui.selectable_value(&mut choice, level, level_label(level));
                }
            });
        if choice == before {
            return;
        }
        let saved = match choice {
            Some(level) => {
                panel.skills.insert(skill.type_id, level);
                cx.store.set_invention_skill(skill.type_id, level)
            }
            None => {
                panel.skills.remove(&skill.type_id);
                cx.store.clear_invention_skill(skill.type_id)
            }
        };
        if let Err(error) = saved {
            cx.notices.push(Notice::error(format!(
                "Could not save the invention skills: {error}"
            )));
        }
    });
}

/// The attempts rule as the dropdown shows it.
fn rule_label(rule: AttemptRule) -> String {
    match rule {
        AttemptRule::Expected => "Expected (average)".to_owned(),
        AttemptRule::Confidence(rate) => format!("{}% sure", rate.ppm() / 10_000),
    }
}

/// The dropdown's choices, in their order: expected, then the three confidences.
fn rule_choices() -> [AttemptRule; 4] {
    let confidence = |ppm| AttemptRule::Confidence(Rate::from_ppm(ppm));
    [
        AttemptRule::Expected,
        confidence(900_000),
        confidence(950_000),
        confidence(990_000),
    ]
}

/// A skill level as the dropdown shows it: not set, or 0 to 5.
fn level_label(level: Option<u8>) -> String {
    match level {
        None => "assumed 0".to_owned(),
        Some(level) => level.to_string(),
    }
}

/// The dropdown's choices, in their order: not set, then 0 to 5.
fn level_choices() -> [Option<u8>; 7] {
    [None, Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
}

/// The skills split into the science skills and the encryption methods, each in the SDE's order.
fn groups(skills: &[InventionSkill]) -> (Vec<&InventionSkill>, Vec<&InventionSkill>) {
    skills.iter().partition(|skill| !skill.encryption)
}

#[cfg(test)]
mod tests {
    use mop_core::invention::AttemptRule;
    use mop_core::money::Rate;
    use mop_sde::invention::InventionSkill;

    use super::super::SetupPanel;
    use super::{groups, level_choices, level_label, rule_choices, rule_label};

    fn confidence(ppm: u32) -> AttemptRule {
        AttemptRule::Confidence(Rate::from_ppm(ppm))
    }

    #[test]
    fn rules_are_labelled_for_the_dropdown() {
        assert_eq!(rule_label(AttemptRule::Expected), "Expected (average)");
        assert_eq!(rule_label(confidence(900_000)), "90% sure");
        assert_eq!(rule_label(confidence(950_000)), "95% sure");
        assert_eq!(rule_label(confidence(990_000)), "99% sure");
        assert_eq!(rule_label(confidence(800_000)), "80% sure");
    }

    #[test]
    fn rule_choices_are_expected_then_the_three_confidences() {
        assert_eq!(
            rule_choices(),
            [
                AttemptRule::Expected,
                confidence(900_000),
                confidence(950_000),
                confidence(990_000),
            ]
        );
        assert_eq!(
            rule_choices().map(rule_label),
            [
                "Expected (average)".to_owned(),
                "90% sure".to_owned(),
                "95% sure".to_owned(),
                "99% sure".to_owned(),
            ]
        );
    }

    #[test]
    fn levels_are_labelled_for_the_dropdown() {
        assert_eq!(level_label(None), "assumed 0");
        assert_eq!(level_label(Some(0)), "0");
        assert_eq!(level_label(Some(5)), "5");
    }

    #[test]
    fn level_choices_start_with_not_set() {
        assert_eq!(
            level_choices(),
            [None, Some(0), Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        assert_eq!(
            level_choices().map(level_label),
            [
                "assumed 0".to_owned(),
                "0".to_owned(),
                "1".to_owned(),
                "2".to_owned(),
                "3".to_owned(),
                "4".to_owned(),
                "5".to_owned(),
            ]
        );
    }

    #[test]
    fn groups_keep_the_sde_order_in_two_lists() {
        let skill = |type_id, name: &str, encryption| InventionSkill {
            type_id,
            name: name.to_owned(),
            encryption,
        };
        let skills = [
            skill(1, "Electronic Engineering", false),
            skill(2, "Amarr Encryption Methods", true),
            skill(3, "Graviton Physics", false),
            skill(4, "Gallente Encryption Methods", true),
            skill(5, "Mechanical Engineering", false),
            skill(6, "Minmatar Encryption Methods", true),
            skill(7, "Minmatar Starship Engineering", false),
        ];
        let (science, encryption) = groups(&skills);
        let names = |group: Vec<&InventionSkill>| -> Vec<String> {
            group.into_iter().map(|skill| skill.name.clone()).collect()
        };
        assert_eq!(
            names(science),
            [
                "Electronic Engineering",
                "Graviton Physics",
                "Mechanical Engineering",
                "Minmatar Starship Engineering",
            ]
        );
        assert_eq!(
            names(encryption),
            [
                "Amarr Encryption Methods",
                "Gallente Encryption Methods",
                "Minmatar Encryption Methods",
            ]
        );
    }

    #[test]
    fn invention_title_is_new_and_the_others_stay() {
        assert_eq!(SetupPanel::Invention.title(), "Invention skills");
        assert_eq!(SetupPanel::Facilities.title(), "Facilities");
        assert_eq!(SetupPanel::FreightServices.title(), "Freight services");
        assert_eq!(SetupPanel::PriceHistory.title(), "Price history");
        assert_eq!(SetupPanel::PriceRule.title(), "Price rule");
        assert_eq!(SetupPanel::Structures.title(), "Structures");
        assert_eq!(SetupPanel::Fees.title(), "Fees and skills");
        assert_eq!(SetupPanel::Display.title(), "Display");
    }
}
