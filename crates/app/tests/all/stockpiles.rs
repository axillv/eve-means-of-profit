//! Acceptance tests for the Stockpiles page: the rail entry, the grouped list, the contents table
//! with the Short counts choice, the buy window and the delete refusal. The clock is fixed at
//! 1,000,000 and the game data is the build 3607705 fixture.
#[cfg(test)]
mod tests {

    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use crate::ui::{click, hover, on_row, shown, snapshot, until};
    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_core::freight::Packing;
    use mop_core::money::Money;
    use mop_core::stock::{Asset, CharacterAssets, Places, StockScope};
    use mop_core::stockpile::{Characters, Filter, Include, Place, Source, Stockpile, Target};
    use mop_esi::character_data::{
        CharacterData, Fetched, Job, JobStatus, Kind, Order, OwnedAssets, Refreshed,
    };
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::saved;
    use mop_market::book::PriceRule;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::reservations::PlanStock;
    use mop_store::{PlanRecord, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const MAIN: u64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;
    const TRITANIUM: u32 = 34;
    const HOBGOBLIN_I: u32 = 2_454;
    const HOBGOBLIN_II: u32 = 2_456;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
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

    /// The Forge summary file: Tritanium's five-percent sell price is 5.00, and no row names
    /// Hobgoblin I.
    struct Forge;

    impl BytesHttp for Forge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let text = "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n\
                 10000002|34|false,5.20,6.00,4.80,0.0,5.10,5000,10,5.00,175414\n";
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(text.as_bytes())
                .map_err(|error| MarketError::Network(error.to_string()))?;
            let body = encoder
                .finish()
                .map_err(|error| MarketError::Network(error.to_string()))?;
            Ok(BytesResponse {
                status: 200,
                etag: Some("\"stockpiles\"".to_owned()),
                last_modified: Some("Mon, 12 Jan 1970 13:46:40 GMT".to_owned()),
                retry_after: None,
                body,
            })
        }
    }

    /// A data folder with the game data imported and the user's store. The login data is saved by
    /// each test.
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
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
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

    /// A login with the characters given. Nothing here refreshes or logs in.
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

    /// Main's assets: Tritanium 25 and Hobgoblin I 7 in Jita, saved at the clock.
    fn save_main(env: &Env) {
        save_main_with(env, Vec::new(), Vec::new());
    }

    /// Main's assets, open orders and jobs, all fetched at the clock.
    fn save_main_with(env: &Env, orders: Vec<Order>, jobs: Vec<Job>) {
        let assets = CharacterAssets {
            character_id: i64::try_from(MAIN).expect("an id that fits"),
            updated: Some(NOW),
            assets: vec![
                asset(1, TRITANIUM, JITA_4_4, 25),
                asset(2, HOBGOBLIN_I, JITA_4_4, 7),
            ],
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
            orders: Some(Fetched {
                data: orders,
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            jobs: Some(Fetched {
                data: jobs,
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            wallet: None,
        };
        saved::save_character(&esi_dir(env), &data).expect("Main's data saved");
    }

    fn asset(item_id: i64, type_id: u32, place: i64, quantity: u64) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id: place,
            flag: Some("Hangar".to_owned()),
            quantity,
            singleton: false,
        }
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

    /// Kit with targets Tritanium 60 and Hobgoblin I 10, counting every character's assets in
    /// Jita.
    fn kit(env: &Env) -> i64 {
        kit_with_filters(env, vec![filter()])
    }

    fn kit_with_filters(env: &Env, filters: Vec<Filter>) -> i64 {
        stockpile(
            env,
            "Kit",
            None,
            &[(TRITANIUM, 60), (HOBGOBLIN_I, 10)],
            filters,
        )
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

    fn main_only(source: Source) -> Filter {
        Filter {
            characters: Characters::Only(
                [i64::try_from(MAIN).expect("an id that fits")]
                    .into_iter()
                    .collect(),
            ),
            place: Place::Everywhere,
            sources: BTreeSet::from([source]),
        }
    }

    fn orders_filter() -> Filter {
        Filter {
            characters: Characters::All,
            place: Place::Everywhere,
            sources: BTreeSet::from([Source::SellOrders, Source::BuyOrders]),
        }
    }

    /// A saved plan of `quantity` of `product`, taking Main's stock everywhere and reserving.
    fn plan(env: &Env, name: &str, product: u32, quantity: u64) -> i64 {
        let record = PlanRecord {
            id: 0,
            name: name.to_owned(),
            product,
            quantity,
            build: BTreeSet::from([HOBGOBLIN_I]),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: PriceRule::WalkSell,
            route: None,
            service: None,
            packing: Packing::default(),
            pasted_stock: String::new(),
            created_at: NOW,
            updated_at: NOW,
        };
        let id = store(env)
            .create_plan(&record, NOW)
            .expect("the plan is saved");
        let scope = StockScope {
            characters: [i64::try_from(MAIN).expect("an id that fits")]
                .into_iter()
                .collect(),
            places: Places::All,
        };
        let mut store = store(env);
        store
            .set_plan_stock(id, &PlanStock::Characters(scope))
            .expect("the plan's stock is saved");
        store.set_reserve(id, true).expect("the plan reserves");
        id
    }

    /// A job of `runs` of `product` at Jita, with the status the page reads.
    fn job(id: i64, product: u32, runs: u32, status: JobStatus) -> Job {
        Job {
            job_id: id,
            activity_id: 1,
            blueprint_type_id: 2455,
            product_type_id: Some(product),
            runs,
            status,
            start: NOW - 60,
            end: NOW + 60,
            facility_id: JITA_4_4,
            station_id: JITA_4_4,
            output_location_id: JITA_4_4,
            installer_id: i64::try_from(MAIN).expect("an id that fits"),
            cost: None,
            licensed_runs: None,
        }
    }

    /// One open order at Jita.
    fn order(id: i64, type_id: u32, buy: bool, remaining: u64) -> Order {
        Order {
            order_id: id,
            type_id,
            location_id: JITA_4_4,
            region_id: 10_000_002,
            is_buy: buy,
            price: Money::from_ten_thousandths(500),
            volume_remain: remaining,
            volume_total: remaining,
            issued: NOW,
            expires: NOW + 86_400,
            range: "station".to_owned(),
            escrow: None,
            min_volume: None,
            is_corporation: false,
        }
    }

    /// Asks the market for the file and waits until the fake's file is held.
    fn fetch_market(harness: &mut Harness<'static, App>) {
        {
            let shared = harness.state_mut().shared_mut();
            let market = Arc::clone(&shared.market);
            market.refresh(&shared.worker, NOW, true);
        }
        for _ in 0..500 {
            harness.run();
            if harness
                .state_mut()
                .shared_mut()
                .market
                .held()
                .published_at
                .is_some()
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the market file did not arrive");
    }

    fn start(env: &Env) -> Harness<'static, App> {
        start_with(env, Box::new(crate::offline::Offline))
    }

    fn start_with(env: &Env, market: Box<dyn BytesHttp + Send + Sync>) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(FakeEve {
                characters: vec![character(MAIN, "Main")],
            }),
            market,
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

    /// Runs frames until the start-up read has arrived: the loading line is gone.
    fn loaded(harness: &mut Harness<'_, App>) {
        until(harness, "the saved data", |h| {
            !shown(h, "Loading character data…")
        });
    }

    /// The text a frame copied to the clipboard.
    fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
    }

    #[test]
    fn the_rail_lists_stockpiles_and_the_page_shows_its_panels() {
        let env = env();
        save_main(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        for panel in ["Stockpiles", "Contents"] {
            assert!(shown(&harness, panel), "{panel} is shown");
        }
    }

    #[test]
    fn kit_lists_as_short_and_its_contents_count_the_holdings() {
        let env = env();
        save_main(&env);
        kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);

        assert!(shown(&harness, "Kit"));
        assert!(shown(&harness, "2 short"));
        click(&mut harness, "Kit");
        assert!(shown(&harness, "CONTENTS  Kit"));
        assert!(shown(&harness, "Full after my plans"));
        assert!(!shown(
            &harness,
            "This stockpile has no filters, so it counts nothing. Edit it to add one."
        ));
        assert!(on_row(&harness, "Tritanium", &["60", "25", "35"]));
        assert!(on_row(&harness, "Hobgoblin I", &["10", "7", "3"]));
        assert!(shown(&harness, "Free"));
        let help = hover(&mut harness, "Full after my plans");
        assert!(
            help.iter().any(|line| line
                == "Short counts what is left after plans with Reserve take their part."),
            "{help:?}"
        );
    }

    #[test]
    fn the_short_mode_is_saved_and_survives_a_restart() {
        let env = env();
        save_main(&env);
        kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Full after my plans");
        click(&mut harness, "Full right now");
        assert!(!shown(&harness, "Free"), "Free leaves the table");
        drop(harness);

        let mut again = start(&env);
        loaded(&mut again);
        click(&mut again, "Kit");
        assert!(shown(&again, "Full right now"));
        assert!(!shown(&again, "Free"));
    }

    #[test]
    fn delete_refuses_a_stockpile_another_one_includes() {
        let env = env();
        save_main(&env);
        let kit = kit(&env);
        let weekly = store(&env)
            .create_stockpile("Weekly", Some("Drones"))
            .expect("Weekly is made");
        let mut store = store(&env);
        let record = store.stockpile(weekly).expect("readable").expect("Weekly");
        store
            .save_stockpile(&Stockpile {
                includes: vec![Include {
                    stockpile_id: kit,
                    count: 1,
                }],
                ..record
            })
            .expect("Weekly is saved");

        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Delete");
        assert!(shown(&harness, "Delete stockpile Kit?"));
        harness
            .get_all_by_label("Delete")
            .last()
            .expect("the dialog's Delete")
            .click();
        harness.run_steps(3);
        assert!(shown(&harness, "Kit is included in: Weekly"));
        assert!(shown(&harness, "Kit"));
    }

    #[test]
    fn the_buy_window_copies_the_short_rows() {
        let env = env();
        save_main(&env);
        kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Buy what is short");
        assert!(shown(&harness, "Buy what is short: Kit"));
        assert!(shown(&harness, "Copy multibuy"));
        assert_eq!(harness.get_all_by_label("Tritanium").count(), 2);
        assert_eq!(
            harness.get_all_by_label("Buy what is short: Kit").count(),
            1
        );
        assert_eq!(harness.get_all_by_label("Copy multibuy").count(), 1);
        harness.get_by_label("Copy multibuy").click_accesskit();
        harness.step();
        let first = copied_text(&harness);
        harness.step();
        assert!(shown(&harness, "Copied 2 lines for multibuy."));
        let copied = first
            .or_else(|| copied_text(&harness))
            .expect("the multibuy text is copied");
        assert!(copied.contains("Tritanium\t35"), "{copied:?}");
        assert!(copied.contains("Hobgoblin I\t3"), "{copied:?}");
    }

    #[test]
    fn j_eve_import_brings_in_the_file_and_renames_a_taken_stockpile() {
        let env = env();
        save_main(&env);
        kit(&env);
        let file = env.data.join("sample.xml");
        fs::write(
            &file,
            r#"<?xml version="1.0"?>
<settings>
<stockpiles>
<stockpile name="Kit" multiplier="1.0" stockpilegroup="Drones">
<item typeid="34" minimum="10.0" runs="false" ignoremultiplier="false"/>
</stockpile>
<stockpile name="Weekly" multiplier="2.0">
<subpile name="Kit" minimum="3.0"/>
</stockpile>
<stockpile name="Half" multiplier="1.5">
<item typeid="34" minimum="100.0" runs="false" ignoremultiplier="false"/>
</stockpile>
</stockpiles>
</settings>"#,
        )
        .expect("the sample is written");

        let mut harness = start(&env);
        loaded(&mut harness);
        assert!(eve_means_of_profit::stockpiles::import_file(
            &file,
            harness.state_mut().shared_mut()
        ));
        harness.run_steps(3);
        assert!(shown(&harness, "Drones"));
        assert!(shown(&harness, "Kit (2)"));
        assert!(shown(&harness, "No group"));
        assert!(shown(&harness, "Weekly"));
        assert!(shown(&harness, "Half"));
        let mut store = store(&env);
        let weekly = store
            .stockpiles()
            .expect("the stockpiles")
            .into_iter()
            .find(|stockpile| stockpile.name == "Weekly")
            .expect("Weekly came in");
        assert_eq!(weekly.includes.len(), 1);
        assert_eq!(weekly.includes[0].count, 3);
        snapshot(&mut harness, "stockpiles_import");
    }

    #[test]
    fn the_claims_price_the_short_rows_and_the_buy_list() {
        let env = env();
        save_main(&env);
        kit(&env);
        let a = plan(&env, "Widget A", HOBGOBLIN_II, 8);
        let b = plan(&env, "Widget B", HOBGOBLIN_II, 1);
        assert!(a > 0 && b > a);

        let mut harness = start_with(&env, Box::new(Forge));
        loaded(&mut harness);
        fetch_market(&mut harness);
        click(&mut harness, "Kit");

        assert!(on_row(
            &harness,
            "Tritanium",
            &["60", "25", "25 · Widget A", "60", "300.00", "short"]
        ));
        assert!(on_row(
            &harness,
            "Hobgoblin I",
            &["10", "7", "7 · Widget A", "10", "—", "short"]
        ));
        assert!(shown(
            &harness,
            "Short value 300.00 (incomplete: 1 without price)"
        ));

        click(&mut harness, "Buy what is short");
        assert!(shown(&harness, "Buy what is short: Kit"));
        assert!(on_row(&harness, "Tritanium", &["60", "300.00"]));
        assert!(on_row(&harness, "Hobgoblin I", &["10", "—"]));
        harness.get_by_label("Copy multibuy").click_accesskit();
        harness.step();
        let first = copied_text(&harness);
        harness.step();
        let copied = first
            .or_else(|| copied_text(&harness))
            .expect("the multibuy text is copied");
        assert_eq!(copied, "Hobgoblin I\t10\nTritanium\t60");
    }

    #[test]
    fn the_have_column_adds_orders_and_only_the_unassigned_jobs() {
        let env = env();
        save_main_with(
            &env,
            vec![
                order(1, HOBGOBLIN_I, false, 4),
                order(2, TRITANIUM, true, 6),
            ],
            vec![
                job(501, HOBGOBLIN_I, 1, JobStatus::Active),
                job(503, HOBGOBLIN_I, 5, JobStatus::Ready),
            ],
        );
        kit_with_filters(&env, vec![filter(), main_only(Source::Jobs)]);
        let a = plan(&env, "Widget A", HOBGOBLIN_II, 8);
        let b = plan(&env, "Widget B", HOBGOBLIN_II, 1);
        assert!(a > 0 && b > a);
        store(&env)
            .assign_job(501, b, NOW)
            .expect("job 501 is assigned to B");
        let orders = stockpile(
            &env,
            "Order pile",
            None,
            &[(HOBGOBLIN_I, 99), (TRITANIUM, 88)],
            vec![orders_filter()],
        );
        assert!(orders > 0);

        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        assert!(on_row(&harness, "Hobgoblin I", &["12", "7 · Widget A"]));
        let job_hover = hover(&mut harness, "12");
        assert!(
            job_hover
                .iter()
                .any(|line| line == "Assets 7 · Sell orders 0 · Buy orders 0 · Jobs 5"),
            "{job_hover:?}"
        );

        click(&mut harness, "Order pile");
        assert!(on_row(&harness, "Hobgoblin I", &["99", "4"]));
        assert!(on_row(&harness, "Tritanium", &["88", "6"]));
        let sell_hover = hover(&mut harness, "4");
        assert!(
            sell_hover
                .iter()
                .any(|line| line == "Assets 0 · Sell orders 4 · Buy orders 0 · Jobs 0"),
            "{sell_hover:?}"
        );
        let buy_hover = hover(&mut harness, "6");
        assert!(
            buy_hover
                .iter()
                .any(|line| line == "Assets 0 · Sell orders 0 · Buy orders 6 · Jobs 0"),
            "{buy_hover:?}"
        );
    }

    #[test]
    fn the_after_plans_snapshot_is_taken() {
        let env = env();
        save_main(&env);
        kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        snapshot(&mut harness, "stockpiles_after_plans");
    }

    #[test]
    fn the_right_now_snapshot_is_taken() {
        let env = env();
        save_main(&env);
        kit(&env);
        let mut harness = start(&env);
        loaded(&mut harness);
        click(&mut harness, "Kit");
        click(&mut harness, "Full after my plans");
        click(&mut harness, "Full right now");
        snapshot(&mut harness, "stockpiles_right_now");
    }
}
