//! Acceptance tests for the stockpile editor: the fields and their errors, targets, includes, the
//! filters, the store's refusals, the recent places, and the snapshot for the owner.

#[cfg(test)]
mod tests {
    use crate::ui::{click, shown, snapshot, top, until};
    use egui::accesskit::Role;
    use egui::vec2;
    use egui::{Key, Modifiers};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::stock::{Asset, CharacterAssets};
    use mop_core::stockpile::{Characters, Filter, Include, Place, Source, Stockpile, Target};
    use mop_esi::character_data::{CharacterData, Fetched, Kind, OwnedAssets, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::Store;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::PathBuf;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const HOME_FORT: i64 = 1_035_466_617_946;
    const TRITANIUM: u32 = 34;
    const HOBGOBLIN_II: u32 = 2_456;
    const THE_FORGE: u32 = 10_000_002;

    fn fixture() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    /// A copy of the fixture with Domain, which the shared one does not hold.
    fn fixture_with_domain(dir: &std::path::Path) -> PathBuf {
        let source = dir.join("fixture");
        copy_dir(&fixture(), &source).expect("the fixture copies");
        let regions = source.join("mapRegions.jsonl");
        let mut content = fs::read_to_string(&regions).expect("the regions file");
        content.push_str("\n{\"_key\": 10000043, \"name\": {\"en\": \"Domain\"}}\n");
        fs::write(&regions, content).expect("Domain is added");
        source
    }

    fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    /// The game data is installed by the test, so the app never asks the network for anything.
    struct NoNetwork;

    impl Fetch for NoNetwork {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// The price file: no plan asks for a price here, so an empty body is enough.
    struct NoPrices;

    impl BytesHttp for NoPrices {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(b"what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n")
                .map_err(|error| MarketError::Network(error.to_string()))?;
            let body = encoder
                .finish()
                .map_err(|error| MarketError::Network(error.to_string()))?;
            Ok(BytesResponse {
                status: 200,
                etag: Some("\"stockpile-edit\"".to_owned()),
                last_modified: Some("Mon, 12 Jan 1970 13:46:40 GMT".to_owned()),
                retry_after: None,
                body,
            })
        }
    }

    /// A data folder with the game data imported, the user's store, and Home Fort's name.
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn env() -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(data.join("esi")).expect("the data folder");
        let source = fixture_with_domain(dir.path());
        import(&source, &data.join("sde.sqlite")).expect("the fixture imports");
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        let places = r#"{"version":1,"structures":{"1035466617946":{"name":"Home Fort","system_id":null,"refused":false,"checked_at":900000}}}"#;
        fs::write(data.join("esi").join("places.json"), places).expect("places written");
        Env {
            _dir: dir,
            data,
            cache,
        }
    }

    fn esi_dir(env: &Env) -> PathBuf {
        env.data.join("esi")
    }

    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
    }

    impl EveAccess for FakeEve {
        fn characters(&self) -> Vec<Character> {
            self.characters.clone()
        }

        fn begin_login(&mut self) -> Result<PendingLogin, String> {
            Err("Logins are not started in these tests.".to_owned())
        }

        fn finish_login(&mut self, _code: &str) -> Result<Character, EsiError> {
            Err(EsiError::Cancelled)
        }

        fn refresh_all(
            &mut self,
            _data: &mut CharacterData,
            _now: i64,
        ) -> Vec<(Kind, Result<Refreshed, EsiError>)> {
            Vec::new()
        }

        fn name_structures(
            &mut self,
            _names: &mut StructureNames,
            _ids: &[i64],
            _now: i64,
        ) -> Result<NamingReport, EsiError> {
            Ok(NamingReport {
                asked: 0,
                failed: Vec::new(),
            })
        }

        fn save_character(&mut self, _data: &CharacterData) -> Result<(), EsiError> {
            Ok(())
        }

        fn remove(&mut self, character_id: u64) -> Result<(), EsiError> {
            Err(EsiError::UnknownCharacter(character_id))
        }

        fn keychain_warning(&self) -> Option<String> {
            None
        }

        fn jeve_folder(&self) -> Option<PathBuf> {
            None
        }
    }

    fn character(id: u64, name: &str) -> Character {
        Character {
            id,
            name: name.to_owned(),
            scopes: BTreeSet::new(),
            added_at: NOW,
            login: Login::Ok,
        }
    }

    /// Main's assets: Tritanium 25 in Jita, saved at the clock.
    fn save_main(env: &Env) {
        let assets = CharacterAssets {
            character_id: i64::try_from(MAIN).expect("an id that fits"),
            updated: Some(NOW),
            assets: vec![Asset {
                item_id: 1,
                type_id: TRITANIUM,
                location_id: JITA_4_4,
                flag: Some("Hangar".to_owned()),
                quantity: 25,
                singleton: false,
            }],
        };
        let data = CharacterData {
            character_id: MAIN,
            assets: Some(Fetched {
                data: OwnedAssets {
                    assets,
                    names: BTreeMap::new(),
                },
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            orders: None,
            jobs: None,
            wallet: None,
        };
        saved::save_character(&esi_dir(env), &data).expect("Main's data saved");
    }

    fn store(env: &Env) -> Store {
        Store::open(&env.data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
    }

    fn filter() -> Filter {
        Filter {
            characters: Characters::All,
            place: Place::Location(JITA_4_4),
            sources: BTreeSet::from([Source::Assets]),
        }
    }

    /// Kit with targets Tritanium 60 and Hobgoblin II 10, counting every character's assets in
    /// Jita.
    fn kit(env: &Env) -> i64 {
        stockpile(
            env,
            "Kit",
            None,
            &[(TRITANIUM, 60), (HOBGOBLIN_II, 10)],
            vec![filter()],
        )
    }

    /// Weekly, including Kit three times.
    fn weekly(env: &Env, kit: i64) -> i64 {
        let id = store(env)
            .create_stockpile("Weekly", Some("Drones"))
            .expect("Weekly is made");
        let mut store = store(env);
        store
            .save_stockpile(&Stockpile {
                id,
                name: "Weekly".to_owned(),
                group: Some("Drones".to_owned()),
                multiplier: 1,
                targets: Vec::new(),
                includes: vec![Include {
                    stockpile_id: kit,
                    count: 3,
                }],
                filters: Vec::new(),
            })
            .expect("Weekly is saved");
        id
    }

    fn stockpile(
        env: &Env,
        name: &str,
        group: Option<&str>,
        targets: &[(u32, u64)],
        filters: Vec<Filter>,
    ) -> i64 {
        let id = store(env)
            .create_stockpile(name, group)
            .expect("the stockpile is made");
        let mut store = store(env);
        store
            .save_stockpile(&Stockpile {
                id,
                name: name.to_owned(),
                group: group.map(str::to_owned),
                multiplier: 1,
                targets: targets
                    .iter()
                    .map(|(type_id, units)| Target {
                        type_id: *type_id,
                        units: *units,
                        ignore_multiplier: false,
                    })
                    .collect(),
                includes: Vec::new(),
                filters,
            })
            .expect("the stockpile is saved");
        id
    }

    fn start(env: &Env) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(FakeEve {
                characters: vec![character(MAIN, "Main")],
            }),
            market: Box::new(NoPrices),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Stockpiles);
        harness
    }

    fn loaded(harness: &mut Harness<'_, App>) {
        until(harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
    }

    /// Clicks the text field with this caption, selects what it holds and types `text`.
    fn type_into(harness: &mut Harness<'_, App>, caption: &str, nth: usize, text: &str) {
        {
            let field = harness
                .query_all_by_label(caption)
                .filter(|node| node.accesskit_node().role() == Role::TextInput)
                .nth(nth)
                .unwrap_or_else(|| panic!("no text field {nth} captioned {caption}"));
            field.scroll_to_me();
        }
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .filter(|node| node.accesskit_node().role() == Role::TextInput)
                .nth(nth)
                .unwrap_or_else(|| panic!("no text field {nth} captioned {caption}"));
            field.hover();
        }
        harness.run_steps(2);
        {
            let field = harness
                .query_all_by_label(caption)
                .filter(|node| node.accesskit_node().role() == Role::TextInput)
                .nth(nth)
                .unwrap_or_else(|| panic!("no text field {nth} captioned {caption}"));
            field.click();
        }
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        {
            let field = harness
                .query_all_by_label(caption)
                .filter(|node| node.accesskit_node().role() == Role::TextInput)
                .nth(nth)
                .unwrap_or_else(|| panic!("no text field {nth} captioned {caption}"));
            field.type_text(text);
        }
        harness.run_steps(3);
    }

    fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .get_all_by_label(label)
            .find(|node| node.accesskit_node().role() == Role::Button)
            .expect("a button")
            .accesskit_node()
            .is_disabled()
    }

    /// The nth checkbox, counting from the top of the window.
    fn tick(harness: &mut Harness<'_, App>, nth: usize) {
        {
            let node = harness
                .get_all_by_role(Role::CheckBox)
                .nth(nth)
                .expect("a checkbox");
            node.scroll_to_me();
        }
        harness.run_steps(2);
        harness
            .get_all_by_role(Role::CheckBox)
            .nth(nth)
            .expect("the same checkbox")
            .click_accesskit();
        harness.run_steps(3);
    }

    /// Scrolls the node labelled `label` into view.
    fn scroll_to(harness: &mut Harness<'_, App>, label: &str) {
        {
            let node = harness
                .query_all_by_label(label)
                .next()
                .unwrap_or_else(|| panic!("no node labelled {label:?}"));
            node.scroll_to_me();
        }
        harness.run_steps(2);
    }

    /// Clicks the last node with this label, which is the one in the editor rather than the page
    /// behind it.
    fn click_last(harness: &mut Harness<'_, App>, label: &str) {
        let node = harness.get_all_by_label(label).last();
        let node = node.unwrap_or_else(|| panic!("no node labelled {label:?}"));
        node.scroll_to_me();
        harness.run_steps(2);
        harness
            .get_all_by_label(label)
            .last()
            .expect("the same node")
            .click();
        harness.run_steps(3);
    }

    fn stored(env: &Env, id: i64) -> Stockpile {
        store(env).stockpile(id).expect("readable").expect("there")
    }

    #[test]
    fn a_new_stockpile_is_saved_with_targets_includes_and_one_filter() {
        let env = env();
        save_main(&env);
        let kit_id = kit(&env);
        let _weekly = weekly(&env, kit_id);
        let mut harness = start(&env);
        loaded(&mut harness);

        click(&mut harness, "New");
        assert!(shown(&harness, "Edit stockpile"));
        type_into(&mut harness, "Name", 0, "Drones");
        type_into(&mut harness, "Group", 0, "Ships");
        type_into(&mut harness, "Multiplier", 0, "2");

        type_into(&mut harness, "Add item", 0, "hobgoblin ii");
        click(&mut harness, "Hobgoblin II");
        type_into(&mut harness, "Units", 0, "10");
        tick(&mut harness, 0);

        type_into(&mut harness, "Add item", 0, "tritanium");
        click(&mut harness, "Tritanium");
        type_into(&mut harness, "Units", 1, "5000");

        click(&mut harness, "Include…");
        click_last(&mut harness, "Kit");
        type_into(&mut harness, "Count", 0, "2");

        click(&mut harness, "Add filter");
        click(&mut harness, "Only these");
        click(&mut harness, "Main");
        click(&mut harness, "Everywhere");
        click(&mut harness, "Region");
        type_into(&mut harness, "Region", 0, "for");
        click(&mut harness, "The Forge");
        click(&mut harness, "Running jobs");

        scroll_to(&mut harness, "Name");
        snapshot(&mut harness, "stockpile_edit");

        click(&mut harness, "Save");
        until(&mut harness, "the editor to close", |h| {
            !shown(h, "Edit stockpile")
        });

        let drones = store(&env)
            .stockpiles()
            .expect("the stockpiles")
            .into_iter()
            .find(|stockpile| stockpile.name == "Drones")
            .expect("Drones was saved");
        assert_eq!(
            drones,
            Stockpile {
                id: drones.id,
                name: "Drones".to_owned(),
                group: Some("Ships".to_owned()),
                multiplier: 2,
                targets: vec![
                    Target {
                        type_id: TRITANIUM,
                        units: 5_000,
                        ignore_multiplier: false,
                    },
                    Target {
                        type_id: HOBGOBLIN_II,
                        units: 10,
                        ignore_multiplier: true,
                    },
                ],
                includes: vec![Include {
                    stockpile_id: kit_id,
                    count: 2,
                }],
                filters: vec![Filter {
                    characters: Characters::Only(BTreeSet::from([i64::try_from(MAIN).unwrap()])),
                    place: Place::Region(THE_FORGE),
                    sources: BTreeSet::from([Source::Assets, Source::Jobs]),
                }],
            }
        );
    }

    #[test]
    fn a_multiplier_or_units_out_of_range_shows_its_error_and_disables_save() {
        let env = env();
        save_main(&env);
        let kit_id = kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Edit");

        type_into(&mut harness, "Multiplier", 0, "0");
        assert!(shown(&harness, "Multiplier: a whole number from 1"));
        assert!(disabled(&harness, "Save"));

        type_into(&mut harness, "Multiplier", 0, "1.5");
        assert!(shown(&harness, "Multiplier: a whole number from 1"));
        assert!(disabled(&harness, "Save"));

        type_into(&mut harness, "Multiplier", 0, "1");
        type_into(&mut harness, "Units", 0, "0");
        assert!(shown(&harness, "Units: a whole number from 1"));
        assert!(disabled(&harness, "Save"));

        type_into(&mut harness, "Units", 0, "60");
        assert!(!disabled(&harness, "Save"));
        assert_eq!(stored(&env, kit_id).multiplier, 1);
    }

    #[test]
    fn including_a_stockpile_that_includes_this_one_is_refused() {
        let env = env();
        save_main(&env);
        let kit_id = kit(&env);
        let _weekly = weekly(&env, kit_id);
        let before = stored(&env, kit_id);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Edit");

        click(&mut harness, "Include…");
        click_last(&mut harness, "Weekly");
        click(&mut harness, "Save");
        until(&mut harness, "the refusal", |h| {
            shown(h, "includes itself: Kit → Weekly → Kit")
        });
        assert!(shown(&harness, "Edit stockpile"));
        assert_eq!(stored(&env, kit_id), before);
    }

    #[test]
    fn a_filter_needs_a_source_and_a_character() {
        let env = env();
        save_main(&env);
        let kit_id = kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Edit");

        // The target rows' ignore ticks come first; the filter's Assets tick is third.
        tick(&mut harness, 2);
        assert_eq!(
            harness
                .get_all_by_role(Role::CheckBox)
                .nth(2)
                .expect("the Assets tick")
                .accesskit_node()
                .toggled(),
            Some(egui::accesskit::Toggled::False)
        );
        click(&mut harness, "Save");
        until(&mut harness, "the source refusal", |h| {
            shown(h, "a filter needs at least one source")
        });

        tick(&mut harness, 2);
        click(&mut harness, "Only these");
        click(&mut harness, "Save");
        until(&mut harness, "the character refusal", |h| {
            shown(h, "a character filter needs at least one character")
        });
        assert!(shown(&harness, "Edit stockpile"));
        assert_eq!(
            stored(&env, kit_id).filters[0].sources,
            BTreeSet::from([Source::Assets])
        );
    }

    #[test]
    fn recent_places_are_kept_and_survive_a_restart() {
        let env = env();
        save_main(&env);
        kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Edit");
        click(&mut harness, "Add filter");
        click(&mut harness, "Everywhere");
        click_last(&mut harness, "Region");
        type_into(&mut harness, "Region", 0, "for");
        click(&mut harness, "The Forge");
        type_into(&mut harness, "Region", 0, "dom");
        click(&mut harness, "Domain");

        click(&mut harness, "Add filter");
        click(&mut harness, "Everywhere");
        click_last(&mut harness, "Region");
        assert!(shown(&harness, "Domain"));
        assert!(shown(&harness, "The Forge"));
        assert!(top(&harness, "Domain") < top(&harness, "The Forge"));

        drop(harness);
        let mut again = start(&env);
        loaded(&mut again);
        click(&mut again, "Kit");
        click(&mut again, "Edit");
        click(&mut again, "Add filter");
        click(&mut again, "Everywhere");
        click_last(&mut again, "Region");
        assert!(shown(&again, "Domain"));
        assert!(shown(&again, "The Forge"));
        assert!(top(&again, "Domain") < top(&again, "The Forge"));
    }

    #[test]
    fn a_known_structure_is_found_by_its_label_and_saved_as_a_location() {
        let env = env();
        save_main(&env);
        let spare = stockpile(&env, "Spare", None, &[], Vec::new());
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Spare");
        click(&mut harness, "Edit");
        click(&mut harness, "Add filter");
        click(&mut harness, "Everywhere");
        click(&mut harness, "Station or structure");
        type_into(&mut harness, "Station or structure", 0, "home");
        click(&mut harness, "Home Fort");
        click(&mut harness, "Save");
        until(&mut harness, "the editor to close", |h| {
            !shown(h, "Edit stockpile")
        });
        let saved = stored(&env, spare);
        assert_eq!(saved.filters[0].place, Place::Location(HOME_FORT));
    }

    #[test]
    fn cancel_and_escape_close_without_saving() {
        let env = env();
        save_main(&env);
        let kit_id = kit(&env);
        let before = stored(&env, kit_id);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Edit");
        type_into(&mut harness, "Name", 0, "Renamed");
        click(&mut harness, "Cancel");
        assert!(!shown(&harness, "Edit stockpile"));

        click(&mut harness, "Edit");
        type_into(&mut harness, "Name", 0, "Renamed");
        harness.key_press(Key::Escape);
        harness.run_steps(3);
        assert!(!shown(&harness, "Edit stockpile"));
        assert_eq!(stored(&env, kit_id), before);
    }
}
