//! Public industry data against canned answers: a fake `Http`, a fixed clock, temporary folders.

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use mop_core::industry::Activity;
    use mop_core::money::{Money, Rate};
    use mop_esi::character_data::Refreshed;
    use mop_esi::client::BASE_URL;
    use mop_esi::error::EsiError;
    use mop_esi::http::{Http, HttpRequest, HttpResponse};
    use mop_esi::industry::{
        IndustryData, IndustryKind, load_industry, refresh_industry, save_industry,
    };
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const NOW: i64 = 1_792_364_100;
    /// After both fixtures' expiry times.
    const LATER: i64 = 1_792_370_000;
    const SYSTEMS: &str = include_str!("../fixtures/industry/industry-systems-extract.json");
    const PRICES: &str = include_str!("../fixtures/industry/markets-prices-extract.json");
    const SYSTEMS_HEADERS: &[(&str, &str)] = &[
        ("last-modified", "Sun, 18 Oct 2026 22:53:55 GMT"),
        ("expires", "Sun, 18 Oct 2026 23:53:55 GMT"),
    ];
    const PRICES_HEADERS: &[(&str, &str)] = &[
        ("last-modified", "Sun, 18 Oct 2026 23:25:25 GMT"),
        ("expires", "Mon, 19 Oct 2026 00:25:25 GMT"),
    ];
    const JITA: u32 = 30_000_142;

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers from a queue and records every request.
    #[derive(Clone, Default)]
    struct Fake {
        replies: Arc<Mutex<VecDeque<HttpResponse>>>,
        calls: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl Fake {
        fn with(replies: Vec<HttpResponse>) -> Self {
            Self {
                replies: Arc::new(Mutex::new(replies.into())),
                calls: Arc::default(),
            }
        }

        fn requests(&self) -> Vec<HttpRequest> {
            locked(&self.calls).clone()
        }
    }

    impl Http for Fake {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, EsiError> {
            locked(&self.calls).push(request.clone());
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

    fn ok(headers: &[(&str, &str)], body: &str) -> HttpResponse {
        reply(200, headers, body)
    }

    fn refresh(data: &mut IndustryData, kind: IndustryKind, fake: &Fake, now: i64) -> Refreshed {
        refresh_industry(data, kind, fake, now).expect("refresh")
    }

    fn header<'r>(request: &'r HttpRequest, name: &str) -> Option<&'r str> {
        request
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Data with the systems fetched at `NOW`, the starting point of the refusal tests.
    fn held_systems() -> IndustryData {
        let fake = Fake::with(vec![ok(SYSTEMS_HEADERS, SYSTEMS)]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Systems, &fake, NOW);
        data
    }

    /// Data with the prices fetched at `NOW`, the starting point of the refusal tests.
    fn held_prices() -> IndustryData {
        let fake = Fake::with(vec![ok(PRICES_HEADERS, PRICES)]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Prices, &fake, NOW);
        data
    }

    #[test]
    fn systems_answer_is_read_with_its_times() {
        let fake = Fake::with(vec![ok(SYSTEMS_HEADERS, SYSTEMS)]);
        let mut data = IndustryData::default();
        assert_eq!(
            refresh(&mut data, IndustryKind::Systems, &fake, NOW),
            Refreshed::Fetched
        );
        let held = data.systems.as_ref().expect("systems held");
        assert_eq!(held.data.len(), 4);
        assert_eq!(held.fetched_at, NOW);
        assert_eq!(held.last_modified, Some(1_792_364_035));
        assert_eq!(held.expires, Some(1_792_367_635));

        let jita = held.data.get(&JITA).expect("Jita");
        assert_eq!(jita.manufacturing, Some(Rate::from_ppm(167_200)));
        assert_eq!(jita.reaction, Some(Rate::from_ppm(1_400)));
        assert_eq!(jita.invention, Some(Rate::from_ppm(1_400)));
        assert_eq!(jita.copying, Some(Rate::from_ppm(1_400)));
        assert_eq!(jita.research_me, Some(Rate::from_ppm(1_400)));
        assert_eq!(jita.research_te, Some(Rate::from_ppm(1_400)));

        let perimeter = held.data.get(&30_000_144).expect("Perimeter");
        assert_eq!(perimeter.manufacturing, Some(Rate::from_ppm(67_200)));
        assert_eq!(perimeter.research_te, Some(Rate::from_ppm(84_700)));
        assert_eq!(perimeter.research_me, Some(Rate::from_ppm(73_600)));
        assert_eq!(perimeter.copying, Some(Rate::from_ppm(54_200)));
        assert_eq!(perimeter.invention, Some(Rate::from_ppm(51_600)));
        assert_eq!(perimeter.reaction, Some(Rate::from_ppm(1_400)));

        let kiainti = held.data.get(&30_000_189).expect("Kiainti");
        assert_eq!(kiainti.manufacturing, Some(Rate::from_ppm(27_700)));

        assert_eq!(
            data.cost_index(JITA, Activity::Manufacturing),
            Some(Rate::from_ppm(167_200))
        );
        assert_eq!(
            data.cost_index(JITA, Activity::Reaction),
            Some(Rate::from_ppm(1_400))
        );
        assert_eq!(data.cost_index(30_000_001, Activity::Manufacturing), None);

        assert_eq!(data.invention_index(JITA), Some(Rate::from_ppm(1_400)));
        assert_eq!(data.invention_index(30_000_001), None);
    }

    #[test]
    fn the_invention_index_is_none_without_systems() {
        assert_eq!(IndustryData::default().invention_index(JITA), None);
    }

    #[test]
    fn the_systems_request_has_the_date_and_no_login() {
        let fake = Fake::with(vec![ok(SYSTEMS_HEADERS, SYSTEMS)]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Systems, &fake, NOW);
        let requests = fake.requests();
        let request = requests.first().expect("one request");
        assert_eq!(request.url, format!("{BASE_URL}/industry/systems"));
        assert_eq!(header(request, "X-Compatibility-Date"), Some("2025-12-16"));
        assert_eq!(header(request, "Authorization"), None);
    }

    #[test]
    fn prices_answer_is_read_exactly() {
        let data = held_prices();
        let held = data.prices.as_ref().expect("prices held");
        assert_eq!(held.data.len(), 8);
        assert_eq!(held.last_modified, Some(1_792_365_925));
        assert_eq!(held.expires, Some(1_792_369_525));

        let price = |type_id: u32| *held.data.get(&type_id).expect("type");
        assert_eq!(
            price(34).adjusted,
            Some(Money::from_ten_thousandths(31_391))
        );
        assert_eq!(price(34).average, Some(Money::from_ten_thousandths(37_300)));
        assert_eq!(
            price(2183).adjusted,
            Some(Money::from_ten_thousandths(209_641_068))
        );
        assert_eq!(
            price(2185).adjusted,
            Some(Money::from_ten_thousandths(5_580_624_295))
        );
        assert_eq!(
            price(11399).adjusted,
            Some(Money::from_ten_thousandths(171_660_204))
        );
    }

    #[test]
    fn a_price_with_only_an_average_has_no_adjusted_price() {
        let mut rows: Vec<Value> = serde_json::from_str(PRICES).expect("prices fixture");
        rows.push(json!({"type_id": 99, "average_price": 1.5}));
        let body = Value::Array(rows).to_string();
        let fake = Fake::with(vec![ok(PRICES_HEADERS, &body)]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Prices, &fake, NOW);

        let held = data.prices.as_ref().expect("prices held");
        let only_average = held.data.get(&99).expect("type 99");
        assert_eq!(only_average.adjusted, None);
        assert_eq!(
            only_average.average,
            Some(Money::from_ten_thousandths(15_000))
        );
        let adjusted = data.adjusted_prices();
        assert_eq!(adjusted.len(), 8);
        assert!(!adjusted.contains_key(&99));
    }

    #[test]
    fn refresh_waits_for_the_expiry_time() {
        let fake = Fake::with(vec![
            ok(SYSTEMS_HEADERS, SYSTEMS),
            ok(SYSTEMS_HEADERS, SYSTEMS),
        ]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Systems, &fake, 1_792_364_100);
        assert_eq!(
            refresh(&mut data, IndustryKind::Systems, &fake, 1_792_367_634),
            Refreshed::NotYet {
                until: 1_792_367_635
            }
        );
        assert_eq!(fake.requests().len(), 1);
        assert_eq!(
            refresh(&mut data, IndustryKind::Systems, &fake, 1_792_367_635),
            Refreshed::Fetched
        );
        assert_eq!(fake.requests().len(), 2);
    }

    #[test]
    fn without_an_expiry_time_the_data_is_due_an_hour_later() {
        let fake = Fake::with(vec![ok(&[], SYSTEMS), ok(&[], SYSTEMS)]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Systems, &fake, 1_000);
        assert_eq!(
            refresh(&mut data, IndustryKind::Systems, &fake, 4_599),
            Refreshed::NotYet { until: 4_600 }
        );
        assert_eq!(fake.requests().len(), 1);
        assert_eq!(
            refresh(&mut data, IndustryKind::Systems, &fake, 4_600),
            Refreshed::Fetched
        );
    }

    fn systems_refusal(body: &str) -> EsiError {
        let mut data = held_systems();
        let before = data.clone();
        let fake = Fake::with(vec![ok(SYSTEMS_HEADERS, body)]);
        let error =
            refresh_industry(&mut data, IndustryKind::Systems, &fake, LATER).expect_err("refused");
        assert_eq!(data, before);
        error
    }

    fn prices_refusal(body: &str) -> EsiError {
        let mut data = held_prices();
        let before = data.clone();
        let fake = Fake::with(vec![ok(PRICES_HEADERS, body)]);
        let error =
            refresh_industry(&mut data, IndustryKind::Prices, &fake, LATER).expect_err("refused");
        assert_eq!(data, before);
        error
    }

    #[test]
    fn a_cost_index_with_too_many_decimals_is_refused() {
        let body = SYSTEMS.replacen("0.1672", "0.12345678", 1);
        let error = systems_refusal(&body);
        assert!(matches!(
            &error,
            EsiError::BadData { field: "cost_index", text, .. } if text == "0.12345678"
        ));
    }

    #[test]
    fn a_cost_index_written_as_text_is_refused_with_the_text_as_sent() {
        let body = SYSTEMS.replacen(r#""cost_index": 0.1672"#, r#""cost_index": "0.1672""#, 1);
        let error = systems_refusal(&body);
        assert!(matches!(
            &error,
            EsiError::BadData { field: "cost_index", text, .. } if text == r#""0.1672""#
        ));
    }

    #[test]
    fn a_zero_price_is_kept() {
        let body = PRICES.replacen("3.1391360812857907", "0", 1);
        let fake = Fake::with(vec![ok(PRICES_HEADERS, &body)]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Prices, &fake, NOW);
        let price = data.prices.as_ref().expect("prices").data.get(&34);
        assert_eq!(price.and_then(|price| price.adjusted), Some(Money::ZERO));
    }

    #[test]
    fn an_unreadable_file_is_refused_not_taken_for_missing() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir(dir.path().join("industry.json")).expect("make a folder in its place");
        assert!(matches!(
            load_industry(dir.path()),
            Err(EsiError::BadFile { .. })
        ));
    }

    #[test]
    fn a_cost_index_above_one_is_refused() {
        let body = SYSTEMS.replacen("0.1672", "1.5", 1);
        assert!(matches!(
            systems_refusal(&body),
            EsiError::BadData {
                field: "cost_index",
                ..
            }
        ));
    }

    #[test]
    fn a_system_listed_twice_is_refused() {
        let mut rows: Vec<Value> = serde_json::from_str(SYSTEMS).expect("systems fixture");
        let first = rows.first().expect("a system").clone();
        rows.push(first);
        let body = Value::Array(rows).to_string();
        assert!(matches!(
            systems_refusal(&body),
            EsiError::BadData {
                field: "solar_system_id",
                ..
            }
        ));
    }

    #[test]
    fn an_activity_listed_twice_is_refused() {
        let body = SYSTEMS.replacen(
            r#"{"activity": "manufacturing", "cost_index": 0.1672}"#,
            r#"{"activity": "manufacturing", "cost_index": 0.1672}, {"activity": "manufacturing", "cost_index": 0.1672}"#,
            1,
        );
        assert!(matches!(
            systems_refusal(&body),
            EsiError::BadData {
                field: "activity",
                ..
            }
        ));
    }

    #[test]
    fn a_negative_adjusted_price_is_refused() {
        let body = PRICES.replacen("3.1391360812857907", "-1.0", 1);
        assert!(matches!(
            prices_refusal(&body),
            EsiError::BadData {
                field: "adjusted_price",
                ..
            }
        ));
    }

    #[test]
    fn an_unknown_activity_is_skipped() {
        let body = SYSTEMS.replacen(
            r#"{"activity": "manufacturing", "cost_index": 0.1672}"#,
            r#"{"activity": "mining", "cost_index": 0.5}, {"activity": "manufacturing", "cost_index": 0.1672}"#,
            1,
        );
        let fake = Fake::with(vec![ok(SYSTEMS_HEADERS, &body)]);
        let mut data = IndustryData::default();
        assert_eq!(
            refresh(&mut data, IndustryKind::Systems, &fake, NOW),
            Refreshed::Fetched
        );
        assert_eq!(
            data.cost_index(JITA, Activity::Manufacturing),
            Some(Rate::from_ppm(167_200))
        );
    }

    #[test]
    fn a_429_waits_for_retry_after_and_keeps_the_data() {
        let mut data = held_systems();
        let before = data.clone();
        let fake = Fake::with(vec![reply(429, &[("retry-after", "30")], "")]);
        assert!(matches!(
            refresh_industry(&mut data, IndustryKind::Systems, &fake, LATER),
            Err(EsiError::RateLimited {
                retry_after_seconds: 30
            })
        ));
        assert_eq!(data, before);
    }

    #[test]
    fn a_503_keeps_the_data() {
        let mut data = held_systems();
        let before = data.clone();
        let fake = Fake::with(vec![reply(503, &[], "")]);
        assert!(matches!(
            refresh_industry(&mut data, IndustryKind::Systems, &fake, LATER),
            Err(EsiError::Server(503))
        ));
        assert_eq!(data, before);
    }

    #[test]
    fn saved_data_comes_back_equal_and_holds_integers_only() {
        let dir = TempDir::new().expect("temp dir");
        let fake = Fake::with(vec![
            ok(SYSTEMS_HEADERS, SYSTEMS),
            ok(PRICES_HEADERS, PRICES),
        ]);
        let mut data = IndustryData::default();
        refresh(&mut data, IndustryKind::Systems, &fake, NOW);
        save_industry(dir.path(), &data).expect("save systems");
        refresh(&mut data, IndustryKind::Prices, &fake, NOW);
        save_industry(dir.path(), &data).expect("save prices");

        assert_eq!(load_industry(dir.path()).expect("load"), data);
        let text = fs::read_to_string(dir.path().join("industry.json")).expect("saved file");
        assert!(text.contains(r#""manufacturing":167200"#));
        assert!(text.contains(r#""adjusted":31391"#));
        assert!(!text.contains('.'));
    }

    #[test]
    fn another_version_is_refused_and_the_file_is_left_alone() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("industry.json");
        fs::write(&path, r#"{"version":2,"systems":null,"prices":null}"#).expect("write");
        let before = fs::read(&path).expect("read");
        assert!(matches!(
            load_industry(dir.path()),
            Err(EsiError::BadFile { .. })
        ));
        assert_eq!(fs::read(&path).expect("read"), before);
    }

    #[test]
    fn a_missing_file_gives_empty_data() {
        let dir = TempDir::new().expect("temp dir");
        assert_eq!(
            load_industry(dir.path()).expect("load"),
            IndustryData::default()
        );
    }

    #[test]
    fn saved_money_is_the_same_after_a_round_trip() {
        let dir = TempDir::new().expect("temp dir");
        let mut data = held_prices();
        save_industry(dir.path(), &data).expect("save");
        data = load_industry(dir.path()).expect("load");
        let price = data.prices.as_ref().expect("prices").data.get(&34).copied();
        assert_eq!(
            price.and_then(|price| price.adjusted),
            Some(Money::from_ten_thousandths(31_391))
        );
    }
}
