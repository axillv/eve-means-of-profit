//! The `Build what is short` button: the stockpile's short rows as one plan, or the rows that
//! cannot be built left out.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Button, RichText, Ui};
use mop_core::freight::Packing;
use mop_core::industry::{RecipeBook, TypeId};
use mop_core::plan::PlanProduct;
use mop_core::reserve::ClaimedLine;
use mop_core::stock::{CharacterId, Places, StockScope};
use mop_core::stockpile::Stockpile;
use mop_store::PlanRecord;
use mop_store::Store;
use mop_store::reservations::PlanStock;

use super::StockpilesPage;
use crate::Shared;
use crate::build::state::report;

/// What the short rows split into: the products of the plan, in the table's order, and the names
/// of the rows left out for want of a recipe.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Split {
    pub(crate) products: Vec<PlanProduct>,
    pub(crate) left_out: Vec<String>,
}

/// Splits the rows short under the stockpile's mode into one product per row, with its short
/// count, and the names of the rows the game data has no recipe for, both in the table's order.
pub(crate) fn split(
    rows: &[ClaimedLine],
    names: &BTreeMap<TypeId, String>,
    recipes: &RecipeBook,
) -> Split {
    let mut split = Split::default();
    for row in rows.iter().filter(|row| row.short > 0) {
        if recipes.get(row.line.type_id).is_none() {
            split.left_out.push(name(row.line.type_id, names));
            continue;
        }
        split.products.push(PlanProduct {
            type_id: row.line.type_id,
            quantity: row.short,
        });
    }
    split
}

/// The name of `type_id`, or `type <id>` when the table had none.
fn name(type_id: TypeId, names: &BTreeMap<TypeId, String>) -> String {
    names
        .get(&type_id)
        .cloned()
        .unwrap_or_else(|| format!("type {type_id}"))
}

/// Everything one click writes with: the stockpile's name, its rows in the table's order, the
/// type names, the game data's recipes, the store, every stock character and the clock.
pub(crate) struct Input<'a> {
    pub(crate) name: &'a str,
    pub(crate) rows: &'a [ClaimedLine],
    pub(crate) names: &'a BTreeMap<TypeId, String>,
    pub(crate) recipes: &'a RecipeBook,
    pub(crate) store: &'a mut Store,
    pub(crate) characters: &'a BTreeSet<CharacterId>,
    /// The plan this stockpile's button wrote earlier this run, if any.
    pub(crate) remembered: Option<i64>,
    pub(crate) now: i64,
}

/// Writes one plan from the rows the app can build. Returns its id, or the text to report: nothing
/// is written when no row can be built or a write is refused.
pub(crate) fn write(mut input: Input<'_>) -> Result<i64, String> {
    let split = split(input.rows, input.names, input.recipes);
    let Some(first) = split.products.first().copied() else {
        return Err(format!(
            "Nothing in {} is short that the app can build",
            input.name
        ));
    };
    let id = match input.remembered {
        Some(id) => match input.store.plan(id) {
            Ok(Some(_)) => id,
            Ok(None) => new_plan(&mut input, first)?,
            Err(error) => return Err(format!("Could not make the plan: {error}")),
        },
        None => new_plan(&mut input, first)?,
    };
    input
        .store
        .set_plan_products(id, &split.products, input.now)
        .map_err(|error| format!("Could not save the plan: {error}"))?;
    Ok(id)
}

/// The rows and context of the Contents panel's short buttons.
pub(crate) struct Buttons<'a> {
    pub(crate) id: i64,
    pub(crate) stockpile: &'a Stockpile,
    pub(crate) rows: &'a [ClaimedLine],
    pub(crate) names: &'a BTreeMap<TypeId, String>,
    /// The game data's recipes, `None` when they could not be read: nothing is called buildable
    /// then.
    pub(crate) recipes: Option<&'a RecipeBook>,
}

/// Draws `Buy what is short` and `Build what is short`, and the rows the app cannot build.
pub(crate) fn buttons(
    page: &mut StockpilesPage,
    ui: &mut Ui,
    cx: &mut Shared,
    buttons: &Buttons<'_>,
) {
    let short = buttons.rows.iter().any(|row| row.short > 0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(short, Button::new("Buy what is short"))
            .on_hover_text("Puts the short rows in a buy list with prices and freight.")
            .clicked()
        {
            page.buy.open(buttons.id, buttons.stockpile.name.clone());
        }
        if ui
            .add_enabled(
                short && buttons.recipes.is_some(),
                Button::new("Build what is short"),
            )
            .on_hover_text("Puts every short row the app can build in one plan.")
            .on_disabled_hover_text("The game data's recipes could not be read.")
            .clicked()
        {
            clicked(page, buttons, cx);
        }
    });
    if let Some(recipes) = buttons.recipes {
        let split = split(buttons.rows, buttons.names, recipes);
        if !split.left_out.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Left out, nothing to build: {}",
                    split.left_out.join(", ")
                ))
                .color(cx.style.neg),
            );
        }
    }
}

/// Does what a click on `Build what is short` does: writes the plan, remembers it under the
/// stockpile, marks the claims stale and asks the Build page to show it.
fn clicked(page: &mut StockpilesPage, buttons: &Buttons<'_>, cx: &mut Shared) {
    let Some(recipes) = buttons.recipes else {
        return;
    };
    let characters = cx
        .session
        .stock_characters()
        .iter()
        .map(|owned| owned.character_id)
        .collect();
    let now = (cx.clock)();
    let made = write(Input {
        name: &buttons.stockpile.name,
        rows: buttons.rows,
        names: buttons.names,
        recipes,
        store: &mut cx.store,
        characters: &characters,
        remembered: page.built.get(&buttons.id).copied(),
        now,
    });
    match made {
        Ok(plan) => {
            cx.claims.mark_stale();
            page.built.insert(buttons.id, plan);
            page.open_plan = Some(plan);
        }
        Err(text) => report(cx, text),
    }
}

/// Creates a plan named `<stockpile name> (short)` or the first free number after it, with `first`
/// as its product and no builds, ME or pasted stock. Returns its id.
fn new_plan(input: &mut Input<'_>, first: PlanProduct) -> Result<i64, String> {
    let taken: Vec<String> = input
        .store
        .plans()
        .map_err(|error| format!("Could not make the plan: {error}"))?
        .into_iter()
        .map(|plan| plan.name)
        .collect();
    let price_rule = input
        .store
        .settings()
        .map_err(|error| format!("Could not make the plan: {error}"))?
        .default_price_rule;
    let record = PlanRecord {
        id: 0,
        name: free_name(&taken, input.name),
        product: first.type_id,
        quantity: first.quantity,
        build: BTreeSet::new(),
        me: BTreeMap::new(),
        manufacturing: None,
        reactions: None,
        price_rule,
        route: None,
        service: None,
        packing: Packing::default(),
        pasted_stock: String::new(),
        created_at: 0,
        updated_at: 0,
    };
    let id = input
        .store
        .create_plan(&record, input.now)
        .map_err(|error| format!("Could not make the plan: {error}"))?;
    let scope = StockScope {
        characters: input.characters.clone(),
        places: Places::All,
    };
    input
        .store
        .set_plan_stock(id, &PlanStock::Characters(scope))
        .map_err(|error| format!("Could not save the plan: {error}"))?;
    input
        .store
        .set_reserve(id, true)
        .map_err(|error| format!("Could not save the plan: {error}"))?;
    Ok(id)
}

/// `<stockpile name> (short)`, then `<stockpile name> (short) 2`, `… 3`…: the first name no saved
/// plan has.
fn free_name(taken: &[String], stockpile: &str) -> String {
    let base = format!("{stockpile} (short)");
    if !taken.iter().any(|name| name == &base) {
        return base;
    }
    let mut number = 2_u32;
    loop {
        let candidate = format!("{base} {number}");
        if !taken.iter().any(|name| name == &candidate) {
            return candidate;
        }
        number += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use mop_core::industry::{Activity, Recipe};
    use mop_core::stock::Asset;
    use mop_core::stock::CharacterAssets;
    use mop_core::stockpile::{Have, Line, Status};
    use mop_esi::character_data::{CharacterData, Fetched, OwnedAssets};
    use mop_esi::characters::{Character, Login};
    use mop_esi::industry::IndustryData;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, Sde, import};
    use mop_store::Density;
    use tempfile::TempDir;

    use super::*;
    use crate::build::prices::Prices;
    use crate::claims::ClaimsState;
    use crate::selection::Selection;
    use crate::session::{NoEve, Session};
    use crate::theme::Style;
    use crate::worker::Worker;

    /// Hobgoblin II, Rifter and Tritanium: the types of the acceptance rows. The fixture has a
    /// recipe for the first two and none for the mineral.
    const HOBGOBLIN_II: TypeId = 2456;
    const RIFTER: TypeId = 587;
    const TRITANIUM: TypeId = 34;
    const NOW: i64 = 1_000_000;
    const MAIN: CharacterId = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;

    /// The build 3607705 fixture imported into `dir`.
    fn fixture_sde(dir: &TempDir) -> Sde {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sde/tests/fixtures/sde-3607705");
        let db = dir.path().join("sde.sqlite");
        import(&source, &db).expect("the fixture imports");
        Sde::open(&db).expect("the fixture opens")
    }

    /// A user store at `dir`.
    fn store(dir: &TempDir) -> Store {
        Store::open(&dir.path().join("user.sqlite"), NOW)
            .expect("the store opens")
            .store
    }

    /// The fixture's recipes, so a short row of Rifter or Hobgoblin II can be built and Tritanium
    /// cannot.
    fn recipes(sde: &Sde) -> RecipeBook {
        sde.recipe_book().expect("the recipes read")
    }

    /// One stockpile row: `type_id` is short `units`.
    fn short(type_id: TypeId, units: u64) -> ClaimedLine {
        ClaimedLine {
            line: Line {
                type_id,
                target: units,
                have: Have::default(),
                total: 0,
                short: units,
                status: Status::Short,
            },
            claimed: 0,
            by: Vec::new(),
            free: 0,
            short: units,
            status: Status::Short,
        }
    }

    fn named(entries: &[(TypeId, &str)]) -> BTreeMap<TypeId, String> {
        entries
            .iter()
            .map(|(type_id, name)| (*type_id, (*name).to_owned()))
            .collect()
    }

    /// One click's input over `rows`, remembered plan `remembered`.
    fn input<'a>(
        store: &'a mut Store,
        rows: &'a [ClaimedLine],
        names: &'a BTreeMap<TypeId, String>,
        recipes: &'a RecipeBook,
        characters: &'a BTreeSet<CharacterId>,
        remembered: Option<i64>,
    ) -> Input<'a> {
        Input {
            name: "Kit",
            rows,
            names,
            recipes,
            store,
            characters,
            remembered,
            now: NOW,
        }
    }

    /// A saved plan named `name` with one Rifter, so its name is taken.
    fn saved(store: &mut Store, name: &str) -> i64 {
        let record = PlanRecord {
            id: 0,
            name: name.to_owned(),
            product: RIFTER,
            quantity: 1,
            build: BTreeSet::new(),
            me: BTreeMap::new(),
            manufacturing: None,
            reactions: None,
            price_rule: Default::default(),
            route: None,
            service: None,
            packing: Packing::default(),
            pasted_stock: String::new(),
            created_at: NOW,
            updated_at: NOW,
        };
        store.create_plan(&record, NOW).expect("the plan is saved")
    }

    fn products(store: &mut Store, id: i64) -> Vec<PlanProduct> {
        store.plan_products(id).expect("the products read")
    }

    #[test]
    fn the_split_keeps_the_table_order_and_names_the_rows_without_a_recipe() {
        let mut recipes = RecipeBook::new();
        for type_id in [HOBGOBLIN_II, RIFTER] {
            recipes.insert(
                type_id,
                Recipe {
                    activity: Activity::Manufacturing,
                    output_per_run: 1,
                    max_runs_per_job: 1,
                    materials: Vec::new(),
                    made_by_invention: false,
                },
            );
        }
        let names = named(&[
            (RIFTER, "Rifter"),
            (TRITANIUM, "Tritanium"),
            (HOBGOBLIN_II, "Hobgoblin II"),
        ]);
        let rows = [
            short(RIFTER, 1),
            short(TRITANIUM, 2),
            short(HOBGOBLIN_II, 3),
            {
                // A row with nothing short stays out of both lists.
                let mut none = short(RIFTER, 0);
                none.short = 0;
                none
            },
        ];
        let rows_split = split(&rows, &names, &recipes);
        assert_eq!(
            rows_split.products,
            vec![
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 1
                },
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 3
                },
            ]
        );
        assert_eq!(rows_split.left_out, vec!["Tritanium".to_owned()]);

        // A type the table had no name for still reads.
        let empty = named(&[]);
        assert_eq!(split(&[short(34, 1)], &empty, &recipes).left_out, {
            vec!["type 34".to_owned()]
        });
        assert_eq!(
            split(&[short(RIFTER, 1)], &names, &recipes),
            Split {
                products: vec![PlanProduct {
                    type_id: RIFTER,
                    quantity: 1
                }],
                left_out: Vec::new(),
            }
        );
    }

    #[test]
    fn one_click_makes_one_plan_from_the_rows_it_can_build() {
        let dir = TempDir::new().expect("a temp folder");
        let mut store = store(&dir);
        let sde = fixture_sde(&dir);
        let recipes = recipes(&sde);
        let rows = [
            short(HOBGOBLIN_II, 3),
            short(RIFTER, 1),
            short(TRITANIUM, 60_000),
        ];
        let names = named(&[
            (HOBGOBLIN_II, "Hobgoblin II"),
            (RIFTER, "Rifter"),
            (TRITANIUM, "Tritanium"),
        ]);
        let characters = BTreeSet::from([MAIN]);

        let id = write(input(
            &mut store,
            &rows,
            &names,
            &recipes,
            &characters,
            None,
        ))
        .expect("the plan is written");

        let record = store
            .plan(id)
            .expect("the plan reads")
            .expect("the plan exists");
        assert_eq!(record.name, "Kit (short)");
        assert_eq!(record.product, HOBGOBLIN_II);
        assert_eq!(record.quantity, 3);
        assert_eq!(
            products(&mut store, id),
            vec![
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 3
                },
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 1
                },
            ]
        );
        assert_eq!(
            store.plan_stock(id).expect("the stock reads"),
            PlanStock::Characters(StockScope {
                characters: BTreeSet::from([MAIN]),
                places: Places::All,
            })
        );
        assert_eq!(store.reserving_plans().expect("the plans read"), vec![id]);
        assert_eq!(store.plans().expect("the plans list").len(), 1);

        // Pressing again with only Rifter short updates that same plan.
        let rows = [short(RIFTER, 1)];
        let again = write(input(
            &mut store,
            &rows,
            &names,
            &recipes,
            &characters,
            Some(id),
        ))
        .expect("the plan is updated");
        assert_eq!(again, id);
        assert_eq!(
            products(&mut store, id),
            vec![PlanProduct {
                type_id: RIFTER,
                quantity: 1
            }]
        );
        assert_eq!(store.plans().expect("the plans list").len(), 1);
    }

    #[test]
    fn a_taken_name_gets_the_next_number_and_a_deleted_plan_is_replaced() {
        let dir = TempDir::new().expect("a temp folder");
        let mut store = store(&dir);
        let sde = fixture_sde(&dir);
        let recipes = recipes(&sde);
        let rows = [short(RIFTER, 1)];
        let names = named(&[(RIFTER, "Rifter")]);
        let characters = BTreeSet::from([MAIN]);
        saved(&mut store, "Kit (short)");

        let id = write(input(
            &mut store,
            &rows,
            &names,
            &recipes,
            &characters,
            None,
        ))
        .expect("the plan is written");
        assert_eq!(
            store
                .plan(id)
                .expect("the plan reads")
                .expect("the plan exists")
                .name,
            "Kit (short) 2"
        );
        assert_eq!(store.plans().expect("the plans list").len(), 2);

        // A remembered plan the player deleted is replaced by a fresh one and a fresh number.
        let deleted = saved(&mut store, "Gone");
        store.delete_plan(deleted).expect("the plan is deleted");
        let fresh = write(input(
            &mut store,
            &rows,
            &names,
            &recipes,
            &characters,
            Some(deleted),
        ))
        .expect("a fresh plan is written");
        assert_eq!(store.plans().expect("the plans list").len(), 3);
        assert_eq!(
            store
                .plan(fresh)
                .expect("the plan reads")
                .expect("the plan exists")
                .name,
            "Kit (short) 3"
        );
    }

    #[test]
    fn nothing_buildable_writes_nothing_and_says_so() {
        let dir = TempDir::new().expect("a temp folder");
        let mut store = store(&dir);
        let sde = fixture_sde(&dir);
        let recipes = recipes(&sde);
        let rows = [short(TRITANIUM, 60_000)];
        let names = named(&[(TRITANIUM, "Tritanium")]);
        let characters = BTreeSet::from([MAIN]);

        let error = write(input(
            &mut store,
            &rows,
            &names,
            &recipes,
            &characters,
            None,
        ))
        .expect_err("nothing can be built");
        assert_eq!(error, "Nothing in Kit is short that the app can build");
        assert!(store.plans().expect("the plans list").is_empty());
    }

    /// The network, never reached: these tests install the game data themselves.
    struct NoFetch;

    impl Fetch for NoFetch {
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

    /// A market that never answers: no price is read in these tests.
    struct NoHttp;

    impl BytesHttp for NoHttp {
        fn get_bytes(
            &self,
            _url: &str,
            _if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            Err(MarketError::Network("not in these tests".to_owned()))
        }
    }

    /// Main, with one hangar entry of `units` of `type_id` at Jita: the stock a click counts.
    fn give_stock(cx: &mut Shared, type_id: TypeId, units: u64) {
        let character = Character {
            id: u64::try_from(MAIN).expect("an id that fits"),
            name: "Main".to_owned(),
            scopes: BTreeSet::new(),
            added_at: NOW,
            login: Login::Ok,
        };
        let assets = CharacterAssets {
            character_id: MAIN,
            updated: Some(NOW),
            assets: vec![Asset {
                item_id: 1,
                type_id,
                location_id: JITA_4_4,
                flag: Some("Hangar".to_owned()),
                quantity: units,
                singleton: false,
            }],
        };
        let data = CharacterData {
            character_id: u64::try_from(MAIN).expect("an id that fits"),
            assets: Some(Fetched {
                data: OwnedAssets {
                    assets,
                    names: BTreeMap::new(),
                },
                fetched_at: NOW,
                last_modified: None,
                expires: None,
            }),
            orders: None,
            jobs: None,
            wallet: None,
        };
        cx.session.put_character(character);
        cx.session.replace(data);
    }

    /// The fixture game data and an empty user store, with everything the Build page reads around
    /// them. The clock is fixed at [`NOW`].
    fn shared(dir: &TempDir) -> Shared {
        let store = store(dir);
        Shared {
            sde: Some(fixture_sde(dir)),
            store,
            worker: Worker::spawn(egui::Context::default(), Box::new(NoFetch)),
            market: Arc::new(Prices::new(Box::new(NoHttp))),
            plex_http: Arc::new(mop_market::http::UreqHttp::new()),
            copy_notices: Arc::new(Mutex::new(Vec::new())),
            style: Style::eve_dense(),
            density: Density::Normal,
            rail_icons_only: false,
            refresh_while_open: false,
            data_dir: dir.path().to_path_buf(),
            notices: Vec::new(),
            clock: Box::new(|| NOW),
            session: Session::new(Box::new(NoEve)),
            selection: Selection::default(),
            industry: IndustryData::default(),
            claims: ClaimsState::default(),
        }
    }

    /// A stockpile the buttons only read the name of.
    fn kit() -> Stockpile {
        Stockpile {
            id: 1,
            name: "Kit".to_owned(),
            group: None,
            multiplier: 1,
            targets: Vec::new(),
            includes: Vec::new(),
            filters: Vec::new(),
        }
    }

    #[test]
    fn a_click_remembers_the_plan_and_asks_the_build_page_to_show_it() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        give_stock(&mut cx, RIFTER, 1);
        let recipes = cx
            .sde
            .as_ref()
            .expect("the game data")
            .recipe_book()
            .expect("the recipes read");
        let names = named(&[
            (HOBGOBLIN_II, "Hobgoblin II"),
            (RIFTER, "Rifter"),
            (TRITANIUM, "Tritanium"),
        ]);
        let stockpile = kit();
        let rows = [
            short(HOBGOBLIN_II, 3),
            short(RIFTER, 1),
            short(TRITANIUM, 60_000),
        ];
        let mut page = StockpilesPage::default();
        let buttons = Buttons {
            id: 1,
            stockpile: &stockpile,
            rows: &rows,
            names: &names,
            recipes: Some(&recipes),
        };

        clicked(&mut page, &buttons, &mut cx);

        let plan = page.open_plan.expect("the plan is shown on Build");
        assert_eq!(page.built.get(&1), Some(&plan));
        assert_eq!(
            products(&mut cx.store, plan),
            vec![
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 3
                },
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 1
                },
            ]
        );
        assert!(cx.claims.needs_refresh(&[]), "the claims are marked stale");

        // Pressing again goes through the remembered plan and keeps it the only one.
        let rows = [short(RIFTER, 1)];
        let buttons = Buttons {
            id: 1,
            stockpile: &stockpile,
            rows: &rows,
            names: &names,
            recipes: Some(&recipes),
        };
        clicked(&mut page, &buttons, &mut cx);
        assert_eq!(page.open_plan, Some(plan));
        assert_eq!(
            products(&mut cx.store, plan),
            vec![PlanProduct {
                type_id: RIFTER,
                quantity: 1
            }]
        );
        assert_eq!(cx.store.plans().expect("the plans list").len(), 1);
    }

    #[test]
    fn a_repeated_click_keeps_the_plans_chosen_stock_and_reserve() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        give_stock(&mut cx, RIFTER, 1);
        let recipes = cx
            .sde
            .as_ref()
            .expect("the game data")
            .recipe_book()
            .expect("the recipes read");
        let names = named(&[(RIFTER, "Rifter")]);
        let stockpile = kit();
        let rows = [short(RIFTER, 1)];
        let mut page = StockpilesPage::default();
        let buttons = Buttons {
            id: 1,
            stockpile: &stockpile,
            rows: &rows,
            names: &names,
            recipes: Some(&recipes),
        };

        clicked(&mut page, &buttons, &mut cx);
        let plan = page.open_plan.expect("the plan is shown on Build");

        // The player edits the plan on Build: only another character's stock, Reserve off.
        let alt = StockScope {
            characters: BTreeSet::from([MAIN + 1]),
            places: Places::All,
        };
        cx.store
            .set_plan_stock(plan, &PlanStock::Characters(alt.clone()))
            .expect("the stock is edited");
        cx.store
            .set_reserve(plan, false)
            .expect("the reserve is turned off");

        // Pressing again only replaces the product list.
        clicked(&mut page, &buttons, &mut cx);
        assert_eq!(
            cx.store.plan_stock(plan).expect("the stock reads"),
            PlanStock::Characters(alt)
        );
        assert!(
            cx.store
                .reserving_plans()
                .expect("the plans read")
                .is_empty()
        );
    }
}
