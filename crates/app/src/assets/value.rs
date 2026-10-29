//! What the owned items are worth on the Assets page: the value of one stack at the app-wide price.
//! Reads nothing; fetches nothing.

use mop_core::money::Money;

/// The value of `quantity` units at `price`; `None` when it does not fit.
pub(super) fn stack_value(price: Money, quantity: u64) -> Option<Money> {
    price.checked_mul_qty(quantity)
}
