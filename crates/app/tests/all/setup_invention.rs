//! Acceptance tests for the Invention skills tab on the Setup page: the attempts rule, the seven
//! skills, and the levels that are saved and read back by a new app. The game data is the build
//! 3607705 fixture, imported in each test; the store is in a temp folder. The clock is fixed at
//! 1,000,000.
#[cfg(test)]
mod tests {

    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};

    use egui::accesskit::Role;
    use egui::{Event, Modifiers, PointerButton, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use eve_means_of_profit::rail::Page;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::{App, AppConfig};
    use mop_core::industry::TypeId;
    use mop_core::invention::AttemptRule;
    use mop_core::money::Rate;
    use mop_sde::{Fetch, FetchError, import};
    use mop_store::Store;
    use tempfile::TempDir;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    const ELECTRONIC_ENGINEERING: TypeId = 11_453;
    const GALLENTE_ENCRYPTION: TypeId = 23_121;

    const SCIENCE: [&str; 4] = [
        "Electronic Engineering",
        "Graviton Physics",
        "Mechanical Engineering",
        "Minmatar Starship Engineering",
    ];
    const ENCRYPTION: [&str; 3] = [
        "Amarr Encryption Methods",
        "Gallente Encryption Methods",
        "Minmatar Encryption Methods",
    ];

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

        /// A folder with a path that does not change between runs, for the picture: the page shows
        /// the data folder, and a temp name would make the picture differ every time.
        fn fixed() -> Self {
            let root = std::env::temp_dir().join("eve-means-of-profit-invention-snapshots");
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
            // The background price refresh stays off: no test here asks for prices.
            let mut history = store.history_settings().unwrap();
            history.auto_refresh = false;
            store.save_history_settings(&history).unwrap();
            folders
        }

        fn store(&self) -> Store {
            Store::open(&self.data.join("user.sqlite"), NOW)
                .unwrap()
                .store
        }
    }

    /// The window the checks run in: tall, so that every skill row fits in its panel.
    fn start(folders: &Folders) -> Harness<'static, App> {
        start_at(folders, vec2(1280.0, 1400.0))
    }

    fn start_at(folders: &Folders, size: egui::Vec2) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: folders.data.clone(),
            cache_dir: folders.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(crate::offline::Offline),
        };
        let mut harness = Harness::builder()
            .with_size(size)
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

    /// How many combo boxes show `value`.
    fn combo_count(harness: &Harness<'_, App>, value: &str) -> usize {
        count_combos_in(&harness.root(), value)
    }

    fn count_combos_in(node: &egui_kittest::Node<'_>, value: &str) -> usize {
        let mut count = usize::from(
            node.accesskit_node().role() == Role::ComboBox
                && node.accesskit_node().value().as_deref() == Some(value),
        );
        for child in node.children() {
            count += count_combos_in(&child, value);
        }
        count
    }

    /// The value of the combo box on the row of the first node labelled `anchor`.
    fn row_value(harness: &Harness<'_, App>, anchor: &str) -> Option<String> {
        let y = harness.query_all_by_label(anchor).next()?.rect().center().y;
        combo_on_row(&harness.root(), y)
    }

    fn combo_on_row(node: &egui_kittest::Node<'_>, y: f32) -> Option<String> {
        let accesskit = node.accesskit_node();
        if accesskit.role() == Role::ComboBox && (node.rect().center().y - y).abs() <= 3.0 {
            return accesskit.value();
        }
        node.children().find_map(|child| combo_on_row(&child, y))
    }

    /// Moves onto the point and presses it: how a tab, whose title the dock paints without a
    /// labelled node, is picked.
    fn press_at(harness: &mut Harness<'_, App>, at: egui::Pos2) {
        harness.event(Event::PointerMoved(at));
        harness.run_steps(2);
        for pressed in [true, false] {
            harness.event(Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.run_steps(2);
        }
        harness.run_steps(3);
    }

    /// The point in the tab bar over the Structures panel, `offset` points after its left edge.
    fn tab_bar_point(harness: &Harness<'_, App>, offset: f32) -> egui::Pos2 {
        let header = harness
            .query_all_by_value("Structures")
            .next()
            .expect("the Structures panel shows")
            .rect();
        egui::pos2(header.min.x + offset, header.min.y - 18.0)
    }

    /// Opens the Invention skills tab, the third one in the node.
    fn open_invention(harness: &mut Harness<'_, App>) {
        press_at(harness, tab_bar_point(harness, 230.0));
    }

    #[test]
    fn a_fresh_store_shows_the_defaults_and_the_seven_skills() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_invention(&mut harness);

        assert!(shown(&harness, "Attempts to buy"), "the rule is named");
        assert_eq!(
            combo_count(&harness, "Expected (average)"),
            1,
            "the attempts rule starts at expected"
        );
        for skill in SCIENCE {
            assert!(shown(&harness, skill), "the science row {skill} shows");
        }
        for skill in ENCRYPTION {
            assert!(shown(&harness, skill), "the encryption row {skill} shows");
        }
        assert_eq!(
            combo_count(&harness, "assumed 0"),
            SCIENCE.len() + ENCRYPTION.len(),
            "every skill starts unset"
        );
    }

    #[test]
    fn levels_and_the_attempts_rule_are_saved_and_read_by_a_new_app() {
        let folders = Folders::new();
        let mut harness = start(&folders);
        open_invention(&mut harness);

        click_combo(&mut harness, "assumed 0", 0);
        click_nth(&mut harness, "4", 0);
        // Gallente is the fifth skill from the top once Electronic Engineering is set.
        click_combo(&mut harness, "assumed 0", 4);
        click_nth(&mut harness, "0", 0);
        click_combo(&mut harness, "Expected (average)", 0);
        click_nth(&mut harness, "95% sure", 0);

        assert_eq!(
            folders.store().invention_skills().unwrap(),
            BTreeMap::from([(ELECTRONIC_ENGINEERING, 4), (GALLENTE_ENCRYPTION, 0)])
        );
        assert_eq!(
            folders.store().attempt_rule().unwrap(),
            AttemptRule::Confidence(Rate::from_ppm(950_000))
        );

        let mut restarted = start(&folders);
        open_invention(&mut restarted);
        assert_eq!(
            row_value(&restarted, "Electronic Engineering").as_deref(),
            Some("4")
        );
        assert_eq!(
            row_value(&restarted, "Gallente Encryption Methods").as_deref(),
            Some("0")
        );
        assert_eq!(combo_count(&restarted, "95% sure"), 1);

        click_combo(&mut restarted, "4", 0);
        click_nth(&mut restarted, "assumed 0", 0);
        assert_eq!(
            folders.store().invention_skills().unwrap(),
            BTreeMap::from([(GALLENTE_ENCRYPTION, 0)]),
            "clearing a level removes it"
        );
    }

    #[test]
    fn the_invention_picture_is_taken_for_the_owner() {
        let folders = Folders::fixed();
        let mut harness = start_at(&folders, vec2(1280.0, 1400.0));
        open_invention(&mut harness);
        click_combo(&mut harness, "assumed 0", 0);
        click_nth(&mut harness, "4", 0);
        click_combo(&mut harness, "assumed 0", 0);
        click_nth(&mut harness, "4", 0);
        // Gallente is the fourth unset skill from the top after two are set.
        click_combo(&mut harness, "assumed 0", 3);
        click_nth(&mut harness, "4", 0);
        click_combo(&mut harness, "Expected (average)", 0);
        click_nth(&mut harness, "90% sure", 0);
        harness.run_steps(2);
        harness.snapshot("setup-invention");
    }
}
