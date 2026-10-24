//! The Build page's state: the plan on screen, its unsaved edits, the last calculation, and the
//! actions that save, load and create plans.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;

use mop_core::freight::Packing;
use mop_core::industry::{Activity, Modifier, RecipeBook, TypeId};
use mop_core::paste::{Pasted, parse_stock};
use mop_core::plan::{self, Plan, PlanError, PlanInput};
use mop_core::stock::{StockError, resolve};
use mop_sde::{Facility, Sde, Security};
use mop_store::reservations::PlanStock;
use mop_store::{FacilityRow, PlanRecord};

use crate::Shared;
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
                Ok(facilities) => plan_for(record, &self.stock, cx, &facilities),
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

/// The plan for `record`, worked out from the game data and the given saved facilities. The error
/// text is shown after `Could not work out the plan:`.
///
/// `facilities` is `Store::facilities()`, read by the caller: that read takes `&mut Store`, which
/// a `&Shared` cannot give. The recipe book is read here, so this reads it once per call.
pub(crate) fn plan_for(
    record: &PlanRecord,
    stock: &PlanStock,
    cx: &Shared,
    facilities: &[FacilityRow],
) -> Result<Plan, String> {
    if record.product == 0 {
        return Err(NO_PRODUCT.to_owned());
    }
    let sde = cx.sde.as_ref().ok_or("the game data is not loaded yet")?;
    let recipes = sde.recipe_book().map_err(|error| error.to_string())?;
    let counted = counted_stock(record, stock, cx, sde)?;
    let input = plan_input(record, sde, &recipes, facilities, counted)?;
    plan::plan(&input, &recipes).map_err(|error| explain(&error, record, &recipes))
}

/// The stock the plan counts: the pasted text when it is pasted, else what the chosen characters
/// own in the chosen places. The pasted text is not read when the characters are used.
fn counted_stock(
    record: &PlanRecord,
    stock: &PlanStock,
    cx: &Shared,
    sde: &Sde,
) -> Result<BTreeMap<TypeId, u64>, String> {
    match stock {
        PlanStock::Pasted => {
            let (pasted, failure) = read_paste(&record.pasted_stock, Some(sde));
            if let Some(error) = failure {
                return Err(format!("the pasted stock could not be read: {error}"));
            }
            Ok(pasted.stock)
        }
        PlanStock::Characters(scope) => resolve(&cx.session.stock_characters())
            .owned(scope)
            .map_err(|error| stock_error_text(error, sde)),
    }
}

/// The plan error for a stock that cannot be counted, naming the type.
fn stock_error_text(error: StockError, sde: &Sde) -> String {
    let StockError::Overflow { type_id } = error;
    let name = match sde.item(type_id) {
        Ok(Some(item)) => item.name,
        _ => format!("type {type_id}"),
    };
    format!("the stock of {name} is too large to count")
}

/// The plan engine's input: the record's product, quantity and built types, a modifier for each
/// built type, and the stock it counts.
fn plan_input(
    record: &PlanRecord,
    sde: &Sde,
    recipes: &RecipeBook,
    facilities: &[FacilityRow],
    stock: BTreeMap<TypeId, u64>,
) -> Result<PlanInput, String> {
    let manufacturing = saved_facility(record.manufacturing, facilities)?.unwrap_or(NPC_STATION);
    let reactions = saved_facility(record.reactions, facilities)?;
    sde.check_facility(&manufacturing, Activity::Manufacturing)
        .map_err(|error| error.to_string())?;
    if let Some(facility) = &reactions {
        sde.check_facility(facility, Activity::Reaction)
            .map_err(|error| error.to_string())?;
    }
    let modifiers = modifiers(record, sde, recipes, &manufacturing, reactions.as_ref())?;
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

/// The ME a built type is made at: the record's value, else 2 for an invented blueprint, else 0.
pub fn me_of(record: &PlanRecord, id: TypeId, made_by_invention: bool) -> u8 {
    record
        .me
        .get(&id)
        .copied()
        .unwrap_or(if made_by_invention { 2 } else { 0 })
}

/// The modifier of every built type that has a recipe. A reaction gets none while no reactions
/// facility is picked; the plan then reports that.
fn modifiers(
    record: &PlanRecord,
    sde: &Sde,
    recipes: &RecipeBook,
    manufacturing: &Facility,
    reactions: Option<&Facility>,
) -> Result<BTreeMap<TypeId, Modifier>, String> {
    let mut modifiers = BTreeMap::new();
    for built in record.build.iter().copied().chain([record.product]) {
        let Some(recipe) = recipes.get(built) else {
            continue;
        };
        let (facility, me) = match recipe.activity {
            Activity::Manufacturing => (
                manufacturing,
                me_of(record, built, recipe.made_by_invention),
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
