//! The `Add to layout…` window: which layout to add a panel to, and whether a plan or stockpile
//! panel follows the selection or pins to the one selected now.

use egui::Context;

use crate::layout::model::{
    Binding, Dir, Follows, Layout, MAX_NAME, Node, PanelInfo, PanelRef, Slot,
};
use crate::layout::saved::unique_name;
use crate::rail::Page;
use crate::status::Notice;

/// Where an outcome adds its panel: a new layout with the name to give it, or an existing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    New(String),
    Existing(usize),
}

/// The answer of the window: where to add the panel, and with which binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub target: Target,
    pub slot: Slot,
}

/// Which layout the window points at while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    New,
    Existing(usize),
}

/// The panel and the choices the open window shows.
#[derive(Debug, Clone)]
struct Open {
    key: &'static str,
    name: String,
    follows: Follows,
    choice: Option<Choice>,
    new_name: String,
    pin: bool,
}

/// The `Add to layout…` window. It stays open across frames and remembers the last layout used.
#[derive(Debug, Default)]
pub struct AddTo {
    open: Option<Open>,
    last: Option<usize>,
}

impl AddTo {
    /// Opens the window for the panel with this name. A name this version does not know is ignored.
    pub fn open(&mut self, panel_name: &str, panels: &[PanelInfo]) {
        let Some(info) = panels.iter().find(|info| info.name == panel_name) else {
            return;
        };
        self.open = Some(Open {
            key: info.key,
            name: info.name.to_owned(),
            follows: info.follows.clone(),
            choice: None,
            new_name: "My layout".to_owned(),
            pin: false,
        });
    }

    /// Draws the window while it is open. Returns the answer when `Add` presses.
    pub fn show(
        &mut self,
        ctx: &Context,
        layouts: &[Layout],
        selected_plan: Option<(i64, &str)>,
        selected_stockpile: Option<(i64, &str)>,
    ) -> Option<Outcome> {
        let last = self.last;
        let open = self.open.as_mut()?;
        let resolved = resolved_choice(open, last, layouts);
        open.choice = Some(resolved.clone());
        let mut choice = resolved;
        let mut outcome = None;
        let mut last_used = None;
        let mut close = false;

        egui::Window::new(format!("Add {} to a layout", open.name))
            .id(egui::Id::new("add to layout window"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                show_layout_choice(ui, layouts, &mut choice, &mut open.new_name);

                let new = matches!(choice, Choice::New);
                let problem = name_problem(&open.new_name, new);
                if let Some(problem) = problem {
                    let color = ui.visuals().error_fg_color;
                    ui.colored_label(color, problem);
                }

                show_follow_choice(ui, open, selected_plan, selected_stockpile);

                let can_add = problem.is_none();
                ui.horizontal(|ui| {
                    if ui.add_enabled(can_add, egui::Button::new("Add")).clicked() {
                        let binding =
                            binding_of(&open.follows, open.pin, selected_plan, selected_stockpile);
                        let slot = Slot {
                            panel: PanelRef::Known(open.key),
                            binding,
                        };
                        let (target, index) = chosen_target(&choice, layouts.len(), &open.new_name);
                        last_used = Some(index);
                        outcome = Some(Outcome { target, slot });
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });

                open.choice = Some(choice.clone());
            });

        if let Some(index) = last_used {
            self.last = Some(index);
        }
        if close {
            self.open = None;
        }
        outcome
    }
}

/// The layout the window points at: the one it already shows, else the last used, else a new one.
fn resolved_choice(open: &Open, last: Option<usize>, layouts: &[Layout]) -> Choice {
    match open.choice.clone() {
        Some(Choice::Existing(index)) if index < layouts.len() => Choice::Existing(index),
        Some(Choice::New) => Choice::New,
        _ => match last.filter(|index| *index < layouts.len()) {
            Some(index) => Choice::Existing(index),
            None => Choice::New,
        },
    }
}

/// The layout list and, for a new layout, its name field.
fn show_layout_choice(
    ui: &mut egui::Ui,
    layouts: &[Layout],
    choice: &mut Choice,
    name: &mut String,
) {
    ui.label("Layout");
    for (index, layout) in layouts.iter().enumerate() {
        ui.radio_value(choice, Choice::Existing(index), layout.name.as_str());
    }
    ui.radio_value(choice, Choice::New, "New layout");
    if matches!(choice, Choice::New) {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(name);
        });
    }
}

/// Why the name field cannot be added, if it cannot.
fn name_problem(name: &str, new: bool) -> Option<&'static str> {
    if !new {
        return None;
    }
    let name = name.trim();
    if name.is_empty() {
        Some("The name cannot be empty.")
    } else if name.chars().count() > MAX_NAME {
        Some("The name is longer than 60 characters.")
    } else {
        None
    }
}

/// The follow or pin choices of a plan or stockpile panel.
fn show_follow_choice(
    ui: &mut egui::Ui,
    open: &mut Open,
    selected_plan: Option<(i64, &str)>,
    selected_stockpile: Option<(i64, &str)>,
) {
    match &open.follows {
        Follows::Plan => {
            ui.radio_value(&mut open.pin, false, "Follow the selected plan");
            match selected_plan {
                Some((_, name)) => {
                    ui.radio_value(&mut open.pin, true, format!("Pin to \"{name}\""));
                }
                None => {
                    open.pin = false;
                    ui.add_enabled(
                        false,
                        egui::RadioButton::new(false, "Pin to the selected plan"),
                    )
                    .on_disabled_hover_text("No plan selected");
                    ui.label("No plan selected");
                }
            }
        }
        Follows::Stockpile => {
            ui.radio_value(&mut open.pin, false, "Follow the selected stockpile");
            match selected_stockpile {
                Some((_, name)) => {
                    ui.radio_value(&mut open.pin, true, format!("Pin to \"{name}\""));
                }
                None => {
                    open.pin = false;
                    ui.add_enabled(
                        false,
                        egui::RadioButton::new(false, "Pin to the selected stockpile"),
                    )
                    .on_disabled_hover_text("No stockpile selected");
                    ui.label("No stockpile selected");
                }
            }
        }
        Follows::Nothing | Follows::Item | Follows::Place | Follows::Character => {}
    }
}

/// The binding the slot takes when `Add` presses.
fn binding_of(
    follows: &Follows,
    pin: bool,
    selected_plan: Option<(i64, &str)>,
    selected_stockpile: Option<(i64, &str)>,
) -> Binding {
    match follows {
        Follows::Plan if pin => selected_plan
            .map(|(id, _)| Binding::PinPlan(id))
            .unwrap_or(Binding::Follow),
        Follows::Stockpile if pin => selected_stockpile
            .map(|(id, _)| Binding::PinStockpile(id))
            .unwrap_or(Binding::Follow),
        Follows::Nothing
        | Follows::Plan
        | Follows::Stockpile
        | Follows::Item
        | Follows::Place
        | Follows::Character => Binding::Follow,
    }
}

/// The target and the index that layout will have once the panel is added.
fn chosen_target(choice: &Choice, count: usize, name: &str) -> (Target, usize) {
    match choice {
        Choice::New => (Target::New(name.trim().to_owned()), count),
        Choice::Existing(index) => (Target::Existing(*index), *index),
    }
}

/// Whether the layout under `root` holds a tab equal to `slot`.
fn holds(root: Option<&Node>, slot: &Slot) -> bool {
    root.is_some_and(|root| root.slots().contains(&slot))
}

/// Adds the outcome's panel to the layout it names. A new layout takes the first free name; an
/// existing layout refuses a tab it already holds. The caller saves the result.
pub fn add(layouts: &mut Vec<Layout>, outcome: Outcome, panel_name: &str) -> (usize, Notice) {
    let Outcome { target, slot } = outcome;
    match target {
        Target::New(wanted) => {
            let mut taken: Vec<&str> = Page::ALL.iter().map(|page| page.label()).collect();
            taken.extend(layouts.iter().map(|layout| layout.name.as_str()));
            let name = unique_name(wanted.trim(), &taken);
            let index = layouts.len();
            layouts.push(Layout {
                name: name.clone(),
                root: Some(Node::Panel(slot)),
            });
            (
                index,
                Notice::info(format!("Added {panel_name} to \"{name}\".")),
            )
        }
        Target::Existing(index) => {
            let Some(layout) = layouts.get_mut(index) else {
                return (index, Notice::error("That layout is no longer there."));
            };
            let name = layout.name.clone();
            if holds(layout.root.as_ref(), &slot) {
                return (
                    index,
                    Notice::error(format!("{panel_name} is already in \"{name}\".")),
                );
            }
            match layout.root.take() {
                None => layout.root = Some(Node::Panel(slot)),
                Some(old) => {
                    layout.root = Some(Node::Split {
                        dir: Dir::Right,
                        percent: 50,
                        first: Box::new(old),
                        second: Box::new(Node::Panel(slot)),
                    });
                }
            }
            (
                index,
                Notice::info(format!("Added {panel_name} to \"{name}\".")),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::panels::panels;
    use crate::layout::text::{self, Mode};
    use egui::accesskit::Toggled;
    use egui::{Key, Modifiers, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    fn slot(key: &'static str, binding: Binding) -> Slot {
        Slot {
            panel: PanelRef::Known(key),
            binding,
        }
    }

    fn buy_list() -> Slot {
        slot("build.buy_list", Binding::Follow)
    }

    fn text_part(layouts: &[Layout], index: usize) -> String {
        let text = text::to_text(&layouts[index], panels(), Mode::Saved);
        text.splitn(3, '\n').nth(2).unwrap_or_default().to_owned()
    }

    #[test]
    fn add_appends_a_new_layout_and_a_split_to_an_existing_one() {
        let mut layouts = Vec::new();
        let (index, notice) = add(
            &mut layouts,
            Outcome {
                target: Target::New("Daily industry".to_owned()),
                slot: buy_list(),
            },
            "Buy list",
        );
        assert_eq!(index, 0);
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].name, "Daily industry");
        assert_eq!(layouts[0].root, Some(Node::Panel(buy_list())));
        assert_eq!(
            notice,
            Notice::info("Added Buy list to \"Daily industry\".")
        );

        let (index, notice) = add(
            &mut layouts,
            Outcome {
                target: Target::Existing(0),
                slot: slot("build.steps", Binding::PinPlan(1)),
            },
            "Steps",
        );
        assert_eq!(index, 0);
        assert_eq!(
            text_part(&layouts, 0),
            "split right 50%\n  Buy list\n  Steps [pinned plan 1]\n"
        );
        assert_eq!(notice, Notice::info("Added Steps to \"Daily industry\"."));

        let mut empty = vec![Layout {
            name: "Empty".to_owned(),
            root: None,
        }];
        add(
            &mut empty,
            Outcome {
                target: Target::Existing(0),
                slot: buy_list(),
            },
            "Buy list",
        );
        assert_eq!(empty[0].root, Some(Node::Panel(buy_list())));
    }

    #[test]
    fn an_existing_layout_refuses_a_panel_it_already_holds() {
        let mut layouts = vec![Layout {
            name: "Daily industry".to_owned(),
            root: Some(Node::Split {
                dir: Dir::Right,
                percent: 50,
                first: Box::new(Node::Panel(buy_list())),
                second: Box::new(Node::Panel(slot("build.steps", Binding::PinPlan(1)))),
            }),
        }];
        let before = text_part(&layouts, 0);
        let (index, notice) = add(
            &mut layouts,
            Outcome {
                target: Target::Existing(0),
                slot: buy_list(),
            },
            "Buy list",
        );
        assert_eq!(index, 0);
        assert_eq!(
            notice,
            Notice::error("Buy list is already in \"Daily industry\".")
        );
        assert_eq!(layouts.len(), 1);
        assert_eq!(text_part(&layouts, 0), before);
        assert_eq!(
            text_part(&layouts, 0),
            "split right 50%\n  Buy list\n  Steps [pinned plan 1]\n"
        );
    }

    #[test]
    fn a_tabs_layout_refuses_a_panel_it_already_holds() {
        let tabs = Node::Tabs(vec![buy_list(), slot("build.steps", Binding::PinPlan(1))]);
        let mut layouts = vec![Layout {
            name: "Ideas".to_owned(),
            root: Some(tabs.clone()),
        }];
        let (index, notice) = add(
            &mut layouts,
            Outcome {
                target: Target::Existing(0),
                slot: buy_list(),
            },
            "Buy list",
        );
        assert_eq!(index, 0);
        assert_eq!(notice, Notice::error("Buy list is already in \"Ideas\"."));
        assert_eq!(layouts[0].root, Some(tabs));
    }

    #[test]
    fn another_binding_is_a_distinct_tab() {
        let mut layouts = vec![Layout {
            name: "Ideas".to_owned(),
            root: Some(Node::Tabs(vec![
                buy_list(),
                slot("build.steps", Binding::PinPlan(1)),
            ])),
        }];
        let (index, notice) = add(
            &mut layouts,
            Outcome {
                target: Target::Existing(0),
                slot: slot("build.buy_list", Binding::PinPlan(1)),
            },
            "Buy list",
        );
        assert_eq!(index, 0);
        assert_eq!(notice, Notice::info("Added Buy list to \"Ideas\"."));
        assert_eq!(
            text_part(&layouts, 0),
            "split right 50%\n  tabs\n    Buy list\n    Steps [pinned plan 1]\n  Buy list [pinned plan 1]\n"
        );
    }

    #[test]
    fn a_new_layout_takes_the_first_free_name() {
        let mut layouts = Vec::new();
        let (_, notice) = add(
            &mut layouts,
            Outcome {
                target: Target::New("  Build  ".to_owned()),
                slot: buy_list(),
            },
            "Buy list",
        );
        assert_eq!(layouts[0].name, "Build (2)");
        assert_eq!(notice, Notice::info("Added Buy list to \"Build (2)\"."));
        assert!(Page::ALL.iter().any(|page| page.label() == "Build"));
    }

    struct Picker {
        add: AddTo,
        layouts: Vec<Layout>,
        selected_plan: Option<(i64, &'static str)>,
        selected_stockpile: Option<(i64, &'static str)>,
        outcome: Option<Outcome>,
    }

    impl Picker {
        fn new(
            layouts: Vec<Layout>,
            selected_plan: Option<(i64, &'static str)>,
            selected_stockpile: Option<(i64, &'static str)>,
        ) -> Harness<'static, Picker> {
            Harness::builder()
                .with_size(vec2(700.0, 600.0))
                .build_ui_state(
                    move |ui, picker: &mut Picker| {
                        if let Some(outcome) = picker.add.show(
                            ui.ctx(),
                            &picker.layouts,
                            picker.selected_plan,
                            picker.selected_stockpile,
                        ) {
                            picker.outcome = Some(outcome);
                        }
                    },
                    Picker {
                        add: AddTo::default(),
                        layouts,
                        selected_plan,
                        selected_stockpile,
                        outcome: None,
                    },
                )
        }
    }

    fn open(harness: &mut Harness<'_, Picker>, name: &str) {
        harness.state_mut().add.open(name, panels());
        harness.run_steps(2);
    }

    fn shown(harness: &Harness<'_, Picker>, label: &str) -> bool {
        harness.query_by_label(label).is_some()
    }

    fn click(harness: &mut Harness<'_, Picker>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    fn type_name(harness: &mut Harness<'_, Picker>, text: &str) {
        harness
            .get_by(|node| node.role() == egui::accesskit::Role::TextInput)
            .click();
        harness.run_steps(1);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(1);
        if text.is_empty() {
            harness.key_press(Key::Backspace);
        } else {
            harness.event(egui::Event::Text(text.to_owned()));
        }
        harness.run_steps(2);
    }

    fn toggled(harness: &Harness<'_, Picker>, label: &str) -> Option<Toggled> {
        harness.get_by_label(label).accesskit_node().toggled()
    }

    #[test]
    fn the_window_defaults_to_a_new_layout_and_follow_the_selected_plan() {
        let mut harness = Picker::new(Vec::new(), Some((1, "Hobgoblin 10")), None);
        open(&mut harness, "Buy list");

        assert!(shown(&harness, "Add Buy list to a layout"));
        assert_eq!(toggled(&harness, "New layout"), Some(Toggled::True));
        assert_eq!(
            harness
                .get_by(|node| node.role() == egui::accesskit::Role::TextInput)
                .accesskit_node()
                .value()
                .as_deref(),
            Some("My layout")
        );
        assert_eq!(
            toggled(&harness, "Follow the selected plan"),
            Some(Toggled::True)
        );

        click(&mut harness, "Add");
        assert_eq!(
            harness.state().outcome,
            Some(Outcome {
                target: Target::New("My layout".to_owned()),
                slot: buy_list(),
            })
        );
        assert!(!shown(&harness, "Add Buy list to a layout"));
    }

    #[test]
    fn the_window_pins_to_the_selected_plan() {
        let mut harness = Picker::new(Vec::new(), Some((1, "Hobgoblin 10")), None);
        open(&mut harness, "Buy list");
        click(&mut harness, "Pin to \"Hobgoblin 10\"");
        click(&mut harness, "Add");

        assert_eq!(
            harness.state().outcome,
            Some(Outcome {
                target: Target::New("My layout".to_owned()),
                slot: slot("build.buy_list", Binding::PinPlan(1)),
            })
        );
    }

    #[test]
    fn an_empty_name_is_refused_and_add_is_disabled() {
        let mut harness = Picker::new(Vec::new(), Some((1, "Hobgoblin 10")), None);
        open(&mut harness, "Buy list");
        type_name(&mut harness, "");

        assert!(shown(&harness, "The name cannot be empty."));
        assert!(
            harness.get_by_label("Add").accesskit_node().is_disabled(),
            "Add cannot take an empty name"
        );
    }

    #[test]
    fn a_name_longer_than_the_limit_is_refused() {
        let mut harness = Picker::new(Vec::new(), Some((1, "Hobgoblin 10")), None);
        open(&mut harness, "Buy list");
        type_name(&mut harness, &"a".repeat(MAX_NAME + 1));

        assert!(shown(
            &harness,
            &format!("The name is longer than {MAX_NAME} characters.")
        ));
        assert!(
            harness.get_by_label("Add").accesskit_node().is_disabled(),
            "Add cannot take an over-long name"
        );
    }

    #[test]
    fn the_pin_choice_is_disabled_without_a_selected_plan() {
        let mut harness = Picker::new(Vec::new(), None, None);
        open(&mut harness, "Buy list");

        assert!(
            harness
                .get_by_label("Pin to the selected plan")
                .accesskit_node()
                .is_disabled(),
            "no plan is selected to pin to"
        );
        assert!(
            shown(&harness, "No plan selected"),
            "the disabled pin choice says why"
        );
    }

    #[test]
    fn a_panel_that_follows_nothing_shows_no_follow_or_pin_choice() {
        let mut harness = Picker::new(Vec::new(), Some((1, "Hobgoblin 10")), None);
        open(&mut harness, "Orders");

        assert!(!shown(&harness, "Follow the selected plan"));
        assert!(!shown(&harness, "Pin to \"Hobgoblin 10\""));
        assert!(!shown(&harness, "Pin to the selected plan"));
        click(&mut harness, "Add");
        assert_eq!(
            harness.state().outcome,
            Some(Outcome {
                target: Target::New("My layout".to_owned()),
                slot: slot("orders.orders", Binding::Follow),
            })
        );
    }

    #[test]
    fn a_stockpile_panel_can_pin_the_selected_stockpile() {
        let mut harness = Picker::new(Vec::new(), None, Some((3, "Kit")));
        open(&mut harness, "Contents");

        assert!(shown(&harness, "Follow the selected stockpile"));
        click(&mut harness, "Pin to \"Kit\"");
        click(&mut harness, "Add");
        assert_eq!(
            harness.state().outcome,
            Some(Outcome {
                target: Target::New("My layout".to_owned()),
                slot: slot("stockpiles.contents", Binding::PinStockpile(3)),
            })
        );
    }

    #[test]
    fn the_window_points_at_the_last_layout_used() {
        let layouts = vec![
            Layout {
                name: "Daily industry".to_owned(),
                root: None,
            },
            Layout {
                name: "Ideas".to_owned(),
                root: None,
            },
        ];
        let mut harness = Picker::new(layouts, Some((1, "Hobgoblin 10")), None);
        open(&mut harness, "Buy list");
        assert_eq!(toggled(&harness, "New layout"), Some(Toggled::True));

        click(&mut harness, "Ideas");
        click(&mut harness, "Add");
        let outcome = harness.state_mut().outcome.take().expect("an outcome");
        assert_eq!(outcome.target, Target::Existing(1));
        add(&mut harness.state_mut().layouts, outcome, "Buy list");

        open(&mut harness, "Steps");
        assert_eq!(toggled(&harness, "Ideas"), Some(Toggled::True));
        assert_eq!(toggled(&harness, "New layout"), Some(Toggled::False));
    }
}
