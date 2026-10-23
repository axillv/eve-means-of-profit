//! What the player has picked on the pages: an item, a place and a character. Held while the app
//! runs; never saved.

use mop_core::industry::TypeId;
use mop_core::stock::LocationId;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub item: Option<TypeId>,
    pub place: Option<LocationId>,
    pub character: Option<u64>,
}
