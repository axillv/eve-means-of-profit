use profit_market::book::PriceRule;
use profit_store::{Density, Settings, Store};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_store_has_the_default_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");

        let settings = store.settings().expect("settings");

        assert_eq!(
            settings,
            Settings {
                default_price_rule: PriceRule::WalkSell,
                density: Density::Normal,
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
            last_sde_check: Some(86_400),
        };
        let mut store = Store::open(&path, 1_000).expect("open");
        store.save_settings(&saved).expect("save");
        drop(store);

        let mut reopened = Store::open(&path, 2_000).expect("reopen");

        assert_eq!(reopened.settings().expect("settings"), saved);
    }

    #[test]
    fn every_density_and_price_rule_is_saved_as_chosen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(&dir.path().join("eve.db"), 1_000).expect("open");
        for density in [Density::Sparse, Density::Normal, Density::Dense] {
            for rule in [
                PriceRule::WalkSell,
                PriceRule::LowestSell,
                PriceRule::HighestBuy,
            ] {
                let saved = Settings {
                    default_price_rule: rule,
                    density,
                    last_sde_check: None,
                };
                store.save_settings(&saved).expect("save");
                assert_eq!(store.settings().expect("settings"), saved);
            }
        }
    }
}
