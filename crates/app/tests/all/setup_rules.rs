//! Acceptance tests for the Setup page's Value prices tab: the app-wide value price and the price
//! rules for an item, a group and a category. The game data is the build 3607705 fixture, imported
//! in each test; the store is in a temp folder. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use egui::accesskit::Role;
    use egui::{Key, Modifiers, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::rail::Page;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig};
    use mop_core::money::Money;
    use mop_market::book::PriceRule;
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::Store;
    use mop_store::settings::{RuleTarget, RuleValue};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    /// Tritanium is group 18, category 4 in the fixture.
    const TRITANIUM: u32 = 34;

    /// The network is never used: the game data is installed and checked the same day.
    struct NoNetwork;

    impl Fetch for NoNetwork {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Network("no network in tests".to_owned()))
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Network("no network in tests".to_owned()))
        }
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// A data folder with the fixture imported as the installed game data, and an empty store.
    struct Folders {
        _dir: Option<TempDir>,
        data: PathBuf,
        cache: PathBuf,
    }

    impl Folders {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let root = dir.path().to_path_buf();
            Self::at(&root, Some(dir))
        }

        /// A folder with a path that does not change between runs, for the picture: the page shows
        /// the data folder, and a temp name would make the picture differ every time.
        fn fixed() -> Self {
            let root = std::env::temp_dir().join("eve-means-of-profit-value-prices-snapshots");
            drop(fs::remove_dir_all(&root));
            Self::at(&root, None)
        }

        fn at(root: &Path, dir: Option<TempDir>) -> Self {
            let data = root.join("data");
            let cache = root.join("cache");
            fs::create_dir_all(&data).unwrap();
            import(&fixture(), &data.join("sde.sqlite")).unwrap();
            let folders = Self {
                _dir: dir,
                data,
                cache,
            };
            // The daily check is not due: the store says it ran today.
            let mut store = folders.store();
            let mut settings = store.settings().unwrap();
            settings.last_sde_check = Some(NOW);
            store.save_settings(&settings).unwrap();
            // The background price refresh stays off: no test here asks for prices.
            let mut history = store.history_settings().unwrap();
            history.auto_refresh = false;
            store.save_history_settings(&history).unwrap();
            folders
        }

        fn store(&self) -> Store {
            Store::open(&self.data.join("user.sqlite"), NOW)
                .unwrap()
                .store
        }
    }

    /// The window the checks run in: tall, so that a drop-down's list fits below its box. The
    /// pictures are taken at the size the owner asked for, see [`start_at`].
    fn start(folders: &Folders) -> Harness<'static, App> {
        start_at(folders, vec2(1280.0, 1400.0))
    }

    fn start_at(folders: &Folders, size: egui::Vec2) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: folders.data.clone(),
            cache_dir: folders.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(size)
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.run_steps(2);
        click(&mut harness, Page::Setup.label());
        assert!(shown(&harness, "Price rule"), "the setup panels show");
        harness
    }

    /// Whether some node shows `text`.
    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    /// Whether a combo box shows `value` as the text it has selected.
    fn combo_shows(harness: &Harness<'_, App>, value: &str) -> bool {
        fn walk(node: &egui_kittest::Node<'_>, value: &str) -> bool {
            let accesskit = node.accesskit_node();
            (accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value))
                || node.children().any(|child| walk(&child, value))
        }
        walk(&harness.root(), value)
    }

    /// Whether every one of `cells` sits on the row of the first node labelled `anchor`.
    fn row_has(harness: &Harness<'_, App>, anchor: &str, cells: &[&str]) -> bool {
        let Some(row) = harness
            .query_all_by_label(anchor)
            .next()
            .map(|node| node.rect().min.y)
        else {
            return false;
        };
        cells.iter().all(|cell| {
            harness
                .query_all_by_label(cell)
                .any(|node| (node.rect().min.y - row).abs() < 1.0)
        })
    }

    fn click(harness: &mut Harness<'_, App>, label: &str) {
        click_nth(harness, label, 0);
    }

    /// Scrolls the `index`th node with this label into view, then clicks it. The Setup panels scroll,
    /// so a control under the fold is brought up first.
    fn click_nth(harness: &mut Harness<'_, App>, label: &str, index: usize) {
        reveal_nth(harness, label, index);
        {
            let target = harness
                .query_all_by_label(label)
                .nth(index)
                .unwrap_or_else(|| panic!("no node labelled {label} number {index}"));
            target.hover();
        }
        // A pick closes its list for a frame; the press must come after that frame.
        harness.run_steps(2);
        {
            let target = harness
                .query_all_by_label(label)
                .nth(index)
                .unwrap_or_else(|| panic!("no node labelled {label} number {index}"));
            target.click();
        }
        harness.run_steps(3);
    }

    fn reveal_nth(harness: &mut Harness<'_, App>, label: &str, index: usize) {
        {
            let target = harness
                .query_all_by_label(label)
                .nth(index)
                .unwrap_or_else(|| panic!("no node labelled {label} number {index}"));
            target.scroll_to_me();
        }
        harness.run_steps(2);
    }

    /// Types into the text field that has this caption, after clicking it.
    fn type_into(harness: &mut Harness<'_, App>, caption: &str, text: &str) {
        reveal_field(harness, caption);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.hover();
        }
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.click();
        }
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.type_text(text);
        }
        harness.run_steps(3);
    }

    fn reveal_field(harness: &mut Harness<'_, App>, caption: &str) {
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.scroll_to_me();
        }
        harness.run_steps(2);
    }

    /// The text in the text field with this caption; empty when it has none.
    fn field_text(harness: &Harness<'_, App>, caption: &str) -> String {
        harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"))
            .accesskit_node()
            .value()
            .as_deref()
            .unwrap_or_default()
            .to_owned()
    }

    /// Replaces the text of a field: selects all of it, then types the new text.
    fn replace_text(harness: &mut Harness<'_, App>, caption: &str, text: &str) {
        reveal_field(harness, caption);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.click();
        }
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.key_press(Key::Backspace);
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.type_text(text);
        }
        harness.run_steps(3);
    }

    /// Clicks the `index`th combo box that shows `value`, counting from the top of the page. A combo
    /// box has no label of its own, only the value it shows.
    fn click_combo(harness: &mut Harness<'_, App>, value: &str, index: usize) {
        let mut seen = 0;
        let found = reveal_combo_in(&harness.root(), value, index, &mut seen);
        assert!(found, "no combo box shows {value} number {index}");
        harness.run_steps(2);
        // A drop-down that was just closed by a pick is still open for a frame: move onto this one
        // and let that frame pass before the press, or the press only closes the old list.
        let mut seen = 0;
        hover_combo_in(&harness.root(), value, index, &mut seen);
        harness.run_steps(2);
        let mut seen = 0;
        click_combo_in(&harness.root(), value, index, &mut seen);
        harness.run_steps(3);
    }

    fn hover_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.hover();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| hover_combo_in(&child, value, index, seen))
    }

    fn reveal_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.scroll_to_me();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| reveal_combo_in(&child, value, index, seen))
    }

    fn click_combo_in(
        node: &egui_kittest::Node<'_>,
        value: &str,
        index: usize,
        seen: &mut usize,
    ) -> bool {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && accesskit.value().as_deref() == Some(value) {
            if *seen == index {
                node.click();
                return true;
            }
            *seen += 1;
        }
        node.children()
            .any(|child| click_combo_in(&child, value, index, seen))
    }

    /// Presses the point: how a tab, whose title the dock paints without a labelled node, is picked.
    fn press_at(harness: &mut Harness<'_, App>, at: egui::Pos2) {
        harness.event(egui::Event::PointerMoved(at));
        harness.run_steps(2);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run_steps(2);
        }
        harness.run_steps(3);
    }

    /// The point in the tab bar over the Price rule panel, `offset` points after its left edge.
    fn tab_bar_point(harness: &Harness<'_, App>, offset: f32) -> egui::Pos2 {
        let header = harness
            .query_all_by_value("Price rule")
            .next()
            .expect("the Price rule panel shows")
            .rect();
        egui::pos2(header.min.x + offset, header.min.y - 18.0)
    }

    /// Opens the Value prices tab, the second one in the node.
    fn open_value_prices(harness: &mut Harness<'_, App>) {
        press_at(harness, tab_bar_point(harness, 120.0));
        assert!(
            shown(harness, "Value items at"),
            "the Value prices panel shows"
        );
    }

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("a price")
    }

    /// Seeds one rule per target: Tritanium, its group and its category.
    fn seed_three_rules(folders: &Folders) {
        let mut store = folders.store();
        store
            .set_price_rule(
                RuleTarget::Item(TRITANIUM),
                Some(RuleValue::Fixed(isk("4.00"))),
                NOW,
            )
            .unwrap();
        store
            .set_price_rule(
                RuleTarget::Group(18),
                Some(RuleValue::Market(PriceRule::BuyFivePercent)),
                NOW,
            )
            .unwrap();
        store
            .set_price_rule(
                RuleTarget::Category(4),
                Some(RuleValue::Market(PriceRule::SellMedian)),
                NOW,
            )
            .unwrap();
    }

    #[test]
    fn a_fresh_store_shows_the_default_value_price_and_saves_a_pick_for_a_new_app() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_value_prices(&mut harness);
        assert!(shown(&harness, "Value items at"));
        assert!(combo_shows(&harness, "Forge sell, 5% price"));
        assert!(shown(&harness, "No rules yet."));
        assert_eq!(
            folders.store().value_price().unwrap(),
            PriceRule::SellFivePercent
        );

        click_combo(&mut harness, "Forge sell, 5% price", 0);
        click_nth(&mut harness, "Forge buy, highest", 0);
        assert_eq!(
            folders.store().value_price().unwrap(),
            PriceRule::HighestBuy
        );
        drop(harness);

        let mut restarted = start(&folders);
        open_value_prices(&mut restarted);
        assert!(combo_shows(&restarted, "Forge buy, highest"));
        assert!(shown(&restarted, "No rules yet."));
    }

    #[test]
    fn rules_for_an_item_a_group_and_a_category_show_and_survive_a_restart() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_value_prices(&mut harness);

        // Tritanium, by its own name in lower case and at a fixed price.
        type_into(&mut harness, "Applies to", "tritanium");
        click_combo(&mut harness, "Forge sell, 5% price", 1);
        click_nth(&mut harness, "Fixed ISK", 0);
        type_into(&mut harness, "Fixed price (ISK)", "4.00");
        click_nth(&mut harness, "Add rule", 0);
        assert!(
            row_has(&harness, "Tritanium", &["Fixed 4.00 ISK"]),
            "the item row shows its name and fixed price"
        );
        assert_eq!(
            field_text(&harness, "Applies to"),
            "",
            "the name is cleared"
        );
        assert_eq!(
            field_text(&harness, "Fixed price (ISK)"),
            "",
            "the fixed price is cleared"
        );
        assert_eq!(
            folders.store().price_rules_at(NOW).unwrap(),
            BTreeMap::from([(RuleTarget::Item(TRITANIUM), RuleValue::Fixed(isk("4.00")))])
        );

        // The group Tritanium belongs to, priced by the 5% buy rule.
        click_nth(&mut harness, "A group", 0);
        type_into(&mut harness, "Applies to", "mineral");
        click_combo(&mut harness, "Fixed ISK", 0);
        click_nth(&mut harness, "Forge buy, 5% price", 0);
        click_nth(&mut harness, "Add rule", 0);
        assert!(
            row_has(&harness, "Mineral (group)", &["Forge buy, 5% price"]),
            "the group row shows its name and rule"
        );

        // The category Tritanium belongs to, priced by the middle sell rule.
        click_nth(&mut harness, "A category", 0);
        type_into(&mut harness, "Applies to", "material");
        click_combo(&mut harness, "Forge buy, 5% price", 0);
        click_nth(&mut harness, "Forge sell, middle", 0);
        click_nth(&mut harness, "Add rule", 0);
        assert!(
            row_has(&harness, "Material (category)", &["Forge sell, middle"]),
            "the category row shows its name and rule"
        );
        assert_eq!(
            harness.query_all_by_label("Remove").count(),
            3,
            "each rule row has its Remove button"
        );

        assert_eq!(
            folders.store().price_rules_at(NOW).unwrap(),
            BTreeMap::from([
                (RuleTarget::Item(TRITANIUM), RuleValue::Fixed(isk("4.00"))),
                (
                    RuleTarget::Group(18),
                    RuleValue::Market(PriceRule::BuyFivePercent)
                ),
                (
                    RuleTarget::Category(4),
                    RuleValue::Market(PriceRule::SellMedian)
                ),
            ])
        );
        drop(harness);

        let mut restarted = start(&folders);
        open_value_prices(&mut restarted);
        assert!(shown(&restarted, "Tritanium"));
        assert!(shown(&restarted, "Mineral (group)"));
        assert!(shown(&restarted, "Material (category)"));
        assert!(shown(&restarted, "Fixed 4.00 ISK"));
    }

    #[test]
    fn a_missing_item_and_a_bad_price_show_errors_and_store_nothing() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_value_prices(&mut harness);

        type_into(&mut harness, "Applies to", "Tritanum");
        click_nth(&mut harness, "Add rule", 0);
        assert!(shown(&harness, "No item called \"Tritanum\"."));
        assert!(folders.store().price_rules_at(NOW).unwrap().is_empty());
        // The status line shows one notice at a time.
        click_nth(&mut harness, "Dismiss", 0);

        replace_text(&mut harness, "Applies to", "Tritanium");
        click_combo(&mut harness, "Forge sell, 5% price", 1);
        click_nth(&mut harness, "Fixed ISK", 0);
        type_into(&mut harness, "Fixed price (ISK)", "abc");
        click_nth(&mut harness, "Add rule", 0);
        assert!(shown(&harness, "Type a price in ISK, like 29,000,000."));
        assert!(folders.store().price_rules_at(NOW).unwrap().is_empty());
    }

    #[test]
    fn removing_the_tritanium_rule_keeps_the_other_two() {
        let folders = Folders::new();
        seed_three_rules(&folders);
        let mut harness = start(&folders);
        open_value_prices(&mut harness);
        assert!(row_has(&harness, "Tritanium", &["Fixed 4.00 ISK"]));
        assert!(row_has(
            &harness,
            "Mineral (group)",
            &["Forge buy, 5% price"]
        ));
        assert!(row_has(
            &harness,
            "Material (category)",
            &["Forge sell, middle"]
        ));
        assert_eq!(harness.query_all_by_label("Remove").count(), 3);

        // The item row comes first in the table.
        click_nth(&mut harness, "Remove", 0);
        assert!(!shown(&harness, "Tritanium"), "the item row is gone");
        assert_eq!(
            harness.query_all_by_label("Remove").count(),
            2,
            "the other two Remove buttons stay"
        );
        assert!(shown(&harness, "Mineral (group)"));
        assert!(shown(&harness, "Material (category)"));
        let rules = folders.store().price_rules_at(NOW).unwrap();
        assert!(!rules.contains_key(&RuleTarget::Item(TRITANIUM)));
        assert_eq!(
            rules.get(&RuleTarget::Group(18)),
            Some(&RuleValue::Market(PriceRule::BuyFivePercent))
        );
        assert_eq!(
            rules.get(&RuleTarget::Category(4)),
            Some(&RuleValue::Market(PriceRule::SellMedian))
        );
    }

    #[test]
    fn the_value_prices_picture_is_taken_for_the_owner() {
        let folders = Folders::fixed();
        seed_three_rules(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        open_value_prices(&mut harness);
        harness.run_steps(2);
        crate::ui::picture(&mut harness, "setup-value-prices");
    }
}
