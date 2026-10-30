//! A character's wallet journal and market transactions from ESI, read through every page EVE
//! serves. Nothing is saved here: the store keeps each row by EVE's id.

use std::collections::BTreeSet;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use serde_json::Value;

use crate::character_data::{Fetched, array, fetched, field, utc_seconds};
use crate::client::{Answer, Client, bad_data};
use crate::error::EsiError;
use crate::token::AccessToken;

/// The most pages a list may claim or a transaction walk may make.
const MAX_PAGES: u32 = 1_000;
const JOURNAL: &str = "journal";
const TRANSACTIONS: &str = "transactions";

/// One line of the wallet journal, as ESI writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: i64,
    pub at: i64,
    pub ref_type: String,
    pub ref_id: Option<i64>,
    pub first_party_id: Option<i64>,
    pub second_party_id: Option<i64>,
    pub amount: Option<Money>,
    pub balance: Option<Money>,
    pub reason: Option<String>,
    pub tax: Option<Money>,
    pub tax_receiver_id: Option<i64>,
    pub context_id: Option<i64>,
    pub context_id_type: Option<String>,
    pub description: String,
}

/// One market transaction of the character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransaction {
    pub transaction_id: i64,
    pub at: i64,
    pub client_id: i64,
    pub is_buy: bool,
    pub is_personal: bool,
    pub journal_ref_id: i64,
    pub location_id: i64,
    pub quantity: u32,
    pub type_id: TypeId,
    pub unit_price: Money,
}

/// The character's whole wallet journal, page by page. A repeated id is kept once, the first time.
pub fn read_journal(
    client: &mut Client<'_>,
    token: &AccessToken,
    character: u32,
    now: i64,
) -> Result<Fetched<Vec<JournalEntry>>, EsiError> {
    let route = format!("/characters/{character}/wallet/{JOURNAL}");
    let group = format!("{character}/wallet");
    let page_one = client.get_json(now, token, &format!("{route}?page=1"), &group)?;
    let pages = page_count(&page_one, &route)?;
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    collect_journal(&page_one, &route, &mut entries, &mut seen)?;
    for page in 2..=pages {
        let answer = client.get_json(now, token, &format!("{route}?page={page}"), &group)?;
        collect_journal(&answer, &route, &mut entries, &mut seen)?;
    }
    fetched(entries, &page_one, &route, now)
}

/// The character's market transactions, oldest page after oldest page. A repeated transaction id
/// is kept once, the first time.
pub fn read_transactions(
    client: &mut Client<'_>,
    token: &AccessToken,
    character: u32,
    now: i64,
) -> Result<Fetched<Vec<WalletTransaction>>, EsiError> {
    let route = format!("/characters/{character}/wallet/{TRANSACTIONS}");
    let group = format!("{character}/wallet");
    let mut page_one: Option<Answer> = None;
    let mut transactions = Vec::new();
    let mut seen = BTreeSet::new();
    let mut asked = BTreeSet::new();
    let mut from_id: Option<i64> = None;
    let mut pages = 0_u32;
    loop {
        if pages == MAX_PAGES {
            return Err(bad_data(
                &route,
                "from_id",
                &from_id.unwrap_or_default().to_string(),
            ));
        }
        pages += 1;
        let path = match from_id {
            Some(id) => format!("{route}?from_id={id}"),
            None => route.clone(),
        };
        let answer = client.get_json(now, token, &path, &group)?;
        let batch = array(&answer.body, &route)?;
        if batch.is_empty() {
            if page_one.is_none() {
                page_one = Some(answer);
            }
            break;
        }
        let mut last = None;
        for row in batch {
            let parsed = transaction(row, &route)?;
            let id = parsed.transaction_id;
            if seen.insert(id) {
                transactions.push(parsed);
            }
            last = Some(id);
        }
        if page_one.is_none() {
            page_one = Some(answer);
        }
        let Some(last) = last else {
            break;
        };
        if !asked.insert(last) {
            return Err(bad_data(&route, "from_id", &last.to_string()));
        }
        from_id = Some(last);
    }
    let page_one = page_one.ok_or_else(|| bad_data(&route, "body", "missing"))?;
    fetched(transactions, &page_one, &route, now)
}

fn collect_journal(
    answer: &Answer,
    route: &str,
    entries: &mut Vec<JournalEntry>,
    seen: &mut BTreeSet<i64>,
) -> Result<(), EsiError> {
    for row in array(&answer.body, route)? {
        let entry = journal_entry(row, route)?;
        if seen.insert(entry.id) {
            entries.push(entry);
        }
    }
    Ok(())
}

/// The number of pages an answer says it has; a missing header means one page.
fn page_count(answer: &Answer, route: &str) -> Result<u32, EsiError> {
    let Some(text) = answer.header("x-pages") else {
        return Ok(1);
    };
    match text.trim().parse::<u32>() {
        Ok(pages) if (1..=MAX_PAGES).contains(&pages) => Ok(pages),
        _ => Err(bad_data(route, "x-pages", text)),
    }
}

fn journal_entry(row: &Value, route: &str) -> Result<JournalEntry, EsiError> {
    let id = integer(row, "id", route)?;
    if id < 0 {
        return Err(bad_data(route, "id", &id.to_string()));
    }
    let ref_type = text(row, "ref_type", route)?;
    if ref_type.is_empty() {
        return Err(bad_data(route, "ref_type", ref_type));
    }
    Ok(JournalEntry {
        id,
        at: time(row, "date", route)?,
        ref_type: ref_type.to_owned(),
        ref_id: optional_integer(row, "ref_id", route)?,
        first_party_id: optional_integer(row, "first_party_id", route)?,
        second_party_id: optional_integer(row, "second_party_id", route)?,
        amount: optional_money(row, "amount", route)?,
        balance: optional_money(row, "balance", route)?,
        reason: optional_text(row, "reason", route)?,
        tax: optional_money(row, "tax", route)?,
        tax_receiver_id: optional_integer(row, "tax_receiver_id", route)?,
        context_id: optional_integer(row, "context_id", route)?,
        context_id_type: optional_text(row, "context_id_type", route)?,
        description: text(row, "description", route)?.to_owned(),
    })
}

fn transaction(row: &Value, route: &str) -> Result<WalletTransaction, EsiError> {
    let transaction_id = integer(row, "transaction_id", route)?;
    if transaction_id < 0 {
        return Err(bad_data(
            route,
            "transaction_id",
            &transaction_id.to_string(),
        ));
    }
    let quantity = whole(row, "quantity", route)?;
    if quantity == 0 {
        return Err(bad_data(route, "quantity", "0"));
    }
    Ok(WalletTransaction {
        transaction_id,
        at: time(row, "date", route)?,
        client_id: integer(row, "client_id", route)?,
        is_buy: boolean(row, "is_buy", route)?,
        is_personal: boolean(row, "is_personal", route)?,
        journal_ref_id: integer(row, "journal_ref_id", route)?,
        location_id: integer(row, "location_id", route)?,
        quantity,
        type_id: whole(row, "type_id", route)?,
        unit_price: money(field(row, "unit_price", route)?, "unit_price", route)?,
    })
}

/// The field `key` as a whole number.
fn integer(row: &Value, key: &'static str, route: &str) -> Result<i64, EsiError> {
    let value = field(row, key, route)?;
    value
        .as_i64()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

/// The field `key` as a whole number within a `u32`.
fn whole(row: &Value, key: &'static str, route: &str) -> Result<u32, EsiError> {
    let value = field(row, key, route)?;
    let number = value
        .as_u64()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))?;
    u32::try_from(number).map_err(|_error| bad_data(route, key, &number.to_string()))
}

/// The field `key` as text; an absent field is `BadData`.
fn text<'a>(row: &'a Value, key: &'static str, route: &str) -> Result<&'a str, EsiError> {
    let value = field(row, key, route)?;
    value
        .as_str()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

fn boolean(row: &Value, key: &'static str, route: &str) -> Result<bool, EsiError> {
    let value = field(row, key, route)?;
    value
        .as_bool()
        .ok_or_else(|| bad_data(route, key, &value.to_string()))
}

/// The field `key` when it is present and not null.
fn optional<'a>(row: &'a Value, key: &str) -> Option<&'a Value> {
    row.get(key).filter(|value| !value.is_null())
}

fn optional_integer(row: &Value, key: &'static str, route: &str) -> Result<Option<i64>, EsiError> {
    optional(row, key)
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| bad_data(route, key, &value.to_string()))
        })
        .transpose()
}

fn optional_text(row: &Value, key: &'static str, route: &str) -> Result<Option<String>, EsiError> {
    optional(row, key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| bad_data(route, key, &value.to_string()))
        })
        .transpose()
}

fn optional_money(row: &Value, key: &'static str, route: &str) -> Result<Option<Money>, EsiError> {
    optional(row, key)
        .map(|value| money(value, key, route))
        .transpose()
}

/// An RFC 3339 UTC time as Unix seconds.
fn time(row: &Value, key: &'static str, route: &str) -> Result<i64, EsiError> {
    let value = text(row, key, route)?;
    utc_seconds(value).ok_or_else(|| bad_data(route, key, value))
}

/// Money as EVE writes it, read exactly: the number's own text is parsed, never a float.
fn money(value: &Value, key: &'static str, route: &str) -> Result<Money, EsiError> {
    match value {
        Value::Number(number) => Money::parse_decimal(&number.to_string())
            .map_err(|_error| bad_data(route, key, &number.to_string())),
        Value::Null | Value::Bool(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => {
            Err(bad_data(route, key, &value.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::client::BASE_URL;
    use crate::http::{Http, HttpRequest, HttpResponse};

    const CHARACTER: u32 = 90_000_001;
    const NOW: i64 = 1_792_404_100;
    const LAST_MODIFIED: i64 = 1_792_404_000;
    const EXPIRES: i64 = 1_792_407_600;
    const SECRET: &str = "TOKEN-SECRET-0226";
    const JOURNAL_ROUTE: &str = "/characters/90000001/wallet/journal";
    const TRANSACTIONS_ROUTE: &str = "/characters/90000001/wallet/transactions";
    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[derive(Clone, Default)]
    struct Fake {
        replies: Arc<Mutex<VecDeque<HttpResponse>>>,
        calls: Arc<Mutex<Vec<HttpRequest>>>,
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

    fn http_date(at: i64) -> String {
        httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(at.unsigned_abs()))
    }

    fn timed(status: u16, pages: Option<u32>, body: &str) -> HttpResponse {
        let modified = http_date(LAST_MODIFIED);
        let expires = http_date(EXPIRES);
        let page_text = pages.map(|pages| pages.to_string());
        let mut headers = vec![
            ("last-modified", modified.as_str()),
            ("expires", expires.as_str()),
        ];
        if let Some(page_text) = page_text.as_deref() {
            headers.push(("x-pages", page_text));
        }
        reply(status, &headers, body)
    }

    fn with_replies<T>(
        replies: Vec<HttpResponse>,
        read: impl FnOnce(&mut Client<'_>, &AccessToken) -> Result<Fetched<T>, EsiError>,
    ) -> (Fake, Result<Fetched<T>, EsiError>) {
        let fake = Fake {
            replies: Arc::new(Mutex::new(replies.into())),
            calls: Arc::default(),
        };
        let mut client = Client::new(&fake);
        let token = AccessToken::new(SECRET);
        let result = read(&mut client, &token);
        (fake, result)
    }

    fn read_journal_with(
        replies: Vec<HttpResponse>,
    ) -> (Fake, Result<Fetched<Vec<JournalEntry>>, EsiError>) {
        with_replies(replies, |client, token| {
            read_journal(client, token, CHARACTER, NOW)
        })
    }

    fn read_transactions_with(
        replies: Vec<HttpResponse>,
    ) -> (Fake, Result<Fetched<Vec<WalletTransaction>>, EsiError>) {
        with_replies(replies, |client, token| {
            read_transactions(client, token, CHARACTER, NOW)
        })
    }

    fn journal_body() -> String {
        concat!(
            r#"[{"id":9000000001,"date":"2026-10-28T23:59:58Z","ref_type":"player_trading","#,
            r#""ref_id":9000000000,"first_party_id":1000000,"second_party_id":1000001,"#,
            r#""reason":"fees","tax":1.25,"tax_receiver_id":1000002,"#,
            r#""context_id":60003760,"context_id_type":"station_id","#,
            r#""amount":-1234567.89,"balance":12345678.90,"description":"sold"}"#,
            r#",{"id":9000000002,"date":"2026-10-28T00:00:00Z","ref_type":"broker_fee","#,
            r#""ref_id":null,"first_party_id":null,"second_party_id":null,"reason":null,"#,
            r#""tax":null,"tax_receiver_id":null,"context_id":null,"context_id_type":null,"#,
            r#""amount":42.5,"balance":null,"description":"fee"}]"#
        )
        .to_owned()
    }

    fn journal_second_page() -> &'static str {
        concat!(
            r#"[{"id":9000000000,"date":"2026-10-27T00:00:00Z","ref_type":"bounty_prizes","#,
            r#""amount":10,"balance":20,"description":"bounty"}]"#
        )
    }

    fn transaction_body() -> &'static str {
        concat!(
            r#"[{"transaction_id":7000000002,"date":"2026-10-27T12:00:00Z","client_id":1000001,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":9000000002,"location_id":60003760,"#,
            r#""quantity":10,"type_id":34,"unit_price":5.75},"#,
            r#"{"transaction_id":7000000001,"date":"2026-10-26T12:00:00Z","client_id":1000002,"#,
            r#""is_buy":false,"is_personal":true,"journal_ref_id":9000000001,"location_id":60003760,"#,
            r#""quantity":1,"type_id":35,"unit_price":0}]"#
        )
    }

    fn transaction_second_page() -> &'static str {
        concat!(
            r#"[{"transaction_id":7000000000,"date":"2026-10-25T12:00:00Z","client_id":1000003,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":9000000000,"location_id":60003760,"#,
            r#""quantity":2,"type_id":36,"unit_price":1000000.0001}]"#
        )
    }

    #[test]
    fn the_journal_is_read_through_its_pages() {
        let (fake, result) = read_journal_with(vec![
            timed(200, Some(2), &journal_body()),
            timed(200, None, journal_second_page()),
        ]);
        let held = result.expect("journal");
        assert_eq!(held.fetched_at, NOW);
        assert_eq!(held.last_modified, Some(LAST_MODIFIED));
        assert_eq!(held.expires, Some(EXPIRES));
        let ids: Vec<i64> = held.data.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [9_000_000_001, 9_000_000_002, 9_000_000_000]);
        let first = &held.data[0];
        assert_eq!(first.at, 1_793_231_998);
        assert_eq!(
            first.amount,
            Some(Money::from_ten_thousandths(-12_345_678_900))
        );
        assert_eq!(
            first.balance,
            Some(Money::from_ten_thousandths(123_456_789_000))
        );
        assert_eq!(first.ref_id, Some(9_000_000_000));
        assert_eq!(first.context_id_type.as_deref(), Some("station_id"));
        assert_eq!(second_balance(&held), None);
        let calls = locked(&fake.calls);
        assert_eq!(calls[0].url, format!("{BASE_URL}{JOURNAL_ROUTE}?page=1"));
        assert_eq!(calls[1].url, format!("{BASE_URL}{JOURNAL_ROUTE}?page=2"));
        assert_eq!(calls.len(), 2);
        let auth = calls[0]
            .headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .map(|(_, value)| value.as_str());
        assert_eq!(auth, Some(format!("Bearer {SECRET}").as_str()));
    }

    fn second_balance(held: &Fetched<Vec<JournalEntry>>) -> Option<Money> {
        held.data.get(1).and_then(|entry| entry.balance)
    }

    #[test]
    fn a_journal_page_without_the_header_has_one_page() {
        let (fake, result) = read_journal_with(vec![timed(200, None, "[]")]);
        let held = result.expect("journal");
        assert!(held.data.is_empty());
        assert_eq!(locked(&fake.calls).len(), 1);
    }

    #[test]
    fn a_repeated_journal_id_is_kept_the_first_time() {
        let body = concat!(
            r#"[{"id":9000000001,"date":"2026-10-28T00:00:00Z","ref_type":"first","#,
            r#""description":"kept"}"#,
            r#",{"id":9000000001,"date":"2026-10-28T00:00:00Z","ref_type":"second","#,
            r#""description":"dropped"}]"#
        );
        let (fake, result) = read_journal_with(vec![timed(200, Some(1), body)]);
        let held = result.expect("journal");
        assert_eq!(held.data.len(), 1);
        assert_eq!(held.data[0].ref_type, "first");
        assert_eq!(locked(&fake.calls).len(), 1);
    }

    #[test]
    fn a_bad_page_count_is_bad_data_naming_x_pages() {
        for pages in ["0", "1001", "many"] {
            let (_fake, result) = read_journal_with(vec![reply(200, &[("x-pages", pages)], "[]")]);
            match result {
                Err(EsiError::BadData { route, field, .. }) => {
                    assert_eq!(route, JOURNAL_ROUTE);
                    assert_eq!(field, "x-pages");
                }
                other => panic!("expected BadData for {pages}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_journal_entry_with_id_zero_is_kept() {
        let body = journal_body().replace("\"id\":9000000001", "\"id\":0");
        let (_fake, result) = read_journal_with(vec![timed(200, Some(1), &body)]);
        let held = result.expect("journal");
        let ids: Vec<i64> = held.data.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, [0, 9_000_000_002]);
        assert_eq!(held.data[0].ref_type, "player_trading");
        assert_eq!(held.data[0].description, "sold");
    }

    #[test]
    fn bad_journal_fields_are_bad_data_naming_the_field() {
        let cases = [
            (
                journal_body().replace("\"id\":9000000001", "\"id\":\"9000000001\""),
                "id",
            ),
            (
                journal_body().replace("\"id\":9000000001", "\"id\":-1"),
                "id",
            ),
            (
                journal_body().replace("\"ref_type\":\"player_trading\"", "\"ref_type\":\"\""),
                "ref_type",
            ),
            (
                journal_body().replace("2026-10-28T23:59:58Z", "2026-10-28 23:59:58"),
                "date",
            ),
            (journal_body().replace("-1234567.89", "1.23456"), "amount"),
            (journal_body().replace("12345678.90", "\"12\""), "balance"),
            (
                journal_body().replace("\"ref_id\":9000000000", "\"ref_id\":\"x\""),
                "ref_id",
            ),
            (
                journal_body().replace("\"reason\":\"fees\"", "\"reason\":7"),
                "reason",
            ),
            (
                journal_body().replace("\"tax\":1.25", "\"tax\":1.23456"),
                "tax",
            ),
            (
                journal_body().replace("\"description\":\"sold\"", "\"description\":7"),
                "description",
            ),
        ];
        for (body, expected) in cases {
            let (_fake, result) = read_journal_with(vec![timed(200, Some(1), &body)]);
            match result {
                Err(EsiError::BadData { route, field, .. }) => {
                    assert_eq!(route, JOURNAL_ROUTE);
                    assert_eq!(field, expected);
                }
                other => panic!("expected BadData for {expected}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_null_journal_field_is_none() {
        let body = concat!(
            r#"[{"id":9000000001,"date":"2026-10-28T00:00:00Z","ref_type":"bounty_prizes","#,
            r#""amount":null,"balance":null,"description":"none"}]"#
        );
        let (_fake, result) = read_journal_with(vec![timed(200, Some(1), body)]);
        let held = result.expect("journal");
        let entry = &held.data[0];
        assert_eq!(entry.amount, None);
        assert_eq!(entry.balance, None);
        assert_eq!(entry.ref_id, None);
        assert_eq!(entry.reason, None);
        assert_eq!(entry.tax, None);
        assert_eq!(entry.context_id, None);
        assert_eq!(entry.context_id_type, None);
    }

    #[test]
    fn transactions_are_walked_by_their_last_id() {
        let (fake, result) = read_transactions_with(vec![
            timed(200, None, transaction_body()),
            timed(200, None, transaction_second_page()),
            timed(200, None, "[]"),
        ]);
        let held = result.expect("transactions");
        assert_eq!(held.fetched_at, NOW);
        assert_eq!(held.last_modified, Some(LAST_MODIFIED));
        assert_eq!(held.expires, Some(EXPIRES));
        let ids: Vec<i64> = held
            .data
            .iter()
            .map(|transaction| transaction.transaction_id)
            .collect();
        assert_eq!(ids, [7_000_000_002, 7_000_000_001, 7_000_000_000]);
        let first = &held.data[0];
        assert_eq!(first.quantity, 10);
        assert_eq!(first.type_id, 34);
        assert_eq!(first.unit_price, Money::from_ten_thousandths(57_500));
        assert!(first.is_buy);
        assert!(!first.is_personal);
        let calls = locked(&fake.calls);
        let urls: Vec<&str> = calls.iter().map(|call| call.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                format!("{BASE_URL}{TRANSACTIONS_ROUTE}"),
                format!("{BASE_URL}{TRANSACTIONS_ROUTE}?from_id=7000000001"),
                format!("{BASE_URL}{TRANSACTIONS_ROUTE}?from_id=7000000000"),
            ]
        );
    }

    #[test]
    fn a_repeated_transaction_id_is_kept_the_first_time() {
        let page_one = concat!(
            r#"[{"transaction_id":7000000002,"date":"2026-10-27T12:00:00Z","client_id":1,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":1,"type_id":34,"unit_price":1},"#,
            r#"{"transaction_id":7000000001,"date":"2026-10-26T12:00:00Z","client_id":1,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":1,"type_id":34,"unit_price":1}]"#
        );
        let page_two = concat!(
            r#"[{"transaction_id":7000000001,"date":"2026-10-26T00:00:00Z","client_id":2,"#,
            r#""is_buy":false,"is_personal":true,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":2,"type_id":34,"unit_price":2},"#,
            r#"{"transaction_id":7000000000,"date":"2026-10-25T12:00:00Z","client_id":1,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":1,"type_id":34,"unit_price":1}]"#
        );
        let (fake, result) = read_transactions_with(vec![
            timed(200, None, page_one),
            timed(200, None, page_two),
            timed(200, None, "[]"),
        ]);
        let held = result.expect("transactions");
        let ids: Vec<i64> = held
            .data
            .iter()
            .map(|transaction| transaction.transaction_id)
            .collect();
        assert_eq!(ids, [7_000_000_002, 7_000_000_001, 7_000_000_000]);
        assert_eq!(held.data[1].quantity, 1);
        assert_eq!(locked(&fake.calls).len(), 3);
    }

    #[test]
    fn a_repeated_from_id_is_bad_data() {
        let page_one = concat!(
            r#"[{"transaction_id":7000000002,"date":"2026-10-27T12:00:00Z","client_id":1,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":1,"type_id":34,"unit_price":1},"#,
            r#"{"transaction_id":7000000001,"date":"2026-10-26T12:00:00Z","client_id":1,"#,
            r#""is_buy":true,"is_personal":false,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":1,"type_id":34,"unit_price":1}]"#
        );
        let page_two = concat!(
            r#"[{"transaction_id":7000000001,"date":"2026-10-26T00:00:00Z","client_id":2,"#,
            r#""is_buy":false,"is_personal":true,"journal_ref_id":1,"location_id":1,"#,
            r#""quantity":2,"type_id":34,"unit_price":2}]"#
        );
        let (fake, result) =
            read_transactions_with(vec![timed(200, None, page_one), timed(200, None, page_two)]);
        match result {
            Err(EsiError::BadData { route, field, .. }) => {
                assert_eq!(route, TRANSACTIONS_ROUTE);
                assert_eq!(field, "from_id");
            }
            other => panic!("expected BadData, got {other:?}"),
        }
        assert_eq!(locked(&fake.calls).len(), 2);
    }

    #[test]
    fn a_thousand_nonempty_pages_are_bad_data() {
        let mut replies = Vec::new();
        for id in 0..MAX_PAGES {
            let body = format!(
                r#"[{{"transaction_id":{id},"date":"2026-10-27T12:00:00Z","client_id":1,"is_buy":true,"is_personal":false,"journal_ref_id":1,"location_id":1,"quantity":1,"type_id":34,"unit_price":1}}]"#
            );
            replies.push(timed(200, None, &body));
        }
        let (fake, result) = read_transactions_with(replies);
        match result {
            Err(EsiError::BadData { route, field, .. }) => {
                assert_eq!(route, TRANSACTIONS_ROUTE);
                assert_eq!(field, "from_id");
            }
            other => panic!("expected BadData, got {other:?}"),
        }
        assert_eq!(
            locked(&fake.calls).len(),
            usize::try_from(MAX_PAGES).expect("pages")
        );
    }

    #[test]
    fn an_empty_transaction_page_gives_no_rows() {
        let (fake, result) = read_transactions_with(vec![timed(200, None, "[]")]);
        let held = result.expect("transactions");
        assert!(held.data.is_empty());
        assert_eq!(locked(&fake.calls).len(), 1);
    }

    #[test]
    fn bad_transaction_fields_are_bad_data_naming_the_field() {
        let cases = [
            (
                transaction_body()
                    .replace("\"transaction_id\":7000000002", "\"transaction_id\":-1"),
                "transaction_id",
            ),
            (
                transaction_body().replace("\"quantity\":10", "\"quantity\":0"),
                "quantity",
            ),
            (
                transaction_body().replace("\"type_id\":34", "\"type_id\":4294967296"),
                "type_id",
            ),
            (
                transaction_body().replace("2026-10-27T12:00:00Z", "yesterday"),
                "date",
            ),
            (
                transaction_body().replace("\"unit_price\":5.75", "\"unit_price\":1.23456"),
                "unit_price",
            ),
            (
                transaction_body().replace("\"is_buy\":true", "\"is_buy\":1"),
                "is_buy",
            ),
            (
                transaction_body().replace("\"is_personal\":false", "\"is_personal\":null"),
                "is_personal",
            ),
            (
                transaction_body().replace("\"location_id\":60003760", "\"location_id\":null"),
                "location_id",
            ),
        ];
        for (body, expected) in cases {
            let (_fake, result) = read_transactions_with(vec![timed(200, None, &body)]);
            match result {
                Err(EsiError::BadData { route, field, .. }) => {
                    assert_eq!(route, TRANSACTIONS_ROUTE);
                    assert_eq!(field, expected);
                }
                other => panic!("expected BadData for {expected}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_403_on_the_wallet_is_forbidden_for_the_route() {
        let (_fake, result) = read_journal_with(vec![reply(403, &[], "{}")]);
        assert!(matches!(result, Err(EsiError::Forbidden { route }) if route == JOURNAL_ROUTE));
    }
}
