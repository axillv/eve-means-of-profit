//! The login against a scripted EVE: PKCE, the browser address, the callback, token checks,
//! refresh, the character list and the secret stores. Tests use fake HTTP and fixed times.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread::{self, sleep};
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use base64::prelude::BASE64_URL_SAFE_NO_PAD;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use profit_esi::auth::{Auth, LoginStart, REVOKE_URL, SCOPES, TOKEN_URL, authorize_url};
    use profit_esi::callback::wait_for_code;
    use profit_esi::characters::Login;
    use profit_esi::error::EsiError;
    use profit_esi::http::{Http, HttpRequest, HttpResponse, Method};
    use profit_esi::jwt::{self, Jwks};
    use profit_esi::pkce::Pkce;
    use profit_esi::secrets::{
        FileStore, KeychainFirst, SecretStore, Secrets, fallback_warning, open_secrets,
    };
    use profit_esi::token::{AccessToken, RefreshToken};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    const CLIENT: &str = "abc123";
    const NOW: i64 = 1_792_404_000;
    const EXPIRES: i64 = 1_792_405_199;
    const CHARACTER: u64 = 2_112_625_428;
    const OTHER: u64 = 95_465_499;
    const STATE: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const VERIFIER: [u8; 32] = [
        116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37,
        77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
    ];
    const EXPECTED_SCOPES: [&str; 17] = [
        "esi-assets.read_assets.v1",
        "esi-universe.read_structures.v1",
        "esi-search.search_structures.v1",
        "esi-markets.read_character_orders.v1",
        "esi-markets.structure_markets.v1",
        "esi-industry.read_character_jobs.v1",
        "esi-industry.read_character_mining.v1",
        "esi-wallet.read_character_wallet.v1",
        "esi-skills.read_skills.v1",
        "esi-characters.read_blueprints.v1",
        "esi-characters.read_standings.v1",
        "esi-characters.read_loyalty.v1",
        "esi-contracts.read_character_contracts.v1",
        "esi-planets.manage_planets.v1",
        "esi-location.read_location.v1",
        "esi-ui.open_window.v1",
        "esi-ui.write_waypoint.v1",
    ];
    const JWKS: &[u8] = include_bytes!("fixtures/jwks.json");
    const RS_DER: &[u8] = include_bytes!("fixtures/rs256.der");
    const RS_OTHER_DER: &[u8] = include_bytes!("fixtures/rs256-other.der");
    const ES_DER: &[u8] = include_bytes!("fixtures/es256.der");
    const ES_OTHER_DER: &[u8] = include_bytes!("fixtures/es256-other.der");
    const JWKS_URL: &str = "https://login.eveonline.com/oauth/jwks";

    type Reply = Result<HttpResponse, EsiError>;

    fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers from a queue, in order, and records every request. Clones share the queue and the log.
    #[derive(Clone, Default)]
    struct Fake {
        replies: Arc<Mutex<VecDeque<Reply>>>,
        calls: Arc<Mutex<Vec<HttpRequest>>>,
    }

    impl Fake {
        fn with(replies: Vec<HttpResponse>) -> Self {
            let fake = Self::default();
            locked(&fake.replies).extend(replies.into_iter().map(Ok));
            fake
        }

        fn calls_to(&self, url: &str) -> Vec<HttpRequest> {
            locked(&self.calls)
                .iter()
                .filter(|call| call.url == url)
                .cloned()
                .collect()
        }

        fn posts_to(&self, url: &str) -> Vec<HttpRequest> {
            self.calls_to(url)
                .into_iter()
                .filter(|call| call.method == Method::Post)
                .collect()
        }

        fn call_count(&self) -> usize {
            locked(&self.calls).len()
        }
    }

    impl Http for Fake {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, EsiError> {
            locked(&self.calls).push(request.clone());
            locked(&self.replies)
                .pop_front()
                .unwrap_or_else(|| Err(EsiError::Network("no reply scripted".to_owned())))
        }
    }

    /// An in-memory secret store. `refuse_set` makes every write fail.
    #[derive(Clone, Default)]
    struct Memory {
        tokens: Arc<Mutex<BTreeMap<u64, String>>>,
        refuse_set: Arc<AtomicBool>,
    }

    impl Memory {
        fn holds(&self, id: u64) -> Option<String> {
            locked(&self.tokens).get(&id).cloned()
        }
    }

    impl SecretStore for Memory {
        fn get(&self, character_id: u64) -> Result<Option<RefreshToken>, EsiError> {
            Ok(self
                .holds(character_id)
                .map(|text| RefreshToken::new(text.as_str())))
        }

        fn set(&self, character_id: u64, token: &RefreshToken) -> Result<(), EsiError> {
            if self.refuse_set.load(Ordering::SeqCst) {
                return Err(EsiError::Keychain("refused".to_owned()));
            }
            locked(&self.tokens).insert(character_id, token.as_str().to_owned());
            Ok(())
        }

        fn delete(&self, character_id: u64) -> Result<(), EsiError> {
            locked(&self.tokens).remove(&character_id);
            Ok(())
        }
    }

    struct Setup {
        dir: TempDir,
        fake: Fake,
        store: Memory,
        auth: Auth,
    }

    fn setup(replies: Vec<HttpResponse>) -> Setup {
        let dir = TempDir::new().expect("temp dir");
        let fake = Fake::with(replies);
        let store = Memory::default();
        let secrets = Secrets {
            store: Box::new(store.clone()),
            fallback: None,
        };
        let auth = Auth::open(dir.path(), Box::new(fake.clone()), secrets, CLIENT).expect("open");
        Setup {
            dir,
            fake,
            store,
            auth,
        }
    }

    fn start() -> LoginStart {
        LoginStart {
            url: String::new(),
            pkce: Pkce::from_bytes(VERIFIER, [0; 16]),
        }
    }

    fn reply(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn token_reply(access: &str, refresh: &str) -> HttpResponse {
        reply(
            200,
            &json!({
                "access_token": access,
                "expires_in": 1199,
                "token_type": "Bearer",
                "refresh_token": refresh,
            })
            .to_string(),
        )
    }

    fn jwks_reply() -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: JWKS.to_vec(),
        }
    }

    #[derive(Clone, Copy)]
    enum Key {
        Rs,
        RsOther,
        Es,
        EsOther,
    }

    fn sign_with_kid(key: Key, kid: &str, claims: &Value) -> String {
        let (algorithm, encoding) = match key {
            Key::Rs => (Algorithm::RS256, EncodingKey::from_rsa_der(RS_DER)),
            Key::RsOther => (Algorithm::RS256, EncodingKey::from_rsa_der(RS_OTHER_DER)),
            Key::Es => (Algorithm::ES256, EncodingKey::from_ec_der(ES_DER)),
            Key::EsOther => (Algorithm::ES256, EncodingKey::from_ec_der(ES_OTHER_DER)),
        };
        let mut header = Header::new(algorithm);
        header.kid = Some(kid.to_owned());
        encode(&header, claims, &encoding).expect("sign")
    }

    fn sign(key: Key, claims: &Value) -> String {
        let kid = match key {
            Key::Rs | Key::RsOther => "test-rs256",
            Key::Es | Key::EsOther => "test-es256",
        };
        sign_with_kid(key, kid, claims)
    }

    fn standard(sub: &str) -> Value {
        json!({
            "sub": sub,
            "name": "Test Pilot",
            "iss": "https://login.eveonline.com",
            "aud": [CLIENT, "EVE Online"],
            "exp": EXPIRES,
            "scp": SCOPES,
        })
    }

    fn token_for(sub: &str) -> String {
        sign(Key::Rs, &standard(sub))
    }

    fn character_token() -> String {
        token_for("CHARACTER:EVE:2112625428")
    }

    fn body_text(request: &HttpRequest) -> String {
        String::from_utf8(request.body.clone().unwrap_or_default()).expect("utf-8 body")
    }

    fn read_rows(dir: &Path) -> Value {
        let bytes = fs::read(dir.join("characters.json")).expect("characters.json");
        serde_json::from_slice(&bytes).expect("characters.json is JSON")
    }

    #[test]
    fn pkce_matches_rfc_7636_appendix_b() {
        let pkce = Pkce::from_bytes(VERIFIER, [0; 16]);
        assert_eq!(
            pkce.verifier(),
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        );
        assert_eq!(
            pkce.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(pkce.state(), "AAAAAAAAAAAAAAAAAAAAAA");
        let random = Pkce::new().expect("random pair");
        assert_eq!(random.verifier().len(), 43);
        assert_eq!(random.state().len(), 22);
    }

    #[test]
    fn authorize_url_is_exact_and_scopes_are_the_seventeen() {
        let pkce = Pkce::from_bytes(VERIFIER, [0; 16]);
        let url = authorize_url(
            "abc123",
            &[
                "esi-assets.read_assets.v1",
                "esi-wallet.read_character_wallet.v1",
            ],
            &pkce,
        );
        assert_eq!(
            url,
            "https://login.eveonline.com/v2/oauth/authorize?response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A8638%2Fcallback&client_id=abc123&scope=esi-assets.read_assets.v1%20esi-wallet.read_character_wallet.v1&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256&state=AAAAAAAAAAAAAAAAAAAAAA"
        );
        assert_eq!(SCOPES.len(), 17);
        assert_eq!(SCOPES, EXPECTED_SCOPES);
    }

    #[test]
    fn empty_client_id_cannot_start_a_login() {
        let dir = TempDir::new().expect("temp dir");
        let secrets = Secrets {
            store: Box::new(Memory::default()),
            fallback: None,
        };
        let auth = Auth::open(dir.path(), Box::new(Fake::default()), secrets, "").expect("open");
        assert!(matches!(auth.begin_login(), Err(EsiError::NoClientId)));
    }

    #[test]
    fn exchange_stores_the_token_and_lists_the_character() {
        let mut setup = setup(vec![
            token_reply(&character_token(), "R1"),
            jwks_reply(),
            token_reply("fresh-access", "R2"),
        ]);
        let character = setup
            .auth
            .finish_login(&start(), "ab/c", NOW)
            .expect("login");

        let posts = setup.fake.posts_to(TOKEN_URL);
        assert_eq!(posts.len(), 1);
        assert_eq!(
            body_text(&posts[0]),
            "grant_type=authorization_code&code=ab%2Fc&client_id=abc123&code_verifier=dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        );
        assert!(
            posts[0]
                .headers
                .iter()
                .all(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        );
        assert_eq!(setup.store.holds(CHARACTER).as_deref(), Some("R1"));

        let rows = read_rows(setup.dir.path());
        assert_eq!(rows["characters"].as_array().map(Vec::len), Some(1));
        assert_eq!(rows["characters"][0]["id"], CHARACTER);
        assert_eq!(rows["characters"][0]["name"], "Test Pilot");
        assert_eq!(rows["characters"][0]["login"], "ok");
        assert_eq!(rows["characters"][0]["added_at"], NOW);
        assert_eq!(
            rows["characters"][0]["scopes"].as_array().map(Vec::len),
            Some(17)
        );
        assert_eq!(character.id, CHARACTER);
        assert_eq!(character.login, Login::Ok);

        // The access token ends at NOW + 1199 = 1,792,405,199: it is held until 60 s before that.
        let calls = setup.fake.call_count();
        let held = setup
            .auth
            .access_token(CHARACTER, 1_792_405_138)
            .expect("held");
        assert_eq!(held.as_str(), character_token());
        assert_eq!(setup.fake.call_count(), calls);
        let renewed = setup
            .auth
            .access_token(CHARACTER, 1_792_405_139)
            .expect("renewed");
        assert_eq!(renewed.as_str(), "fresh-access");
    }

    #[test]
    fn token_check_accepts_and_refuses_the_right_things() {
        let keys = Jwks::parse(JWKS).expect("keys");
        let now_ok = 1_792_405_258;
        let now_late = 1_792_405_260;

        let token = character_token();
        jwt::verify(&token, &keys, CLIENT, now_ok).expect("accepted at 59 s past expiry");
        assert!(matches!(
            jwt::verify(&token, &keys, CLIENT, now_late),
            Err(EsiError::BadToken("expiry"))
        ));
        // Exactly 60 s past expiry is already too late.
        assert!(matches!(
            jwt::verify(&token, &keys, CLIENT, 1_792_405_259),
            Err(EsiError::BadToken("expiry"))
        ));

        let mut claims = standard("CHARACTER:EVE:2112625428");
        claims["iss"] = json!("login.eveonline.com");
        jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW)
            .expect("login.eveonline.com issuer");
        claims["iss"] = json!("https://evil.example");
        assert!(matches!(
            jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("issuer"))
        ));

        let mut claims = standard("CHARACTER:EVE:2112625428");
        claims["aud"] = json!(["other-app", "EVE Online"]);
        assert!(matches!(
            jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("audience"))
        ));
        claims["aud"] = json!([CLIENT, "other-audience"]);
        assert!(matches!(
            jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("audience"))
        ));
        claims["aud"] = json!(CLIENT);
        assert!(matches!(
            jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("audience"))
        ));
        claims["aud"] = json!("other-app");
        assert!(matches!(
            jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("audience"))
        ));

        for key in [Key::RsOther, Key::EsOther] {
            let forged = sign(key, &standard("CHARACTER:EVE:2112625428"));
            assert!(matches!(
                jwt::verify(&forged, &keys, CLIENT, NOW),
                Err(EsiError::BadToken("signature"))
            ));
        }

        let header = BASE64_URL_SAFE_NO_PAD.encode(br#"{"alg":"none","kid":"test-rs256"}"#);
        let body = BASE64_URL_SAFE_NO_PAD.encode(standard("CHARACTER:EVE:2112625428").to_string());
        assert!(matches!(
            jwt::verify(&format!("{header}.{body}."), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("algorithm"))
        ));

        assert!(matches!(
            jwt::verify(&token_for("CORPORATION:EVE:98000001"), &keys, CLIENT, NOW),
            Err(EsiError::BadToken("subject"))
        ));

        let mut claims = standard("CHARACTER:EVE:2112625428");
        claims["scp"] = json!("esi-assets.read_assets.v1");
        let single = jwt::verify(&sign(Key::Rs, &claims), &keys, CLIENT, NOW).expect("accepted");
        assert_eq!(single.scopes.len(), 1);
        assert_eq!(single.character_id, CHARACTER);
        assert_eq!(single.name, "Test Pilot");
    }

    #[test]
    fn es256_tokens_are_accepted() {
        let keys = Jwks::parse(JWKS).expect("keys");
        let token = sign(Key::Es, &standard("CHARACTER:EVE:2112625428"));
        jwt::verify(&token, &keys, CLIENT, NOW).expect("ES256 accepted");
    }

    #[test]
    fn unknown_key_asks_for_the_key_set_once_more() {
        let unknown = sign_with_kid(Key::Rs, "not-ours", &standard("CHARACTER:EVE:2112625428"));
        let mut setup = setup(vec![
            token_reply(&unknown, "R1"),
            jwks_reply(),
            jwks_reply(),
        ]);
        assert!(matches!(
            setup.auth.finish_login(&start(), "code", NOW),
            Err(EsiError::BadToken("unknown key"))
        ));
        assert_eq!(setup.fake.calls_to(JWKS_URL).len(), 2);
        assert!(setup.auth.characters().is_empty());
    }

    #[test]
    fn renewal_happens_only_inside_the_last_minute_and_rotates_the_token() {
        let mut setup = setup(vec![
            token_reply(&character_token(), "R1"),
            jwks_reply(),
            token_reply("fresh-access", "R2"),
        ]);
        setup
            .auth
            .finish_login(&start(), "code", NOW)
            .expect("login");
        let calls = setup.fake.call_count();

        let held = setup
            .auth
            .access_token(CHARACTER, 1_792_405_100)
            .expect("held");
        assert_eq!(held.as_str(), character_token());
        assert_eq!(setup.fake.call_count(), calls);

        let renewed = setup
            .auth
            .access_token(CHARACTER, 1_792_405_140)
            .expect("renewed");
        assert_eq!(renewed.as_str(), "fresh-access");
        let refresh = setup.fake.posts_to(TOKEN_URL);
        assert_eq!(
            body_text(&refresh[1]),
            "grant_type=refresh_token&refresh_token=R1&client_id=abc123"
        );
        assert_eq!(setup.store.holds(CHARACTER).as_deref(), Some("R2"));
    }

    #[test]
    fn a_failed_store_write_returns_the_error_and_keeps_the_old_token() {
        let mut setup = setup(vec![
            token_reply(&character_token(), "R1"),
            jwks_reply(),
            token_reply("fresh-access", "R2"),
        ]);
        setup
            .auth
            .finish_login(&start(), "code", NOW)
            .expect("login");
        setup.store.refuse_set.store(true, Ordering::SeqCst);

        let result = setup.auth.access_token(CHARACTER, 1_792_405_140);
        let error = result.expect_err("refused");
        assert!(matches!(error, EsiError::Keychain(_)));
        assert_eq!(setup.store.holds(CHARACTER).as_deref(), Some("R1"));
        let text = format!("{error:?} {error}");
        assert!(!text.contains("R1") && !text.contains("R2") && !text.contains("fresh-access"));
    }

    #[test]
    fn refused_and_denied_errors_carry_no_token() {
        let mut setup = setup(vec![reply(403, r#"{"error":"access_denied"}"#)]);
        let error = setup
            .auth
            .finish_login(&start(), "R1", NOW)
            .expect_err("refused");
        assert!(matches!(error, EsiError::Sso { status: 403, .. }));
        let denied = EsiError::Denied("User cancelled".to_owned());
        let text = format!("{error:?} {error} {denied:?} {denied}");
        assert!(!text.contains("R1") && !text.contains("R2"));
    }

    #[test]
    fn subjects_must_be_plain_numbers() {
        let keys = Jwks::parse(JWKS).expect("keys");
        for sub in [
            "CHARACTER:EVE:+2112625428",
            "CHARACTER:EVE:02112625428",
            "CHARACTER:EVE:",
        ] {
            assert!(matches!(
                jwt::verify(&token_for(sub), &keys, CLIENT, NOW),
                Err(EsiError::BadToken("subject"))
            ));
        }
    }

    #[test]
    fn a_refused_refresh_expires_the_login_and_leaks_nothing() {
        let mut setup = setup(vec![
            token_reply(&character_token(), "R1"),
            jwks_reply(),
            reply(
                400,
                r#"{"error":"invalid_grant","error_description":"expired"}"#,
            ),
        ]);
        setup
            .auth
            .finish_login(&start(), "code", NOW)
            .expect("login");

        let error = setup
            .auth
            .access_token(CHARACTER, 1_792_405_140)
            .expect_err("expired");
        assert!(matches!(error, EsiError::LoginExpired));
        assert_eq!(
            read_rows(setup.dir.path())["characters"][0]["login"],
            "expired"
        );
        assert_eq!(setup.store.holds(CHARACTER), None);

        let token = character_token();
        let debug = format!("{:?} {error:?} {error}", setup.auth);
        assert!(!debug.contains("R1") && !debug.contains("R2") && !debug.contains(&token));
        assert_eq!(
            format!("{:?}", AccessToken::new(token.as_str())),
            "AccessToken(…)"
        );
        assert_eq!(format!("{:?}", RefreshToken::new("R1")), "RefreshToken(…)");
        assert!(format!("{:?}", setup.auth).starts_with("Auth {"));
        assert!(error.to_string().contains("expired"));
    }

    #[test]
    fn two_characters_then_re_add_then_remove() {
        let second = token_for("CHARACTER:EVE:95465499");
        let mut readd = standard("CHARACTER:EVE:2112625428");
        readd["scp"] = json!(["esi-assets.read_assets.v1", "esi-skills.read_skills.v1"]);
        let mut setup = setup(vec![
            token_reply(&character_token(), "R1"),
            jwks_reply(),
            token_reply(&second, "R9"),
            token_reply(&sign(Key::Rs, &readd), "R1"),
            reply(200, ""),
            reply(500, ""),
        ]);
        setup.auth.finish_login(&start(), "a", NOW).expect("first");
        setup.auth.finish_login(&start(), "b", NOW).expect("second");
        assert_eq!(setup.fake.calls_to(JWKS_URL).len(), 1);
        assert_eq!(setup.auth.characters().len(), 2);

        let replaced = setup.auth.finish_login(&start(), "c", NOW).expect("re-add");
        assert_eq!(replaced.scopes.len(), 2);
        assert_eq!(setup.auth.characters().len(), 2);
        assert_eq!(setup.auth.characters()[0].id, CHARACTER);
        assert_eq!(setup.auth.characters()[0].scopes.len(), 2);

        setup.auth.remove(OTHER).expect("remove other");
        let revokes = setup.fake.posts_to(REVOKE_URL);
        assert_eq!(revokes.len(), 1);
        assert_eq!(
            body_text(&revokes[0]),
            "token_type_hint=refresh_token&token=R9&client_id=abc123"
        );
        assert_eq!(setup.auth.characters().len(), 1);
        assert_eq!(setup.store.holds(OTHER), None);

        setup
            .auth
            .remove(CHARACTER)
            .expect("remove with failing revoke");
        assert!(setup.auth.characters().is_empty());
        assert_eq!(setup.store.holds(CHARACTER), None);
    }

    #[test]
    fn file_store_writes_a_private_file_and_refuses_unknown_versions() {
        let dir = TempDir::new().expect("temp dir");
        let file = dir.path().join("tokens.json");
        FileStore::new(dir.path())
            .set(CHARACTER, &RefreshToken::new("R1"))
            .expect("set");
        assert_eq!(
            fs::read_to_string(&file).expect("file"),
            r#"{"version":1,"tokens":{"2112625428":"R1"}}"#
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        fs::write(&file, r#"{"version":2,"tokens":{}}"#).expect("write");
        let store = FileStore::new(dir.path());
        assert!(matches!(
            store.get(CHARACTER),
            Err(EsiError::BadFile { .. })
        ));
        assert!(matches!(
            store.set(CHARACTER, &RefreshToken::new("R2")),
            Err(EsiError::BadFile { .. })
        ));
        assert_eq!(
            fs::read_to_string(&file).expect("file"),
            r#"{"version":2,"tokens":{}}"#
        );
    }

    #[test]
    fn keychain_first_reads_the_file_and_a_keychain_write_clears_it() {
        let keychain = Memory::default();
        let file = Memory::default();
        file.set(CHARACTER, &RefreshToken::new("R1")).expect("seed");
        let store = KeychainFirst {
            keychain: Box::new(keychain.clone()),
            file: Box::new(file.clone()),
        };
        assert_eq!(
            store
                .get(CHARACTER)
                .expect("get")
                .map(|token| token.as_str().to_owned()),
            Some("R1".to_owned())
        );
        store.set(CHARACTER, &RefreshToken::new("R2")).expect("set");
        assert_eq!(keychain.holds(CHARACTER).as_deref(), Some("R2"));
        assert_eq!(file.holds(CHARACTER), None);
    }

    #[test]
    fn reading_a_keychain_token_removes_an_old_copy_left_in_the_file() {
        let keychain = Memory::default();
        let file = Memory::default();
        keychain
            .set(CHARACTER, &RefreshToken::new("R2"))
            .expect("seed keychain");
        file.set(CHARACTER, &RefreshToken::new("R1"))
            .expect("seed file");
        let store = KeychainFirst {
            keychain: Box::new(keychain),
            file: Box::new(file.clone()),
        };
        let read = store.get(CHARACTER).expect("get").expect("a token");
        assert_eq!(read.as_str(), "R2");
        assert_eq!(file.holds(CHARACTER), None);
    }

    #[test]
    fn a_keychain_write_deletes_the_emptied_file() {
        let dir = TempDir::new().expect("temp dir");
        FileStore::new(dir.path())
            .set(CHARACTER, &RefreshToken::new("R1"))
            .expect("seed");
        let store = KeychainFirst {
            keychain: Box::new(Memory::default()),
            file: Box::new(FileStore::new(dir.path())),
        };
        store.set(CHARACTER, &RefreshToken::new("R2")).expect("set");
        assert!(!dir.path().join("tokens.json").exists());
    }

    #[test]
    fn fallback_warning_has_the_exact_wording() {
        assert_eq!(
            fallback_warning(
                "/home/a/.local/share/eve-means-of-profit/esi",
                "no secret service"
            ),
            "No system keychain found (no secret service). Logins are kept in a plain file: /home/a/.local/share/eve-means-of-profit/esi/tokens.json. Anyone who can read this file can read these characters' data."
        );
    }

    #[test]
    fn the_keychain_probe_leaves_a_usable_store_or_a_reason() {
        let dir = TempDir::new().expect("temp dir");
        let secrets = open_secrets(dir.path());
        if let Some(reason) = &secrets.fallback {
            assert!(!reason.is_empty());
        }
        // Whichever store was picked must hold a token and give it back.
        secrets
            .store
            .set(OTHER, &RefreshToken::new("probe-check"))
            .expect("set");
        // The plain file is written only when the keychain is missing.
        assert_eq!(
            dir.path().join("tokens.json").exists(),
            secrets.fallback.is_some()
        );
        let read = secrets.store.get(OTHER).expect("get").expect("a token");
        assert_eq!(read.as_str(), "probe-check");
        secrets.store.delete(OTHER).expect("delete");
        assert!(secrets.store.get(OTHER).expect("get").is_none());
        // Either store leaves nothing behind once the token is deleted.
        assert!(!dir.path().join("tokens.json").exists());
    }

    #[test]
    fn renewal_starts_at_exactly_sixty_seconds_left() {
        let mut setup = setup(vec![
            token_reply(&character_token(), "R1"),
            jwks_reply(),
            token_reply("fresh-access", "R2"),
        ]);
        setup
            .auth
            .finish_login(&start(), "code", NOW)
            .expect("login");
        let renewed = setup
            .auth
            .access_token(CHARACTER, 1_792_405_139)
            .expect("renewed");
        assert_eq!(renewed.as_str(), "fresh-access");
    }

    #[test]
    fn unreadable_saved_files_are_bad_files_and_stay_untouched() {
        let dir = TempDir::new().expect("temp dir");
        // A folder where the file should be cannot be read as a file.
        fs::create_dir(dir.path().join("tokens.json")).expect("folder");
        fs::create_dir(dir.path().join("characters.json")).expect("folder");
        assert!(matches!(
            FileStore::new(dir.path()).get(CHARACTER),
            Err(EsiError::BadFile { .. })
        ));
        let secrets = Secrets {
            store: Box::new(Memory::default()),
            fallback: None,
        };
        let opened = Auth::open(dir.path(), Box::new(Fake::default()), secrets, CLIENT);
        assert!(matches!(opened, Err(EsiError::BadFile { .. })));

        let versioned = TempDir::new().expect("temp dir");
        let file = versioned.path().join("characters.json");
        fs::write(&file, r#"{"version":2,"characters":[]}"#).expect("write");
        let secrets = Secrets {
            store: Box::new(Memory::default()),
            fallback: None,
        };
        let opened = Auth::open(versioned.path(), Box::new(Fake::default()), secrets, CLIENT);
        assert!(matches!(opened, Err(EsiError::BadFile { .. })));
        assert_eq!(
            fs::read_to_string(&file).expect("file"),
            r#"{"version":2,"characters":[]}"#
        );
    }

    #[test]
    fn keychain_first_delete_clears_both_stores() {
        let keychain = Memory::default();
        let file = Memory::default();
        keychain
            .set(CHARACTER, &RefreshToken::new("R1"))
            .expect("seed");
        file.set(CHARACTER, &RefreshToken::new("R0")).expect("seed");
        let store = KeychainFirst {
            keychain: Box::new(keychain.clone()),
            file: Box::new(file.clone()),
        };
        store.delete(CHARACTER).expect("delete");
        assert_eq!(keychain.holds(CHARACTER), None);
        assert_eq!(file.holds(CHARACTER), None);
    }

    #[test]
    fn a_missing_token_file_is_empty_and_deleting_from_it_writes_nothing() {
        let dir = TempDir::new().expect("temp dir");
        let store = FileStore::new(dir.path());
        assert!(store.get(CHARACTER).expect("get").is_none());
        store.delete(CHARACTER).expect("delete");
        assert!(!dir.path().join("tokens.json").exists());
        store.set(CHARACTER, &RefreshToken::new("R1")).expect("set");
        store.delete(CHARACTER).expect("delete");
        assert!(!dir.path().join("tokens.json").exists());
    }

    #[test]
    fn token_server_failures_are_told_apart() {
        let long_body = "x".repeat(500);
        let cases = [
            (reply(400, r#"{"error":"invalid_client"}"#), "sso"),
            (reply(401, r#"{"error":"invalid_grant"}"#), "expired"),
            (reply(403, &long_body), "sso"),
            (reply(500, "oops"), "server"),
            (reply(503, "oops"), "server"),
        ];
        for (answer, expected) in cases {
            let mut setup = setup(vec![answer]);
            let error = setup
                .auth
                .finish_login(&start(), "code", NOW)
                .expect_err("refused");
            match (expected, &error) {
                ("sso", EsiError::Sso { body, .. }) => assert!(body.len() <= 300),
                ("expired", EsiError::LoginExpired) => {}
                ("server", EsiError::Server(status)) => assert!(*status >= 500),
                _ => panic!("unexpected error {error:?}"),
            }
        }
    }

    #[test]
    fn percent_decoding_accepts_both_hex_cases_and_refuses_bad_escapes() {
        use profit_esi::http::{decode, encode};
        assert_eq!(decode("a%2fb%2Fc").as_deref(), Some("a/b/c"));
        assert_eq!(decode("%41%5a%7a").as_deref(), Some("AZz"));
        assert_eq!(decode("%g1"), None);
        assert_eq!(decode("%4"), None);
        assert_eq!(encode("a b/c~"), "a%20b%2Fc~");
    }

    #[test]
    fn error_texts_name_the_problem() {
        assert!(EsiError::PortInUse(8638).to_string().contains("8638"));
        assert!(
            EsiError::Denied("nope".to_owned())
                .to_string()
                .contains("nope")
        );
        assert!(EsiError::UnknownCharacter(7).to_string().contains('7'));
        assert!(
            EsiError::BadToken("subject")
                .to_string()
                .contains("subject")
        );
        assert!(EsiError::Server(502).to_string().contains("502"));
    }

    /// The callback tests share port 8638, so they run one at a time.
    static PORT_LOCK: Mutex<()> = Mutex::new(());

    fn port_lock() -> MutexGuard<'static, ()> {
        PORT_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn far_deadline() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    fn assert_port_free() {
        let probe = TcpListener::bind(("127.0.0.1", 8638));
        probe.expect("port 8638 must be free after the wait");
    }

    fn connect() -> TcpStream {
        for _ in 0..200 {
            if let Ok(stream) = TcpStream::connect(("127.0.0.1", 8638)) {
                return stream;
            }
            sleep(Duration::from_millis(10));
        }
        panic!("the callback listener did not start");
    }

    /// One raw HTTP request to the listener; returns the whole answer.
    fn request(path: &str) -> String {
        let mut stream = connect();
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .expect("write request");
        let mut answer = String::new();
        stream.read_to_string(&mut answer).expect("read answer");
        answer
    }

    fn wait_in_thread(
        state: &'static str,
        deadline: Instant,
    ) -> thread::JoinHandle<Result<String, EsiError>> {
        thread::spawn(move || wait_for_code(state, deadline, &AtomicBool::new(false)))
    }

    #[test]
    fn callback_ignores_other_paths_and_returns_the_code() {
        let _guard = port_lock();
        let handle = wait_in_thread(STATE, far_deadline());
        assert!(request("/favicon.ico").starts_with("HTTP/1.1 404"));
        let answer = request(&format!("/callback?code=ab%2Fc&state={STATE}"));
        assert!(answer.starts_with("HTTP/1.1 200"));
        assert!(answer.contains("Login complete."));
        assert_eq!(handle.join().expect("thread").expect("code"), "ab/c");
        assert_port_free();
    }

    #[test]
    fn callback_refuses_another_state() {
        let _guard = port_lock();
        let handle = wait_in_thread(STATE, far_deadline());
        let answer = request("/callback?code=x&state=BBBBBBBBBBBBBBBBBBBBBB");
        assert!(answer.starts_with("HTTP/1.1 400"));
        assert!(matches!(
            handle.join().expect("thread"),
            Err(EsiError::StateMismatch)
        ));
        assert_port_free();
    }

    #[test]
    fn callback_reports_the_player_cancelling() {
        let _guard = port_lock();
        let handle = wait_in_thread(STATE, far_deadline());
        let answer = request("/callback?error=access_denied&error_description=User%20cancelled");
        assert!(answer.starts_with("HTTP/1.1 400"));
        assert!(matches!(
            handle.join().expect("thread"),
            Err(EsiError::Denied(text)) if text == "User cancelled"
        ));
        assert_port_free();
    }

    #[test]
    fn callback_stops_soon_after_cancel() {
        let _guard = port_lock();
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = thread::spawn({
            let cancel = Arc::clone(&cancel);
            move || {
                (
                    wait_for_code(STATE, far_deadline(), &cancel),
                    Instant::now(),
                )
            }
        });
        sleep(Duration::from_millis(100));
        let cancelled_at = Instant::now();
        cancel.store(true, Ordering::SeqCst);
        let (result, returned_at) = handle.join().expect("thread");
        assert!(matches!(result, Err(EsiError::Cancelled)));
        assert!(returned_at.duration_since(cancelled_at) < Duration::from_millis(200));
        assert_port_free();
    }

    #[test]
    fn callback_times_out_without_an_answer() {
        let _guard = port_lock();
        let deadline = Instant::now() + Duration::from_millis(200);
        let result = wait_for_code(STATE, deadline, &AtomicBool::new(false));
        assert!(matches!(result, Err(EsiError::TimedOut)));
        assert_port_free();
    }

    #[test]
    fn callback_reports_a_port_that_is_taken() {
        let _guard = port_lock();
        let taken = TcpListener::bind(("127.0.0.1", 8638)).expect("hold the port");
        let result = wait_for_code(STATE, far_deadline(), &AtomicBool::new(false));
        assert!(matches!(result, Err(EsiError::PortInUse(8638))));
        drop(taken);
        assert_port_free();
    }
}
