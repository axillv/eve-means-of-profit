//! What the owned items are worth on the Assets page: the app-wide price basis and the saved prices
//! of each type. Reads the store only; fetches nothing.

use std::collections::BTreeSet;

use mop_core::industry::TypeId;
use mop_core::money::Money;
use mop_core::net_worth::PriceBasis;
use mop_store::{PriceHistory, Store, StoreError};

/// The saved prices a page values stacks with: the basis, today's day, and each type's saved prices.
pub(super) struct Prices {
    basis: PriceBasis,
    today: i64,
    history: PriceHistory,
}

impl Prices {
    /// Reads the basis and the prices saved for `types` up to `today`.
    pub(super) fn load(
        store: &mut Store,
        types: &BTreeSet<TypeId>,
        today: i64,
    ) -> Result<Self, StoreError> {
        let basis = store.net_worth_settings()?.basis;
        let history = store.price_history(types, today, today)?;
        Ok(Self {
            basis,
            today,
            history,
        })
    }

    /// The price of one unit of `type_id` at the basis, from its latest saved day up to today; `None`
    /// when the app has no such price.
    pub(super) fn price(&self, type_id: TypeId) -> Option<Money> {
        self.history.get(type_id, self.today)?.price(self.basis)
    }

    /// The newest saved day that a priced type among `types` is priced from; `None` when none has a price.
    pub(super) fn newest_day(&self, types: impl IntoIterator<Item = TypeId>) -> Option<i64> {
        types
            .into_iter()
            .filter(|type_id| self.price(*type_id).is_some())
            .filter_map(|type_id| self.history.saved_day(type_id, self.today))
            .max()
    }
}

/// The name of a basis as the price rule shows it.
pub(super) fn basis_label(basis: PriceBasis) -> &'static str {
    match basis {
        PriceBasis::Midpoint => "Forge midpoint",
        PriceBasis::HighestBuy => "Forge highest buy",
        PriceBasis::LowestSell => "Forge lowest sell",
        PriceBasis::Average => "EVE average",
    }
}

/// The value of `quantity` units at `price`; `None` when it does not fit.
pub(super) fn stack_value(price: Money, quantity: u64) -> Option<Money> {
    price.checked_mul_qty(quantity)
}
