//! The Build page's state: the plan on screen, its unsaved edits, the last calculation, and the
//! actions that save, load and create plans.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use mop_core::freight::Packing;
use mop_core::industry::{Activity, Modifier, RecipeBook, TypeId};
use mop_core::invention::{Decryptor, invented_copy};
use mop_core::paste::{Pasted, parse_stock};
use mop_core::plan::{self, Plan, PlanError, PlanInput, PlanProduct, PlanProducts};
use mop_core::reserve::{self, ReservingPlan, RunningJob};
use mop_core::stock::resolve;
use mop_esi::character_data::CharacterData;
use mop_sde::{Facility, Sde, Security};
use mop_store::reservations::PlanStock;
use mop_store::{FacilityRow, InventionChoice, PlanRecord, Store};

use crate::Shared;
use crate::claims::running_jobs;
use crate::session::Session;
use crate::status::{Level, Notice};

/// What a plan request does once the plan on screen can be left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Select(i64),
    New,
    Duplicate,
}

/// A dialog over the Plans panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// The plan on screen has unsaved edits, and the next request would replace it.
    Leave(Next),
    Rename {
        id: i64,
        text: String,
    },
    Delete {
        id: i64,
        name: String,
    },
}

#[derive(Debug)]
pub struct BuildPage {
    /// The saved plan on screen, `None` until one is picked.
    pub selected: Option<i64>,
    /// The plan on screen as edited.
    pub record: Option<PlanRecord>,
    /// Edited since it was last saved or loaded.
    pub dirty: bool,
    /// The last calculation: the plan, or the one line that says why there is none.
    pub result: Result<Plan, String>,
    /// The pasted text, read against the game data.
    pub pasted: Pasted,
    /// Where the plan on screen takes its stock from, as saved with the plan.
    pub stock: PlanStock,
    /// Why the last stock choice was refused, shown under the Stock row.
    pub stock_error: Option<String>,
    /// The stored invention choices that differ from the default, by T2 part.
    pub invention: BTreeMap<TypeId, InventionChoice>,
    /// The ME of the invented copy of each part whose stored choice names a decryptor.
    pub copy_mes: BTreeMap<TypeId, u8>,
    /// The recipes of the game data, for the product search and the steps.
    pub recipes: RecipeBook,
    /// The plan's products, in order. The record's `product` and `quantity` follow the first one.
    pub products: Vec<PlanProduct>,
    /// Each product's quantity as typed, and whether it reads as a whole number of 1 or more.
    pub quantity_texts: Vec<String>,
    pub quantity_ok: Vec<bool>,
    /// The product search box.
    pub product_query: String,
    pub dialog: Option<Dialog>,
}

impl Default for BuildPage {
    fn default() -> Self {
        Self {
            selected: None,
            record: None,
            dirty: false,
            result: Err(NO_PRODUCT.to_owned()),
            pasted: Pasted {
                stock: BTreeMap::new(),
                problems: Vec::new(),
            },
            stock: PlanStock::Pasted,
            stock_error: None,
            invention: BTreeMap::new(),
            copy_mes: BTreeMap::new(),
            recipes: RecipeBook::new(),
            products: Vec::new(),
            quantity_texts: Vec::new(),
            quantity_ok: Vec::new(),
            product_query: String::new(),
            dialog: None,
        }
    }
}

const NO_PRODUCT: &str = "pick a product first";
pub(crate) const NO_REACTIONS: &str = "Pick a reactions facility first";

impl BuildPage {
    /// Works the plan on screen out again and reads the pasted text. Called after every change.
    pub fn recalc(&mut self, cx: &mut Shared) {
        refresh_claims(cx);
        self.copy_mes = match cx.sde.as_ref() {
            Some(sde) => copy_mes(&self.invention, sde),
            None => BTreeMap::new(),
        };
        let text = self.record.as_ref().map_or("", |r| r.pasted_stock.as_str());
        let (pasted, failure) = read_paste(text, cx.sde.as_ref());
        self.pasted = pasted;
        if let Some(error) = failure {
            report(cx, format!("Could not read the pasted stock: {error}"));
        }
        let recipes = match cx.sde.as_ref().map(Sde::recipe_book) {
            Some(Ok(book)) => Ok(book),
            Some(Err(error)) => Err(error.to_string()),
            None => Err("the game data is not loaded yet".to_owned()),
        };
        self.result = match self.record.as_ref() {
            None => Err(NO_PRODUCT.to_owned()),
            Some(record) => match cx.store.facilities() {
                Ok(facilities) => plan_with_products(
                    record,
                    &self.products,
                    &self.stock,
                    cx,
                    &facilities,
                    &self.copy_mes,
                ),
                Err(error) => Err(error.to_string()),
            },
        };
        self.recipes = recipes.unwrap_or_else(|_| RecipeBook::new());
    }

    /// Records an edit and works the plan out again.
    pub fn edited(&mut self, cx: &mut Shared) {
        self.dirty = true;
        self.recalc(cx);
    }

    /// Shows `products` as the plan on screen's list, with one quantity text per product, all
    /// reading as numbers.
    fn set_products(&mut self, products: Vec<PlanProduct>) {
        self.quantity_ok = vec![true; products.len()];
        self.quantity_texts = products
            .iter()
            .map(|product| product.quantity.to_string())
            .collect();
        self.products = products;
        self.mirror_first();
    }

    /// The record's single product follows the list's first, so the pages that still read one
    /// product keep working until M1-216 to M1-218.
    fn mirror_first(&mut self) {
        let Some(first) = self.products.first().copied() else {
            return;
        };
        if let Some(record) = self.record.as_mut() {
            record.product = first.type_id;
            record.quantity = first.quantity;
        }
    }

    /// Adds a searched product to the plan on screen: fills the only row while the plan holds no
    /// product yet, else appends it. A type already in the list is not added again, and an added
    /// product starts at one unit. Records the edit when the list changes.
    pub fn add_product(&mut self, type_id: TypeId, cx: &mut Shared) {
        let fills = self.products.len() == 1
            && self
                .products
                .first()
                .is_some_and(|product| product.type_id == 0);
        if fills {
            if let Some(first) = self.products.first_mut() {
                first.type_id = type_id;
            }
        } else {
            if self
                .products
                .iter()
                .any(|product| product.type_id == type_id)
            {
                return;
            }
            self.products.push(PlanProduct {
                type_id,
                quantity: 1,
            });
            self.quantity_texts.push("1".to_owned());
            self.quantity_ok.push(true);
        }
        self.mirror_first();
        self.edited(cx);
    }

    /// Removes the product at `row` from the plan on screen. The list cannot become empty: the
    /// last product stays. Records the edit when a product is removed.
    pub fn remove_product(&mut self, row: usize, cx: &mut Shared) {
        if !self.can_remove(row) {
            return;
        }
        self.products.remove(row);
        self.quantity_texts.remove(row);
        self.quantity_ok.remove(row);
        self.mirror_first();
        self.edited(cx);
    }

    /// Whether the product at `row` can be removed: the plan always keeps one product.
    pub fn can_remove(&self, row: usize) -> bool {
        row < self.products.len() && self.products.len() > 1
    }

    /// Sets the product at `row` from its typed quantity text. Answers whether it was set: a text
    /// that does not read as a whole number of 1 or more leaves the old quantity and shows as not
    /// ok instead.
    pub fn type_quantity(&mut self, row: usize) -> bool {
        let Some(text) = self.quantity_texts.get(row) else {
            return false;
        };
        let Some(quantity) = parse_quantity(text) else {
            if let Some(ok) = self.quantity_ok.get_mut(row) {
                *ok = false;
            }
            return false;
        };
        if let Some(ok) = self.quantity_ok.get_mut(row) {
            *ok = true;
        }
        let Some(product) = self.products.get_mut(row) else {
            return false;
        };
        product.quantity = quantity;
        self.mirror_first();
        true
    }

    /// Shows a saved plan, with nothing edited.
    pub fn load(&mut self, id: i64, cx: &mut Shared) {
        match cx.store.plan(id) {
            Ok(Some(record)) => {
                match cx.store.plan_stock(id) {
                    Ok(stock) => self.stock = stock,
                    Err(error) => {
                        self.stock = PlanStock::Pasted;
                        report(cx, format!("Could not read the plan's stock: {error}"));
                    }
                }
                match cx.store.plan_invention(id) {
                    Ok(choices) => self.invention = choices,
                    Err(error) => {
                        self.invention.clear();
                        report(
                            cx,
                            format!("Could not read the plan's invention choices: {error}"),
                        );
                    }
                }
                self.stock_error = None;
                let fallback = PlanProduct {
                    type_id: record.product,
                    quantity: record.quantity,
                };
                let products = match cx.store.plan_products(id) {
                    Ok(products) => products,
                    Err(error) => {
                        report(cx, format!("Could not read the plan's products: {error}"));
                        vec![fallback]
                    }
                };
                self.product_query.clear();
                self.selected = Some(id);
                self.record = Some(record);
                self.set_products(products);
                self.dirty = false;
                self.recalc(cx);
            }
            Ok(None) => {
                self.clear();
                report(cx, "That plan is no longer saved.");
            }
            Err(error) => report(cx, format!("Could not open the plan: {error}")),
        }
    }

    /// Takes the plan off the screen. Nothing is selected afterwards.
    pub fn clear(&mut self) {
        self.selected = None;
        self.record = None;
        self.dirty = false;
        self.stock = PlanStock::Pasted;
        self.stock_error = None;
        self.invention.clear();
        self.copy_mes.clear();
        self.products.clear();
        self.quantity_texts.clear();
        self.quantity_ok.clear();
        self.product_query.clear();
        self.result = Err(NO_PRODUCT.to_owned());
    }

    /// Saves the plan on screen. Returns whether it was saved.
    pub fn save(&mut self, cx: &mut Shared) -> bool {
        let Some(record) = self.record.as_ref() else {
            return false;
        };
        let now = (cx.clock)();
        if let Err(error) = cx.store.set_plan_products(record.id, &self.products, now) {
            report(cx, format!("Could not save the plan: {error}"));
            return false;
        }
        match cx.store.save_plan(record, now) {
            Ok(()) => {
                self.dirty = false;
                // The claims are worked out from the saved plans, so a save can change them.
                cx.claims.mark_stale();
                true
            }
            Err(error) => {
                report(cx, format!("Could not save the plan: {error}"));
                false
            }
        }
    }

    /// Throws away the unsaved edits by loading the selected plan again.
    pub fn discard(&mut self, cx: &mut Shared) {
        if let Some(id) = self.selected {
            self.load(id, cx);
        }
    }

    /// Saves the invention choice of one T2 part of the plan on screen at once and works the plan
    /// out again. The choice is not an unsaved edit, so `dirty` stays as it is. Returns whether the
    /// choice was saved; a choice the store refuses is not kept.
    pub fn set_invention(
        &mut self,
        cx: &mut Shared,
        part: TypeId,
        choice: InventionChoice,
    ) -> bool {
        let Some(id) = self.selected else {
            return false;
        };
        if let Err(error) = cx.store.set_plan_invention(id, part, choice) {
            report(cx, format!("Could not save the invention choice: {error}"));
            return false;
        }
        if choice == InventionChoice::default() {
            self.invention.remove(&part);
        } else {
            self.invention.insert(part, choice);
        }
        self.recalc(cx);
        true
    }

    /// Asks a question of the plan list: goes ahead now, or first asks about unsaved edits.
    pub fn request(&mut self, next: Next, cx: &mut Shared) {
        if matches!(next, Next::Select(id) if self.selected == Some(id)) {
            return;
        }
        if self.dirty {
            self.dialog = Some(Dialog::Leave(next));
        } else {
            self.go(next, cx);
        }
    }

    /// Does what a request asks for.
    pub fn go(&mut self, next: Next, cx: &mut Shared) {
        match next {
            Next::Select(id) => self.load(id, cx),
            Next::New => self.new_plan(cx),
            Next::Duplicate => self.duplicate(cx),
        }
    }

    /// Makes an empty plan named `New plan`, or the first free `New plan N`, and shows it.
    fn new_plan(&mut self, cx: &mut Shared) {
        let names: Vec<String> = match cx.store.plans() {
            Ok(list) => list.into_iter().map(|plan| plan.name).collect(),
            Err(error) => return report(cx, format!("Could not read the plans: {error}")),
        };
        let price_rule = match cx.store.settings() {
            Ok(settings) => settings.default_price_rule,
            Err(error) => return report(cx, format!("Could not read the settings: {error}")),
        };
        let record = PlanRecord {
            id: 0,
            name: free_name(&names),
            product: 0,
            quantity: 1,
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
        let now = (cx.clock)();
        match cx.store.create_plan(&record, now) {
            Ok(id) => self.load(id, cx),
            Err(error) => report(cx, format!("Could not make the plan: {error}")),
        }
    }

    /// Copies the saved selected plan. The selection stays where it is.
    fn duplicate(&mut self, cx: &mut Shared) {
        let Some(id) = self.selected else {
            return;
        };
        let now = (cx.clock)();
        if let Err(error) = cx.store.duplicate_plan(id, now) {
            report(cx, format!("Could not copy the plan: {error}"));
        }
    }
}

/// Brings the live claims and running jobs up to date with the login, the stock, the reserving
/// plans and the job assignments. A no-op while the key is unchanged and the claims are not stale.
///
/// Pages that read the shared claims must call this first. Without the game data nothing can be
/// counted yet: the claims are left alone and the next call tries again.
pub(crate) fn refresh_claims(cx: &mut Shared) {
    let key = claims_key(&cx.session);
    if !cx.claims.needs_refresh(&key) {
        return;
    }
    let Some(sde) = cx.sde.as_ref() else {
        return;
    };
    let recipes = match sde.recipe_book() {
        Ok(recipes) => recipes,
        Err(error) => return claims_failed(&mut cx.notices, error),
    };
    let assets = resolve(&cx.session.stock_characters()).assets;
    let characters: Vec<&CharacterData> = cx
        .session
        .characters()
        .iter()
        .filter_map(|character| cx.session.character_data(character.id))
        .collect();
    let (running, messages) = running_jobs(&characters, &assets, &recipes);
    for message in messages {
        push_error(&mut cx.notices, message);
    }
    let assignments = match cx.store.job_assignments() {
        Ok(assignments) => assignments,
        Err(error) => return claims_failed(&mut cx.notices, error),
    };
    let facilities = match cx.store.facilities() {
        Ok(facilities) => facilities,
        Err(error) => return claims_failed(&mut cx.notices, error),
    };
    let ids = match cx.store.reserving_plans() {
        Ok(ids) => ids,
        Err(error) => return claims_failed(&mut cx.notices, error),
    };
    let mut plans = Vec::new();
    let mut products = Vec::new();
    for id in ids {
        match reserving_plan(
            &mut cx.store,
            id,
            sde,
            &recipes,
            &facilities,
            &running,
            &assignments,
        ) {
            Ok((plan, held)) => {
                plans.push(plan);
                products.push(held);
            }
            Err(text) => return claims_failed(&mut cx.notices, text),
        }
    }
    let outcome = reserve::claims_many(&plans, &products, &assets, &recipes);
    cx.claims
        .set(key, running, assignments, outcome, &mut cx.notices);
}

/// The claims' key: one row per character id in the stock or the login, ascending. A character's
/// row is its id, how many times its data has been replaced, its asset count and its job count, so
/// any new data changes the key.
fn claims_key(session: &Session) -> Vec<[u64; 4]> {
    let stock = session.stock_characters();
    let mut rows: BTreeMap<i64, [u64; 4]> = BTreeMap::new();
    for owned in &stock {
        let assets = u64::try_from(owned.assets.len()).unwrap_or(u64::MAX);
        rows.insert(owned.character_id, [0, 0, assets, 0]);
    }
    for character in session.characters() {
        if let Ok(id) = i64::try_from(character.id) {
            rows.entry(id).or_insert([0; 4]);
        }
    }
    rows.into_iter()
        .filter_map(|(id, [_, _, assets, _])| {
            let id = u64::try_from(id).ok()?;
            let jobs = session
                .character_data(id)
                .and_then(|data| data.jobs.as_ref())
                .map_or(0, |jobs| u64::try_from(jobs.data.len()).unwrap_or(u64::MAX));
            Some([id, session.revision(id), assets, jobs])
        })
        .collect()
}

/// One reserving plan as the claims need it: its saved record's input, with no stock of its own,
/// its saved scope, and the units of running jobs assigned to it; and its products, which the
/// claims work out instead of the input's single product.
fn reserving_plan(
    store: &mut Store,
    id: i64,
    sde: &Sde,
    recipes: &RecipeBook,
    facilities: &[FacilityRow],
    running: &[RunningJob],
    assignments: &BTreeMap<i64, i64>,
) -> Result<(ReservingPlan, Vec<PlanProduct>), String> {
    let record = store
        .plan(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("plan {id} is no longer saved"))?;
    let products = store.plan_products(id).map_err(|error| error.to_string())?;
    let scope = match store.plan_stock(id).map_err(|error| error.to_string())? {
        PlanStock::Characters(scope) => scope,
        PlanStock::Pasted => {
            return Err(format!("plan {id} does not take its stock from characters"));
        }
    };
    let built = plan_input(
        &record,
        sde,
        recipes,
        facilities,
        &products,
        BTreeMap::new(),
        &BTreeMap::new(),
    )?;
    let first = products.first().copied().unwrap_or(PlanProduct {
        type_id: record.product,
        quantity: record.quantity,
    });
    let input = PlanInput {
        product: first.type_id,
        quantity: first.quantity,
        build: built.build,
        modifiers: built.modifiers,
        stock: built.stock,
    };
    let assigned =
        reserve::assigned_to(id, running, assignments).map_err(|error| error.to_string())?;
    Ok((
        ReservingPlan {
            plan_id: id,
            input,
            scope,
            assigned,
        },
        products,
    ))
}

/// The notice for claims that could not be worked out. Nothing is stored, so the next call tries
/// again; the notice is shown once.
fn claims_failed(notices: &mut Vec<Notice>, text: impl Display) {
    push_error(
        notices,
        format!("Could not work out plan reservations: {text}"),
    );
}

/// `New plan`, then `New plan 2`, `New plan 3`… the first one no saved plan has.
fn free_name(taken: &[String]) -> String {
    let mut number = 1_u32;
    loop {
        let candidate = if number == 1 {
            "New plan".to_owned()
        } else {
            format!("New plan {number}")
        };
        if !taken.contains(&candidate) {
            return candidate;
        }
        number += 1;
    }
}

/// Adds an error notice for something the page could not do.
pub fn report(cx: &mut Shared, text: impl Display) {
    push_error(&mut cx.notices, text.to_string());
}

/// Adds an error notice unless one with the same text is already showing, so a failure that comes
/// back every frame is shown once.
pub fn push_error(notices: &mut Vec<Notice>, text: String) {
    if !notices
        .iter()
        .any(|notice| notice.level == Level::Error && notice.text == text)
    {
        notices.push(Notice::error(text));
    }
}

/// The name of a type in the game data, or `type <id>` when the data has no such type. A lookup
/// that fails is reported to `notices` and also reads as `type <id>`.
pub fn type_name(id: TypeId, sde: Option<&Sde>, notices: &mut Vec<Notice>) -> String {
    let Some(sde) = sde else {
        return format!("type {id}");
    };
    match sde.item(id) {
        Ok(Some(item)) => item.name,
        Ok(None) => format!("type {id}"),
        Err(error) => {
            push_error(notices, format!("Could not read type {id}: {error}"));
            format!("type {id}")
        }
    }
}

/// Reads pasted text against the game data. Without game data nothing resolves. A failed lookup
/// leaves its line unread, and the first such error is returned with the text.
fn read_paste(text: &str, sde: Option<&Sde>) -> (Pasted, Option<String>) {
    let failure = RefCell::new(None);
    let pasted = parse_stock(text, &|name| {
        let sde = sde?;
        match sde.find_exact(name) {
            Ok(id) => id,
            Err(error) => {
                let mut slot = failure.borrow_mut();
                if slot.is_none() {
                    *slot = Some(error.to_string());
                }
                None
            }
        }
    });
    (pasted, failure.into_inner())
}

/// The plan for `record`, worked out from the game data and the given saved facilities, with the
/// default ME for every invented part. The error text is shown after `Could not work out the plan:`.
///
/// `facilities` is `Store::facilities()`, read by the caller: that read takes `&mut Store`, which
/// a `&Shared` cannot give. The recipe book is read here, so this reads it once per call.
pub(crate) fn plan_for(
    record: &PlanRecord,
    stock: &PlanStock,
    cx: &Shared,
    facilities: &[FacilityRow],
) -> Result<Plan, String> {
    plan_with(record, stock, cx, facilities, &BTreeMap::new())
}

/// The plan for `record`, worked out with the ME of the invented copy of each part in `copy_mes`.
pub(crate) fn plan_with(
    record: &PlanRecord,
    stock: &PlanStock,
    cx: &Shared,
    facilities: &[FacilityRow],
    copy_mes: &BTreeMap<TypeId, u8>,
) -> Result<Plan, String> {
    let products = [PlanProduct {
        type_id: record.product,
        quantity: record.quantity,
    }];
    plan_with_products(record, &products, stock, cx, facilities, copy_mes)
}

/// The plan for `record` holding all of `products`, sharing the stock between them, worked out
/// with the ME of the invented copy of each part in `copy_mes`. The error text is shown after
/// `Could not work out the plan:`.
pub(crate) fn plan_with_products(
    record: &PlanRecord,
    products: &[PlanProduct],
    stock: &PlanStock,
    cx: &Shared,
    facilities: &[FacilityRow],
    copy_mes: &BTreeMap<TypeId, u8>,
) -> Result<Plan, String> {
    if products.first().is_none_or(|product| product.type_id == 0) {
        return Err(NO_PRODUCT.to_owned());
    }
    let sde = cx.sde.as_ref().ok_or("the game data is not loaded yet")?;
    let recipes = sde.recipe_book().map_err(|error| error.to_string())?;
    let counted = counted_stock(record, stock, cx, sde)?;
    let input = plan_input(
        record, sde, &recipes, facilities, products, counted, copy_mes,
    )?;
    plan::plan_products(&input, &recipes).map_err(|error| explain(&error, record, &recipes))
}

/// The plan error for a stock that cannot be counted.
const STOCK_TOO_LARGE: &str = "the stock is too large to count";

/// The stock the plan counts. From characters: the in-scope units left after the plans served
/// before it claimed theirs; from a paste: the pasted units. Either way the units of the running
/// jobs assigned to the plan are added, since the plan serves those jobs' output itself.
fn counted_stock(
    record: &PlanRecord,
    stock: &PlanStock,
    cx: &Shared,
    sde: &Sde,
) -> Result<BTreeMap<TypeId, u64>, String> {
    let mut counted = match stock {
        PlanStock::Pasted => {
            let (pasted, failure) = read_paste(&record.pasted_stock, Some(sde));
            if let Some(error) = failure {
                return Err(format!("the pasted stock could not be read: {error}"));
            }
            pasted.stock
        }
        PlanStock::Characters(scope) => {
            let assets = resolve(&cx.session.stock_characters()).assets;
            reserve::available(scope, &assets, &cx.claims.claims, Some(record.id))
                .map_err(|_error| STOCK_TOO_LARGE.to_owned())?
        }
    };
    let assigned = reserve::assigned_to(record.id, &cx.claims.running, &cx.claims.assignments)
        .map_err(|_error| STOCK_TOO_LARGE.to_owned())?;
    for (type_id, units) in assigned {
        let total = counted.entry(type_id).or_insert(0);
        *total = total
            .checked_add(units)
            .ok_or_else(|| STOCK_TOO_LARGE.to_owned())?;
    }
    Ok(counted)
}

/// The plan engine's input: the record's product list, quantity and built types, a modifier for
/// each built type, and the stock it counts.
fn plan_input(
    record: &PlanRecord,
    sde: &Sde,
    recipes: &RecipeBook,
    facilities: &[FacilityRow],
    products: &[PlanProduct],
    stock: BTreeMap<TypeId, u64>,
    copy_mes: &BTreeMap<TypeId, u8>,
) -> Result<PlanProducts, String> {
    let manufacturing = saved_facility(record.manufacturing, facilities)?.unwrap_or(NPC_STATION);
    let reactions = saved_facility(record.reactions, facilities)?;
    sde.check_facility(&manufacturing, Activity::Manufacturing)
        .map_err(|error| error.to_string())?;
    if let Some(facility) = &reactions {
        sde.check_facility(facility, Activity::Reaction)
            .map_err(|error| error.to_string())?;
    }
    let modifiers = modifiers(
        record,
        sde,
        recipes,
        &manufacturing,
        reactions.as_ref(),
        products,
        copy_mes,
    )?;
    Ok(PlanProducts {
        products: products.to_vec(),
        build: record.build.clone(),
        modifiers,
        stock,
    })
}

/// The NPC station in high sec, used when no Manufacturing facility is picked.
const NPC_STATION: Facility = Facility {
    structure: None,
    security: Security::High,
    rigs: Vec::new(),
};

/// The facility with `id`, `None` when none is picked.
fn saved_facility(id: Option<i64>, facilities: &[FacilityRow]) -> Result<Option<Facility>, String> {
    let Some(id) = id else {
        return Ok(None);
    };
    facilities
        .iter()
        .find(|row| row.id == id)
        .map(|row| Some(row.facility.clone()))
        .ok_or_else(|| "the facility is no longer saved".to_owned())
}

/// The ME a built type is made at: the record's value, else the invented copy's ME, else 2 for an
/// invented blueprint, else 0. `me_of` keeps the default copy: no stored decryptor.
pub fn me_of(record: &PlanRecord, id: TypeId, made_by_invention: bool) -> u8 {
    me_with(record, id, made_by_invention, &BTreeMap::new())
}

/// The ME a built type is made at: the record's value, else the invented copy's ME from
/// `copy_mes`, else 2 for an invented blueprint, else 0.
pub fn me_with(
    record: &PlanRecord,
    id: TypeId,
    made_by_invention: bool,
    copy_mes: &BTreeMap<TypeId, u8>,
) -> u8 {
    if let Some(me) = record.me.get(&id) {
        return *me;
    }
    if let Some(me) = copy_mes.get(&id) {
        return *me;
    }
    if made_by_invention { 2 } else { 0 }
}

/// The ME of the invented copy of a part under `choice`: `None` unless the part is invented with a
/// decryptor and the copy works out.
pub fn copy_me(
    choice: InventionChoice,
    decryptor: Option<&Decryptor>,
    base_runs: u32,
) -> Option<u8> {
    if !choice.invent {
        return None;
    }
    let decryptor = decryptor?;
    invented_copy(base_runs, Some(decryptor))
        .ok()
        .map(|copy| copy.me)
}

/// The ME of the invented copy of each chosen part that names a decryptor, from the game data. A
/// lookup that fails or finds nothing gives no entry, so the default ME applies.
pub fn copy_mes(choices: &BTreeMap<TypeId, InventionChoice>, sde: &Sde) -> BTreeMap<TypeId, u8> {
    let mut copy_mes = BTreeMap::new();
    for (&part, &choice) in choices {
        if !choice.invent {
            continue;
        }
        let Some(decryptor_id) = choice.decryptor else {
            continue;
        };
        let Ok(Some(decryptor)) = sde.decryptor(decryptor_id) else {
            continue;
        };
        let Ok(Some((_activity, product))) = sde.invention_for(part) else {
            continue;
        };
        if let Some(me) = copy_me(choice, Some(&decryptor), product.quantity) {
            copy_mes.insert(part, me);
        }
    }
    copy_mes
}

/// The modifier of every built type that has a recipe: each built type and each product. A
/// reaction gets none while no reactions facility is picked; the plan then reports that.
fn modifiers(
    record: &PlanRecord,
    sde: &Sde,
    recipes: &RecipeBook,
    manufacturing: &Facility,
    reactions: Option<&Facility>,
    products: &[PlanProduct],
    copy_mes: &BTreeMap<TypeId, u8>,
) -> Result<BTreeMap<TypeId, Modifier>, String> {
    let mut modifiers = BTreeMap::new();
    let built = record
        .build
        .iter()
        .copied()
        .chain(products.iter().map(|product| product.type_id));
    for built in built {
        let Some(recipe) = recipes.get(built) else {
            continue;
        };
        let (facility, me) = match recipe.activity {
            Activity::Manufacturing => (
                manufacturing,
                me_with(record, built, recipe.made_by_invention, copy_mes),
            ),
            Activity::Reaction => match reactions {
                Some(facility) => (facility, 0),
                None => continue,
            },
        };
        let modifier = sde
            .modifier(facility, recipe.activity, built, me)
            .map_err(|error| error.to_string())?;
        modifiers.insert(built, modifier);
    }
    Ok(modifiers)
}

/// The plan engine's error as the page shows it. A reaction with no reactions facility gets the
/// plain instruction instead of the engine's wording.
fn explain(error: &PlanError, record: &PlanRecord, recipes: &RecipeBook) -> String {
    if let PlanError::NoModifier(built) = error
        && record.reactions.is_none()
        && recipes
            .get(*built)
            .is_some_and(|recipe| recipe.activity == Activity::Reaction)
    {
        return NO_REACTIONS.to_owned();
    }
    error.to_string()
}

/// A whole number of 1 or more, or `None`.
fn parse_quantity(text: &str) -> Option<u64> {
    text.trim()
        .parse::<u64>()
        .ok()
        .filter(|quantity| *quantity >= 1)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use mop_core::stock::{Asset, CharacterAssets, Places, StockScope};
    use mop_esi::character_data::{CharacterData, Fetched, OwnedAssets};
    use mop_esi::characters::{Character, Login};
    use mop_esi::industry::IndustryData;
    use mop_market::error::MarketError;
    use mop_market::fuzzwork::{BytesHttp, BytesResponse};
    use mop_sde::{Fetch, FetchError, Sde, import};
    use mop_store::Density;
    use tempfile::TempDir;

    use super::*;
    use crate::build::inputs::{header_text, product_matches};
    use crate::build::prices::Prices;
    use crate::claims::ClaimsState;
    use crate::selection::Selection;
    use crate::session::NoEve;
    use crate::theme::Style;
    use crate::worker::Worker;

    /// Hobgoblin II, the T2 part the acceptance numbers are for.
    const HOBGOBLIN_II: TypeId = 2456;
    const RIFTER: TypeId = 587;
    const TRITANIUM: TypeId = 34;
    /// R.A.M.- Robotics, the second product of the shared-Tritanium plans.
    const ROBOTICS: TypeId = 11_481;
    const NOW: i64 = 1_000_000;
    const MAIN: i64 = 2_112_000_001;
    const JITA_4_4: i64 = 60_003_760;

    const ATTAINMENT: Decryptor = Decryptor {
        type_id: 34202,
        chance_hundredths: 180,
        me: -1,
        te: 4,
        runs: 4,
    };

    fn invented(decryptor: Option<TypeId>) -> InventionChoice {
        InventionChoice {
            invent: true,
            decryptor,
        }
    }

    fn not_invented(decryptor: Option<TypeId>) -> InventionChoice {
        InventionChoice {
            invent: false,
            decryptor,
        }
    }

    /// A plan whose product is Hobgoblin II, with only the typed ME values given.
    fn record(me: BTreeMap<TypeId, u8>) -> PlanRecord {
        PlanRecord {
            id: 1,
            name: "Test plan".to_owned(),
            product: HOBGOBLIN_II,
            quantity: 1,
            build: BTreeSet::new(),
            me,
            manufacturing: None,
            reactions: None,
            price_rule: Default::default(),
            route: None,
            service: None,
            packing: Packing::default(),
            pasted_stock: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The build 3607705 fixture imported into `dir`, so the decryptors and invention activities
    /// are the game's.
    fn fixture_sde(dir: &TempDir) -> Sde {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../sde/tests/fixtures/sde-3607705");
        let db = dir.path().join("sde.sqlite");
        import(&source, &db).expect("the fixture imports");
        Sde::open(&db).expect("the fixture opens")
    }

    #[test]
    fn copy_me_needs_an_invented_part_and_a_decryptor() {
        assert_eq!(
            copy_me(invented(Some(34202)), Some(&ATTAINMENT), 10),
            Some(1)
        );
        assert_eq!(copy_me(invented(None), None, 10), None);
        assert_eq!(
            copy_me(not_invented(Some(34202)), Some(&ATTAINMENT), 10),
            None
        );
        // A copy that does not work out has no ME either.
        assert_eq!(copy_me(invented(Some(34202)), Some(&ATTAINMENT), 0), None);
    }

    #[test]
    fn the_typed_me_comes_first_then_the_copy_then_the_default() {
        let typed = record(BTreeMap::from([(HOBGOBLIN_II, 5)]));
        let copy = BTreeMap::from([(HOBGOBLIN_II, 1)]);
        assert_eq!(me_with(&typed, HOBGOBLIN_II, true, &copy), 5);

        let untyped = record(BTreeMap::new());
        assert_eq!(me_with(&untyped, HOBGOBLIN_II, true, &copy), 1);
        assert_eq!(me_with(&untyped, HOBGOBLIN_II, true, &BTreeMap::new()), 2);
        // A part not made by invention has no invented copy.
        assert_eq!(me_with(&untyped, 2454, false, &BTreeMap::new()), 0);
    }

    #[test]
    fn me_of_is_me_with_no_copy_me() {
        let typed = record(BTreeMap::from([(HOBGOBLIN_II, 5)]));
        assert_eq!(
            me_of(&typed, HOBGOBLIN_II, true),
            me_with(&typed, HOBGOBLIN_II, true, &BTreeMap::new())
        );
        assert_eq!(me_of(&typed, HOBGOBLIN_II, true), 5);

        let untyped = record(BTreeMap::new());
        assert_eq!(me_of(&untyped, HOBGOBLIN_II, true), 2);
        assert_eq!(me_of(&untyped, 2454, false), 0);
    }

    #[test]
    fn copy_mes_reads_the_decryptor_and_the_parts_copy() {
        let dir = TempDir::new().expect("a temp folder");
        let sde = fixture_sde(&dir);

        let chosen = BTreeMap::from([
            (HOBGOBLIN_II, invented(Some(34202))),
            // A part whose blueprint no invention makes has no copy ME.
            (2454, invented(Some(34202))),
        ]);
        assert_eq!(copy_mes(&chosen, &sde), BTreeMap::from([(HOBGOBLIN_II, 1)]));

        let no_decryptor = BTreeMap::from([(HOBGOBLIN_II, invented(None))]);
        assert_eq!(copy_mes(&no_decryptor, &sde), BTreeMap::new());

        let not_invented = BTreeMap::from([(HOBGOBLIN_II, not_invented(Some(34202)))]);
        assert_eq!(copy_mes(&not_invented, &sde), BTreeMap::new());

        // A decryptor type the game data does not have gives no entry.
        let unknown_decryptor = BTreeMap::from([(HOBGOBLIN_II, invented(Some(999_999)))]);
        assert_eq!(copy_mes(&unknown_decryptor, &sde), BTreeMap::new());

        assert_eq!(copy_mes(&BTreeMap::new(), &sde), BTreeMap::new());
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

    /// The fixture game data and an empty user store in `dir`, with everything `Shared` holds
    /// around them. The clock is fixed at [`NOW`].
    fn shared(dir: &TempDir) -> Shared {
        let store = Store::open(&dir.path().join("user.sqlite"), NOW)
            .expect("the store opens")
            .store;
        Shared {
            sde: Some(fixture_sde(dir)),
            store,
            worker: Worker::spawn(egui::Context::default(), Box::new(NoFetch)),
            market: Arc::new(Prices::new(Box::new(NoHttp))),
            // The net worth copy job that would ask for PLEX never runs in these tests, so the
            // client is never used.
            plex_http: Arc::new(mop_market::http::UreqHttp::new()),
            copy_notices: Arc::new(Mutex::new(Vec::new())),
            style: Style::eve_dense(),
            density: Density::Normal,
            rail_icons_only: false,
            data_dir: dir.path().to_path_buf(),
            notices: Vec::new(),
            clock: Box::new(|| NOW),
            session: Session::new(Box::new(NoEve)),
            selection: Selection::default(),
            industry: IndustryData::default(),
            claims: ClaimsState::default(),
        }
    }

    /// The type the product search finds first for `query`.
    fn found(query: &str, page: &BuildPage, cx: &mut Shared) -> TypeId {
        product_matches(query, page, cx)
            .first()
            .unwrap_or_else(|| panic!("no product matches {query:?}"))
            .0
    }

    /// Main, with one hangar entry of `units` of `type_id` at Jita: the only stock any plan
    /// counts.
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

    /// A saved plan of `quantity` of `product` at the NPC station, taking Main's stock everywhere
    /// and reserving. Its id.
    fn reserving(cx: &mut Shared, name: &str, product: TypeId, quantity: u64) -> i64 {
        let record = PlanRecord {
            id: 0,
            name: name.to_owned(),
            product,
            quantity,
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
        let id = cx
            .store
            .create_plan(&record, NOW)
            .expect("the plan is saved");
        let scope = StockScope {
            characters: [MAIN].into(),
            places: Places::All,
        };
        cx.store
            .set_plan_stock(id, &PlanStock::Characters(scope))
            .expect("the stock is saved");
        cx.store
            .set_reserve(id, true)
            .expect("the reserve is saved");
        id
    }

    /// A new plan on screen holding Hobgoblin II × 1 then Rifter × 2, both saved.
    fn two_products(dir: &TempDir) -> (Shared, BuildPage) {
        let mut cx = shared(dir);
        let mut page = BuildPage::default();
        page.go(Next::New, &mut cx);
        let hobgoblin = found("Hobgoblin II", &page, &mut cx);
        page.add_product(hobgoblin, &mut cx);
        let rifter = found("Rifter", &page, &mut cx);
        page.add_product(rifter, &mut cx);
        page.quantity_texts[1] = "2".to_owned();
        assert!(page.type_quantity(1));
        page.edited(&mut cx);
        assert!(page.save(&mut cx));
        (cx, page)
    }

    #[test]
    fn a_new_plan_holds_one_product_and_the_search_fills_it() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        let mut page = BuildPage::default();
        page.go(Next::New, &mut cx);

        assert_eq!(
            page.products,
            vec![PlanProduct {
                type_id: 0,
                quantity: 1
            }]
        );
        assert_eq!(page.quantity_texts, vec!["1".to_owned()]);

        let hobgoblin = found("Hobgoblin II", &page, &mut cx);
        assert_eq!(hobgoblin, HOBGOBLIN_II);
        page.add_product(hobgoblin, &mut cx);
        assert_eq!(
            page.products,
            vec![PlanProduct {
                type_id: HOBGOBLIN_II,
                quantity: 1
            }]
        );
        assert_eq!(
            page.record.as_ref().map(|record| record.product),
            Some(HOBGOBLIN_II)
        );
        assert!(page.dirty);
        // Saved, the heading is exactly the first product and its quantity.
        assert!(page.save(&mut cx));
        assert_eq!(header_text(&page, &mut cx), "PLAN  Hobgoblin II × 1");

        // A quantity text of 10 gives a result whose only step is Hobgoblin II, needed 10.
        page.quantity_texts[0] = "10".to_owned();
        assert!(page.type_quantity(0));
        page.edited(&mut cx);
        let plan = page.result.as_ref().expect("the plan works out");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].type_id, HOBGOBLIN_II);
        assert_eq!(plan.steps[0].needed, 10);
    }

    #[test]
    fn a_second_product_is_appended_and_the_heading_counts_it() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        let mut page = BuildPage::default();
        page.go(Next::New, &mut cx);
        let hobgoblin = found("Hobgoblin II", &page, &mut cx);
        page.add_product(hobgoblin, &mut cx);

        let rifter = found("Rifter", &page, &mut cx);
        assert_eq!(rifter, RIFTER);
        page.add_product(rifter, &mut cx);
        assert_eq!(
            page.products,
            vec![
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 1
                },
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 1
                },
            ]
        );
        assert_eq!(
            header_text(&page, &mut cx),
            "PLAN  Hobgoblin II × 1 + 1 more (unsaved)"
        );
        assert!(page.save(&mut cx));
        assert_eq!(
            header_text(&page, &mut cx),
            "PLAN  Hobgoblin II × 1 + 1 more"
        );

        let plan = page.result.as_ref().expect("the plan works out");
        let steps: Vec<TypeId> = plan.steps.iter().map(|step| step.type_id).collect();
        assert_eq!(steps, vec![RIFTER, HOBGOBLIN_II]);

        // A product already in the list is not added again.
        page.add_product(RIFTER, &mut cx);
        assert_eq!(page.products.len(), 2);
    }

    #[test]
    fn the_products_are_saved_with_the_plan_and_read_back() {
        let dir = TempDir::new().expect("a temp folder");
        let (mut cx, page) = two_products(&dir);
        let id = page.selected.expect("a plan is selected");

        assert_eq!(
            cx.store.plan_products(id).expect("the stored products"),
            vec![
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 1
                },
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 2
                },
            ]
        );
        // The plan's own columns follow the first product.
        let record = cx
            .store
            .plan(id)
            .expect("the record")
            .expect("the saved plan");
        assert_eq!((record.product, record.quantity), (HOBGOBLIN_II, 1));

        let mut reloaded = BuildPage::default();
        reloaded.load(id, &mut cx);
        assert_eq!(
            reloaded.products,
            vec![
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 1
                },
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 2
                },
            ]
        );
        assert_eq!(
            reloaded.quantity_texts,
            vec!["1".to_owned(), "2".to_owned()]
        );
        assert!(!reloaded.dirty);
        assert_eq!(
            header_text(&reloaded, &mut cx),
            "PLAN  Hobgoblin II × 1 + 1 more"
        );
    }

    #[test]
    fn removing_a_product_leaves_the_others_and_the_last_stays() {
        let dir = TempDir::new().expect("a temp folder");
        let (mut cx, mut page) = two_products(&dir);
        let id = page.selected.expect("a plan is selected");

        page.remove_product(0, &mut cx);
        assert_eq!(
            page.products,
            vec![PlanProduct {
                type_id: RIFTER,
                quantity: 2
            }]
        );
        assert!(page.save(&mut cx));
        assert_eq!(header_text(&page, &mut cx), "PLAN  Rifter × 2");
        assert_eq!(
            cx.store.plan_products(id).expect("the stored products"),
            vec![PlanProduct {
                type_id: RIFTER,
                quantity: 2
            }]
        );
        let record = cx
            .store
            .plan(id)
            .expect("the record")
            .expect("the saved plan");
        assert_eq!((record.product, record.quantity), (RIFTER, 2));

        // With one product Remove is disabled and the list cannot become empty.
        assert!(!page.can_remove(0));
        page.remove_product(0, &mut cx);
        assert_eq!(
            page.products,
            vec![PlanProduct {
                type_id: RIFTER,
                quantity: 2
            }]
        );
    }

    #[test]
    fn a_quantity_text_that_does_not_read_keeps_the_old_quantity() {
        let dir = TempDir::new().expect("a temp folder");
        let (_cx, mut page) = two_products(&dir);

        page.quantity_texts[1] = "three".to_owned();
        assert!(!page.type_quantity(1));
        assert!(!page.quantity_ok[1]);
        assert_eq!(
            page.products,
            vec![
                PlanProduct {
                    type_id: HOBGOBLIN_II,
                    quantity: 1
                },
                PlanProduct {
                    type_id: RIFTER,
                    quantity: 2
                },
            ]
        );

        page.quantity_texts[1] = "0".to_owned();
        assert!(!page.type_quantity(1));
        assert_eq!(page.products[1].quantity, 2);

        page.quantity_texts[1] = "5".to_owned();
        assert!(page.type_quantity(1));
        assert!(page.quantity_ok[1]);
        assert_eq!(page.products[1].quantity, 5);
        assert_eq!(
            page.record.as_ref().map(|record| record.quantity),
            Some(1),
            "the record follows only the first product"
        );
    }

    #[test]
    fn the_claims_read_each_reserving_plans_products() {
        let dir = TempDir::new().expect("a temp folder");
        let mut cx = shared(&dir);
        give_stock(&mut cx, TRITANIUM, 500_000);
        let robotics = reserving(&mut cx, "Robotics", ROBOTICS, 10);
        let rifter = reserving(&mut cx, "Rifter plan", RIFTER, 20);
        assert_eq!(
            cx.store.reserving_plans().expect("the reserving plans"),
            vec![robotics, rifter]
        );

        refresh_claims(&mut cx);
        assert!(cx.notices.is_empty(), "{:?}", cx.notices);
        // R.A.M.- Robotics × 10 needs one run: 556 Tritanium, taken first.
        let first = cx
            .claims
            .claims
            .by_plan
            .get(&robotics)
            .expect("the first claim");
        assert_eq!(first.from_assets.get(&TRITANIUM), Some(&556));
        // The Rifter × 20 needs 640,000 Tritanium: what is left of the 500,000 is taken and the
        // rest is bought.
        let second = cx
            .claims
            .claims
            .by_plan
            .get(&rifter)
            .expect("the second claim");
        let line = second
            .plan
            .buy
            .iter()
            .find(|line| line.type_id == TRITANIUM)
            .expect("the Tritanium line");
        assert_eq!(
            (line.needed, line.from_stock, line.to_buy),
            (640_000, 499_444, 140_556)
        );
        assert_eq!(second.from_assets.get(&TRITANIUM), Some(&499_444));
    }
}
