//! Acceptance tests for what the rail actions and the File menu do to the player's layouts: a
//! shipped page is copied into a layout, layouts are copied, saved to a file, renamed and deleted,
//! and one is added from a file or from pasted text. The game data is the build 3607705 fixture,
//! the clock is fixed at 1,000,000, and the window is 1280 by 800.
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use egui::accesskit::Role;
    use egui::{Event, Key, Modifiers, OutputCommand};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use eve_means_of_profit::layout::files::Files;
    use eve_means_of_profit::session::NoEve;
    use eve_means_of_profit::status::Level;
    use eve_means_of_profit::{App, AppConfig};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Facility, Fetch, FetchError, Security, import};
    use mop_store::Store;
    use tempfile::TempDir;

    use crate::ui;

    const NOW: i64 = 1_000_000;
    const FIXTURE: &str = "../sde/tests/fixtures/sde-3607705";
    /// The rail entry a copy of the `Build` page takes.
    const BUILD_COPY: &str = "EVE Means of Profit layout 1\nname: Build (copy)\nsplit right 20%\n  Plans\n  split down 35%\n    Plan\n    split right 50%\n      Steps\n      Buy list\n";
    /// Two layouts: one split in two panels, one empty.
    const DAILY: &str = "EVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps\n\nEVE Means of Profit layout 1\nname: Ideas\nempty\n";
    /// The chat text `Copy layout` gives for `Daily industry`.
    const COPY_TEXT: &str = "```\nEVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps\n```\n";
    /// What a user layout with no panel at all says.
    const EMPTY_TEXT: &str =
        "This layout is empty. Add panels with \"Add to layout…\" in any panel's Layout menu.";

    /// The game data is installed by the test, so the app never asks the network for anything.
    struct NoNetwork;

    impl Fetch for NoNetwork {
        fn get_text(&self, _url: &str) -> Result<String, FetchError> {
            Err(FetchError::Http { status: 503 })
        }

        fn download(
            &self,
            _url: &str,
            _dest: &mut dyn Write,
            _progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            Err(FetchError::Http { status: 503 })
        }
    }

    /// A market file with no rows at all, dated the test clock, so the Buy list never reaches the
    /// network and no price notice reaches the status line.
    struct EmptyForge;

    impl BytesHttp for EmptyForge {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            let header = "what,weightedaverage,maxval,minval,stddev,median,volume,numorders,fivepercent,orderSet\n";
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(header.as_bytes())
                .map_err(|error| MarketError::Network(error.to_string()))?;
            let body = encoder
                .finish()
                .map_err(|error| MarketError::Network(error.to_string()))?;
            Ok(BytesResponse {
                status: 200,
                etag: Some("\"0\"".to_owned()),
                last_modified: Some("Mon, 12 Jan 1970 13:46:40 GMT".to_owned()),
                retry_after: None,
                body,
            })
        }
    }

    /// A data folder with the game data installed and the facilities the plans use.
    struct Env {
        dir: TempDir,
        data: PathBuf,
        cache: PathBuf,
    }

    fn make_env() -> Env {
        let dir = TempDir::new().expect("a temp folder");
        let data = dir.path().join("data");
        let cache = dir.path().join("cache");
        fs::create_dir_all(&data).expect("the data folder");
        import(&fixture(), &data.join("sde.sqlite")).expect("the fixture imports");
        let mut store = Store::open(&data.join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        let mut settings = store.settings().expect("settings");
        settings.last_sde_check = Some(NOW);
        store.save_settings(&settings).expect("settings saved");
        store
            .add_facility(
                "Raitaru null",
                &Facility {
                    structure: Some(35825),
                    security: Security::Null,
                    rigs: vec![37157],
                },
            )
            .expect("facility saved");
        store
            .add_facility(
                "Athanor null",
                &Facility {
                    structure: Some(35835),
                    security: Security::Null,
                    rigs: vec![46487],
                },
            )
            .expect("facility saved");
        Env { dir, data, cache }
    }

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
    }

    fn write_layouts(env: &Env, text: &str) {
        fs::write(env.data.join("layouts.txt"), text).expect("the layouts file is written");
    }

    fn layouts(env: &Env) -> String {
        fs::read_to_string(env.data.join("layouts.txt")).expect("the layouts file is read")
    }

    fn start(env: &Env) -> Harness<'static, App> {
        start_with_files(
            env,
            Files {
                open: Box::new(|| None),
                save: Box::new(|_| None),
            },
        )
    }

    fn start_with_files(env: &Env, files: Files) -> Harness<'static, App> {
        let config = AppConfig {
            data_dir: env.data.clone(),
            cache_dir: env.cache.clone(),
            fetch: Box::new(NoNetwork),
            clock: Box::new(|| NOW),
            eve: Box::new(NoEve),
            market: Box::new(EmptyForge),
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .with_max_steps(crate::SETTLE_STEPS)
            .build_eframe(move |cc| App::new(&cc.egui_ctx, config).expect("the app starts"));
        ui::no_images(&mut harness);
        harness.state_mut().set_files(files);
        harness
            .state_mut()
            .show_page(eve_means_of_profit::rail::Page::Build);
        harness
    }

    /// A files hook whose open dialog answers with the given queued paths in turn, and whose save
    /// dialog answers with `save`.
    fn queued_files(paths: Vec<PathBuf>, saves: Vec<Option<PathBuf>>) -> Files {
        let queue = Rc::new(RefCell::new(VecDeque::from(paths)));
        let saves = Rc::new(RefCell::new(VecDeque::from(saves)));
        Files {
            open: Box::new(move || queue.borrow_mut().pop_front()),
            save: Box::new(move |_| saves.borrow_mut().pop_front().flatten()),
        }
    }

    fn right_click(harness: &mut Harness<'_, App>, label: &str) {
        harness.get_by_label(label).click_secondary();
        harness.run_steps(3);
    }

    fn click(harness: &mut Harness<'_, App>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    /// Opens the File menu and picks `item`.
    fn file_menu(harness: &mut Harness<'_, App>, item: &str) {
        ui::click(harness, "File");
        ui::click(harness, item);
    }

    /// The text put on the clipboard by the frame just drawn, if any.
    fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
        harness
            .output()
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
    }

    fn notices(harness: &mut Harness<'_, App>) -> Vec<String> {
        harness
            .state_mut()
            .shared_mut()
            .notices
            .iter()
            .map(|notice| notice.text.clone())
            .collect()
    }

    fn errors(harness: &mut Harness<'_, App>) -> Vec<String> {
        harness
            .state_mut()
            .shared_mut()
            .notices
            .iter()
            .filter(|notice| notice.level == Level::Error)
            .map(|notice| notice.text.clone())
            .collect()
    }

    /// The first text input on screen, clicked and cleared, then given `text` as pasted text.
    fn paste_into_the_box(harness: &mut Harness<'_, App>, text: &str) {
        harness
            .get_by(|node| matches!(node.role(), Role::TextInput | Role::MultilineTextInput))
            .click();
        harness.run_steps(1);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(1);
        harness.event(Event::Paste(text.to_owned()));
        harness.run_steps(2);
    }

    #[test]
    fn save_a_copy_on_the_rail_puts_the_page_in_a_layout() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        right_click(&mut harness, "Build");
        click(&mut harness, "Save a copy");

        assert!(
            ui::shown(&harness, "Build (copy)"),
            "the copy is a rail entry"
        );
        assert!(
            harness.get_all_by_label("follows plan").count() > 0,
            "the copy is the layout on show, not the Build page"
        );
        assert_eq!(
            layouts(&env),
            format!("{DAILY}\n{BUILD_COPY}"),
            "the copy is written under the page's own name"
        );

        right_click(&mut harness, "Build");
        click(&mut harness, "Save a copy");
        assert!(ui::shown(&harness, "Build (copy) (2)"));
        assert!(
            layouts(&env)
                .ends_with(&BUILD_COPY.replace("name: Build (copy)\n", "name: Build (copy) (2)\n"))
        );
    }

    #[test]
    fn copy_and_save_to_file_use_the_layout_text() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let saved_path = env.dir.path().join("d.txt");
        let mut harness = start_with_files(
            &env,
            queued_files(Vec::new(), vec![Some(saved_path.clone()), None]),
        );
        harness.run();

        right_click(&mut harness, "Daily industry");
        harness.get_by_label("Copy layout").click();
        // The copy is made in the frame of the click, and `run_steps` would leave only the last
        // frame's output.
        harness.step();
        assert_eq!(copied_text(&harness).as_deref(), Some(COPY_TEXT));
        assert!(
            notices(&mut harness).contains(
                &"Layout copied. Paste it in Discord or in the app (File > Paste layout)."
                    .to_owned()
            ),
            "the copy says where it can be pasted"
        );

        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Save to file…");
        assert_eq!(
            fs::read_to_string(&saved_path).expect("the layout file"),
            COPY_TEXT
                .lines()
                .skip(1)
                .take_while(|line| *line != "```")
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
            "the file holds the share text without the fences"
        );
        assert!(
            notices(&mut harness)
                .contains(&format!("Saved the layout to {}.", saved_path.display())),
            "the save says where the layout went"
        );
        let saved = fs::read_to_string(&saved_path).expect("the layout file");

        // A save dialog the player cancels writes nothing and reports nothing.
        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Save to file…");
        assert_eq!(
            fs::read_to_string(&saved_path).expect("the layout file"),
            saved,
            "a cancelled save leaves the file alone"
        );
        assert_eq!(
            notices(&mut harness)
                .iter()
                .filter(|text| text.starts_with("Saved the layout to "))
                .count(),
            1,
            "a cancelled save reports nothing"
        );
    }

    #[test]
    fn a_save_dialog_that_is_cancelled_writes_nothing() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Save to file…");
        assert!(
            !notices(&mut harness)
                .iter()
                .any(|text| text.starts_with("Saved the layout to ")),
            "no save is reported"
        );
    }

    #[test]
    fn an_open_dialog_that_is_cancelled_adds_nothing() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        file_menu(&mut harness, "Open layout file…");
        assert_eq!(layouts(&env), DAILY, "a cancelled open writes nothing");
        assert!(
            !ui::shown(&harness, "Daily industry (2)"),
            "a cancelled open adds no layout"
        );
    }

    #[test]
    fn rename_starts_from_the_first_free_name_and_the_file_follows() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Rename…");
        harness
            .get_by(|node| node.role() == Role::TextInput)
            .click();
        harness.run_steps(1);
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness.run_steps(1);
        harness.event(Event::Text("Build".to_owned()));
        harness.run_steps(2);
        click(&mut harness, "Rename");

        assert!(
            ui::shown(&harness, "Build (2)"),
            "the page label is taken, so the layout gets (2)"
        );
        assert!(layouts(&env).contains("name: Build (2)\n"));
    }

    #[test]
    fn deleting_keeps_the_layout_on_show_when_it_can() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        ui::click(&mut harness, "Ideas");
        assert!(ui::shown(&harness, EMPTY_TEXT), "Ideas is shown");

        // Keep changes nothing.
        right_click(&mut harness, "Ideas");
        click(&mut harness, "Delete…");
        click(&mut harness, "Keep");
        assert!(ui::shown(&harness, EMPTY_TEXT), "Ideas is still shown");
        assert_eq!(layouts(&env), DAILY);

        // Deleting a layout before the shown one leaves the shown one on screen.
        right_click(&mut harness, "Daily industry");
        click(&mut harness, "Delete…");
        click(&mut harness, "Delete");
        assert!(
            ui::shown(&harness, EMPTY_TEXT),
            "Ideas stays shown after Daily industry goes"
        );
        assert!(!ui::shown(&harness, "Daily industry"));
        assert!(
            layouts(&env).contains("name: Ideas\n"),
            "Ideas is still in the file"
        );

        // Deleting the shown layout shows the first page in Page::ALL, Today, not the Build page
        // the player was on before the layout hid it: the rail's Today entry and the page heading.
        right_click(&mut harness, "Ideas");
        click(&mut harness, "Delete…");
        click(&mut harness, "Delete");
        assert!(!ui::shown(&harness, EMPTY_TEXT), "Ideas is gone");
        assert!(
            !ui::shown(&harness, "New plan"),
            "the Build page does not come back"
        );
        assert_eq!(
            harness.get_all_by_label("Today").count(),
            2,
            "Today shows: its rail entry and its heading"
        );
        assert_eq!(layouts(&env), "", "no layout is left to save");
    }

    #[test]
    fn open_layout_file_adds_a_file_and_reports_what_it_could_not_use() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let copy_path = env.dir.path().join("copy.txt");
        fs::write(&copy_path, COPY_TEXT).expect("the copy file is written");
        let unknown_path = env.dir.path().join("unknown.txt");
        fs::write(
            &unknown_path,
            "EVE Means of Profit layout 1\nname: Odd\nMarket depth\n",
        )
        .expect("the unknown file is written");
        let big_path = env.dir.path().join("big.txt");
        fs::write(&big_path, vec![b'x'; 100_001]).expect("the big file is written");
        let mut harness = start_with_files(
            &env,
            queued_files(vec![copy_path, unknown_path, big_path], Vec::new()),
        );
        harness.run();

        file_menu(&mut harness, "Open layout file…");
        assert!(ui::shown(&harness, "Daily industry (2)"));
        assert!(ui::shown(&harness, "Buy list"), "the added layout is shown");
        assert!(!ui::shown(&harness, "Plans"));
        assert!(
            notices(&mut harness).contains(&"Added layout \"Daily industry (2)\".".to_owned()),
            "the add is reported"
        );
        assert!(layouts(&env).contains("name: Daily industry (2)\n"));

        let after_copy = layouts(&env);
        file_menu(&mut harness, "Open layout file…");
        assert!(ui::shown(&harness, "Odd"));
        assert!(
            notices(&mut harness).contains(
                &"Not in this version: Market depth. They show as empty boxes.".to_owned()
            ),
            "the unknown panel is named"
        );
        assert!(ui::shown(
            &harness,
            "\"Market depth\" is not in this version of the app."
        ));
        assert!(
            layouts(&env).contains("Market depth\n"),
            "the file keeps it"
        );

        let after_unknown = layouts(&env);
        assert_ne!(after_copy, after_unknown);
        file_menu(&mut harness, "Open layout file…");
        assert!(
            errors(&mut harness)
                .contains(&"Could not read the layout: the file is larger than 100 KB.".to_owned()),
            "a file over the limit is refused"
        );
        assert_eq!(layouts(&env), after_unknown, "nothing was added");
    }

    #[test]
    fn paste_layout_adds_the_text_and_cancel_adds_nothing() {
        let env = make_env();
        write_layouts(&env, DAILY);
        let mut harness = start(&env);
        harness.run();

        file_menu(&mut harness, "Paste layout");
        assert!(ui::shown(&harness, "Paste layout"), "the window is open");
        paste_into_the_box(&mut harness, COPY_TEXT);
        click(&mut harness, "Add");
        assert!(!ui::shown(&harness, "Paste layout"), "the window is closed");
        assert!(ui::shown(&harness, "Daily industry (2)"));
        assert!(
            ui::shown(&harness, "Buy list"),
            "the pasted layout is shown"
        );
        assert!(!ui::shown(&harness, "Plans"));
        assert!(
            layouts(&env).contains("name: Daily industry (2)\n"),
            "the paste is written"
        );
        let after_paste = layouts(&env);

        file_menu(&mut harness, "Paste layout");
        paste_into_the_box(&mut harness, "hello");
        click(&mut harness, "Add");
        assert!(
            errors(&mut harness).contains(
                &"Could not read the layout: No layout found: the text has no line \"EVE Means of Profit layout 1\"."
                    .to_owned()
            ),
            "text that is not a layout is refused"
        );
        assert_eq!(layouts(&env), after_paste, "nothing was added");

        file_menu(&mut harness, "Paste layout");
        paste_into_the_box(&mut harness, COPY_TEXT);
        click(&mut harness, "Cancel");
        assert!(!ui::shown(&harness, "Paste layout"), "Cancel closes it");
        assert_eq!(layouts(&env), after_paste, "Cancel adds nothing");
    }
}
