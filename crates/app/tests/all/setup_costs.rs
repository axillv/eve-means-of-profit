//! Acceptance tests for the Setup page's Fees and skills panel and the system and tax of a facility.
//! The game data is the build 3607705 fixture, imported in each test; the store is in a temp folder.
//! The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

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
    use mop_core::fees::Standing;
    use mop_core::industry::TypeId;
    use mop_core::money::Rate;
    use mop_sde::{Fetch, FetchError, Security, import};
    use mop_store::{FeeSettings, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    /// The Raitaru, an engineering complex, and the four rigs that fit it for manufacturing.
    const RAITARU: TypeId = 35_825;

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
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    impl Folders {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let data = dir.path().join("data");
            let cache = dir.path().join("cache");
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

    /// How many nodes labelled `label` sit on the row of `anchor`.
    fn count_on_row(harness: &Harness<'_, App>, anchor: &str, label: &str) -> usize {
        let row = harness
            .get_all_by_label(anchor)
            .next()
            .expect("the row")
            .rect()
            .min
            .y;
        harness
            .query_all_by_label(label)
            .filter(|node| (node.rect().min.y - row).abs() < 1.0)
            .count()
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

    /// Whether the first node with this label is disabled.
    fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .accesskit_node()
            .is_disabled()
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

    /// Opens the Fees and skills tab. A tab's title is painted by the dock, not a labelled node, so the
    /// tab is pressed where it is drawn: in the tab bar over the Structures panel, after its title.
    fn open_fees(harness: &mut Harness<'_, App>) {
        let header = harness
            .query_all_by_value("Structures")
            .next()
            .expect("the Structures panel shows")
            .rect();
        let at = egui::pos2(header.min.x + 120.0, header.min.y - 18.0);
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

    fn standing(hundredths: i16) -> Standing {
        Standing::from_hundredths(hundredths).unwrap()
    }

    #[test]
    fn a_fresh_store_shows_the_default_sales_tax_and_broker_fee() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_fees(&mut harness);
        assert!(shown(&harness, "Sales tax: 7.50%"));
        assert!(shown(&harness, "Broker fee: 3.00%"));
    }

    #[test]
    fn typed_fees_are_saved_and_shown_by_a_new_app() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_fees(&mut harness);
        click_combo(&mut harness, "0", 0);
        click_nth(&mut harness, "4", 0);
        click_combo(&mut harness, "0", 0);
        click_nth(&mut harness, "5", 0);
        replace_text(&mut harness, "Caldari State standing", "5.23");
        replace_text(&mut harness, "Caldari Navy standing", "-1.25");
        assert!(shown(&harness, "Sales tax: 4.20%"));
        assert!(shown(&harness, "Broker fee: 1.3681%"));
        let stored = folders.store().fee_settings().unwrap();
        assert_eq!(
            stored,
            FeeSettings {
                accounting: 4,
                broker_relations: 5,
                faction_standing: standing(523),
                corporation_standing: standing(-125),
                broker_override: None,
            }
        );

        let mut restarted = start(&folders);
        open_fees(&mut restarted);
        assert!(shown(&restarted, "Sales tax: 4.20%"));
        assert!(shown(&restarted, "Broker fee: 1.3681%"));
    }

    #[test]
    fn a_typed_broker_fee_overrides_the_skills_until_cleared() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_fees(&mut harness);
        replace_text(&mut harness, "Broker fee override %", "1.5");
        assert!(shown(&harness, "Broker fee: 1.50% (typed)"));
        assert_eq!(
            folders.store().fee_settings().unwrap().broker_override,
            Some(Rate::from_ppm(15_000))
        );
        replace_text(&mut harness, "Broker fee override %", "");
        assert!(shown(&harness, "Broker fee: 3.00%"));
        assert_eq!(
            folders.store().fee_settings().unwrap().broker_override,
            None
        );
    }

    #[test]
    fn a_standing_out_of_range_saves_nothing_until_it_is_fixed() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_fees(&mut harness);
        replace_text(&mut harness, "Caldari State standing", "10.5");
        assert!(shown(
            &harness,
            "Caldari State standing: standing must be between -10.00 and 10.00"
        ));
        assert_eq!(
            folders.store().fee_settings().unwrap().faction_standing,
            standing(0)
        );
        replace_text(&mut harness, "Caldari State standing", "11");
        assert!(shown(
            &harness,
            "Caldari State standing: standing must be between -10.00 and 10.00"
        ));
        assert_eq!(
            folders.store().fee_settings().unwrap().faction_standing,
            standing(0)
        );
        replace_text(&mut harness, "Caldari State standing", "10");
        assert_eq!(
            folders.store().fee_settings().unwrap().faction_standing,
            standing(1000)
        );
    }

    /// Saves `Raitaru Jita`: a Raitaru, High, in Jita, with a tax of 1.
    fn save_raitaru_jita(harness: &mut Harness<'_, App>) {
        click_nth(harness, "New", 0);
        type_into(harness, "Name", "Raitaru Jita");
        click_combo(harness, "NPC station", 0);
        click_nth(harness, "Raitaru", 0);
        type_into(harness, "System", "jita");
        replace_text(harness, "Tax %", "1");
        click_nth(harness, "Save", 0);
    }

    #[test]
    fn a_new_facility_saves_its_system_and_tax() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        save_raitaru_jita(&mut harness);

        let stored = folders.store().facilities().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].facility.structure, Some(RAITARU));
        assert_eq!(stored[0].facility.security, Security::High);
        assert!(stored[0].facility.rigs.is_empty());
        let costs = folders.store().facility_costs(stored[0].id).unwrap();
        assert_eq!(costs.system, Some(30_000_142));
        assert_eq!(costs.tax, Some(Rate::from_ppm(10_000)));
        assert!(
            row_has(
                &harness,
                "Raitaru Jita",
                &["Raitaru", "High", "Jita", "1.00%"]
            ),
            "the row holds the structure, security, system and tax"
        );
    }

    #[test]
    fn a_new_npc_station_has_the_default_tax_and_no_system() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        click_nth(&mut harness, "New", 0);
        type_into(&mut harness, "Name", "NPC hisec");
        assert_eq!(field_text(&harness, "Tax %"), "0.25");
        assert_eq!(field_text(&harness, "System"), "");
        click_nth(&mut harness, "Save", 0);

        let stored = folders.store().facilities().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].facility.structure, None);
        let costs = folders.store().facility_costs(stored[0].id).unwrap();
        assert_eq!(costs.system, None);
        assert_eq!(costs.tax, Some(Rate::from_ppm(2_500)));
        assert!(
            row_has(&harness, "NPC hisec", &["0.25%", "—"]),
            "the row shows the tax and a dash for the system"
        );
        assert_eq!(count_on_row(&harness, "NPC hisec", "—"), 1);
    }

    #[test]
    fn editing_shows_the_system_and_tax_and_clearing_the_tax_stores_none() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        save_raitaru_jita(&mut harness);
        click(&mut harness, "Raitaru Jita");
        click(&mut harness, "Edit");
        assert_eq!(field_text(&harness, "System"), "Jita");
        assert_eq!(field_text(&harness, "Tax %"), "1");

        replace_text(&mut harness, "Tax %", "");
        click_nth(&mut harness, "Save", 0);
        let stored = folders.store().facilities().unwrap();
        assert_eq!(stored.len(), 1);
        let costs = folders.store().facility_costs(stored[0].id).unwrap();
        assert_eq!(costs.system, Some(30_000_142));
        assert_eq!(costs.tax, None);
        assert!(
            row_has(&harness, "Raitaru Jita", &["Jita", "—"]),
            "the row shows the system and a dash for the tax"
        );
        assert_eq!(count_on_row(&harness, "Raitaru Jita", "—"), 1);
    }

    #[test]
    fn picking_a_structure_sets_the_tax_until_the_user_types_one() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        click_nth(&mut harness, "New", 0);
        type_into(&mut harness, "Name", "Azbel test");
        click_combo(&mut harness, "NPC station", 0);
        click_nth(&mut harness, "Azbel", 0);
        assert_eq!(field_text(&harness, "Tax %"), "");

        click_combo(&mut harness, "Azbel", 0);
        click_nth(&mut harness, "NPC station", 0);
        assert_eq!(field_text(&harness, "Tax %"), "0.25");

        replace_text(&mut harness, "Tax %", "0.5");
        click_combo(&mut harness, "NPC station", 0);
        click_nth(&mut harness, "Azbel", 0);
        assert_eq!(field_text(&harness, "Tax %"), "0.5");
    }

    #[test]
    fn a_tax_out_of_range_disables_save() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        click_nth(&mut harness, "New", 0);
        type_into(&mut harness, "Name", "Too taxed");
        replace_text(&mut harness, "Tax %", "101");
        assert!(shown(
            &harness,
            "Tax %: The number is outside the supported range."
        ));
        assert!(disabled(&harness, "Save"));
    }

    #[test]
    fn the_setup_panels_show_their_headers() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        for header in ["Facilities", "Freight services", "Price rule", "Structures"] {
            assert!(shown(&harness, header), "the {header} panel shows");
        }
        open_fees(&mut harness);
        assert!(
            shown(&harness, "Fees and skills"),
            "the Fees and skills panel shows"
        );
        assert!(shown(&harness, "Sales tax: 7.50%"));
    }

    #[test]
    fn the_fees_picture_is_taken_for_the_owner() {
        let folders = Folders::new();
        let mut harness = start_at(&folders, vec2(1280.0, 800.0));
        crate::ui::hide_database_size(&harness);
        open_fees(&mut harness);
        click_combo(&mut harness, "0", 0);
        click_nth(&mut harness, "4", 0);
        click_combo(&mut harness, "0", 0);
        click_nth(&mut harness, "5", 0);
        replace_text(&mut harness, "Caldari State standing", "5.23");
        replace_text(&mut harness, "Caldari Navy standing", "-1.25");
        harness.run_steps(2);
        harness.snapshot("setup-costs-fees");
    }
}
