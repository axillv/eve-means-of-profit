//! Structure names against canned answers: a fake `Http` that also answers token refreshes, a fixed clock, temporary folders.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use profit_esi::auth::{Auth, TOKEN_URL};
    use profit_esi::characters::{Character, CharacterList, Login};
    use profit_esi::client::BASE_URL;
    use profit_esi::error::EsiError;
    use profit_esi::http::{Http, HttpRequest, HttpResponse};
    use profit_esi::places::{
        FIRST_STRUCTURE_ID, NamingReport, StructureEntry, StructureNames, load_places,
        name_structures, place_label,
    };
    use profit_esi::secrets::{FileStore, SecretStore, Secrets};
    use profit_esi::token::RefreshToken;
    use tempfile::TempDir;

    const CHARACTER_A: i64 = 2_112_625_428;
    const CHARACTER_B: i64 = 95_465_499;
    const CHARACTERS: [i64; 2] = [CHARACTER_A, CHARACTER_B];
    const STRUCTURE: i64 = 1_035_466_617_946;
    const OTHER_STRUCTURE: i64 = 1_035_466_617_947;
    const JITA_STATION: i64 = 60_003_760;
    const JITA: i64 = 30_000_142;
    const NOW: i64 = 1_792_404_000;
    const NAMED_AGE: i64 = 604_800;
    const REFUSED_AGE: i64 = 86_400;
    const NAME: &str = "1DQ1-A - Test Keepstar";
    const KEEPSTAR: &str =
        r#"{"name":"1DQ1-A - Test Keepstar","owner_id":98000001,"solar_system_id":30004759}"#;
    const STRUCTURE_URL: &str = "https://esi.evetech.net/universe/structures/1035466617946";

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What one structure request gets: an HTTP answer, or no connection at all.
    #[derive(Clone)]
    enum Canned {
        Answer(HttpResponse),
        Unreachable,
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &str) -> Canned {
        Canned::Answer(HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
        })
    }

    fn network() -> Canned {
        Canned::Unreachable
    }

    fn access_of(character: i64) -> String {
        format!("ACCESS-{character}")
    }

    /// Answers the token refresh with the character's own token, and each structure request by its bearer token; a token with no answer set gets 403.
    #[derive(Clone, Default)]
    struct Fake {
        answers: Arc<Mutex<BTreeMap<String, Canned>>>,
        calls: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl Fake {
        /// The answer for every structure this character asks about.
        fn answer(&self, character: i64, canned: Canned) {
            locked(&self.answers).insert(access_of(character), canned);
        }

        /// The answer for one structure only; it wins over `answer`.
        fn answer_structure(&self, character: i64, structure: i64, canned: Canned) {
            locked(&self.answers).insert(format!("{}/{structure}", access_of(character)), canned);
        }

        /// The structure requests sent to EVE, in order; token refreshes are left out.
        fn esi_calls(&self) -> Vec<HttpRequest> {
            locked(&self.calls)
                .iter()
                .filter(|call| call.url.starts_with(BASE_URL))
                .cloned()
                .collect()
        }
    }

    impl Http for Fake {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, EsiError> {
            locked(&self.calls).push(request.clone());
            if request.url.starts_with(TOKEN_URL) {
                let refresh = form_field(request, "refresh_token");
                let character = refresh.trim_start_matches("REFRESH-");
                let body = format!(
                    r#"{{"access_token":"ACCESS-{character}","expires_in":1199,"token_type":"Bearer","refresh_token":"{refresh}"}}"#
                );
                return Ok(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: body.into_bytes(),
                });
            }
            let token = bearer(request).unwrap_or_default();
            let structure = request.url.rsplit('/').next().unwrap_or_default();
            let answers = locked(&self.answers);
            let canned = answers
                .get(&format!("{token}/{structure}"))
                .or_else(|| answers.get(&token))
                .cloned()
                .unwrap_or_else(|| reply(403, &[], ""));
            match canned {
                Canned::Answer(response) => Ok(response),
                Canned::Unreachable => Err(EsiError::Network("no route to host".to_owned())),
            }
        }
    }

    fn form_field(request: &HttpRequest, key: &str) -> String {
        let body =
            String::from_utf8_lossy(request.body.as_deref().unwrap_or_default()).into_owned();
        body.split('&')
            .find_map(|pair| {
                pair.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .unwrap_or_default()
            .to_owned()
    }

    /// The token text after `Bearer ` in the request's `Authorization` header.
    fn bearer(request: &HttpRequest) -> Option<String> {
        request
            .headers
            .iter()
            .find(|(name, _)| name == "Authorization")
            .map(|(_, value)| value.trim_start_matches("Bearer ").to_owned())
    }

    struct Setup {
        dir: TempDir,
        fake: Fake,
        auth: Auth,
    }

    fn setup() -> Setup {
        let dir = TempDir::new().expect("temp dir");
        let fake = Fake::default();
        let store = FileStore::new(dir.path());
        let mut list = CharacterList::open(dir.path()).expect("open list");
        for id in CHARACTERS {
            let id = u64::try_from(id).expect("character id");
            store
                .set(id, &RefreshToken::new(format!("REFRESH-{id}")))
                .expect("store token");
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

    fn run(
        setup: &mut Setup,
        names: &mut StructureNames,
        ids: &[i64],
        now: i64,
    ) -> Result<NamingReport, EsiError> {
        name_structures(
            setup.dir.path(),
            names,
            ids,
            &CHARACTERS,
            &mut setup.auth,
            &setup.fake,
            now,
        )
    }

    /// Structure `STRUCTURE` named by the second character after the first refuses it, at `NOW`.
    fn named(setup: &mut Setup) -> StructureNames {
        setup.fake.answer(CHARACTER_A, reply(403, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(200, &[], KEEPSTAR));
        let mut names = StructureNames::new();
        run(setup, &mut names, &[STRUCTURE], NOW).expect("name");
        names
    }

    fn entry(names: &StructureNames) -> &StructureEntry {
        &names[&STRUCTURE]
    }

    /// A refused entry checked at `checked_at`, for a structure whose next check is worked out from that time.
    fn refused_at(checked_at: i64) -> StructureEntry {
        StructureEntry {
            name: None,
            system_id: None,
            refused: true,
            checked_at,
        }
    }

    #[test]
    fn the_second_character_names_a_structure_the_first_refuses() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, reply(403, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(200, &[], KEEPSTAR));
        let mut names = StructureNames::new();
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("name");
        let calls = setup.fake.esi_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].url, STRUCTURE_URL);
        assert_eq!(bearer(&calls[0]).as_deref(), Some("ACCESS-2112625428"));
        assert_eq!(calls[1].url, STRUCTURE_URL);
        assert_eq!(bearer(&calls[1]).as_deref(), Some("ACCESS-95465499"));
        let found = entry(&names);
        assert_eq!(found.name.as_deref(), Some(NAME));
        assert_eq!(found.system_id, Some(30_004_759));
        assert!(!found.refused);
        assert_eq!(found.checked_at, NOW);
        assert_eq!(report.asked, 1);
        assert!(report.failed.is_empty());
        assert_eq!(place_label(STRUCTURE, None, None, &names), NAME);
    }

    #[test]
    fn a_name_is_asked_again_only_after_a_week() {
        let mut setup = setup();
        let mut names = named(&mut setup);
        let before = setup.fake.esi_calls().len();
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW + NAMED_AGE - 1).expect("run");
        assert_eq!(setup.fake.esi_calls().len(), before);
        assert_eq!(report.asked, 0);
        run(&mut setup, &mut names, &[STRUCTURE], NOW + NAMED_AGE).expect("run");
        assert_eq!(setup.fake.esi_calls().len(), before + 2);
    }

    #[test]
    fn every_character_refused_means_no_access_asked_again_after_a_day() {
        let mut setup = setup();
        let mut names = StructureNames::new();
        run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("run");
        let found = entry(&names);
        assert!(found.refused);
        assert_eq!(found.name, None);
        assert_eq!(
            place_label(STRUCTURE, None, None, &names),
            "Structure 1035466617946 (no access)"
        );
        let before = setup.fake.esi_calls().len();
        run(&mut setup, &mut names, &[STRUCTURE], NOW + REFUSED_AGE - 1).expect("run");
        assert_eq!(setup.fake.esi_calls().len(), before);
        run(&mut setup, &mut names, &[STRUCTURE], NOW + REFUSED_AGE).expect("run");
        assert_eq!(setup.fake.esi_calls().len(), before + 2);
    }

    #[test]
    fn a_refusal_keeps_the_old_name_and_is_asked_after_a_day() {
        let mut setup = setup();
        let mut names = named(&mut setup);
        let later = NOW + NAMED_AGE;
        setup.fake.answer(CHARACTER_B, reply(403, &[], ""));
        run(&mut setup, &mut names, &[STRUCTURE], later).expect("run");
        let found = entry(&names);
        assert_eq!(found.name.as_deref(), Some(NAME));
        assert!(found.refused);
        assert_eq!(found.checked_at, later);
        assert_eq!(place_label(STRUCTURE, None, None, &names), NAME);
        let before = setup.fake.esi_calls().len();
        run(
            &mut setup,
            &mut names,
            &[STRUCTURE],
            later + REFUSED_AGE - 1,
        )
        .expect("run");
        assert_eq!(setup.fake.esi_calls().len(), before);
        run(&mut setup, &mut names, &[STRUCTURE], later + REFUSED_AGE).expect("run");
        assert_eq!(setup.fake.esi_calls().len(), before + 2);
    }

    #[test]
    fn a_server_error_keeps_the_entry_and_the_next_call_asks_again() {
        let mut setup = setup();
        let mut names = named(&mut setup);
        let before = names.clone();
        let later = NOW + NAMED_AGE;
        setup.fake.answer(CHARACTER_A, reply(503, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(403, &[], ""));
        let report = run(&mut setup, &mut names, &[STRUCTURE], later).expect("run");
        assert_eq!(names, before);
        assert_eq!(report.asked, 1);
        assert!(matches!(
            report.failed.as_slice(),
            [(id, EsiError::Server(503))] if *id == STRUCTURE
        ));
        assert_eq!(place_label(STRUCTURE, None, None, &names), NAME);
        let calls = setup.fake.esi_calls().len();
        run(&mut setup, &mut names, &[STRUCTURE], later + 1).expect("run");
        assert_eq!(setup.fake.esi_calls().len(), calls + 2);
    }

    #[test]
    fn a_server_error_with_no_entry_leaves_no_entry() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, reply(503, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(403, &[], ""));
        let mut names = StructureNames::new();
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("run");
        assert!(names.is_empty());
        assert!(matches!(
            report.failed.as_slice(),
            [(_, EsiError::Server(503))]
        ));
        assert_eq!(
            place_label(STRUCTURE, None, None, &names),
            "Structure 1035466617946"
        );
    }

    #[test]
    fn network_and_unauthorized_and_empty_names_are_failures_with_no_entry() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, network());
        setup.fake.answer(CHARACTER_B, network());
        let mut names = StructureNames::new();
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("run");
        assert!(names.is_empty());
        assert!(matches!(
            report.failed.as_slice(),
            [(_, EsiError::Network(_))]
        ));

        setup.fake.answer(CHARACTER_A, reply(401, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(401, &[], ""));
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("run");
        assert!(names.is_empty());
        assert!(matches!(
            report.failed.as_slice(),
            [(_, EsiError::Unexpected { status: 401, .. })]
        ));

        let empty = r#"{"name":"","owner_id":98000001,"solar_system_id":30004759}"#;
        setup.fake.answer(CHARACTER_A, reply(200, &[], empty));
        setup.fake.answer(CHARACTER_B, reply(200, &[], empty));
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("run");
        assert!(names.is_empty());
        assert!(matches!(
            report.failed.as_slice(),
            [(_, EsiError::BadData { field: "name", .. })]
        ));
    }

    #[test]
    fn an_error_limit_stops_the_run_at_once_and_keeps_the_entries() {
        let mut setup = setup();
        let mut names = named(&mut setup);
        let before = names.clone();
        let later = NOW + NAMED_AGE;
        setup.fake.answer(
            CHARACTER_A,
            reply(420, &[("x-esi-error-limit-reset", "41")], ""),
        );
        let calls = setup.fake.esi_calls().len();
        let result = run(&mut setup, &mut names, &[STRUCTURE, OTHER_STRUCTURE], later);
        assert!(matches!(
            result,
            Err(EsiError::ErrorLimited { reset_seconds: 41 })
        ));
        assert_eq!(setup.fake.esi_calls().len(), calls + 1);
        assert_eq!(names, before);
    }

    #[test]
    fn stations_systems_and_duplicates_are_not_asked() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, reply(403, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(403, &[], ""));
        let mut names = StructureNames::new();
        let report = run(
            &mut setup,
            &mut names,
            &[JITA_STATION, JITA, STRUCTURE, STRUCTURE],
            NOW,
        )
        .expect("run");
        let calls = setup.fake.esi_calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|call| call.url == STRUCTURE_URL));
        assert_eq!(report.asked, 1);
    }

    #[test]
    fn no_character_or_no_usable_character_asks_nothing() {
        let mut setup = setup();
        let mut names = StructureNames::new();
        let report = name_structures(
            setup.dir.path(),
            &mut names,
            &[STRUCTURE],
            &[],
            &mut setup.auth,
            &setup.fake,
            NOW,
        )
        .expect("run");
        assert_eq!(report.asked, 0);
        let report = name_structures(
            setup.dir.path(),
            &mut names,
            &[STRUCTURE],
            &[-1],
            &mut setup.auth,
            &setup.fake,
            NOW,
        )
        .expect("run");
        assert_eq!(report.asked, 0);
        assert!(setup.fake.esi_calls().is_empty());
        assert!(names.is_empty());
    }

    #[test]
    fn labels_follow_the_order_for_structures_and_for_stations() {
        let mut setup = setup();
        let names = named(&mut setup);
        assert_eq!(
            place_label(STRUCTURE, Some("Home Raitaru"), None, &names),
            "Home Raitaru"
        );
        assert_eq!(place_label(STRUCTURE, Some("   "), None, &names), NAME);
        assert_eq!(
            place_label(
                JITA_STATION,
                Some("X"),
                Some("Jita IV - Moon 4 - Caldari Navy Assembly Plant"),
                &names
            ),
            "Jita IV - Moon 4 - Caldari Navy Assembly Plant"
        );
        assert_eq!(
            place_label(JITA_STATION, None, None, &names),
            "Location 60003760"
        );
        assert_eq!(place_label(JITA, None, Some("Jita"), &names), "Jita");
        assert_eq!(
            place_label(STRUCTURE, None, None, &StructureNames::new()),
            "Structure 1035466617946"
        );
    }

    #[test]
    fn saved_names_load_back_equal_and_a_failed_run_leaves_the_file_alone() {
        let mut setup = setup();
        let mut names = named(&mut setup);
        let path = setup.dir.path().join("places.json");
        assert_eq!(load_places(setup.dir.path()).expect("load"), names);
        let before = fs::read(&path).expect("read");
        let later = NOW + NAMED_AGE;
        setup.fake.answer(CHARACTER_A, reply(503, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(403, &[], ""));
        run(&mut setup, &mut names, &[STRUCTURE], later).expect("run");
        assert_eq!(fs::read(&path).expect("read"), before);
    }

    #[test]
    fn a_saved_file_of_another_version_is_bad_and_left_alone() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("places.json");
        fs::write(&path, r#"{"version":2,"structures":{}}"#).expect("write");
        let before = fs::read(&path).expect("read");
        assert!(matches!(
            load_places(dir.path()),
            Err(EsiError::BadFile { .. })
        ));
        assert_eq!(fs::read(&path).expect("read"), before);
    }

    #[test]
    fn a_missing_saved_file_is_an_empty_map() {
        let dir = TempDir::new().expect("temp dir");
        assert!(load_places(dir.path()).expect("load").is_empty());
    }

    #[test]
    fn no_token_text_appears_in_urls_errors_or_debug_output() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, network());
        setup.fake.answer(CHARACTER_B, reply(503, &[], ""));
        let mut names = StructureNames::new();
        let report = run(&mut setup, &mut names, &[STRUCTURE], NOW).expect("run");
        let shown = format!("{report:?} {:?}", setup.auth);
        assert!(!shown.contains("ACCESS-"));
        assert!(!shown.contains("REFRESH-"));
        for (_, error) in &report.failed {
            assert!(!error.to_string().contains("ACCESS-"));
        }
        for call in setup.fake.esi_calls() {
            assert!(!call.url.contains("ACCESS-"));
            assert!(!call.url.contains("REFRESH-"));
        }
    }

    #[test]
    fn a_checked_time_that_overflows_is_that_structures_failure_only() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, reply(403, &[], ""));
        setup.fake.answer(CHARACTER_B, reply(403, &[], ""));
        let mut names = StructureNames::new();
        names.insert(STRUCTURE, refused_at(i64::MAX));
        let report = run(&mut setup, &mut names, &[STRUCTURE, OTHER_STRUCTURE], NOW).expect("run");
        assert_eq!(report.asked, 1);
        assert!(matches!(
            report.failed.as_slice(),
            [(id, EsiError::BadData { field: "checked_at", .. })] if *id == STRUCTURE
        ));
        assert!(
            setup
                .fake
                .esi_calls()
                .iter()
                .all(|call| call.url.ends_with(&OTHER_STRUCTURE.to_string()))
        );
        assert_eq!(setup.fake.esi_calls().len(), 2);
    }

    #[test]
    fn an_error_limit_is_returned_even_when_saving_the_run_fails() {
        let mut setup = setup();
        let mut names = named(&mut setup);
        let later = NOW + NAMED_AGE;
        // A saved file cannot be replaced while a folder sits where its temporary file goes.
        fs::create_dir(setup.dir.path().join("places.json.tmp")).expect("obstacle");
        setup.fake.answer_structure(
            CHARACTER_A,
            OTHER_STRUCTURE,
            reply(420, &[("x-esi-error-limit-reset", "41")], ""),
        );
        let result = run(&mut setup, &mut names, &[STRUCTURE, OTHER_STRUCTURE], later);
        assert!(matches!(
            result,
            Err(EsiError::ErrorLimited { reset_seconds: 41 })
        ));
        assert_eq!(entry(&names).checked_at, later);
    }

    #[test]
    fn the_first_structure_id_is_asked_and_labelled_as_a_structure() {
        let mut setup = setup();
        setup.fake.answer(CHARACTER_A, reply(200, &[], KEEPSTAR));
        let mut names = StructureNames::new();
        let report = run(&mut setup, &mut names, &[FIRST_STRUCTURE_ID], NOW).expect("run");
        assert_eq!(report.asked, 1);
        assert_eq!(place_label(FIRST_STRUCTURE_ID, None, None, &names), NAME);
        assert_eq!(
            place_label(FIRST_STRUCTURE_ID - 1, None, None, &names),
            format!("Location {}", FIRST_STRUCTURE_ID - 1)
        );
    }

    #[test]
    fn a_saved_file_that_cannot_be_read_is_bad_and_left_alone() {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir(dir.path().join("places.json")).expect("make a folder in its place");
        assert!(matches!(
            load_places(dir.path()),
            Err(EsiError::BadFile { .. })
        ));
        assert!(dir.path().join("places.json").is_dir());
    }
}
