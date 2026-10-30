//! Acceptance tests for the Structures panel on the Setup page: the rows, the names the user types
//! and clears, the error for a name that is too long, and the empty text. The game data is the build
//! 3607705 fixture; the store and the structure names are in a temp folder. The clock is fixed at
//! 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use egui::{Event, Key, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::rail::Page;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig};
    use mop_esi::characters::{Character, Login};
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::{Store, StoreError};
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const HOME_FORT: i64 = 1_035_466_617_946;
    const REFUSED: i64 = 1_035_466_617_999;
    const STAGING: i64 = 1_035_466_618_000;
    const MUTE: &str = "Names EVE gives come from your characters' data. A name you type here shows everywhere instead.";
    const WHY: &str = "No structures yet.";
    const WHAT_ADD: &str =
        "The app lists the structures your items sit in once you add a character and refresh it.";
    const WHAT_REFRESH: &str =
        "Refresh a character on the Characters page to find the structures your items are in.";

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
    struct Folders {
        _dir: Option<TempDir>,
        data: PathBuf,
        cache: PathBuf,
    }

    impl Folders {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let root = dir.path().to_path_buf();
            Self::at(&root, Some(dir))
        }

        /// A folder with a path that does not change between runs, for the pictures: the page shows
        /// the data folder, and a temp name would make the picture differ every time.
        fn fixed() -> Self {
            let root = std::env::temp_dir().join("eve-means-of-profit-structures-snapshots");
            drop(fs::remove_dir_all(&root));
            Self::at(&root, None)
        }

        fn at(root: &Path, dir: Option<TempDir>) -> Self {
            let data = root.join("data");
            let cache = root.join("cache");
            fs::create_dir_all(&data).unwrap();
            import(&fixture(), &data.join("sde.sqlite")).unwrap();
            let folders = Self {
                _dir: dir,
                data,
                cache,
            };
            // The daily check is not due: the store says it ran today.
            let mut store = folders.store();
            let mut settings = store.settings().unwrap();
            settings.last_sde_check = Some(NOW);
            store.save_settings(&settings).unwrap();
            folders
        }

        fn store(&self) -> Store {
            Store::open(&self.data.join("user.sqlite"), NOW)
                .unwrap()
                .store
        }

        /// The structures EVE gave, as the naming step writes them: Home Fort is named and in Jita,
        /// and the refused one has no name.
        fn write_places(&self) {
            let text = format!(
                "{{\"version\":1,\"structures\":{{\
                 \"{HOME_FORT}\":{{\"name\":\"Home Fort\",\"system_id\":30000142,\"refused\":false,\"checked_at\":990000}},\
                 \"{REFUSED}\":{{\"name\":null,\"system_id\":null,\"refused\":true,\"checked_at\":999000}}}}}}"
            );
            fs::create_dir_all(self.data.join("esi")).unwrap();
            fs::write(self.data.join("esi").join("places.json"), text).unwrap();
        }

        /// The typed names the rows show: one for a named structure and one for a structure EVE has
        /// no entry for.
        fn type_names(&self) {
            let mut store = self.store();
            store
                .set_structure_name(HOME_FORT, "Home Raitaru", NOW)
                .unwrap();
            store.set_structure_name(STAGING, "Staging", NOW).unwrap();
        }
    }

    /// The window the checks run in: tall, so that the Structures panel shows its rows.
    fn start(folders: &Folders) -> Harness<'static, App> {
        history_off(&folders.data.join("user.sqlite"));
        let config = AppConfig {
            data_dir: folders.data.clone(),
            cache_dir: folders.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(vec2(1280.0, 1400.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        harness.run_steps(2);
        click(&mut harness, Page::Setup.label());
        assert!(shown(&harness, "Price rule"), "the setup panels show");
        harness
    }

    /// Whether some node shows `text`.
    fn shown(harness: &Harness<'_, App>, text: &str) -> bool {
        harness.query_all_by_label(text).next().is_some()
    }

    fn click(harness: &mut Harness<'_, App>, label: &str) {
        {
            let target = harness
                .query_all_by_label(label)
                .next()
                .unwrap_or_else(|| panic!("no node labelled {label}"));
            target.scroll_to_me();
        }
        harness.run_steps(2);
        {
            let target = harness
                .query_all_by_label(label)
                .next()
                .unwrap_or_else(|| panic!("no node labelled {label}"));
            target.click();
        }
        harness.run_steps(3);
    }

    /// Types into the field that has the keyboard focus.
    fn type_text(harness: &mut Harness<'_, App>, text: &str) {
        harness.event(Event::Text(text.to_owned()));
        harness.run_steps(2);
    }

    fn press(harness: &mut Harness<'_, App>, key: Key, times: usize) {
        for _ in 0..times {
            harness.key_press(key);
            harness.run_steps(2);
        }
    }

    /// The label of the place a name shows, from the session the page reads.
    fn label_of(harness: &mut Harness<'_, App>, id: i64) -> String {
        let shared = harness.state_mut().shared_mut();
        shared.session.place_label(id, shared.sde.as_ref())
    }

    /// The one node with this label, and where it is on the screen.
    fn rect_of(harness: &Harness<'_, App>, text: &str) -> egui::Rect {
        let mut nodes = harness.query_all_by_label(text);
        let node = nodes
            .next()
            .unwrap_or_else(|| panic!("no node labelled {text}"));
        assert!(nodes.next().is_none(), "more than one node labelled {text}");
        node.rect()
    }

    /// Every cell of a row is on the row's line, and they run left to right.
    fn assert_row(cells: &[egui::Rect], y: f32) {
        for cell in cells {
            assert!((cell.center().y - y).abs() < 2.0, "a cell is off its row");
        }
        for pair in cells.windows(2) {
            assert!(pair[0].min.x < pair[1].min.x, "the cells run left to right");
        }
    }

    /// The background price refresh stays off here: the test asks for each download itself.
    fn history_off(db: &Path) {
        let mut store = Store::open(db, NOW).expect("the store opens").store;
        let mut settings = store.history_settings().expect("history settings");
        settings.auto_refresh = false;
        store
            .save_history_settings(&settings)
            .expect("history settings saved");
    }

    #[test]
    fn the_panel_lists_the_structures_in_order() {
        let folders = Folders::new();
        folders.write_places();
        folders.type_names();
        let harness = start(&folders);

        for header in ["Facilities", "Freight services", "Price rule", "Structures"] {
            assert!(shown(&harness, header), "the {header} panel shows");
        }
        assert!(!shown(&harness, WHY), "the empty text is absent");
        assert!(
            !shown(&harness, "Add character"),
            "the empty fill is absent"
        );
        assert!(
            !shown(&harness, "Refresh a character"),
            "the empty fill is absent"
        );

        // Row 1: EVE's name, typed name, system, check time.
        let row_one = [
            rect_of(&harness, "Home Fort"),
            rect_of(&harness, "Home Raitaru"),
            rect_of(&harness, "Jita"),
            rect_of(&harness, "2h"),
        ];
        // Row 2: refused, so no typed name and no system.
        let row_two = [
            rect_of(&harness, "Structure 1035466617999 (no access)"),
            rect_of(&harness, "16m · no access"),
        ];
        // Row 3: a typed name only, so no system and no check time.
        let row_three = [
            rect_of(&harness, "Structure 1035466618000"),
            rect_of(&harness, "Staging"),
        ];
        assert_row(&row_one, row_one[0].center().y);
        assert_row(&row_two, row_two[0].center().y);
        assert_row(&row_three, row_three[0].center().y);
        assert!(
            row_one[0].center().y < row_two[0].center().y,
            "row 1 is above row 2"
        );
        assert!(
            row_two[0].center().y < row_three[0].center().y,
            "row 2 is above row 3"
        );

        // Row 2 has no system, row 3 has no system and no check time: three dashes, two on row 3.
        // The Price history panel on the same page also shows dashes; only those on these rows count.
        let (row_two_y, row_three_y) = (row_two[0].center().y, row_three[0].center().y);
        let mut dashes: Vec<f32> = harness
            .query_all_by_label("—")
            .map(|node| node.rect().center().y)
            .filter(|y| (y - row_two_y).abs() < 2.0 || (y - row_three_y).abs() < 2.0)
            .collect();
        dashes.sort_by(f32::total_cmp);
        assert_eq!(
            dashes.len(),
            3,
            "rows 2 and 3 show a dash for each missing value"
        );
        assert!(
            (dashes[0] - row_two_y).abs() < 2.0,
            "the first dash is on row 2"
        );
        assert!(
            (dashes[1] - row_three_y).abs() < 2.0,
            "the second dash is on row 3"
        );
        assert!(
            (dashes[2] - row_three_y).abs() < 2.0,
            "the third dash is on row 3"
        );

        assert!(
            shown(&harness, "3 structures"),
            "the footer counts the rows"
        );
        assert!(
            rect_of(&harness, "3 structures").center().y > row_three_y,
            "the footer is below the rows"
        );
    }

    #[test]
    fn a_typed_name_saves_and_shows_everywhere() {
        let folders = Folders::new();
        folders.write_places();
        folders.type_names();
        let mut harness = start(&folders);

        click(&mut harness, "Structure 1035466617999 (no access)");
        press(&mut harness, Key::Enter, 1);
        type_text(&mut harness, "  Keepstar  ");
        press(&mut harness, Key::Enter, 1);

        assert_eq!(
            folders.store().structure_name(REFUSED).unwrap().as_deref(),
            Some("Keepstar")
        );
        assert_eq!(label_of(&mut harness, REFUSED), "Keepstar");
        assert!(shown(&harness, "Keepstar"), "the row shows the new name");
    }

    #[test]
    fn clearing_a_typed_name_brings_eves_name_back() {
        let folders = Folders::new();
        folders.write_places();
        folders.type_names();
        let mut harness = start(&folders);

        click(&mut harness, "Home Fort");
        press(&mut harness, Key::Enter, 1);
        // The field holds "Home Raitaru", twelve characters.
        press(&mut harness, Key::Backspace, 12);
        press(&mut harness, Key::Enter, 1);

        assert_eq!(folders.store().structure_name(HOME_FORT).unwrap(), None);
        assert_eq!(label_of(&mut harness, HOME_FORT), "Home Fort");
        assert!(
            !shown(&harness, "Home Raitaru"),
            "the row has no typed name"
        );
    }

    #[test]
    fn a_name_too_long_keeps_the_field_open_until_escape() {
        let folders = Folders::new();
        folders.write_places();
        folders.type_names();
        let mut harness = start(&folders);

        click(&mut harness, "Home Fort");
        press(&mut harness, Key::Enter, 1);
        // The field holds "Home Raitaru", twelve characters.
        press(&mut harness, Key::Backspace, 12);
        type_text(&mut harness, &"a".repeat(101));
        press(&mut harness, Key::Enter, 1);

        let error = StoreError::TooLarge.to_string();
        assert!(shown(&harness, &error), "the error shows under the field");
        assert_eq!(
            folders
                .store()
                .structure_name(HOME_FORT)
                .unwrap()
                .as_deref(),
            Some("Home Raitaru")
        );

        press(&mut harness, Key::Escape, 1);
        assert!(!shown(&harness, &error), "Esc closes the field");
        assert!(
            shown(&harness, "Home Raitaru"),
            "the row shows its name again"
        );
        assert_eq!(label_of(&mut harness, HOME_FORT), "Home Raitaru");
    }

    #[test]
    fn with_no_structures_the_panel_says_so() {
        let folders = Folders::new();
        let harness = start(&folders);

        assert!(shown(&harness, WHY), "the empty text shows");
        assert!(shown(&harness, MUTE), "the panel's explanation stays");
        assert!(
            rect_of(&harness, MUTE).bottom() < rect_of(&harness, WHY).top(),
            "the explanation stays above the empty state"
        );
        assert!(!shown(&harness, "0 structures"), "no table or footer shows");
    }

    #[test]
    fn with_no_characters_the_empty_state_offers_add_character() {
        let folders = Folders::new();
        let mut harness = start(&folders);

        assert!(shown(&harness, WHY));
        assert!(shown(&harness, WHAT_ADD), "it says what adds a character");
        assert!(shown(&harness, "Add character"), "the one fill shows");
        assert!(
            !shown(&harness, "Refresh a character"),
            "the empty state draws no second button"
        );

        click(&mut harness, "Add character");
        assert!(
            shown(&harness, "jEve data"),
            "Add character opens the Characters page"
        );
    }

    #[test]
    fn with_a_character_but_no_structures_the_empty_state_offers_refresh() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        harness
            .state_mut()
            .shared_mut()
            .session
            .put_character(Character {
                id: 42,
                name: "Main".to_owned(),
                scopes: BTreeSet::new(),
                added_at: NOW,
                login: Login::Ok,
            });
        harness.run_steps(2);

        assert!(shown(&harness, WHY));
        assert!(
            shown(&harness, WHAT_REFRESH),
            "it says what finds a structure"
        );
        assert!(shown(&harness, "Refresh a character"), "the one fill shows");
        assert!(
            !shown(&harness, "Add character"),
            "the empty state draws no second button"
        );

        click(&mut harness, "Refresh a character");
        assert!(
            shown(&harness, "jEve data"),
            "Refresh a character opens the Characters page"
        );
    }

    #[test]
    fn the_panel_is_taken_for_the_owner() {
        let folders = Folders::fixed();
        folders.write_places();
        folders.type_names();
        let mut harness = start(&folders);
        crate::ui::hide_database_size(&harness);
        assert!(shown(&harness, "Structures"), "the Structures panel shows");
        crate::ui::picture(&mut harness, "setup_structures");

        click(&mut harness, "Structure 1035466617999 (no access)");
        press(&mut harness, Key::Enter, 1);
        crate::ui::picture(&mut harness, "setup_structures_edit");
    }
}
