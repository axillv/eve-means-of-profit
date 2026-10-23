//! Character data against a scripted EVE: canned answers through a fake `Http`, a fixed clock.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::fs;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use mop_core::money::Money;
    use mop_core::stock::Asset;
    use mop_esi::auth::{Auth, TOKEN_URL};
    use mop_esi::character_data::{
        CharacterData, Job, JobStatus, Kind, Order, Refreshed, refresh, refresh_all,
    };
    use mop_esi::characters::{Character, CharacterList, Login};
    use mop_esi::client::{BASE_URL, Client};
    use mop_esi::error::EsiError;
    use mop_esi::http::{Http, HttpRequest, HttpResponse};
    use mop_esi::saved::{forget, load_character, save_character};
    use mop_esi::secrets::{FileStore, SecretStore, Secrets};
    use mop_esi::token::RefreshToken;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const CHARACTER: u64 = 2_112_625_428;
    const OTHER: u64 = 95_465_499;
    const NOW: i64 = 1_792_404_100;
    const SECRET: &str = "TOKEN-SECRET-0001";
    const MODIFIED: &str = "Mon, 19 Oct 2026 10:00:00 GMT";
    const MODIFIED_LATER: &str = "Mon, 19 Oct 2026 10:00:05 GMT";
    const EXPIRES: &str = "Mon, 19 Oct 2026 11:00:00 GMT";
    const PAGE_ONE: &str = r#"[{"is_singleton":false,"item_id":1040000000001,"location_flag":"Hangar","location_id":60003760,"location_type":"station","quantity":120,"type_id":34},{"is_singleton":true,"item_id":1040000000002,"location_flag":"Hangar","location_id":60003760,"location_type":"station","quantity":1,"type_id":17366}]"#;
    const PAGE_TWO: &str = r#"[{"is_singleton":false,"item_id":1040000000003,"location_flag":"Cargo","location_id":1040000000002,"location_type":"item","quantity":5,"type_id":2454}]"#;
    const NAMES: &str = r#"[{"item_id":1040000000002,"name":"Hauler One"}]"#;
    const ORDER_FULL: &str = r#"{"duration":90,"escrow":1234567.89,"is_buy_order":true,"is_corporation":false,"issued":"2026-10-01T12:00:00Z","location_id":60003760,"min_volume":1,"order_id":6800000001,"price":478.6,"range":"station","region_id":10000002,"type_id":11481,"volume_remain":40,"volume_total":100}"#;
    const ORDER_BARE: &str = r#"{"duration":90,"is_corporation":false,"issued":"2026-10-01T12:00:00Z","location_id":60003760,"order_id":6800000001,"price":478.6,"range":"station","region_id":10000002,"type_id":11481,"volume_remain":40,"volume_total":100}"#;
    const JOB: &str = r#"{"activity_id":1,"blueprint_id":1040000000009,"blueprint_location_id":60003760,"blueprint_type_id":2048,"cost":110080.0,"duration":3600,"end_date":"2026-10-19T10:30:00Z","facility_id":60003760,"installer_id":2112625428,"job_id":500000001,"output_location_id":60003760,"product_type_id":2047,"runs":10,"start_date":"2026-10-19T09:30:00Z","station_id":60003760,"status":"active"}"#;

    type Reply = HttpResponse;

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers from a queue and records every request. Token requests are answered apart from the queue.
    #[derive(Clone, Default)]
    struct Fake {
        replies: Arc<Mutex<VecDeque<Reply>>>,
        calls: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl Fake {
        fn esi_calls(&self) -> Vec<HttpRequest> {
            locked(&self.calls)
                .iter()
                .filter(|request| request.url.starts_with(BASE_URL))
                .cloned()
                .collect()
        }
    }

    impl Http for Fake {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, EsiError> {
            locked(&self.calls).push(request.clone());
            if request.url.starts_with(TOKEN_URL) {
                let body = json!({
                    "access_token": SECRET,
                    "expires_in": 1199,
                    "token_type": "Bearer",
                    "refresh_token": "REFRESH-2",
                });
                return Ok(reply(200, &[], &body.to_string()));
            }
            locked(&self.replies)
                .pop_front()
                .ok_or_else(|| EsiError::Network("no canned answer".to_owned()))
        }
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn ok(headers: &[(&str, &str)], body: &str) -> Reply {
        reply(200, headers, body)
    }

    fn failed(status: u16, headers: &[(&str, &str)]) -> Reply {
        reply(status, headers, "")
    }

    struct Setup {
        dir: TempDir,
        fake: Fake,
        auth: Auth,
    }

    fn setup(replies: Vec<Reply>) -> Setup {
        let dir = TempDir::new().expect("temp dir");
        let fake = Fake {
            replies: Arc::new(Mutex::new(replies.into())),
            calls: Arc::default(),
        };
        for id in [CHARACTER, OTHER] {
            FileStore::new(dir.path())
                .set(id, &RefreshToken::new("REFRESH-1"))
                .expect("store token");
        }
        let mut list = CharacterList::open(dir.path()).expect("open list");
        for id in [CHARACTER, OTHER] {
            list.upsert(Character {
                id,
                name: format!("Pilot {id}"),
                scopes: BTreeSet::new(),
                added_at: NOW,
                login: Login::Ok,
            })
            .expect("add character");
        }
        let secrets = Secrets {
            store: Box::new(FileStore::new(dir.path())),
            fallback: None,
        };
        let auth =
            Auth::open(dir.path(), Box::new(fake.clone()), secrets, "client-id").expect("open");
        Setup { dir, fake, auth }
    }

    /// Two pages of assets and the names answer, the pages carrying `second_modified` on page 2.
    fn asset_run(second_modified: &str) -> Vec<Reply> {
        vec![
            ok(
                &[
                    ("x-pages", "2"),
                    ("last-modified", MODIFIED),
                    ("expires", EXPIRES),
                ],
                PAGE_ONE,
            ),
            ok(&[("last-modified", second_modified)], PAGE_TWO),
        ]
    }

    fn assets_and_names() -> Vec<Reply> {
        let mut replies = asset_run(MODIFIED);
        replies.push(ok(&[], NAMES));
        replies
    }

    fn run(
        setup: &mut Setup,
        data: &mut CharacterData,
        kind: Kind,
        now: i64,
    ) -> Result<Refreshed, EsiError> {
        let mut client = Client::new(&setup.fake);
        refresh(data, kind, &mut setup.auth, &mut client, now)
    }

    fn header<'r>(request: &'r HttpRequest, name: &str) -> Option<&'r str> {
        request
            .headers
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, value)| value.as_str())
    }

    fn money(ten_thousandths: i64) -> Money {
        Money::from_ten_thousandths(ten_thousandths)
    }

    fn bad_field(result: Result<Refreshed, EsiError>) -> &'static str {
        match result {
            Err(EsiError::BadData { field, .. }) => field,
            other => panic!("expected BadData, got {other:?}"),
        }
    }

    fn hangar(item_id: i64, type_id: u32, quantity: u64, singleton: bool) -> Asset {
        Asset {
            item_id,
            type_id,
            location_id: 60_003_760,
            flag: Some("Hangar".to_owned()),
            quantity,
            singleton,
        }
    }

    #[test]
    fn assets_from_two_pages_with_names() {
        let mut setup = setup(assets_and_names());
        let mut data = CharacterData::new(CHARACTER);
        let outcome = run(&mut setup, &mut data, Kind::Assets, NOW).expect("assets");
        assert_eq!(outcome, Refreshed::Fetched);
        let held = data.assets.expect("assets held");
        assert_eq!(held.fetched_at, NOW);
        assert_eq!(held.last_modified, Some(1_792_404_000));
        assert_eq!(held.expires, Some(1_792_407_600));
        assert_eq!(held.data.assets.updated, Some(1_792_404_000));
        assert_eq!(held.data.assets.character_id, 2_112_625_428);
        let cargo = Asset {
            item_id: 1_040_000_000_003,
            type_id: 2454,
            location_id: 1_040_000_000_002,
            flag: Some("Cargo".to_owned()),
            quantity: 5,
            singleton: false,
        };
        assert_eq!(
            held.data.assets.assets,
            vec![
                hangar(1_040_000_000_001, 34, 120, false),
                hangar(1_040_000_000_002, 17_366, 1, true),
                cargo
            ]
        );
        assert_eq!(held.data.names.len(), 1);
        assert_eq!(
            held.data.names.get(&1_040_000_000_002).map(String::as_str),
            Some("Hauler One")
        );
        let calls = setup.fake.esi_calls();
        let urls: Vec<&str> = calls.iter().map(|call| call.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://esi.evetech.net/characters/2112625428/assets?page=1",
                "https://esi.evetech.net/characters/2112625428/assets?page=2",
                "https://esi.evetech.net/characters/2112625428/assets/names",
            ]
        );
        for call in &calls {
            assert!(
                header(call, "Authorization").is_some_and(|value| value.starts_with("Bearer "))
            );
            assert_eq!(header(call, "X-Compatibility-Date"), Some("2025-12-16"));
            assert!(!call.url.contains("/universe/"));
        }
        let names_request = calls.last().expect("names request");
        assert_eq!(
            names_request.body.as_deref(),
            Some(b"[1040000000002]".as_slice())
        );
    }

    #[test]
    fn names_answered_503_leave_assets_unnamed() {
        let mut replies = asset_run(MODIFIED);
        replies.push(failed(503, &[]));
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        let outcome = run(&mut setup, &mut data, Kind::Assets, NOW).expect("assets");
        assert_eq!(outcome, Refreshed::Fetched);
        let held = data.assets.expect("assets held");
        assert_eq!(held.data.assets.assets.len(), 3);
        assert!(held.data.names.is_empty());
    }

    #[test]
    fn a_page_that_changed_repeats_the_run_once() {
        let mut replies = asset_run(MODIFIED_LATER);
        replies.extend(asset_run(MODIFIED));
        replies.push(ok(&[], NAMES));
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        let outcome = run(&mut setup, &mut data, Kind::Assets, NOW).expect("assets");
        assert_eq!(outcome, Refreshed::Fetched);
        let pages = setup
            .fake
            .esi_calls()
            .iter()
            .filter(|call| call.url.contains("/assets?page="))
            .count();
        assert_eq!(pages, 4);
        assert_eq!(data.assets.expect("held").data.assets.assets.len(), 3);
    }

    #[test]
    fn pages_that_keep_changing_are_inconsistent_and_keep_the_old_assets() {
        let mut replies = assets_and_names();
        replies.extend(asset_run(MODIFIED_LATER));
        replies.extend(asset_run(MODIFIED_LATER));
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Assets, NOW).expect("first fetch");
        let before = data.assets.clone();
        let calls_before = setup.fake.esi_calls().len();
        let result = run(&mut setup, &mut data, Kind::Assets, 1_792_407_600);
        assert!(matches!(result, Err(EsiError::Inconsistent { .. })));
        assert_eq!(setup.fake.esi_calls().len() - calls_before, 4);
        assert_eq!(data.assets, before);
    }

    #[test]
    fn assets_are_fetched_again_only_when_due() {
        let mut replies = assets_and_names();
        replies.extend(assets_and_names());
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Assets, NOW).expect("first fetch");
        let calls = setup.fake.esi_calls().len();
        let early = run(&mut setup, &mut data, Kind::Assets, 1_792_407_599).expect("early");
        assert_eq!(
            early,
            Refreshed::NotYet {
                until: 1_792_407_600
            }
        );
        assert_eq!(setup.fake.esi_calls().len(), calls);
        let due = run(&mut setup, &mut data, Kind::Assets, 1_792_407_600).expect("due");
        assert_eq!(due, Refreshed::Fetched);
        assert_eq!(setup.fake.esi_calls().len(), calls * 2);
    }

    #[test]
    fn assets_without_an_expiry_header_wait_an_hour() {
        let replies = vec![ok(&[("last-modified", MODIFIED)], PAGE_TWO)];
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Assets, NOW).expect("assets");
        let held = data.assets.as_ref().expect("held");
        assert_eq!(held.expires, None);
        assert_eq!(held.next_refresh(3_600).expect("next"), 1_792_407_700);
        let early = run(&mut setup, &mut data, Kind::Assets, 1_792_407_699).expect("early");
        assert_eq!(
            early,
            Refreshed::NotYet {
                until: 1_792_407_700
            }
        );
    }

    #[test]
    fn orders_are_read_exactly() {
        let body = format!("[{ORDER_FULL},{ORDER_BARE}]");
        let mut setup = setup(vec![ok(&[("expires", EXPIRES)], &body)]);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Orders, NOW).expect("orders");
        let held = data.orders.expect("orders held");
        assert_eq!(held.expires, Some(1_792_407_600));
        let full = Order {
            order_id: 6_800_000_001,
            type_id: 11_481,
            location_id: 60_003_760,
            region_id: 10_000_002,
            is_buy: true,
            price: money(4_786_000),
            volume_remain: 40,
            volume_total: 100,
            issued: 1_790_856_000,
            expires: 1_798_632_000,
            range: "station".to_owned(),
            escrow: Some(money(12_345_678_900)),
            min_volume: Some(1),
            is_corporation: false,
        };
        let bare = Order {
            is_buy: false,
            escrow: None,
            min_volume: None,
            ..full.clone()
        };
        assert_eq!(held.data, vec![full, bare]);
        let calls = setup.fake.esi_calls();
        assert_eq!(
            calls.last().map(|call| call.url.as_str()),
            Some("https://esi.evetech.net/characters/2112625428/orders")
        );
    }

    #[test]
    fn bad_order_values_are_refused() {
        let cases = [
            ("\"price\":478.6", "\"price\":\"abc\"", "price"),
            ("\"price\":478.6", "\"price\":1e400", "price"),
            ("\"price\":478.6", "\"price\":478.60001", "price"),
            (
                "\"volume_remain\":40",
                "\"volume_remain\":-1",
                "volume_remain",
            ),
            ("\"type_id\":11481", "\"type_id\":4294967296", "type_id"),
        ];
        for (from, to, field) in cases {
            let body = format!("[{}]", ORDER_BARE.replace(from, to));
            let mut setup = setup(vec![ok(&[], &body)]);
            let mut data = CharacterData::new(CHARACTER);
            let result = run(&mut setup, &mut data, Kind::Orders, NOW);
            assert_eq!(bad_field(result), field, "{to}");
            assert_eq!(data.orders, None);
        }
    }

    #[test]
    fn jobs_are_read_with_finished_ones() {
        let body = format!("[{JOB}]");
        let mut setup = setup(vec![ok(&[], &body)]);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Jobs, NOW).expect("jobs");
        let held = data.jobs.expect("jobs held");
        assert_eq!(
            held.data,
            vec![Job {
                job_id: 500_000_001,
                activity_id: 1,
                blueprint_type_id: 2048,
                product_type_id: Some(2047),
                runs: 10,
                status: JobStatus::Active,
                start: 1_792_402_200,
                end: 1_792_405_800,
                facility_id: 60_003_760,
                station_id: 60_003_760,
                output_location_id: 60_003_760,
                installer_id: 2_112_625_428,
                cost: Some(money(1_100_800_000)),
                licensed_runs: None,
            }]
        );
        let calls = setup.fake.esi_calls();
        assert!(
            calls
                .iter()
                .all(|call| call.url.ends_with("/industry/jobs?include_completed=true"))
        );
    }

    #[test]
    fn bad_job_values_are_refused() {
        let cases = [
            ("\"status\":\"active\"", "\"status\":\"unknown\"", "status"),
            ("\"runs\":10", "\"runs\":-1", "runs"),
        ];
        for (from, to, field) in cases {
            let body = format!("[{}]", JOB.replace(from, to));
            let mut setup = setup(vec![ok(&[], &body)]);
            let mut data = CharacterData::new(CHARACTER);
            let result = run(&mut setup, &mut data, Kind::Jobs, NOW);
            assert_eq!(bad_field(result), field, "{to}");
        }
    }

    #[test]
    fn wallet_is_read_exactly_and_can_be_negative() {
        for (body, expected) in [
            ("123456789.12", 1_234_567_891_200),
            ("-5000.5", -50_005_000),
        ] {
            let mut setup = setup(vec![ok(&[], body)]);
            let mut data = CharacterData::new(CHARACTER);
            run(&mut setup, &mut data, Kind::Wallet, NOW).expect("wallet");
            assert_eq!(data.wallet.expect("held").data, money(expected));
        }
    }

    /// A wallet fetched at `NOW`, then `answer` to a second fetch that is due.
    fn second_wallet_error(answer: Reply) -> EsiError {
        let mut setup = setup(vec![ok(&[], "123456789.12"), answer]);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Wallet, NOW).expect("first wallet");
        let before = data.wallet.clone();
        let error = run(&mut setup, &mut data, Kind::Wallet, NOW + 200).expect_err("second wallet");
        assert_eq!(data.wallet, before);
        error
    }

    #[test]
    fn http_errors_map_to_their_variants() {
        assert!(matches!(
            second_wallet_error(failed(403, &[])),
            EsiError::Forbidden { route } if route == "/characters/2112625428/wallet"
        ));
        assert!(matches!(
            second_wallet_error(failed(401, &[])),
            EsiError::Unexpected { status: 401, .. }
        ));
        assert!(matches!(
            second_wallet_error(failed(404, &[])),
            EsiError::NotFound { .. }
        ));
        assert!(matches!(
            second_wallet_error(failed(503, &[])),
            EsiError::Server(503)
        ));
        assert!(matches!(
            second_wallet_error(failed(418, &[])),
            EsiError::Unexpected { status: 418, .. }
        ));
        assert!(matches!(
            second_wallet_error(failed(304, &[])),
            EsiError::Unexpected { status: 304, .. }
        ));
        assert!(matches!(
            second_wallet_error(failed(429, &[("retry-after", "30")])),
            EsiError::RateLimited {
                retry_after_seconds: 30
            }
        ));
        assert!(matches!(
            second_wallet_error(failed(420, &[("x-esi-error-limit-reset", "41")])),
            EsiError::ErrorLimited { reset_seconds: 41 }
        ));
    }

    #[test]
    fn a_429_without_retry_after_waits_a_minute_for_that_kind_only() {
        let mut setup = setup(vec![failed(429, &[]), ok(&[], "5")]);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        let result = refresh(&mut data, Kind::Orders, &mut setup.auth, &mut client, NOW);
        assert!(matches!(
            result,
            Err(EsiError::RateLimited {
                retry_after_seconds: 60
            })
        ));
        let again = refresh(
            &mut data,
            Kind::Orders,
            &mut setup.auth,
            &mut client,
            NOW + 10,
        );
        assert!(matches!(
            again,
            Err(EsiError::RateLimited {
                retry_after_seconds: 50
            })
        ));
        assert_eq!(setup.fake.esi_calls().len(), 1);
        refresh(
            &mut data,
            Kind::Wallet,
            &mut setup.auth,
            &mut client,
            NOW + 10,
        )
        .expect("wallet is not held back");
    }

    #[test]
    fn an_error_limit_stops_the_remaining_kinds() {
        let mut setup = setup(vec![failed(420, &[("x-esi-error-limit-reset", "41")])]);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        let results = refresh_all(&mut data, &mut setup.auth, &mut client, NOW);
        let kinds: Vec<Kind> = results.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, Kind::ALL);
        for (kind, result) in &results {
            assert!(
                matches!(result, Err(EsiError::ErrorLimited { reset_seconds: 41 })),
                "{kind:?}"
            );
        }
        assert_eq!(setup.fake.esi_calls().len(), 1);
        let mut other = CharacterData::new(OTHER);
        let result = refresh(
            &mut other,
            Kind::Wallet,
            &mut setup.auth,
            &mut client,
            NOW + 1,
        );
        assert!(matches!(
            result,
            Err(EsiError::ErrorLimited { reset_seconds: 40 })
        ));
    }

    #[test]
    fn refresh_all_fetches_every_kind_in_order() {
        let replies = vec![ok(&[], "[]"), ok(&[], "[]"), ok(&[], "[]"), ok(&[], "10.5")];
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        let results = refresh_all(&mut data, &mut setup.auth, &mut client, NOW);
        assert_eq!(results.len(), 4);
        for (_, result) in &results {
            assert!(matches!(result, Ok(Refreshed::Fetched)));
        }
        let calls = setup.fake.esi_calls();
        let urls: Vec<&str> = calls.iter().map(|call| call.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://esi.evetech.net/characters/2112625428/assets?page=1",
                "https://esi.evetech.net/characters/2112625428/orders",
                "https://esi.evetech.net/characters/2112625428/industry/jobs?include_completed=true",
                "https://esi.evetech.net/characters/2112625428/wallet",
            ]
        );
    }

    #[test]
    fn few_error_allowances_left_hold_the_next_request() {
        let mut setup = setup(vec![ok(&[("x-esi-error-limit-remain", "9")], "1")]);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        refresh(&mut data, Kind::Wallet, &mut setup.auth, &mut client, NOW).expect("wallet");
        let result = refresh(&mut data, Kind::Orders, &mut setup.auth, &mut client, NOW);
        assert!(matches!(
            result,
            Err(EsiError::ErrorLimited { reset_seconds: 60 })
        ));
        assert_eq!(setup.fake.esi_calls().len(), 1);
    }

    #[test]
    fn ten_error_allowances_left_let_the_next_request_go() {
        let replies = vec![
            ok(&[("x-esi-error-limit-remain", "10")], "1"),
            ok(&[], "[]"),
        ];
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        refresh(&mut data, Kind::Wallet, &mut setup.auth, &mut client, NOW).expect("wallet");
        refresh(&mut data, Kind::Orders, &mut setup.auth, &mut client, NOW).expect("orders");
        assert_eq!(setup.fake.esi_calls().len(), 2);
    }

    fn saved_path(setup: &Setup) -> std::path::PathBuf {
        setup.dir.path().join("data-2112625428.json")
    }

    fn assets_and_wallet() -> (Setup, CharacterData) {
        let mut replies = assets_and_names();
        replies.push(ok(&[], "123456789.12"));
        let mut setup = setup(replies);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Assets, NOW).expect("assets");
        run(&mut setup, &mut data, Kind::Wallet, NOW).expect("wallet");
        (setup, data)
    }

    #[test]
    fn saved_data_loads_back_equal_with_integer_money() {
        let (setup, data) = assets_and_wallet();
        save_character(setup.dir.path(), &data).expect("save");
        let loaded = load_character(setup.dir.path(), CHARACTER).expect("load");
        assert_eq!(loaded, Some(data));
        let text = fs::read_to_string(saved_path(&setup)).expect("file");
        let file: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(file["version"], 1);
        assert_eq!(file["wallet"]["data"], json!(1_234_567_891_200_i64));
        assert!(text.contains("1234567891200"));
        assert!(!text.contains('.'));
    }

    #[test]
    fn saved_orders_and_jobs_keep_money_as_integers() {
        let orders = format!("[{ORDER_FULL}]");
        let jobs = format!("[{JOB}]");
        let mut setup = setup(vec![ok(&[], &orders), ok(&[], &jobs)]);
        let mut data = CharacterData::new(CHARACTER);
        run(&mut setup, &mut data, Kind::Orders, NOW).expect("orders");
        run(&mut setup, &mut data, Kind::Jobs, NOW).expect("jobs");
        save_character(setup.dir.path(), &data).expect("save");
        let text = fs::read_to_string(saved_path(&setup)).expect("file");
        assert!(!text.contains('.'));
        assert!(text.contains("12345678900"));
        assert!(text.contains("1100800000"));
        assert_eq!(
            load_character(setup.dir.path(), CHARACTER).expect("load"),
            Some(data)
        );
    }

    #[test]
    fn unreadable_saved_files_are_bad_and_left_alone() {
        let (setup, data) = assets_and_wallet();
        for content in [r#"{"version":2,"character_id":2112625428}"#, "{"] {
            fs::write(saved_path(&setup), content).expect("write");
            let result = load_character(setup.dir.path(), CHARACTER);
            assert!(matches!(result, Err(EsiError::BadFile { .. })), "{content}");
            assert_eq!(
                fs::read_to_string(saved_path(&setup)).expect("read"),
                content
            );
        }
        assert_eq!(
            load_character(setup.dir.path(), OTHER).expect("missing"),
            None
        );
        save_character(setup.dir.path(), &data).expect("overwrite");
        assert!(
            load_character(setup.dir.path(), CHARACTER)
                .expect("load")
                .is_some()
        );
    }

    #[test]
    fn forget_deletes_the_file_and_repeats_quietly() {
        let (setup, data) = assets_and_wallet();
        save_character(setup.dir.path(), &data).expect("save");
        forget(setup.dir.path(), CHARACTER).expect("forget");
        assert!(!saved_path(&setup).exists());
        forget(setup.dir.path(), CHARACTER).expect("forget again");
    }

    #[test]
    fn a_failed_fetch_leaves_the_saved_file_as_it_was() {
        let (mut setup, mut data) = assets_and_wallet();
        save_character(setup.dir.path(), &data).expect("save");
        let before = fs::read(saved_path(&setup)).expect("read");
        locked(&setup.fake.replies).push_back(failed(503, &[]));
        let result = run(&mut setup, &mut data, Kind::Wallet, NOW + 200);
        assert!(matches!(result, Err(EsiError::Server(503))));
        assert_eq!(fs::read(saved_path(&setup)).expect("read"), before);
    }

    #[test]
    fn the_access_token_never_appears_in_urls_errors_or_debug_output() {
        let (mut setup, mut data) = assets_and_wallet();
        locked(&setup.fake.replies).push_back(failed(403, &[]));
        let error = run(&mut setup, &mut data, Kind::Wallet, NOW + 200).expect_err("forbidden");
        let mut texts = vec![
            format!("{error}"),
            format!("{error:?}"),
            format!("{data:?}"),
        ];
        texts.extend(setup.fake.esi_calls().iter().map(|call| call.url.clone()));
        for text in texts {
            assert!(!text.contains(SECRET), "{text}");
        }
    }

    fn issued_of(text: &str) -> Result<i64, &'static str> {
        let body = format!("[{}]", ORDER_BARE.replace("2026-10-01T12:00:00Z", text));
        let mut setup = setup(vec![ok(&[], &body)]);
        let mut data = CharacterData::new(CHARACTER);
        match run(&mut setup, &mut data, Kind::Orders, NOW) {
            Ok(_) => data
                .orders
                .and_then(|held| held.data.first().map(|order| order.issued))
                .ok_or("no order"),
            Err(EsiError::BadData { field, .. }) => Err(field),
            Err(_) => Err("other error"),
        }
    }

    #[test]
    fn times_are_read_at_their_limits_and_refused_when_malformed() {
        let good = [
            ("2026-01-01T00:00:00Z", 1_767_225_600),
            ("2026-12-31T23:59:59Z", 1_798_761_599),
            ("2026-10-19T12:34:56Z", 1_792_413_296),
            ("2024-02-29T00:00:00Z", 1_709_164_800),
            ("2000-02-29T00:00:00Z", 951_782_400),
        ];
        for (text, seconds) in good {
            assert_eq!(issued_of(text), Ok(seconds), "{text}");
        }
        let bad = [
            "2026-00-10T12:00:00Z",
            "2026-13-10T12:00:00Z",
            "2026-10-00T12:00:00Z",
            "2026-10-32T12:00:00Z",
            "2026-02-31T12:00:00Z",
            "2026-02-29T12:00:00Z",
            "2026-04-31T12:00:00Z",
            "1900-02-29T12:00:00Z",
            "2026-10-10T24:00:00Z",
            "2026-10-10T12:60:00Z",
            "2026-10-10T12:00:60Z",
            "2026-10-10 12:00:00Z",
            "2026/10/10T12:00:00Z",
            "2026-10-10T12-00:00Z",
            "2026-10-10T12:00-00Z",
            "2026-1a-10T12:00:00Z",
            "2026-10-10T12:00:00+0",
            "2026-10-10T12:00:00",
            "2026-10-10T12:00:00X",
            "2026-10-10T12:00:00Zx",
            "",
        ];
        for text in bad {
            assert_eq!(issued_of(text), Err("issued"), "{text}");
        }
    }

    #[test]
    fn bad_data_keeps_the_text_as_sent() {
        for (to, text) in [("\"abc\"", "abc"), ("-1", "-1")] {
            let body = format!(
                "[{}]",
                ORDER_BARE.replace("\"price\":478.6", &format!("\"price\":{to}"))
            );
            let mut setup = setup(vec![ok(&[], &body)]);
            let mut data = CharacterData::new(CHARACTER);
            let result = run(&mut setup, &mut data, Kind::Orders, NOW);
            if to == "-1" {
                assert!(result.is_ok(), "a negative price is still a price");
            } else {
                assert!(matches!(
                    result,
                    Err(EsiError::BadData { text: ref found, .. }) if found == text
                ));
            }
        }
        let mut setup = setup(vec![ok(
            &[],
            &ORDER_BARE
                .replace("\"duration\":90", "\"duration\":-1")
                .replacen("{", "[{", 1)
                .replacen("}", "}]", 1),
        )]);
        let mut data = CharacterData::new(CHARACTER);
        let result = run(&mut setup, &mut data, Kind::Orders, NOW);
        assert!(matches!(
            result,
            Err(EsiError::BadData { field: "duration", text, .. }) if text == "-1"
        ));
    }

    #[test]
    fn an_answer_that_is_not_json_is_bad_data_with_its_start() {
        let long = "x".repeat(200);
        for (body, expected) in [
            ("not json", "not json".to_owned()),
            (long.as_str(), "x".repeat(80)),
        ] {
            let mut setup = setup(vec![ok(&[], body)]);
            let mut data = CharacterData::new(CHARACTER);
            let result = run(&mut setup, &mut data, Kind::Wallet, NOW);
            assert!(matches!(
                result,
                Err(EsiError::BadData { field: "body", text, .. }) if text == expected
            ));
        }
    }

    #[test]
    fn a_pause_ends_at_its_time() {
        let mut setup = setup(vec![failed(429, &[("retry-after", "30")]), ok(&[], "7")]);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        refresh(&mut data, Kind::Wallet, &mut setup.auth, &mut client, NOW).expect_err("limited");
        let held = refresh(
            &mut data,
            Kind::Wallet,
            &mut setup.auth,
            &mut client,
            NOW + 29,
        );
        assert!(matches!(
            held,
            Err(EsiError::RateLimited {
                retry_after_seconds: 1
            })
        ));
        refresh(
            &mut data,
            Kind::Wallet,
            &mut setup.auth,
            &mut client,
            NOW + 30,
        )
        .expect("free again");

        let mut setup = self::setup(vec![
            ok(
                &[
                    ("x-esi-error-limit-remain", "0"),
                    ("x-esi-error-limit-reset", "20"),
                ],
                "7",
            ),
            ok(&[], "[]"),
        ]);
        let mut data = CharacterData::new(CHARACTER);
        let mut client = Client::new(&setup.fake);
        refresh(&mut data, Kind::Wallet, &mut setup.auth, &mut client, NOW).expect("wallet");
        let held = refresh(
            &mut data,
            Kind::Orders,
            &mut setup.auth,
            &mut client,
            NOW + 19,
        );
        assert!(matches!(
            held,
            Err(EsiError::ErrorLimited { reset_seconds: 1 })
        ));
        refresh(
            &mut data,
            Kind::Orders,
            &mut setup.auth,
            &mut client,
            NOW + 20,
        )
        .expect("free again");
    }

    #[test]
    fn a_page_count_below_one_or_unreadable_is_refused() {
        for value in ["0", "two", "-1", "1001"] {
            let mut setup = setup(vec![ok(&[("x-pages", value)], "[]")]);
            let mut data = CharacterData::new(CHARACTER);
            let result = run(&mut setup, &mut data, Kind::Assets, NOW);
            assert_eq!(bad_field(result), "x-pages", "{value}");
        }
    }

    #[test]
    fn a_saved_file_that_cannot_be_read_or_removed_is_an_error() {
        let setup = setup(Vec::new());
        let path = setup.dir.path().join("data-95465499.json");
        fs::create_dir(&path).expect("directory in the way");
        let loaded = load_character(setup.dir.path(), OTHER);
        assert!(matches!(loaded, Err(EsiError::BadFile { .. })));
        let removed = forget(setup.dir.path(), OTHER);
        assert!(matches!(removed, Err(EsiError::Io { .. })));
    }
}
