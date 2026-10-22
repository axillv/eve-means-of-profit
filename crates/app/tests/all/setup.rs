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
    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::{App, AppConfig};
    use profit_core::freight::{Packing, RateCard};
    use profit_core::industry::{Activity, TypeId};
    use profit_core::money::{Money, Rate};
    use profit_core::volume::Volume;
    use profit_market::book::PriceRule;
    use profit_sde::{Facility, Fetch, FetchError, RouteKind, Sde, Security, import};
    use profit_store::{PlanRecord, Store};
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
        };
        let mut harness = Harness::builder()
            .with_size(size)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.run_steps(2);
        click(&mut harness, "Setup");
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
    fn type_ids(rigs: &[profit_sde::Rig]) -> Vec<TypeId> {
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

    #[test]
    fn the_setup_pictures_for_the_owner_are_taken() {
        let folders = Folders::new();
        let mut harness = start_at(&folders, vec2(1280.0, 800.0));
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
        harness.snapshot("setup-facilities");
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
        harness.snapshot("setup-freight");

        click(&mut harness, "Forge buy, highest");
        harness.run_steps(2);
        harness.snapshot("setup-price-rule");
    }
}
