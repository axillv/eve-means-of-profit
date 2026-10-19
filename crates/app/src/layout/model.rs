//! The layout model: named panels arranged in splits and tabs. Plain data; the text, the saved
//! file and the screen conversion all read and write it.

/// Most panels one layout may hold.
pub const MAX_PANELS: usize = 64;
/// Most levels of splits and tabs one layout may nest.
pub const MAX_NESTING: usize = 16;
/// Longest layout name, in characters, after trimming.
pub const MAX_NAME: usize = 60;

/// The selection kind a panel follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Follows {
    Nothing,
    Plan,
    Stockpile,
    Item,
    Place,
    Character,
}

/// A panel this version knows. `key` is stable and never shown; `name` is what the player sees
/// and what the text uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelInfo {
    pub key: &'static str,
    pub name: &'static str,
    pub follows: Follows,
}

/// What a panel shows: the current selection, or one plan or stockpile by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Follow,
    PinPlan(i64),
    PinStockpile(i64),
}

/// A panel this version knows, by key; or a name this version does not know, kept as written so
/// the layout survives a load and a save (design note 5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelRef {
    Known(&'static str),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub panel: PanelRef,
    pub binding: Binding,
}

/// Which way a split divides its space: `Right` puts `first` on the left, `Down` on top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dir {
    Right,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Panel(Slot),
    /// Two or more panels as tabs; the first is shown first.
    Tabs(Vec<Slot>),
    /// `percent` (1 to 99) is the share of `first`.
    Split {
        dir: Dir,
        percent: u8,
        first: Box<Node>,
        second: Box<Node>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub name: String,
    /// `None` is an empty layout: every panel closed.
    pub root: Option<Node>,
}

/// A pin that the panel cannot take: only a plan panel takes a plan pin, only a stockpile panel a
/// stockpile pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    BadPin { panel: String },
}

impl Slot {
    /// Refuses a pin on a panel that cannot be pinned to that kind.
    pub fn check(&self, panels: &[PanelInfo]) -> Result<(), CheckError> {
        let follows = match &self.panel {
            PanelRef::Known(key) => panels
                .iter()
                .find(|info| info.key == *key)
                .map(|info| info.follows.clone()),
            PanelRef::Unknown(_) => None,
        };
        let allowed = match (&self.binding, follows) {
            (Binding::Follow, _)
            | (Binding::PinPlan(_), Some(Follows::Plan))
            | (Binding::PinStockpile(_), Some(Follows::Stockpile)) => true,
            (Binding::PinPlan(_) | Binding::PinStockpile(_), _) => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(CheckError::BadPin {
                panel: display_name(&self.panel, panels).to_string(),
            })
        }
    }
}

/// The name the player sees for a panel: the listed name, or the name as written for a panel this
/// version does not know.
pub fn display_name<'a>(panel: &'a PanelRef, panels: &'a [PanelInfo]) -> &'a str {
    match panel {
        PanelRef::Known(key) => panels
            .iter()
            .find(|info| info.key == *key)
            .map_or(*key, |info| info.name),
        PanelRef::Unknown(name) => name,
    }
}

impl Node {
    /// Every slot under this node, in the order they are shown.
    pub fn slots(&self) -> Vec<&Slot> {
        match self {
            Node::Panel(slot) => vec![slot],
            Node::Tabs(slots) => slots.iter().collect(),
            Node::Split { first, second, .. } => {
                let mut all = first.slots();
                all.extend(second.slots());
                all
            }
        }
    }
}

impl Layout {
    /// Refuses the first pin that its panel cannot take.
    pub fn check(&self, panels: &[PanelInfo]) -> Result<(), CheckError> {
        match &self.root {
            None => Ok(()),
            Some(root) => root
                .slots()
                .into_iter()
                .try_for_each(|slot| slot.check(panels)),
        }
    }
}
