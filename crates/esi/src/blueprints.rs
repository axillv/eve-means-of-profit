//! A character's blueprints from ESI: originals and copies, their stacks, runs, material and time
//! efficiency. Read on a click for the what-to-build feed, never saved to disk here.

use std::collections::BTreeSet;

use mop_core::industry::TypeId;
use mop_core::stock::{ItemId, LocationId};
use serde_json::Value;

use crate::character_data::{Fetched, array, fetched, signed, text, unsigned};
use crate::client::{Answer, Client, bad_data};
use crate::error::EsiError;
use crate::token::AccessToken;

/// The most material efficiency a blueprint can have.
const MAX_ME: u8 = 10;
/// The most time efficiency a blueprint can have.
const MAX_TE: u8 = 20;
/// What EVE sends as `runs` for an original.
const ORIGINAL_RUNS: i64 = -1;
const BLUEPRINTS_PATH: &str = "blueprints";

/// Whether a blueprint is an original or a copy, and how many runs a copy has left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintKind {
    Original,
    Copy { runs: u32 },
}

/// One blueprint the character owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedBlueprint {
    pub item_id: ItemId,
    pub type_id: TypeId,
    pub location_id: LocationId,
    pub flag: Option<String>,
    pub kind: BlueprintKind,
    /// How many blueprints are stacked here; copies are always a single one.
    pub stack: u32,
    pub me: u8,
    pub te: u8,
}

/// The character's blueprints, read from ESI on each call.
pub fn read_blueprints(
    client: &mut Client<'_>,
    token: &AccessToken,
    character: u32,
    now: i64,
) -> Result<Fetched<Vec<OwnedBlueprint>>, EsiError> {
    let path = format!("/characters/{character}/{BLUEPRINTS_PATH}");
    let group = format!("{character}/{BLUEPRINTS_PATH}");
    let answer = client.get_json(now, token, &path, &group)?;
    let blueprints = parse_blueprints(&answer, &path)?;
    fetched(blueprints, &answer, &path, now)
}

fn parse_blueprints(answer: &Answer, route: &str) -> Result<Vec<OwnedBlueprint>, EsiError> {
    let rows = array(&answer.body, route)?;
    let mut blueprints = Vec::with_capacity(rows.len());
    let mut seen = BTreeSet::new();
    for row in rows {
        let blueprint = blueprint(row, route)?;
        if !seen.insert(blueprint.item_id) {
            return Err(bad_data(
                route,
                "item_id",
                &format!("{} repeated", blueprint.item_id),
            ));
        }
        blueprints.push(blueprint);
    }
    Ok(blueprints)
}

fn blueprint(row: &Value, route: &str) -> Result<OwnedBlueprint, EsiError> {
    let item_id: ItemId = signed(row, "item_id", route)?;
    if item_id < 0 {
        return Err(bad_data(route, "item_id", &item_id.to_string()));
    }
    let location_id: LocationId = signed(row, "location_id", route)?;
    if location_id < 0 {
        return Err(bad_data(route, "location_id", &location_id.to_string()));
    }
    let type_id: TypeId = unsigned(row, "type_id", route)?;
    let me = level(row, "material_efficiency", MAX_ME, route)?;
    let te = level(row, "time_efficiency", MAX_TE, route)?;
    let flag = text(row, "location_flag", route)?.to_owned();
    let quantity = signed(row, "quantity", route)?;
    let (kind, stack) = match quantity {
        -1 => {
            original_runs(row, route)?;
            (BlueprintKind::Original, 1)
        }
        -2 => (
            BlueprintKind::Copy {
                runs: copy_runs(row, route)?,
            },
            1,
        ),
        copies if copies >= 1 => {
            original_runs(row, route)?;
            let stack = u32::try_from(copies)
                .ok()
                .ok_or_else(|| bad_data(route, "quantity", &copies.to_string()))?;
            (BlueprintKind::Original, stack)
        }
        other => return Err(bad_data(route, "quantity", &other.to_string())),
    };
    Ok(OwnedBlueprint {
        item_id,
        type_id,
        location_id,
        flag: Some(flag),
        kind,
        stack,
        me,
        te,
    })
}

/// The runs of a copy: at least one, and within a whole number.
fn copy_runs(row: &Value, route: &str) -> Result<u32, EsiError> {
    let runs: u32 = unsigned(row, "runs", route)?;
    if runs == 0 {
        return Err(bad_data(route, "runs", "0"));
    }
    Ok(runs)
}

/// An original's `runs` must be EVE's -1.
fn original_runs(row: &Value, route: &str) -> Result<(), EsiError> {
    let runs: i64 = signed(row, "runs", route)?;
    if runs == ORIGINAL_RUNS {
        Ok(())
    } else {
        Err(bad_data(route, "runs", &runs.to_string()))
    }
}

/// A whole number EVE allows from 0 up to `max`; anything else is `BadData` naming the field.
fn level(row: &Value, key: &'static str, max: u8, route: &str) -> Result<u8, EsiError> {
    let value: u8 = unsigned(row, key, route)?;
    if value > max {
        return Err(bad_data(route, key, &value.to_string()));
    }
    Ok(value)
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
    const SECRET: &str = "TOKEN-SECRET-0003";
    const ROUTE: &str = "/characters/90000001/blueprints";
    const BODY: &str = concat!(
        r#"[{"item_id":1003,"type_id":2455,"location_id":60003760,"location_flag":"Hangar","#,
        r#""quantity":-2,"material_efficiency":10,"time_efficiency":20,"runs":7},"#,
        r#"{"item_id":1007,"type_id":2455,"location_id":60003760,"location_flag":"Hangar","#,
        r#""quantity":-1,"material_efficiency":9,"time_efficiency":14,"runs":-1},"#,
        r#"{"item_id":2002,"type_id":691,"location_id":60003760,"location_flag":"Hangar","#,
        r#""quantity":3,"material_efficiency":0,"time_efficiency":0,"runs":-1}]"#
    );

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers from a queue and records every request.
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

    fn read_with(
        replies: Vec<HttpResponse>,
    ) -> (Fake, Result<Fetched<Vec<OwnedBlueprint>>, EsiError>) {
        let fake = Fake {
            replies: Arc::new(Mutex::new(replies.into())),
            calls: Arc::default(),
        };
        let mut client = Client::new(&fake);
        let token = AccessToken::new(SECRET);
        let result = read_blueprints(&mut client, &token, CHARACTER, NOW);
        (fake, result)
    }

    fn bad_field(body: &str) -> &'static str {
        let (_fake, result) = read_with(vec![reply(200, &[], body)]);
        match result {
            Err(EsiError::BadData { route, field, .. }) => {
                assert_eq!(route, ROUTE);
                field
            }
            other => panic!("expected BadData, got {other:?}"),
        }
    }

    fn http_date(at: i64) -> String {
        httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(at.unsigned_abs()))
    }

    #[test]
    fn blueprints_are_read_with_the_fetch_time_and_eves_times() {
        let modified = http_date(LAST_MODIFIED);
        let expires = http_date(EXPIRES);
        let headers = [
            ("last-modified", modified.as_str()),
            ("expires", expires.as_str()),
        ];
        let (fake, result) = read_with(vec![reply(200, &headers, BODY)]);
        let held = result.expect("blueprints");
        assert_eq!(held.fetched_at, NOW);
        assert_eq!(held.last_modified, Some(LAST_MODIFIED));
        assert_eq!(held.expires, Some(EXPIRES));
        let expected = vec![
            OwnedBlueprint {
                item_id: 1003,
                type_id: 2455,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                kind: BlueprintKind::Copy { runs: 7 },
                stack: 1,
                me: 10,
                te: 20,
            },
            OwnedBlueprint {
                item_id: 1007,
                type_id: 2455,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                kind: BlueprintKind::Original,
                stack: 1,
                me: 9,
                te: 14,
            },
            OwnedBlueprint {
                item_id: 2002,
                type_id: 691,
                location_id: 60_003_760,
                flag: Some("Hangar".to_owned()),
                kind: BlueprintKind::Original,
                stack: 3,
                me: 0,
                te: 0,
            },
        ];
        assert_eq!(held.data, expected);
        let calls = locked(&fake.calls);
        assert_eq!(calls[0].url, format!("{BASE_URL}{ROUTE}"));
        let auth = calls[0]
            .headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .map(|(_, value)| value.as_str());
        assert_eq!(auth, Some(format!("Bearer {SECRET}").as_str()));
        let cached = calls[0]
            .headers
            .iter()
            .any(|(name, _)| name == "If-None-Match");
        assert!(!cached);
    }

    #[test]
    fn an_answer_without_eves_headers_gives_no_times() {
        let (_fake, result) = read_with(vec![reply(200, &[], BODY)]);
        let held = result.expect("blueprints");
        assert_eq!(held.last_modified, None);
        assert_eq!(held.expires, None);
    }

    #[test]
    fn a_zero_quantity_is_bad_data() {
        let body = BODY.replace("\"quantity\":3", "\"quantity\":0");
        assert_eq!(bad_field(&body), "quantity");
    }

    #[test]
    fn a_quantity_below_minus_two_is_bad_data() {
        let body = BODY.replace("\"quantity\":-2", "\"quantity\":-3");
        assert_eq!(bad_field(&body), "quantity");
    }

    #[test]
    fn a_copy_without_runs_is_bad_data() {
        let body = BODY.replace("\"runs\":7", "\"runs\":0");
        assert_eq!(bad_field(&body), "runs");
    }

    #[test]
    fn an_original_with_runs_is_bad_data() {
        // Only the -1 original (1007) gets runs; 2002's own "runs":-1 stays untouched.
        let body = BODY.replace(
            "\"time_efficiency\":14,\"runs\":-1",
            "\"time_efficiency\":14,\"runs\":5",
        );
        assert_eq!(bad_field(&body), "runs");
    }

    #[test]
    fn a_stacked_original_with_runs_is_bad_data() {
        // Only the positive-quantity original (2002) gets runs.
        let body = BODY.replace(
            "\"time_efficiency\":0,\"runs\":-1",
            "\"time_efficiency\":0,\"runs\":5",
        );
        assert_eq!(bad_field(&body), "runs");
    }

    #[test]
    fn an_original_without_runs_is_bad_data() {
        let body = BODY.replace(
            "\"time_efficiency\":14,\"runs\":-1",
            "\"time_efficiency\":14",
        );
        assert_eq!(bad_field(&body), "runs");
    }

    #[test]
    fn a_material_efficiency_above_ten_is_bad_data() {
        let body = BODY.replace("\"material_efficiency\":10", "\"material_efficiency\":11");
        assert_eq!(bad_field(&body), "material_efficiency");
    }

    #[test]
    fn a_time_efficiency_above_twenty_is_bad_data() {
        let body = BODY.replace("\"time_efficiency\":20", "\"time_efficiency\":21");
        assert_eq!(bad_field(&body), "time_efficiency");
    }

    #[test]
    fn a_type_id_outside_a_whole_number_is_bad_data() {
        let body = BODY.replace("\"type_id\":2455", "\"type_id\":4294967296");
        assert_eq!(bad_field(&body), "type_id");
    }

    #[test]
    fn a_negative_item_id_is_bad_data() {
        let body = BODY.replace("\"item_id\":1003", "\"item_id\":-1");
        assert_eq!(bad_field(&body), "item_id");
    }

    #[test]
    fn a_zero_item_id_is_accepted() {
        let body = BODY.replace("\"item_id\":1003", "\"item_id\":0");
        let (_fake, result) = read_with(vec![reply(200, &[], &body)]);
        let held = result.expect("blueprints");
        assert_eq!(held.data[0].item_id, 0);
    }

    #[test]
    fn a_missing_location_id_is_bad_data() {
        let body = BODY.replace("\"location_id\":60003760,", "");
        assert_eq!(bad_field(&body), "location_id");
    }

    #[test]
    fn a_negative_location_id_is_bad_data() {
        let body = BODY.replace("\"location_id\":60003760,", "\"location_id\":-1,");
        assert_eq!(bad_field(&body), "location_id");
    }

    #[test]
    fn a_zero_location_id_is_accepted() {
        let body = BODY.replace("\"location_id\":60003760,", "\"location_id\":0,");
        let (_fake, result) = read_with(vec![reply(200, &[], &body)]);
        let held = result.expect("blueprints");
        assert!(held.data.iter().all(|row| row.location_id == 0));
    }

    #[test]
    fn a_repeated_item_id_is_bad_data() {
        let body = BODY.replace("\"item_id\":1007", "\"item_id\":1003");
        assert_eq!(bad_field(&body), "item_id");
    }

    #[test]
    fn a_403_is_forbidden_for_the_route() {
        let (_fake, result) = read_with(vec![reply(403, &[], "{}")]);
        assert!(matches!(result, Err(EsiError::Forbidden { route }) if route == ROUTE));
    }

    #[test]
    fn a_429_gives_rate_limited_with_retry_after() {
        let (_fake, result) = read_with(vec![reply(429, &[("retry-after", "30")], "{}")]);
        assert!(matches!(
            result,
            Err(EsiError::RateLimited {
                retry_after_seconds: 30
            })
        ));
    }
}
