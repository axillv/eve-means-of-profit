//! Skills and standings against canned answers: a fake `Http` that records requests.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use mop_esi::client::{BASE_URL, Client};
    use mop_esi::error::EsiError;
    use mop_esi::http::{Http, HttpRequest, HttpResponse};
    use mop_esi::skills::{NpcStanding, StandingFrom, read_skills, read_standings};
    use mop_esi::token::AccessToken;

    const CHARACTER: u32 = 90_000_001;
    const NOW: i64 = 1_792_404_100;
    const SECRET: &str = "TOKEN-SECRET-0002";
    const SKILLS: &str = include_str!("../fixtures/skills/skills.json");
    const STANDINGS: &str = include_str!("../fixtures/skills/standings.json");

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

    fn fake_with(replies: Vec<HttpResponse>) -> Fake {
        Fake {
            replies: Arc::new(Mutex::new(replies.into())),
            calls: Arc::default(),
        }
    }

    fn token() -> AccessToken {
        AccessToken::new(SECRET)
    }

    fn skills_of(body: &str) -> Result<mop_esi::skills::CharacterSkills, EsiError> {
        let fake = fake_with(vec![reply(200, &[], body)]);
        let mut client = Client::new(&fake);
        read_skills(&mut client, NOW, &token(), CHARACTER)
    }

    fn standings_of(body: &str) -> Result<Vec<NpcStanding>, EsiError> {
        let fake = fake_with(vec![reply(200, &[], body)]);
        let mut client = Client::new(&fake);
        read_standings(&mut client, NOW, &token(), CHARACTER)
    }

    fn bad_field(result: Result<impl std::fmt::Debug, EsiError>) -> &'static str {
        match result {
            Err(EsiError::BadData { route, field, .. }) => {
                assert_eq!(route, "/characters/90000001/skills");
                field
            }
            other => panic!("expected BadData, got {other:?}"),
        }
    }

    fn bad_standing_field(result: Result<Vec<NpcStanding>, EsiError>) -> &'static str {
        match result {
            Err(EsiError::BadData { route, field, .. }) => {
                assert_eq!(route, "/characters/90000001/standings");
                field
            }
            other => panic!("expected BadData, got {other:?}"),
        }
    }

    #[test]
    fn skills_are_read_at_active_levels_with_the_bearer_token() {
        let fake = fake_with(vec![reply(200, &[], SKILLS)]);
        let mut client = Client::new(&fake);
        let skills = read_skills(&mut client, NOW, &token(), CHARACTER).expect("skills");
        let expected: BTreeMap<u32, u8> = [(3446, 5), (11453, 4), (16622, 4), (23121, 3)]
            .into_iter()
            .collect();
        assert_eq!(skills.levels, expected);
        assert_eq!(skills.total_sp, 5_000_000);
        let calls = locked(&fake.calls);
        assert_eq!(
            calls[0].url,
            format!("{BASE_URL}/characters/90000001/skills")
        );
        let auth = calls[0]
            .headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .map(|(_, value)| value.as_str());
        assert_eq!(auth, Some(format!("Bearer {SECRET}").as_str()));
    }

    #[test]
    fn standings_are_read_in_answer_order_and_rounded() {
        let standings = standings_of(STANDINGS).expect("standings");
        assert_eq!(
            standings,
            vec![
                NpcStanding {
                    from_id: 500_001,
                    from: StandingFrom::Faction,
                    hundredths: 523,
                },
                NpcStanding {
                    from_id: 1_000_035,
                    from: StandingFrom::NpcCorp,
                    hundredths: -126,
                },
                NpcStanding {
                    from_id: 3_008_416,
                    from: StandingFrom::Agent,
                    hundredths: 200,
                },
            ]
        );
    }

    #[test]
    fn a_level_above_five_is_bad_data() {
        let body = SKILLS.replace("\"active_skill_level\":3", "\"active_skill_level\":6");
        assert_eq!(bad_field(skills_of(&body)), "active_skill_level");
    }

    #[test]
    fn a_negative_level_is_bad_data() {
        let body = SKILLS.replace("\"active_skill_level\":3", "\"active_skill_level\":-1");
        assert_eq!(bad_field(skills_of(&body)), "active_skill_level");
    }

    #[test]
    fn a_repeated_skill_id_is_bad_data() {
        let body = r#"{"skills":[{"skill_id":3300,"active_skill_level":3},{"skill_id":3300,"active_skill_level":4}],"total_sp":1000}"#;
        assert_eq!(bad_field(skills_of(body)), "skill_id");
    }

    #[test]
    fn a_missing_total_sp_is_bad_data() {
        let body = SKILLS.replace("\"total_sp\":5000000,", "");
        assert_eq!(bad_field(skills_of(&body)), "total_sp");
    }

    #[test]
    fn a_standing_outside_range_is_bad_data() {
        let body = STANDINGS.replace("5.2345", "10.5");
        assert_eq!(bad_standing_field(standings_of(&body)), "standing");
    }

    #[test]
    fn an_unknown_from_type_is_bad_data() {
        let body = STANDINGS.replace("\"faction\"", "\"alliance\"");
        assert_eq!(bad_standing_field(standings_of(&body)), "from_type");
    }

    #[test]
    fn a_403_is_forbidden_for_the_route() {
        let fake = fake_with(vec![reply(403, &[], "{}")]);
        let mut client = Client::new(&fake);
        let result = read_standings(&mut client, NOW, &token(), CHARACTER);
        assert!(matches!(
            result,
            Err(EsiError::Forbidden { route }) if route == "/characters/90000001/standings"
        ));
    }

    #[test]
    fn a_429_gives_rate_limited_with_retry_after() {
        let fake = fake_with(vec![reply(429, &[("retry-after", "30")], "{}")]);
        let mut client = Client::new(&fake);
        let result = read_skills(&mut client, NOW, &token(), CHARACTER);
        assert!(matches!(
            result,
            Err(EsiError::RateLimited {
                retry_after_seconds: 30
            })
        ));
    }
}
