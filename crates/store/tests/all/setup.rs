#[cfg(test)]
mod tests {
    use mop_market::book::PriceRule;
    use mop_store::{Density, InterfaceSize, Settings, Store, StoreError};
    use rusqlite::Connection;
    use std::path::Path;

    fn open_store(path: &Path, now: i64) -> Store {
        Store::open(path, now).expect("open").store
    }

    fn app_setting_rows(path: &Path) -> i64 {
        Connection::open(path)
            .expect("open")
            .query_row("SELECT count(*) FROM app_settings", [], |row| row.get(0))
            .expect("count")
    }

    #[test]
    fn a_fresh_store_has_the_default_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = open_store(&dir.path().join("eve.db"), 1_000);

        let settings = store.settings().expect("settings");

        assert_eq!(
            settings,
            Settings {
                default_price_rule: PriceRule::SellFivePercent,
                density: Density::Normal,
                interface_size: InterfaceSize::P100,
                rail_icons_only: false,
                last_sde_check: None,
            }
        );
    }

    #[test]
    fn saved_settings_read_back_after_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let saved = Settings {
            default_price_rule: PriceRule::HighestBuy,
            density: Density::Dense,
            interface_size: InterfaceSize::P100,
            rail_icons_only: false,
            last_sde_check: Some(86_400),
        };
        let mut store = open_store(&path, 1_000);
        store.save_settings(&saved).expect("save");
        drop(store);

        let mut reopened = open_store(&path, 2_000);

        assert_eq!(reopened.settings().expect("settings"), saved);
        reopened
            .save_settings(&Settings {
                last_sde_check: None,
                ..saved
            })
            .expect("save");
        assert_eq!(reopened.settings().expect("settings").last_sde_check, None);
    }

    const ALL_RULES: [PriceRule; 9] = [
        PriceRule::WalkSell,
        PriceRule::LowestSell,
        PriceRule::HighestBuy,
        PriceRule::SellFivePercent,
        PriceRule::SellWeightedAverage,
        PriceRule::SellMedian,
        PriceRule::BuyFivePercent,
        PriceRule::BuyWeightedAverage,
        PriceRule::BuyMedian,
    ];

    #[test]
    fn every_density_and_price_rule_is_saved_as_chosen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = open_store(&dir.path().join("eve.db"), 1_000);
        for density in [Density::Sparse, Density::Normal, Density::Dense] {
            for rule in ALL_RULES {
                let saved = Settings {
                    default_price_rule: rule,
                    density,
                    interface_size: InterfaceSize::P100,
                    rail_icons_only: false,
                    last_sde_check: None,
                };
                store.save_settings(&saved).expect("save");
                assert_eq!(store.settings().expect("settings"), saved);
            }
        }
    }

    #[test]
    fn a_stored_density_or_rule_that_is_not_valid_is_bad_data_naming_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = open_store(&path, 1_000);
        for (key, value) in [
            ("density", "7"),
            ("density", "x"),
            ("default_price_rule", "x"),
        ] {
            Connection::open(&path)
                .expect("open")
                .execute(
                    "INSERT INTO app_settings (key, value) VALUES (?1, ?2) \
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![key, value],
                )
                .expect("write");

            let err = store.settings().expect_err("must refuse");

            assert!(
                matches!(&err, StoreError::BadData(text) if text.contains(key)),
                "{key} {value}: {err}"
            );
        }
    }

    #[test]
    fn a_setting_is_absent_until_set_and_survives_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let key = "columns.build_buy_list";
        let mut store = open_store(&path, 1_000);
        assert_eq!(store.setting("density_x").expect("setting"), None);

        store.set_setting(key, "volume,share").expect("set");
        drop(store);
        let mut reopened = open_store(&path, 2_000);
        assert_eq!(
            reopened.setting(key).expect("setting"),
            Some("volume,share".to_owned())
        );

        reopened.set_setting(key, "").expect("set empty");
        assert_eq!(reopened.setting(key).expect("setting"), Some(String::new()));

        reopened.clear_setting(key).expect("clear");
        assert_eq!(reopened.setting(key).expect("setting"), None);
        reopened.clear_setting(key).expect("clear again");
    }

    #[test]
    fn a_setting_name_outside_the_rules_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = open_store(&path, 1_000);
        let too_long = "a".repeat(65);
        for key in ["", "Density", "a b", too_long.as_str()] {
            let err = store.set_setting(key, "x").expect_err("must refuse");
            assert!(matches!(err, StoreError::BadData(_)), "{key:?}: {err}");
            assert!(matches!(store.setting(key), Err(StoreError::BadData(_))));
        }
        assert_eq!(app_setting_rows(&path), 0);
    }

    #[test]
    fn a_name_of_64_characters_is_accepted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = open_store(&dir.path().join("eve.db"), 1_000);
        let longest = "a".repeat(64);

        store.set_setting(&longest, "x").expect("set");

        assert_eq!(
            store.setting(&longest).expect("setting"),
            Some("x".to_owned())
        );
    }

    #[test]
    fn a_value_of_4096_bytes_is_kept_and_a_longer_one_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = open_store(&dir.path().join("eve.db"), 1_000);
        let largest = "x".repeat(4_096);
        store.set_setting("note", &largest).expect("set");

        let err = store
            .set_setting("note", &"x".repeat(4_097))
            .expect_err("must refuse");

        assert!(matches!(err, StoreError::TooLarge), "{err}");
        assert_eq!(store.setting("note").expect("setting"), Some(largest));
    }

    fn default_settings() -> Settings {
        Settings {
            default_price_rule: PriceRule::SellFivePercent,
            density: Density::Normal,
            interface_size: InterfaceSize::P100,
            rail_icons_only: false,
            last_sde_check: None,
        }
    }

    #[test]
    fn a_new_store_has_the_default_interface_size_and_rail_choice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = open_store(&dir.path().join("eve.db"), 1_000);

        let settings = store.settings().expect("settings");

        assert_eq!(settings.interface_size, InterfaceSize::P100);
        assert!(!settings.rail_icons_only);
    }

    #[test]
    fn each_interface_size_is_saved_and_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = open_store(&path, 1_000);
        for size in InterfaceSize::ALL {
            let saved = Settings {
                interface_size: size,
                ..default_settings()
            };
            store.save_settings(&saved).expect("save");
            assert_eq!(store.settings().expect("settings"), saved);
        }
        let saved = Settings {
            interface_size: InterfaceSize::P175,
            ..default_settings()
        };
        store.save_settings(&saved).expect("save");
        let stored = Connection::open(&path)
            .expect("open")
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'interface_size'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("row");
        assert_eq!(stored, "175");
    }

    #[test]
    fn a_stored_interface_size_that_is_not_valid_is_bad_data_naming_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = open_store(&path, 1_000);
        for (value, shown) in [("130", "\"130\""), ("abc", "\"abc\"")] {
            Connection::open(&path)
                .expect("open")
                .execute(
                    "INSERT OR REPLACE INTO app_settings (key, value) VALUES ('interface_size', ?1)",
                    [value],
                )
                .expect("write");
            let err = store.settings().expect_err("must refuse");
            let expected =
                format!("setting interface_size has the value {shown}, which is not valid");
            assert!(
                matches!(&err, StoreError::BadData(text) if *text == expected),
                "{err}"
            );
        }
    }

    #[test]
    fn saving_a_size_leaves_the_other_settings_as_saved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let saved = Settings {
            default_price_rule: PriceRule::HighestBuy,
            density: Density::Dense,
            interface_size: InterfaceSize::P150,
            rail_icons_only: false,
            last_sde_check: Some(86_400),
        };
        let mut store = open_store(&path, 1_000);
        store.save_settings(&saved).expect("save");
        drop(store);

        let mut reopened = open_store(&path, 2_000);

        assert_eq!(reopened.settings().expect("settings"), saved);
    }

    #[test]
    fn rail_icons_only_is_saved_as_one_or_zero_and_other_text_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("eve.db");
        let mut store = open_store(&path, 1_000);
        let saved = Settings {
            rail_icons_only: true,
            ..default_settings()
        };
        store.save_settings(&saved).expect("save");
        assert_eq!(store.settings().expect("settings"), saved);
        let stored = Connection::open(&path)
            .expect("open")
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'rail_icons_only'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("row");
        assert_eq!(stored, "1");

        Connection::open(&path)
            .expect("open")
            .execute(
                "UPDATE app_settings SET value = 'yes' WHERE key = 'rail_icons_only'",
                [],
            )
            .expect("write");
        let err = store.settings().expect_err("must refuse");
        assert!(
            matches!(&err, StoreError::BadData(text) if text == "setting rail_icons_only has the value \"yes\", which is not valid"),
            "{err}"
        );
    }

    #[test]
    fn from_percent_accepts_only_the_five_sizes() {
        assert_eq!(InterfaceSize::from_percent(150), Some(InterfaceSize::P150));
        assert_eq!(InterfaceSize::from_percent(0), None);
        assert_eq!(InterfaceSize::from_percent(201), None);
    }
}
