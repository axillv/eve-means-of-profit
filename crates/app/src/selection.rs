//! What the player has picked on the pages: an item, a place and a character. Held while the app
//! runs; never saved.

use mop_core::industry::TypeId;
use mop_core::stock::LocationId;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub item: Option<TypeId>,
    pub place: Option<LocationId>,
    pub character: Option<u64>,
    /// The stockpile picked on the Stockpiles page.
    pub stockpile: Option<i64>,
    /// Whether the Item window's pin keeps it open against Escape and the close button.
    pub item_pinned: bool,
}

impl Selection {
    /// Clears the picked item unless the pin holds it. This is the only place `item` becomes
    /// `None`, and it runs only unpinned, so nothing picked means not pinned.
    pub fn close_item(&mut self) {
        if !self.item_pinned {
            self.item = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    #[test]
    fn the_pin_holds_the_item_against_close() {
        let mut selection = Selection::default();
        assert_eq!(selection.item, None);
        assert!(!selection.item_pinned);

        selection.item = Some(11_688);
        selection.close_item();
        assert_eq!(selection.item, None);

        let mut pinned = Selection {
            item: Some(11_688),
            item_pinned: true,
            ..Selection::default()
        };
        pinned.close_item();
        assert_eq!(pinned.item, Some(11_688));
        assert!(pinned.item_pinned);
    }
}
