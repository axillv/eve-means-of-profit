//! Acceptance tests for the Setup page: the facility form, the freight routes and services, the field
//! checks and the price rule. The game data is the build 3607705 fixture, imported in each test; the
//! store is in a temp folder. The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::{BTreeMap, BTreeSet};
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
    use mop_core::freight::{Packing, RateCard};
    use mop_core::industry::{Activity, TypeId};
    use mop_core::money::{Money, Rate};
    use mop_core::price_history::Retention;
    use mop_core::volume::Volume;
    use mop_market::book::PriceRule;
    use mop_market::fuzzwork::{Side, SummaryRow};
    use mop_sde::{Facility, Fetch, FetchError, RouteKind, Sde, Security, import};
    use mop_store::price_history::PointPrices;
    use mop_store::{PlanRecord, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    /// The Raitaru, an engineering complex, and the four rigs that fit it for manufacturing.
    const RAITARU: TypeId = 35_825;
    const RIGS: [TypeId; 4] = [37_154, 37_155, 37_156, 37_157];

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
            folders
        }

        fn store(&self) -> Store {
            Store::open(&self.data.join("user.sqlite"), NOW)
                .unwrap()
                .store
        }

        fn sde(&self) -> Sde {
            Sde::open(&self.data.join("sde.sqlite")).unwrap()
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

    /// Replaces the text of the field with this caption: selects all of it, then types the new text.
    fn replace_into(harness: &mut Harness<'_, App>, caption: &str, text: &str) {
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
        harness.run_steps(1);
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

    /// The rect of the text field with this caption.
    fn field_rect(harness: &Harness<'_, App>, caption: &str) -> egui::Rect {
        harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"))
            .rect()
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

    /// The type ids of a list of rigs, in the order given.
    fn type_ids(rigs: &[mop_sde::Rig]) -> Vec<TypeId> {
        rigs.iter().map(|rig| rig.type_id).collect()
    }

    fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .accesskit_node()
            .is_disabled()
    }

    /// Adds a facility straight to the store, for tests that start from one.
    fn facility(folders: &Folders, name: &str, facility: &Facility) -> i64 {
        folders.store().add_facility(name, facility).unwrap()
    }

    fn raitaru_null() -> Facility {
        Facility {
            structure: Some(RAITARU),
            security: Security::Null,
            rigs: vec![RIGS[3]],
        }
    }

    /// Fills the facility form: a new facility named `name` in `structure`, then the rigs and
    /// security given. The first rig dropdown is slot 0.
    fn new_facility(harness: &mut Harness<'_, App>, name: &str) {
        click_nth(harness, "New", 0);
        type_into(harness, "Name", name);
    }

    #[test]
    fn a_new_null_raitaru_lists_its_rigs_and_saves_the_pick() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        new_facility(&mut harness, "Raitaru null");

        click_combo(&mut harness, "NPC station", 0);
        click_nth(&mut harness, "Raitaru", 0);
        click_combo(&mut harness, "High", 0);
        click_nth(&mut harness, "Null", 0);
        click_combo(&mut harness, "(none)", 0);
        let listed = type_ids(
            &folders
                .sde()
                .rigs_for(RAITARU, Activity::Manufacturing)
                .unwrap(),
        );
        assert_eq!(listed, RIGS.to_vec(), "the rigs are listed in this order");
        for rig in RIGS {
            let name = folders.sde().item(rig).unwrap().unwrap().name;
            assert!(shown(&harness, &name), "rig {rig} is listed");
        }
        click_nth(
            &mut harness,
            &folders.sde().item(RIGS[3]).unwrap().unwrap().name,
            0,
        );
        click_nth(&mut harness, "Save", 0);

        let rig_name = folders.sde().item(RIGS[3]).unwrap().unwrap().name;
        assert!(shown(&harness, "Raitaru null"));
        // The row's System and Tax are dashes: the facility has no system and no tax yet.
        let row = harness
            .query_all_by_label("Raitaru null")
            .next()
            .unwrap()
            .rect()
            .min
            .y;
        let on_row = |text: &str| {
            harness
                .query_all_by_label(text)
                .filter(|node| (node.rect().min.y - row).abs() < 1.0)
                .count()
        };
        assert_eq!(on_row("Raitaru"), 1);
        assert_eq!(on_row("Null"), 1);
        assert_eq!(on_row("—"), 2);
        assert!(shown(&harness, &rig_name));
        let stored = folders.store().facilities().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Raitaru null");
        assert_eq!(stored[0].facility, raitaru_null());
    }

    #[test]
    fn a_rig_twice_shows_its_error_and_disables_save() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        new_facility(&mut harness, "Raitaru null");
        click_combo(&mut harness, "NPC station", 0);
        click_nth(&mut harness, "Raitaru", 0);
        click_combo(&mut harness, "High", 0);
        click_nth(&mut harness, "Null", 0);
        click_combo(&mut harness, "(none)", 0);
        click_nth(
            &mut harness,
            &folders.sde().item(RIGS[2]).unwrap().unwrap().name,
            0,
        );
        click_combo(&mut harness, "(none)", 0);
        click_nth(
            &mut harness,
            &folders.sde().item(RIGS[3]).unwrap().unwrap().name,
            0,
        );

        let name = folders.sde().item(RIGS[3]).unwrap().unwrap().name;
        let error = format!("{name} is a second rig of a group that is already fitted");
        assert!(shown(&harness, &error));
        assert!(disabled(&harness, "Save"));
    }

    #[test]
    fn an_athanor_is_low_or_null_with_its_own_rigs_and_an_npc_station_has_none() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        new_facility(&mut harness, "Athanor low");
        click_combo(&mut harness, "NPC station", 0);
        click_nth(&mut harness, "Athanor", 0);
        // The refinery takes Low from High, and the security list offers no High.
        click_combo(&mut harness, "Low", 0);
        assert!(!shown(&harness, "High"), "a refinery is never in high sec");
        assert!(shown(&harness, "Null"));
        click_nth(&mut harness, "Low", 0);
        click_combo(&mut harness, "(none)", 0);
        let sde = folders.sde();
        let athanor = sde
            .structures()
            .unwrap()
            .into_iter()
            .find(|structure| structure.name == "Athanor")
            .unwrap()
            .type_id;
        let listed = type_ids(&sde.rigs_for(athanor, Activity::Reaction).unwrap());
        assert_eq!(
            listed,
            vec![46_486, 46_487],
            "the Athanor rigs are exactly these"
        );
        for rig in listed {
            let name = sde.item(rig).unwrap().unwrap().name;
            assert!(shown(&harness, &name), "Athanor rig {rig} is listed");
        }
    }

    #[test]
    fn an_npc_station_shows_no_rig_dropdowns() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        new_facility(&mut harness, "Station");
        assert!(
            !shown(&harness, "Rig 1"),
            "an NPC station shows no rig dropdowns"
        );
    }

    #[test]
    fn a_second_facility_with_the_same_name_shows_the_name_taken_text() {
        let folders = Folders::new();
        facility(&folders, "Raitaru null", &raitaru_null());
        let mut harness = start(&folders);
        new_facility(&mut harness, " raitaru NULL ");
        click_nth(&mut harness, "Save", 0);
        assert!(shown(
            &harness,
            "A facility named raitaru NULL already exists."
        ));
    }

    #[test]
    fn deleting_a_facility_a_plan_uses_shows_the_plan_name_and_keeps_it() {
        let folders = Folders::new();
        let id = facility(&folders, "Raitaru null", &raitaru_null());
        let product = folders.sde().find_exact("Hobgoblin II").unwrap().unwrap();
        folders
            .store()
            .create_plan(
                &PlanRecord {
                    id: 0,
                    name: "Hobgoblin 10".to_owned(),
                    product,
                    quantity: 10,
                    build: BTreeSet::new(),
                    me: BTreeMap::new(),
                    manufacturing: Some(id),
                    reactions: None,
                    price_rule: PriceRule::WalkSell,
                    route: None,
                    service: None,
                    packing: Packing::TopUp,
                    pasted_stock: String::new(),
                    created_at: NOW,
                    updated_at: NOW,
                },
                NOW,
            )
            .unwrap();
        let mut harness = start(&folders);
        click(&mut harness, "Raitaru null");
        click_nth(&mut harness, "Delete", 0);
        assert!(shown(&harness, "In use by: Hobgoblin 10"));
        assert_eq!(folders.store().facilities().unwrap().len(), 1);
    }

    #[test]
    fn a_route_lists_its_jumps_and_a_second_add_says_it_exists() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        click_nth(&mut harness, "New route", 0);
        type_into(&mut harness, "From", "jita");
        type_into(&mut harness, "To", "kiai");
        click(&mut harness, "Kiainti");
        click_nth(&mut harness, "Add route", 0);
        assert!(shown(&harness, "Jita → Kiainti · 11 jumps · 12 high sec"));

        click_nth(&mut harness, "New route", 0);
        type_into(&mut harness, "From", "jita");
        type_into(&mut harness, "To", "kiainti");
        click_nth(&mut harness, "Add route", 0);
        assert!(shown(&harness, "This route already exists."));
    }

    #[test]
    fn a_route_to_obe_has_no_high_sec_route() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        click_nth(&mut harness, "New route", 0);
        type_into(&mut harness, "From", "jita");
        type_into(&mut harness, "To", "obe");
        click_nth(&mut harness, "Add route", 0);
        assert!(shown(&harness, "Jita → Obe · 16 jumps · no high-sec route"));
    }

    /// Adds a route from the route form, by the names typed.
    fn add_route(harness: &mut Harness<'_, App>, from: &str, to: &str) {
        click_nth(harness, "New route", 0);
        type_into(harness, "From", from);
        type_into(harness, "To", to);
        click_nth(harness, "Add route", 0);
    }

    #[test]
    fn alliance_stores_its_rate_card_and_shows_the_map_jumps_until_overridden() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        add_route(&mut harness, "jita", "kiainti");
        click(&mut harness, "Jita → Kiainti · 11 jumps · 12 high sec");
        // The facility panel's New comes first on the page; the service's New is the second.
        click_nth(&mut harness, "New", 1);
        type_into(&mut harness, "Name", "Alliance");
        type_into(&mut harness, "ISK/m³", "850");
        type_into(&mut harness, "Collateral %", "0.5");
        type_into(&mut harness, "Minimum", "5,000,000");
        type_into(&mut harness, "Max m³", "360,000");
        type_into(&mut harness, "Max collateral", "10,000,000,000");
        click_nth(&mut harness, "Save", 0);

        let route = folders.store().routes().unwrap()[0].id;
        let services = folders.store().services(route).unwrap();
        assert_eq!(services.len(), 1);
        let service = &services[0];
        assert_eq!(service.name, "Alliance");
        assert_eq!(
            service.card,
            RateCard {
                per_m3: Some(Money::from_ten_thousandths(8_500_000)),
                per_jump: None,
                collateral: Some(Rate::from_ppm(5_000)),
                minimum: Some(Money::from_ten_thousandths(50_000_000_000)),
                max_m3: Some(Volume::from_ten_thousandths(3_600_000_000)),
                max_collateral: Some(Money::from_ten_thousandths(100_000_000_000_000)),
            }
        );
        assert_eq!(service.route_kind, RouteKind::Shortest);
        assert_eq!(service.jumps_override, None);
        assert!(shown(&harness, "11 (map)"));

        click_nth(&mut harness, "Edit", 1);
        type_into(&mut harness, "Jumps", "9");
        click_nth(&mut harness, "Save", 0);
        assert!(shown(&harness, "9"));
        assert_eq!(
            folders.store().services(route).unwrap()[0].jumps_override,
            Some(9)
        );
    }

    #[test]
    fn each_bad_field_shows_its_error_and_disables_save() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        add_route(&mut harness, "jita", "kiainti");
        click(&mut harness, "Jita → Kiainti · 11 jumps · 12 high sec");
        click_nth(&mut harness, "New", 1);
        type_into(&mut harness, "Name", "Alliance");

        type_into(&mut harness, "ISK/m³", "8.50001");
        assert!(shown(
            &harness,
            "ISK/m³: The number has more decimals than are supported."
        ));
        assert!(disabled(&harness, "Save"));
        type_into(&mut harness, "ISK/m³", "");

        type_into(&mut harness, "Collateral %", "101");
        assert!(shown(
            &harness,
            "Collateral %: The number is outside the supported range."
        ));
        assert!(disabled(&harness, "Save"));
        type_into(&mut harness, "Collateral %", "");

        type_into(&mut harness, "Max m³", "1,00");
        assert!(shown(&harness, "Max m³: The text is not a valid number."));
        assert!(disabled(&harness, "Save"));
        type_into(&mut harness, "Max m³", "");

        for jumps in ["-1", "1001"] {
            type_into(&mut harness, "Jumps", jumps);
            assert!(shown(
                &harness,
                "Jumps: must be a whole number from 0 to 1,000"
            ));
            assert!(disabled(&harness, "Save"));
            type_into(&mut harness, "Jumps", "");
        }
    }

    #[test]
    fn a_fresh_store_lists_the_eight_rules_and_saves_a_pick_for_new_plans_only() {
        let folders = Folders::new();
        let product = folders.sde().find_exact("Hobgoblin II").unwrap().unwrap();
        folders
            .store()
            .create_plan(
                &PlanRecord {
                    id: 0,
                    name: "Old plan".to_owned(),
                    product,
                    quantity: 1,
                    build: BTreeSet::new(),
                    me: BTreeMap::new(),
                    manufacturing: None,
                    reactions: None,
                    price_rule: PriceRule::WalkSell,
                    route: None,
                    service: None,
                    packing: Packing::TopUp,
                    pasted_stock: String::new(),
                    created_at: NOW,
                    updated_at: NOW,
                },
                NOW,
            )
            .unwrap();
        assert_eq!(
            folders.store().settings().unwrap().default_price_rule,
            PriceRule::SellFivePercent
        );
        let mut harness = start(&folders);
        let labels = [
            "Forge sell, 5% price",
            "Forge sell, cheapest",
            "Forge sell, weighted average",
            "Forge sell, middle",
            "Forge buy, highest",
            "Forge buy, 5% price",
            "Forge buy, weighted average",
            "Forge buy, middle",
        ];
        let helps = [
            "The price at the cheapest 5% of the sell volume in The Forge.",
            "The lowest sell price in The Forge.",
            "The average sell price, weighted by volume.",
            "The middle sell price.",
            "The highest buy order, for people who place buy orders.",
            "The price at the highest 5% of the buy volume in The Forge.",
            "The average buy price, weighted by volume.",
            "The middle buy price.",
        ];
        for label in labels {
            assert!(shown(&harness, label), "{label} is listed");
        }
        for help in helps {
            assert!(shown(&harness, help), "the help for {help:?} is listed");
        }
        let tops: Vec<f32> = labels.iter().map(|label| top_of(&harness, label)).collect();
        assert!(
            tops.windows(2).all(|pair| pair[0] < pair[1]),
            "the labels are listed in this order"
        );
        assert!(selected(&harness, labels[0]));

        click(&mut harness, "Forge buy, middle");
        assert!(selected(&harness, "Forge buy, middle"));
        drop(harness);

        let mut store = folders.store();
        assert_eq!(
            store.settings().unwrap().default_price_rule,
            PriceRule::BuyMedian
        );
        let old = store.plans().unwrap();
        let old_id = old.iter().find(|plan| plan.name == "Old plan").unwrap().id;
        assert_eq!(
            store.plan(old_id).unwrap().unwrap().price_rule,
            PriceRule::WalkSell
        );
    }

    /// The top edge of the first node labelled `label`, to check the order of the radio list.
    fn top_of(harness: &Harness<'_, App>, label: &str) -> f32 {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .rect()
            .min
            .y
    }

    fn selected(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .accesskit_node()
            .toggled()
            == Some(egui::accesskit::Toggled::True)
    }

    /// A Forge history of 92 days, three rows a reading, under the default rule.
    fn seed_forge_history(folders: &Folders) {
        let mut store = Store::open(&folders.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let market = store
            .history_markets()
            .expect("the markets")
            .iter()
            .find(|market| market.place == 10_000_002)
            .expect("the Forge is listed")
            .id;
        let row = || SummaryRow {
            min: Money::from_ten_thousandths(500),
            max: Money::ZERO,
            weighted_average: Money::ZERO,
            median: Money::ZERO,
            five_percent: Money::ZERO,
            volume: 1,
            orders: 1,
        };
        for day in 20_000..20_092 {
            store
                .record(
                    market,
                    day,
                    &[
                        (34, Side::Sell, row()),
                        (34, Side::Buy, row()),
                        (11481, Side::Sell, row()),
                    ],
                )
                .expect("the reading is recorded");
        }
    }

    /// Two net worth price copies: PAU Sell and Buy at `NOW`, Tritanium Sell a day later.
    fn save_net_worth_copies(folders: &Folders) {
        let mut store = Store::open(&folders.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let row = |min: i64| SummaryRow {
            min: Money::from_ten_thousandths(min),
            max: Money::ZERO,
            weighted_average: Money::ZERO,
            median: Money::ZERO,
            five_percent: Money::ZERO,
            volume: 1,
            orders: 1,
        };
        let pau = PointPrices {
            at: NOW,
            published_at: None,
            rows: BTreeMap::from([
                ((12_612, Side::Sell), row(500)),
                ((12_612, Side::Buy), row(400)),
            ]),
        };
        store.save_point_prices(&pau).expect("the PAU copy saves");
        let tritanium = PointPrices {
            at: NOW + 86_400,
            published_at: None,
            rows: BTreeMap::from([((34, Side::Sell), row(600))]),
        };
        store
            .save_point_prices(&tritanium)
            .expect("the Tritanium copy saves");
        assert_eq!(
            store.point_prices_size().expect("the size"),
            (2, 3),
            "two copies, three price rows"
        );
    }

    /// The `Total ... estimated` part of the footer line, without the database file size.
    fn estimated_total(harness: &Harness<'_, App>) -> String {
        harness
            .query_all_by_label_contains("Total ")
            .filter_map(|node| node.value())
            .find_map(|line| {
                line.split_once(" · database file ")
                    .map(|(total, _)| total.to_owned())
            })
            .unwrap_or_else(|| panic!("the total line shows"))
    }

    /// The count in the `Dropped <n> days` message, when it shows.
    fn dropped_days(harness: &Harness<'_, App>) -> Option<u64> {
        harness
            .query_all_by_label_contains("Dropped ")
            .filter_map(|node| node.value())
            .find_map(|line| {
                line.strip_prefix("Dropped ")
                    .and_then(|rest| rest.strip_suffix(" days"))
                    .and_then(|days| days.parse().ok())
            })
    }

    /// The Forge's saved retention rule.
    fn forge_retention(folders: &Folders) -> Retention {
        Store::open(&folders.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
            .history_markets()
            .expect("the markets")
            .into_iter()
            .find(|market| market.place == 10_000_002)
            .expect("the Forge is listed")
            .retention
    }

    /// The saved size limit.
    fn history_limit(folders: &Folders) -> Option<u64> {
        Store::open(&folders.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
            .history_settings()
            .expect("the settings")
            .limit_bytes
    }

    #[test]
    fn net_worth_price_copies_show_apart_from_the_daily_history() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let harness = start_at(&folders, vec2(1280.0, 1400.0));
        assert!(shown(&harness, "Net worth prices: none yet"));
        let without_copies = estimated_total(&harness);
        drop(harness);

        save_net_worth_copies(&folders);
        let harness = start_at(&folders, vec2(1280.0, 1400.0));
        assert!(shown(
            &harness,
            "Net worth prices: 2 copies, 3 rows, about 300 B"
        ));
        assert!(shown(
            &harness,
            "Kept for good: these prices cannot be fetched again, so no rule or limit above thins them."
        ));
        assert_eq!(
            estimated_total(&harness),
            without_copies,
            "the copies are not added to the daily history total"
        );
    }

    #[test]
    fn a_size_limit_thins_the_history_but_keeps_the_net_worth_copies() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        save_net_worth_copies(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        assert!(shown(
            &harness,
            "Net worth prices: 2 copies, 3 rows, about 300 B"
        ));

        type_into(&mut harness, "Keep at most", "1");
        click(&mut harness, "Save limit");

        let dropped = dropped_days(&harness).expect("the limit drops days");
        assert!(dropped > 0, "the Forge history is thinned: {dropped} days");
        assert!(shown(
            &harness,
            "Net worth prices: 2 copies, 3 rows, about 300 B"
        ));
        drop(harness);

        let store = Store::open(&folders.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        assert_eq!(store.point_prices_size().expect("the size"), (2, 3));
    }

    #[test]
    fn the_price_history_panel_has_no_background_refresh_control() {
        let folders = Folders::new();
        let harness = start_at(&folders, vec2(1280.0, 1400.0));
        assert!(!shown(&harness, "Refresh prices in the background"));
        assert!(shown(&harness, "Keep at most"));
        assert!(shown(
            &harness,
            "Kept for good: these prices cannot be fetched again, so no rule or limit above thins them."
        ));
    }

    #[test]
    fn a_zero_daily_day_shows_its_error_under_the_daily_field() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        click_nth(&mut harness, "The Forge", 0);
        replace_into(&mut harness, "Daily for", "0");

        let error = "Daily days must be 1 or more";
        assert_eq!(
            harness.query_all_by_label(error).count(),
            1,
            "one node reads the daily error"
        );
        let error_rect = harness
            .query_all_by_label(error)
            .next()
            .expect("the daily error shows")
            .rect();
        let daily = field_rect(&harness, "Daily for");
        let weekly = field_rect(&harness, "Then weekly for");
        assert!(
            error_rect.min.y >= daily.max.y,
            "the error is below the daily field: {error_rect:?} against {daily:?}"
        );
        assert!(
            error_rect.min.y < weekly.min.y,
            "the error is above the weekly field: {error_rect:?} against {weekly:?}"
        );
        assert!(disabled(&harness, "Save rule"), "Save rule is off");

        harness.run_steps(3);
        let retention = forge_retention(&folders);
        assert_eq!(
            retention.daily_days, 90,
            "the saved daily days are unchanged"
        );
        assert_eq!(retention.total_days, None, "nothing weekly was saved");
    }

    #[test]
    fn a_weekly_part_shorter_than_the_daily_part_shows_under_the_weekly_field() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        click_nth(&mut harness, "The Forge", 0);
        replace_into(&mut harness, "Daily for", "30");
        replace_into(&mut harness, "Then weekly for", "20");

        let error = "The weekly part cannot be shorter than the daily part";
        assert_eq!(
            harness.query_all_by_label(error).count(),
            1,
            "one node reads the weekly error"
        );
        let error_rect = harness
            .query_all_by_label(error)
            .next()
            .expect("the weekly error shows")
            .rect();
        let weekly = field_rect(&harness, "Then weekly for");
        let save = harness
            .query_all_by_label("Save rule")
            .next()
            .expect("the Save rule button shows")
            .rect();
        assert!(
            error_rect.min.y >= weekly.max.y,
            "the error is below the weekly field: {error_rect:?} against {weekly:?}"
        );
        assert!(
            error_rect.max.y <= save.min.y,
            "the error is above Save rule: {error_rect:?} against {save:?}"
        );
        assert!(disabled(&harness, "Save rule"), "Save rule is off");

        harness.run_steps(3);
        let retention = forge_retention(&folders);
        assert_eq!(
            retention.daily_days, 90,
            "the saved daily days are unchanged"
        );
        assert_eq!(retention.total_days, None, "nothing weekly was saved");
    }

    #[test]
    fn a_bad_size_limit_shows_its_error_under_the_row() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        click_nth(&mut harness, "The Forge", 0);
        type_into(&mut harness, "Keep at most", "abc");

        let error = "Keep at most must be a whole number.";
        assert_eq!(
            harness.query_all_by_label(error).count(),
            1,
            "one node reads the limit error"
        );
        let error_rect = harness
            .query_all_by_label(error)
            .next()
            .expect("the limit error shows")
            .rect();
        let save = harness
            .query_all_by_label("Save limit")
            .next()
            .expect("the Save limit button shows")
            .rect();
        assert!(
            error_rect.min.y >= save.max.y,
            "the error is below the row: {error_rect:?} against {save:?}"
        );
        assert!(
            error_rect.min.y - save.max.y <= 8.0,
            "the error is at most 8 px under the row: {error_rect:?} against {save:?}"
        );
        assert!(disabled(&harness, "Save limit"), "Save limit is off");
        drop(harness);
        assert_eq!(history_limit(&folders), None, "no limit was saved");
    }

    #[test]
    fn a_good_size_limit_saves_and_drops_days() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        click_nth(&mut harness, "The Forge", 0);
        type_into(&mut harness, "Keep at most", "10");
        click(&mut harness, "Save limit");

        assert!(
            !shown(&harness, "Keep at most must be a whole number."),
            "the error is gone"
        );
        let dropped = dropped_days(&harness).expect("the limit drops days");
        assert!(dropped > 0, "the Forge history is thinned: {dropped} days");
        drop(harness);
        assert_eq!(history_limit(&folders), Some(10_000), "10 kB is saved");
    }

    #[test]
    fn clearing_the_size_limit_saves_no_limit() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut store = folders.store();
        let mut settings = store.history_settings().expect("the settings");
        settings.limit_bytes = Some(10_000);
        store
            .save_history_settings(&settings)
            .expect("the limit saves");
        drop(store);

        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        assert_eq!(
            harness
                .query_all_by_label("Keep at most")
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .expect("the limit field shows")
                .value()
                .as_deref(),
            Some("10"),
            "the limit shows its saved number"
        );

        replace_into(&mut harness, "Keep at most", "");
        assert_eq!(
            harness
                .query_all_by_label("Keep at most")
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .expect("the limit field shows")
                .value()
                .as_deref(),
            Some(""),
            "the limit field is empty"
        );
        click(&mut harness, "Save limit");
        drop(harness);
        assert_eq!(history_limit(&folders), None, "the limit is cleared");
    }

    #[test]
    fn the_price_history_field_errors_picture_for_the_owner_is_taken() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        click_nth(&mut harness, "The Forge", 0);
        replace_into(&mut harness, "Daily for", "0");
        type_into(&mut harness, "Keep at most", "abc");

        // The fields are typed at the taller size, where they are on screen; shrink for the picture.
        harness.set_size(vec2(1280.0, 800.0));
        harness.run_steps(3);
        reveal_field(&mut harness, "Daily for");
        harness.run_steps(2);
        // Scroll down a little so the limit row below the rule also shows.
        {
            let field = harness
                .query_all_by_label("Daily for")
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .expect("the daily field shows");
            field.scroll_down();
        }
        harness.run_steps(2);
        // Nudge the content back down a few pixels so the daily row clears the panel's top edge.
        {
            let field = harness
                .query_all_by_label("Then weekly for")
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .expect("the weekly field shows");
            field.hover();
        }
        harness.run_steps(1);
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 14.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::default(),
        });
        harness.run_steps(2);
        let daily_text = harness
            .query_all_by_label("Daily for")
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .expect("the daily field shows")
            .value();
        assert_eq!(daily_text.as_deref(), Some("0"), "the daily text is typed");
        assert_eq!(
            harness
                .query_all_by_label("Daily days must be 1 or more")
                .count(),
            1,
            "the daily error shows"
        );
        crate::ui::picture(&mut harness, "history-field-errors");
    }

    #[test]
    fn the_price_history_picture_for_the_owner_is_taken() {
        let folders = Folders::new();
        seed_forge_history(&folders);
        let mut harness = start_at(&folders, vec2(1280.0, 800.0));
        crate::ui::hide_database_size(&harness);
        harness.run_steps(2);
        click_nth(&mut harness, "The Forge", 0);
        harness.run_steps(3);
        assert!(shown(
            &harness,
            "Forecast for The Forge: about 39.0 kB after 1 year"
        ));
        assert!(
            !shown(&harness, "database file"),
            "the picture leaves the database file size out"
        );
        crate::ui::picture(&mut harness, "history-panel");
    }

    #[test]
    fn the_setup_pictures_for_the_owner_are_taken() {
        let folders = Folders::new();
        let mut harness = start_at(&folders, vec2(1280.0, 800.0));
        crate::ui::hide_database_size(&harness);
        new_facility(&mut harness, "Raitaru null");
        click_combo(&mut harness, "NPC station", 0);
        click_nth(&mut harness, "Raitaru", 0);
        click_combo(&mut harness, "High", 0);
        click_nth(&mut harness, "Null", 0);
        click_combo(&mut harness, "(none)", 0);
        click_nth(
            &mut harness,
            &folders.sde().item(RIGS[2]).unwrap().unwrap().name,
            0,
        );
        click_combo(&mut harness, "(none)", 0);
        click_nth(
            &mut harness,
            &folders.sde().item(RIGS[3]).unwrap().unwrap().name,
            0,
        );
        harness.run_steps(2);
        crate::ui::picture(&mut harness, "setup-facilities");
        // The facility form is left unsaved for the picture: close it before the services.
        click_nth(&mut harness, "Cancel", 0);

        add_route(&mut harness, "jita", "kiainti");
        add_route(&mut harness, "jita", "obe");
        click(&mut harness, "Jita → Kiainti · 11 jumps · 12 high sec");
        click_nth(&mut harness, "New", 1);
        type_into(&mut harness, "Name", "Alliance");
        type_into(&mut harness, "ISK/m³", "850");
        type_into(&mut harness, "Collateral %", "0.5");
        type_into(&mut harness, "Minimum", "5,000,000");
        type_into(&mut harness, "Max m³", "360,000");
        type_into(&mut harness, "Max collateral", "10,000,000,000");
        click_nth(&mut harness, "Save", 0);
        click(&mut harness, "Alliance");
        harness.run_steps(2);
        crate::ui::picture(&mut harness, "setup-freight");

        click(&mut harness, "Forge buy, highest");
        harness.run_steps(2);
        crate::ui::picture(&mut harness, "setup-price-rule");
    }
}
