//! The Build page's state: the plan on screen, its unsaved edits, the last calculation, and the
//! actions that save, load and create plans.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use mop_core::freight::Packing;
use mop_core::industry::{Activity, Modifier, RecipeBook, TypeId};
use mop_core::invention::{Decryptor, invented_copy};
use mop_core::paste::{Pasted, parse_stock};
use mop_core::plan::{self, Plan, PlanError, PlanInput};
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
    /// The quantity as typed, and whether it reads as a whole number of 1 or more.
    pub quantity_text: String,
    pub quantity_ok: bool,
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
            quantity_text: String::new(),
            quantity_ok: true,
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
                Ok(facilities) => plan_with(record, &self.stock, cx, &facilities, &self.copy_mes),
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
                self.quantity_text = record.quantity.to_string();
                self.quantity_ok = true;
                self.product_query.clear();
                self.selected = Some(id);
                self.record = Some(record);
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
        self.quantity_text.clear();
        self.product_query.clear();
        self.result = Err(NO_PRODUCT.to_owned());
    }

    /// Saves the plan on screen. Returns whether it was saved.
    pub fn save(&mut self, cx: &mut Shared) -> bool {
        let Some(record) = self.record.as_ref() else {
            return false;
        };
        let now = (cx.clock)();
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
            Ok(plan) => plans.push(plan),
            Err(text) => return claims_failed(&mut cx.notices, text),
        }
    }
    let outcome = reserve::claims(&plans, &assets, &recipes);
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
/// its saved scope, and the units of running jobs assigned to it.
fn reserving_plan(
    store: &mut Store,
    id: i64,
    sde: &Sde,
    recipes: &RecipeBook,
    facilities: &[FacilityRow],
    running: &[RunningJob],
    assignments: &BTreeMap<i64, i64>,
) -> Result<ReservingPlan, String> {
    let record = store
        .plan(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("plan {id} is no longer saved"))?;
    let scope = match store.plan_stock(id).map_err(|error| error.to_string())? {
        PlanStock::Characters(scope) => scope,
        PlanStock::Pasted => {
            return Err(format!("plan {id} does not take its stock from characters"));
        }
    };
    let input = plan_input(
        &record,
        sde,
        recipes,
        facilities,
        BTreeMap::new(),
        &BTreeMap::new(),
    )?;
    let assigned =
        reserve::assigned_to(id, running, assignments).map_err(|error| error.to_string())?;
    Ok(ReservingPlan {
        plan_id: id,
        input,
        scope,
        assigned,
    })
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
    if record.product == 0 {
        return Err(NO_PRODUCT.to_owned());
    }
    let sde = cx.sde.as_ref().ok_or("the game data is not loaded yet")?;
    let recipes = sde.recipe_book().map_err(|error| error.to_string())?;
    let counted = counted_stock(record, stock, cx, sde)?;
    let input = plan_input(record, sde, &recipes, facilities, counted, copy_mes)?;
    plan::plan(&input, &recipes).map_err(|error| explain(&error, record, &recipes))
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

/// The plan engine's input: the record's product, quantity and built types, a modifier for each
/// built type, and the stock it counts.
fn plan_input(
    record: &PlanRecord,
    sde: &Sde,
    recipes: &RecipeBook,
    facilities: &[FacilityRow],
    stock: BTreeMap<TypeId, u64>,
    copy_mes: &BTreeMap<TypeId, u8>,
) -> Result<PlanInput, String> {
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
        copy_mes,
    )?;
    Ok(PlanInput {
        product: record.product,
        quantity: record.quantity,
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

/// The modifier of every built type that has a recipe. A reaction gets none while no reactions
/// facility is picked; the plan then reports that.
fn modifiers(
    record: &PlanRecord,
    sde: &Sde,
    recipes: &RecipeBook,
    manufacturing: &Facility,
    reactions: Option<&Facility>,
    copy_mes: &BTreeMap<TypeId, u8>,
) -> Result<BTreeMap<TypeId, Modifier>, String> {
    let mut modifiers = BTreeMap::new();
    for built in record.build.iter().copied().chain([record.product]) {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mop_sde::{Sde, import};
    use tempfile::TempDir;

    use super::*;

    /// Hobgoblin II, the T2 part the acceptance numbers are for.
    const HOBGOBLIN_II: TypeId = 2456;

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
}
