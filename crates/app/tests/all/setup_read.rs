//! Acceptance tests for the Fees and skills panel's Read row: reading a character's skills and
//! standings, keeping a typed broker fee, the refusal notice, and the no-character state.
//! The game data is the build 3607705 fixture, imported in each test; the store is in a temp folder.
//! The clock is fixed at 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use crate::ui::shown;
    use egui::accesskit::Role;
    use egui::{Key, Modifiers, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::rail::Page;
    use eve_means_of_profit::session::{EveAccess, PendingLogin};
    use eve_means_of_profit::status::Level;
    use eve_means_of_profit::{App, AppConfig};
    use mop_core::fees::Standing;
    use mop_core::money::Rate;
    use mop_esi::character_data::{CharacterData, Kind, Refreshed};
    use mop_esi::characters::{Character, Login};
    use mop_esi::error::EsiError;
    use mop_esi::places::{NamingReport, StructureNames};
    use mop_esi::skills::{CharacterSkills, NpcStanding, StandingFrom};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::{FeeSettings, Store};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    /// The character these tests read.
    const ANA: u64 = 90_000_001;

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
    struct Env {
        _dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    impl Env {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let data = dir.path().join("data");
            let cache = dir.path().join("cache");
            fs::create_dir_all(&data).unwrap();
            import(&fixture(), &data.join("sde.sqlite")).unwrap();
            let env = Self {
                _dir: dir,
                data,
                cache,
            };
            // The daily check is not due: the store says it ran today.
            let mut store = env.store();
            let mut settings = store.settings().unwrap();
            settings.last_sde_check = Some(NOW);
            store.save_settings(&settings).unwrap();
            // The background price refresh stays off: no test here asks for prices.
            let mut history = store.history_settings().unwrap();
            history.auto_refresh = false;
            store.save_history_settings(&history).unwrap();
            env
        }

        fn store(&self) -> Store {
            Store::open(&self.data.join("user.sqlite"), NOW)
                .unwrap()
                .store
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

    fn ana() -> Character {
        character(ANA, "Ana Tester")
    }

    /// A login with the characters given. Its read answers Ana's skills and standings; `refuse`
    /// makes it answer EVE's refusal instead.
    #[derive(Clone)]
    struct FakeEve {
        characters: Vec<Character>,
        refuse: Arc<AtomicBool>,
    }

    impl FakeEve {
        fn new(characters: Vec<Character>) -> (Self, Arc<AtomicBool>) {
            let refuse = Arc::new(AtomicBool::new(false));
            (
                Self {
                    characters,
                    refuse: Arc::clone(&refuse),
                },
                refuse,
            )
        }
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

        fn read_skills_and_standings(
            &mut self,
            character_id: u64,
            _now: i64,
        ) -> Result<(CharacterSkills, Vec<NpcStanding>), EsiError> {
            if self.refuse.load(Ordering::Relaxed) {
                return Err(EsiError::Server(403));
            }
            if character_id != ANA {
                return Err(EsiError::UnknownCharacter(character_id));
            }
            Ok((ana_skills(), ana_standings()))
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

    /// Ana's skills: the fee skills and two more, as ESI sends every trained skill.
    fn ana_skills() -> CharacterSkills {
        CharacterSkills {
            levels: [(3_446, 5), (11_453, 4), (16_622, 4), (23_121, 3)]
                .into_iter()
                .collect(),
            total_sp: 21_000_000,
        }
    }

    /// Ana's standings: the Caldari State and the Caldari Navy.
    fn ana_standings() -> Vec<NpcStanding> {
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
        ]
    }

    /// The window the checks run in: tall, so that a drop-down's list fits below its box.
    fn start(env: &Env, eve: FakeEve) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(eve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 1400.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.run_steps(2);
        click(&mut harness, Page::Setup.label());
        assert!(shown(&harness, "Price rule"), "the setup panels show");
        wait_for_session(&mut harness);
        harness
    }

    /// Runs frames until the worker's start-up read has arrived, so the login is known.
    fn wait_for_session(harness: &mut Harness<'_, App>) {
        for _ in 0..500 {
            harness.step();
            if harness.state_mut().shared_mut().session.loaded() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the start-up read did not arrive");
    }

    /// Whether a combo box showing `value` sits on the row of the first node labelled `anchor`.
    fn value_on_row(harness: &Harness<'_, App>, anchor: &str, value: &str) -> bool {
        let Some(row) = harness
            .query_all_by_label(anchor)
            .next()
            .map(|node| node.rect().center().y)
        else {
            return false;
        };
        harness
            .query_all_by_value(value)
            .any(|node| (node.rect().center().y - row).abs() <= 3.0)
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

    /// The text in the text field with this caption; empty when it has none.
    fn field_text(harness: &Harness<'_, App>, caption: &str) -> String {
        harness
            .query_all_by_label(caption)
            .find(|node| node.accesskit_node().role() == Role::TextInput)
            .unwrap_or_else(|| panic!("no text field captioned {caption}"))
            .accesskit_node()
            .value()
            .as_deref()
            .unwrap_or_default()
            .to_owned()
    }

    /// Whether the first node with this label is disabled.
    fn disabled(harness: &Harness<'_, App>, label: &str) -> bool {
        harness
            .query_all_by_label(label)
            .next()
            .unwrap_or_else(|| panic!("no node labelled {label}"))
            .accesskit_node()
            .is_disabled()
    }

    /// Replaces the text of a field: selects all of it, then types the new text.
    fn replace_text(harness: &mut Harness<'_, App>, caption: &str, text: &str) {
        reveal_field(harness, caption);
        {
            let field = harness
                .query_all_by_label(caption)
                .find(|node| node.accesskit_node().role() == Role::TextInput)
                .unwrap_or_else(|| panic!("no text field captioned {caption}"));
            field.click();
        }
        harness.run_steps(2);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.key_press(Key::Backspace);
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

    /// Opens the Fees and skills tab. A tab's title is painted by the dock, not a labelled node, so the
    /// tab is pressed where it is drawn: in the tab bar over the Structures panel, after its title.
    fn open_fees(harness: &mut Harness<'_, App>) {
        let header = harness
            .query_all_by_value("Structures")
            .next()
            .expect("the Structures panel shows")
            .rect();
        let at = egui::pos2(header.min.x + 120.0, header.min.y - 18.0);
        harness.event(egui::Event::PointerMoved(at));
        harness.run_steps(2);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run_steps(2);
        }
        harness.run_steps(3);
    }

    fn standing(hundredths: i16) -> Standing {
        Standing::from_hundredths(hundredths).unwrap()
    }

    /// Runs frames until a notice of this level and text is held.
    fn wait_for_notice(harness: &mut Harness<'_, App>, level: Level, text: &str) {
        for _ in 0..500 {
            harness.step();
            if harness
                .state_mut()
                .shared_mut()
                .notices
                .iter()
                .any(|notice| notice.level == level && notice.text == text)
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the notice {text:?} did not show");
    }

    #[test]
    fn reading_a_character_fills_the_skills_standings_and_fees() {
        let env = Env::new();
        let (eve, _refuse) = FakeEve::new(vec![ana()]);
        let mut harness = start(&env, eve);
        open_fees(&mut harness);
        assert!(shown(&harness, "Ana Tester"), "the character is picked");

        click(&mut harness, "Read");
        wait_for_notice(
            &mut harness,
            Level::Notice,
            "Read skills and standings from Ana Tester.",
        );
        harness.run_steps(2);

        assert!(value_on_row(&harness, "Accounting", "4"), "Accounting is 4");
        assert!(
            value_on_row(&harness, "Broker Relations", "5"),
            "Broker Relations is 5"
        );
        assert_eq!(field_text(&harness, "Caldari State standing"), "5.23");
        assert_eq!(field_text(&harness, "Caldari Navy standing"), "-1.26");
        assert!(shown(&harness, "Sales tax: 4.20%"));
        assert!(shown(&harness, "Broker fee: 1.3683%"));
        assert_eq!(
            env.store().fee_settings().unwrap(),
            FeeSettings {
                accounting: 4,
                broker_relations: 5,
                faction_standing: standing(523),
                corporation_standing: standing(-126),
                broker_override: None,
            }
        );
    }

    #[test]
    fn a_read_keeps_a_broker_fee_typed_before_it() {
        let env = Env::new();
        let (eve, _refuse) = FakeEve::new(vec![ana()]);
        let mut harness = start(&env, eve);
        open_fees(&mut harness);
        replace_text(&mut harness, "Broker fee override %", "1.5");
        assert_eq!(
            env.store().fee_settings().unwrap().broker_override,
            Some(Rate::from_ppm(15_000))
        );

        click(&mut harness, "Read");
        wait_for_notice(
            &mut harness,
            Level::Notice,
            "Read skills and standings from Ana Tester.",
        );
        harness.run_steps(2);

        assert_eq!(field_text(&harness, "Broker fee override %"), "1.5");
        assert!(shown(&harness, "Broker fee: 1.50% (typed)"));
        assert_eq!(
            env.store().fee_settings().unwrap(),
            FeeSettings {
                accounting: 4,
                broker_relations: 5,
                faction_standing: standing(523),
                corporation_standing: standing(-126),
                broker_override: Some(Rate::from_ppm(15_000)),
            }
        );
    }

    #[test]
    fn a_refused_read_leaves_the_fields_and_says_to_log_in_again() {
        let env = Env::new();
        let (eve, refuse) = FakeEve::new(vec![ana()]);
        let mut harness = start(&env, eve);
        open_fees(&mut harness);
        click_combo(&mut harness, "0", 0);
        click_nth(&mut harness, "3", 0);
        click_combo(&mut harness, "0", 0);
        click_nth(&mut harness, "2", 0);
        replace_text(&mut harness, "Caldari State standing", "2.00");
        replace_text(&mut harness, "Caldari Navy standing", "-0.50");
        let before = env.store().fee_settings().unwrap();
        assert_eq!(
            before,
            FeeSettings {
                accounting: 3,
                broker_relations: 2,
                faction_standing: standing(200),
                corporation_standing: standing(-50),
                broker_override: None,
            }
        );

        refuse.store(true, Ordering::Relaxed);
        click(&mut harness, "Read");
        wait_for_notice(
            &mut harness,
            Level::Error,
            "Could not read Ana Tester: EVE refused. Log in again and allow skills and standings.",
        );
        harness.run_steps(2);

        assert!(shown(&harness, "ERROR"));
        assert!(shown(
            &harness,
            "Could not read Ana Tester: EVE refused. Log in again and allow skills and standings."
        ));
        assert!(
            value_on_row(&harness, "Accounting", "3"),
            "Accounting stays 3"
        );
        assert!(
            value_on_row(&harness, "Broker Relations", "2"),
            "Broker Relations stays 2"
        );
        assert_eq!(field_text(&harness, "Caldari State standing"), "2.00");
        assert_eq!(field_text(&harness, "Caldari Navy standing"), "-0.50");
        assert_eq!(env.store().fee_settings().unwrap(), before);
    }

    #[test]
    fn with_no_character_the_read_row_says_so_and_disables_read() {
        let env = Env::new();
        let (eve, _refuse) = FakeEve::new(Vec::new());
        let mut harness = start(&env, eve);
        open_fees(&mut harness);

        assert!(shown(&harness, "No character logged in"));
        assert!(disabled(&harness, "Read"));
    }
}
