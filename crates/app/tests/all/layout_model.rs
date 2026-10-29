use egui_dock::{DockState, NodeIndex};
use eve_means_of_profit::layout::dock_convert::{from_dock, to_dock};
use eve_means_of_profit::layout::model::{
    Binding, CheckError, Dir, Follows, Layout, Node, PanelInfo, PanelRef, Slot,
};
use eve_means_of_profit::layout::panels::panels;
use eve_means_of_profit::layout::saved::{load, load_or_set_aside, save, unique_name};
use eve_means_of_profit::layout::text::{
    Mode, Parsed, TextError, copy_text, parse_saved, parse_shared, to_text,
};

#[cfg(test)]
mod tests {
    use super::*;

    const STOCK: &str = "test.stockpiles";
    const INDUSTRY: &str = "test.industry";
    const BUY: &str = "test.buy";
    const STEPS: &str = "test.steps";
    const ITEM: &str = "test.item";

    const TEST_PANELS: &[PanelInfo] = &[
        PanelInfo {
            key: STOCK,
            name: "Stockpiles",
            follows: Follows::Stockpile,
        },
        PanelInfo {
            key: INDUSTRY,
            name: "Industry jobs",
            follows: Follows::Nothing,
        },
        PanelInfo {
            key: BUY,
            name: "Buy list",
            follows: Follows::Plan,
        },
        PanelInfo {
            key: STEPS,
            name: "Steps",
            follows: Follows::Plan,
        },
        PanelInfo {
            key: ITEM,
            name: "Item",
            follows: Follows::Item,
        },
    ];

    const EXAMPLE: &str = concat!(
        "EVE Means of Profit layout 1\n",
        "name: Daily industry\n",
        "split right 60%\n",
        "  Stockpiles\n",
        "  split down 50%\n",
        "    Industry jobs\n",
        "    Buy list\n",
    );

    fn slot(key: &'static str) -> Slot {
        Slot {
            panel: PanelRef::Known(key),
            binding: Binding::Follow,
        }
    }

    fn pinned(key: &'static str, binding: Binding) -> Slot {
        Slot {
            panel: PanelRef::Known(key),
            binding,
        }
    }

    fn panel(key: &'static str) -> Node {
        Node::Panel(slot(key))
    }

    fn split(dir: Dir, percent: u8, first: Node, second: Node) -> Node {
        Node::Split {
            dir,
            percent,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn example() -> Layout {
        Layout {
            name: "Daily industry".to_string(),
            root: Some(split(
                Dir::Right,
                60,
                panel(STOCK),
                split(Dir::Down, 50, panel(INDUSTRY), panel(BUY)),
            )),
        }
    }

    fn named(name: &str, root: Node) -> Layout {
        Layout {
            name: name.to_string(),
            root: Some(root),
        }
    }

    /// The parse error of a text that must not parse.
    fn error_of(text: &str) -> TextError {
        parse_shared(text, TEST_PANELS).unwrap_err()
    }

    #[test]
    fn example_parses_and_writes_back() {
        let parsed = parse_shared(EXAMPLE, TEST_PANELS).expect("the example parses");
        assert_eq!(parsed.layout, example());
        assert!(parsed.unknown.is_empty());
        assert_eq!(to_text(&example(), TEST_PANELS, Mode::Share), EXAMPLE);
    }

    #[test]
    fn copy_text_pasted_in_a_chat_reads_back() {
        let copied = copy_text(&example(), TEST_PANELS);
        assert_eq!(copied, format!("```\n{EXAMPLE}```\n"));
        let pasted = format!("here is mine:\n{copied}\nenjoy");
        let parsed = parse_shared(&pasted, TEST_PANELS).expect("the pasted layout parses");
        assert_eq!(parsed.layout, example());
    }

    #[test]
    fn pins_are_kept_in_saved_text_and_dropped_in_shared_text() {
        let pinned_layout = named(
            "Daily industry",
            split(
                Dir::Right,
                60,
                Node::Panel(pinned(STOCK, Binding::PinStockpile(3))),
                split(
                    Dir::Down,
                    50,
                    panel(INDUSTRY),
                    Node::Panel(pinned(BUY, Binding::PinPlan(12))),
                ),
            ),
        );
        let saved = to_text(&pinned_layout, TEST_PANELS, Mode::Saved);
        assert_eq!(
            saved,
            EXAMPLE
                .replace("  Stockpiles\n", "  Stockpiles [pinned stockpile 3]\n")
                .replace("    Buy list\n", "    Buy list [pinned plan 12]\n")
        );
        assert_eq!(
            parse_saved(&saved, TEST_PANELS)
                .expect("saved text parses")
                .layout,
            pinned_layout
        );
        assert_eq!(to_text(&pinned_layout, TEST_PANELS, Mode::Share), EXAMPLE);
        assert_eq!(
            parse_shared(&saved, TEST_PANELS)
                .expect("saved text reads as shared")
                .layout,
            example()
        );
    }

    #[test]
    fn a_pin_on_a_panel_that_cannot_take_it_is_an_error() {
        let saved = concat!(
            "EVE Means of Profit layout 1\n",
            "name: Daily industry\n",
            "split right 60%\n",
            "  Stockpiles\n",
            "  split down 50%\n",
            "    Industry jobs [pinned plan 4]\n",
            "    Buy list\n",
        );
        let err = parse_saved(saved, TEST_PANELS).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Line 6: \"Industry jobs\" cannot be pinned."
        );
    }

    #[test]
    fn check_refuses_a_stockpile_pin_on_a_plan_panel() {
        let bad = named("Bad", Node::Panel(pinned(BUY, Binding::PinStockpile(3))));
        assert_eq!(
            bad.check(TEST_PANELS),
            Err(CheckError::BadPin {
                panel: "Buy list".to_string()
            })
        );
        assert_eq!(example().check(TEST_PANELS), Ok(()));
    }

    #[test]
    fn tabs_match_names_loosely_and_keep_unknown_panels_as_written() {
        let text = concat!(
            "EVE Means of Profit layout 1\n",
            "name: Tabbed\n",
            "tabs\n",
            "  Steps\n",
            "  Item\n",
        );
        let parsed = parse_shared(text, TEST_PANELS).expect("tabs parse");
        assert_eq!(
            parsed.layout.root,
            Some(Node::Tabs(vec![slot(STEPS), slot(ITEM)]))
        );

        let loose = concat!(
            "EVE Means of Profit layout 1\n",
            "name: Tabbed\n",
            "tabs\n",
            "  buy LIST  \n",
            "  Steps\n",
        );
        let parsed = parse_shared(loose, TEST_PANELS).expect("loose name matches");
        assert_eq!(
            parsed.layout.root,
            Some(Node::Tabs(vec![slot(BUY), slot(STEPS)]))
        );

        let unknown = concat!(
            "EVE Means of Profit layout 1\n",
            "name: Depth\n",
            "tabs\n",
            "  Market depth\n",
            "  Steps\n",
        );
        let parsed = parse_shared(unknown, TEST_PANELS).expect("unknown names parse");
        let market = Slot {
            panel: PanelRef::Unknown("Market depth".to_string()),
            binding: Binding::Follow,
        };
        assert_eq!(
            parsed.layout.root,
            Some(Node::Tabs(vec![market, slot(STEPS)]))
        );
        assert_eq!(parsed.unknown, vec!["Market depth".to_string()]);
        assert_eq!(to_text(&parsed.layout, TEST_PANELS, Mode::Share), unknown);
    }

    #[test]
    fn split_size_one_child_is_an_error_with_its_line() {
        let text = "EVE Means of Profit layout 1\nname: Daily\nsplit right 60%\n  Stockpiles\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 3: \"split right 60%\" needs two parts below it, found 1."
        );
    }

    #[test]
    fn split_size_100_is_an_error_with_its_line() {
        let text = "EVE Means of Profit layout 1\nname: Daily\nsplit down 100%\n  Stockpiles\n  Buy list\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 3: the split size must be 1% to 99%, found 100%."
        );
    }

    #[test]
    fn a_newer_format_is_an_error_on_the_header_line() {
        let text = "EVE Means of Profit layout 2\nname: X\nempty\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 1: this layout was made by a newer version of the app (format 2)."
        );
    }

    #[test]
    fn text_without_a_header_is_an_error_without_a_line() {
        assert_eq!(
            error_of("hello\nworld\n").to_string(),
            "No layout found: the text has no line \"EVE Means of Profit layout 1\"."
        );
    }

    #[test]
    fn sixty_five_panels_is_an_error_without_a_line() {
        let mut text = String::from("EVE Means of Profit layout 1\nname: Many\ntabs\n");
        for number in 1..=65 {
            text.push_str(&format!("  P{number}\n"));
        }
        assert_eq!(
            error_of(&text).to_string(),
            "too many panels: 65 (at most 64)."
        );
    }

    #[test]
    fn a_61_character_name_is_an_error_on_the_name_line() {
        let text = format!(
            "EVE Means of Profit layout 1\nname: {}\nempty\n",
            "a".repeat(61)
        );
        assert_eq!(
            error_of(&text).to_string(),
            "Line 2: the name is longer than 60 characters."
        );
    }

    #[test]
    fn a_missing_name_is_an_error_on_the_name_line() {
        let text = "EVE Means of Profit layout 1\nname:   \nempty\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 2: the layout needs a name."
        );
    }

    #[test]
    fn odd_indent_and_tab_indent_are_errors() {
        let odd = "EVE Means of Profit layout 1\nname: X\ntabs\n   Steps\n  Item\n";
        assert_eq!(
            error_of(odd).to_string(),
            "Line 4: indent with two spaces per level."
        );
        let tab = "EVE Means of Profit layout 1\nname: X\ntabs\n\tSteps\n  Item\n";
        assert_eq!(
            error_of(tab).to_string(),
            "Line 4: indent with two spaces per level."
        );
    }

    #[test]
    fn tabs_with_one_panel_is_an_error_on_the_tabs_line() {
        let text = "EVE Means of Profit layout 1\nname: X\ntabs\n  Steps\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 3: \"tabs\" needs two or more panels below it, found 1."
        );
    }

    #[test]
    fn a_split_inside_tabs_is_an_error_on_its_line() {
        let text = "EVE Means of Profit layout 1\nname: X\ntabs\n  split right 50%\n    Steps\n    Item\n  Item\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 4: only panels can go inside \"tabs\"."
        );
    }

    #[test]
    fn a_line_indented_below_a_panel_is_an_error() {
        let text = "EVE Means of Profit layout 1\nname: X\nsplit right 50%\n    Steps\n  Item\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 4: this line is not below a split or tabs."
        );
    }

    #[test]
    fn a_second_root_is_an_error() {
        let text = "EVE Means of Profit layout 1\nname: X\nSteps\nItem\n";
        assert_eq!(
            error_of(text).to_string(),
            "Line 4: this line is not below a split or tabs."
        );
    }

    #[test]
    fn sixteen_levels_parse_and_seventeen_do_not() {
        // A chain of splits, each with its panel beside the next split, ending in two panels at
        // `leaf_level`.
        fn chain(out: &mut String, level: usize, leaf_level: usize) {
            let indent = "  ".repeat(level);
            if level == leaf_level {
                out.push_str(&format!("{indent}Steps\n"));
                return;
            }
            out.push_str(&format!("{indent}split right 50%\n"));
            chain(out, level + 1, leaf_level);
            out.push_str(&format!("{indent}  Item\n"));
        }

        // Splits at levels 0 to 14, panels at level 15: sixteen levels in all.
        let mut ok = String::from("EVE Means of Profit layout 1\nname: Deep\n");
        chain(&mut ok, 0, 15);
        parse_shared(&ok, TEST_PANELS).expect("sixteen levels parse");

        // One more level: the panels sit at level 16.
        let mut too_deep = String::from("EVE Means of Profit layout 1\nname: Deep\n");
        chain(&mut too_deep, 0, 16);
        assert_eq!(
            error_of(&too_deep).to_string(),
            "Line 19: nested too deep (at most 16 levels)."
        );
    }

    #[test]
    fn empty_layout_parses_and_writes_back_with_crlf_too() {
        let text = "EVE Means of Profit layout 1\nname: Nothing\nempty\n";
        let parsed = parse_shared(text, TEST_PANELS).expect("empty parses");
        assert_eq!(parsed.layout.root, None);
        assert_eq!(to_text(&parsed.layout, TEST_PANELS, Mode::Share), text);

        let crlf = EXAMPLE.replace('\n', "\r\n");
        assert_eq!(
            parse_shared(&crlf, TEST_PANELS)
                .expect("crlf parses")
                .layout,
            example()
        );
    }

    #[test]
    fn saved_file_holds_two_layouts_in_order() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("layouts.txt");
        let first = example();
        let second = Layout {
            name: "Empty one".to_string(),
            root: None,
        };
        save(&path, &[first.clone(), second.clone()], TEST_PANELS).expect("save");

        let written = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            written,
            format!(
                "{}\n{}",
                to_text(&first, TEST_PANELS, Mode::Saved),
                to_text(&second, TEST_PANELS, Mode::Saved)
            )
        );
        let loaded = load(&path, TEST_PANELS).expect("load");
        assert_eq!(loaded.layouts, vec![first, second]);
        assert_eq!(loaded.notice, None);
    }

    #[test]
    fn a_missing_saved_file_has_no_layouts() {
        let dir = tempfile::tempdir().expect("temp dir");
        let loaded = load(&dir.path().join("layouts.txt"), TEST_PANELS).expect("load");
        assert!(loaded.layouts.is_empty());
    }

    #[test]
    fn an_unreadable_saved_file_is_set_aside_with_a_notice() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("layouts.txt");
        std::fs::write(&path, "nonsense").expect("write");

        let loaded = load_or_set_aside(&path, TEST_PANELS, 1000).expect("set aside");
        assert!(loaded.layouts.is_empty());
        assert_eq!(
            loaded.notice.as_deref(),
            Some(
                "Your saved layouts could not be read (No layout found: the text has no line \"EVE Means of Profit layout 1\".). The file was kept as layouts.txt.unreadable-1000."
            )
        );
        assert!(!path.exists());
        let kept = dir.path().join("layouts.txt.unreadable-1000");
        assert_eq!(
            std::fs::read_to_string(kept).expect("kept file"),
            "nonsense"
        );
    }

    #[test]
    fn unique_name_adds_the_first_free_number() {
        assert_eq!(unique_name("Build", &["Build", "Setup"]), "Build (2)");
        assert_eq!(unique_name("daily", &["Daily", "Daily (2)"]), "daily (3)");
        assert_eq!(unique_name("Mine", &["Build"]), "Mine");
    }

    #[test]
    fn dock_round_trip_keeps_the_example() {
        let dock = to_dock(&example());
        assert_eq!(from_dock(&dock, "Daily industry"), example());
    }

    /// A two-panel dock split at `fraction`, read back as a layout.
    fn read_back(fraction: f32) -> Option<Node> {
        let mut dock = DockState::new(vec![slot(STOCK)]);
        dock.main_surface_mut()
            .split_right(NodeIndex::root(), fraction, vec![slot(BUY)]);
        from_dock(&dock, "Split").root
    }

    #[test]
    fn dock_fractions_round_to_whole_percents_within_1_and_99() {
        assert_eq!(
            read_back(0.004),
            Some(split(Dir::Right, 1, panel(STOCK), panel(BUY)))
        );
        assert_eq!(
            read_back(0.996),
            Some(split(Dir::Right, 99, panel(STOCK), panel(BUY)))
        );
        assert_eq!(
            read_back(0.6051),
            Some(split(Dir::Right, 61, panel(STOCK), panel(BUY)))
        );
        assert_eq!(
            read_back(0.6049),
            Some(split(Dir::Right, 60, panel(STOCK), panel(BUY)))
        );
    }

    #[test]
    fn panel_list_is_unique_and_holds_the_shipped_panels() {
        let list = panels();
        let names: Vec<&str> = list.iter().map(|info| info.name).collect();
        assert_eq!(
            names,
            [
                "Plans",
                "Plan",
                "Steps",
                "Buy list",
                "Facilities",
                "Freight services",
                "Price rule",
            ]
        );
        let keys: Vec<&str> = list.iter().map(|info| info.key).collect();
        assert_eq!(
            keys,
            [
                "build.plans",
                "build.plan",
                "build.steps",
                "build.buy_list",
                "setup.facilities",
                "setup.freight_services",
                "setup.price_rule",
            ]
        );
        let mut lowered: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
        lowered.sort();
        lowered.dedup();
        assert_eq!(lowered.len(), names.len());
    }

    #[test]
    fn parsed_reports_unknown_names_once_in_order() {
        let text = "EVE Means of Profit layout 1\nname: X\ntabs\n  Depth\n  Ladder\n  depth\n";
        let Parsed { unknown, .. } = parse_shared(text, TEST_PANELS).expect("parses");
        assert_eq!(unknown, vec!["Depth".to_string(), "Ladder".to_string()]);
    }

    #[test]
    fn a_header_alone_is_an_error_on_the_line_for_the_name() {
        assert_eq!(
            error_of("EVE Means of Profit layout 1\n").to_string(),
            "Line 2: the layout needs a name."
        );
    }

    #[test]
    fn a_name_alone_is_an_error_on_the_line_for_the_node() {
        assert_eq!(
            error_of("EVE Means of Profit layout 1\nname: X\n").to_string(),
            "Line 3: the layout needs a panel, a split or \"empty\" below the name."
        );
    }

    #[test]
    fn a_60_character_name_and_64_panels_are_allowed() {
        let name = "a".repeat(60);
        let mut text = format!("EVE Means of Profit layout 1\nname: {name}\ntabs\n");
        for number in 1..=64 {
            text.push_str(&format!("  P{number}\n"));
        }
        let parsed = parse_shared(&text, TEST_PANELS).expect("limits are inclusive");
        assert_eq!(parsed.layout.name, name);
        assert_eq!(parsed.unknown.len(), 64);
    }

    #[test]
    fn a_saved_path_that_is_not_a_file_is_an_error_not_no_layouts() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("layouts.txt");
        std::fs::create_dir(&path).expect("make a directory in the way");
        load(&path, TEST_PANELS).expect_err("a directory is not a readable file");
    }

    #[test]
    fn split_sizes_with_a_sign_or_leading_zero_are_errors() {
        for size in ["+60%", "060%", "6 0%", "abc%"] {
            let text = format!(
                "EVE Means of Profit layout 1\nname: X\nsplit right {size}\n  Steps\n  Item\n"
            );
            assert_eq!(
                error_of(&text).to_string(),
                format!("Line 3: the split size must be 1% to 99%, found {size}.")
            );
        }
    }

    #[test]
    fn a_split_line_without_a_size_is_an_error() {
        for word in ["right", "down"] {
            let text =
                format!("EVE Means of Profit layout 1\nname: X\nsplit {word}\n  Steps\n  Item\n");
            assert_eq!(
                error_of(&text).to_string(),
                "Line 3: the split size must be 1% to 99%, found nothing."
            );
        }
    }

    #[test]
    fn a_name_with_surrounding_spaces_is_written_trimmed() {
        let layout = Layout {
            name: "  Spaced  ".to_string(),
            root: None,
        };
        assert_eq!(
            to_text(&layout, TEST_PANELS, Mode::Share),
            "EVE Means of Profit layout 1\nname: Spaced\nempty\n"
        );
    }

    #[test]
    fn save_refuses_a_layout_that_would_not_read_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("layouts.txt");
        let bad = named("Bad", Node::Panel(pinned(BUY, Binding::PinStockpile(3))));
        let err = save(&path, &[bad], TEST_PANELS).expect_err("a bad pin is refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!path.exists());
    }

    #[test]
    fn setting_aside_never_overwrites_an_earlier_kept_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("layouts.txt");
        let earlier = dir.path().join("layouts.txt.unreadable-1000");
        std::fs::write(&earlier, "first").expect("write");
        std::fs::write(&path, "nonsense").expect("write");

        let err = load_or_set_aside(&path, TEST_PANELS, 1000).expect_err("refused");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&earlier).expect("read"), "first");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "nonsense");
    }
}
