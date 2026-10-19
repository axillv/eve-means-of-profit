//! The layout as text: the lines a player copies into a chat, or saves to a file, and the reader
//! for them. Format 1 is a header line, the name line, then one node: a panel, `tabs` over two or
//! more panels, or `split right|down <p>%` over exactly two nodes. Children are indented two
//! spaces more than their parent.

use std::fmt;

use crate::layout::model::{
    Binding, CheckError, Dir, Layout, MAX_NAME, MAX_NESTING, MAX_PANELS, Node, PanelInfo, PanelRef,
    Slot, display_name,
};

const HEADER: &str = "EVE Means of Profit layout";
const FORMAT: u32 = 1;
const FENCE: &str = "```";

const NEED_NAME: &str = "the layout needs a name.";
const NEED_NODE: &str = "the layout needs a panel, a split or \"empty\" below the name.";
const NOT_BELOW: &str = "this line is not below a split or tabs.";
const INDENT: &str = "indent with two spaces per level.";
const ONLY_PANELS: &str = "only panels can go inside \"tabs\".";

/// A reading or writing error. `line` is the 1-based line in the text, or `None` for a problem
/// with the whole text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextError {
    pub line: Option<usize>,
    pub message: String,
}

impl TextError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "Line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for TextError {}

/// Which text to write. `Share` has no pins, for a chat or a file a stranger reads; `Saved` keeps
/// the pins, for the user's own file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Share,
    Saved,
}

/// A layout read from text, and the panel names this version did not know, in order of
/// appearance, each once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub layout: Layout,
    pub unknown: Vec<String>,
}

/// The layout as text, ending in one newline.
pub fn to_text(layout: &Layout, panels: &[PanelInfo], mode: Mode) -> String {
    let mut out = format!("{HEADER} {FORMAT}\nname: {}\n", layout.name.trim());
    match &layout.root {
        None => out.push_str("empty\n"),
        Some(node) => write_node(node, panels, &mode, 0, &mut out),
    }
    out
}

/// The shared text inside a code block, so Discord keeps the leading spaces.
pub fn copy_text(layout: &Layout, panels: &[PanelInfo]) -> String {
    format!("{FENCE}\n{}{FENCE}\n", to_text(layout, panels, Mode::Share))
}

fn write_node(node: &Node, panels: &[PanelInfo], mode: &Mode, level: usize, out: &mut String) {
    let indent = "  ".repeat(level);
    match node {
        Node::Panel(slot) => write_slot(slot, panels, mode, level, out),
        Node::Tabs(slots) => {
            out.push_str(&format!("{indent}tabs\n"));
            for slot in slots {
                write_slot(slot, panels, mode, level + 1, out);
            }
        }
        Node::Split {
            dir,
            percent,
            first,
            second,
        } => {
            let word = match dir {
                Dir::Right => "right",
                Dir::Down => "down",
            };
            out.push_str(&format!("{indent}split {word} {percent}%\n"));
            write_node(first, panels, mode, level + 1, out);
            write_node(second, panels, mode, level + 1, out);
        }
    }
}

fn write_slot(slot: &Slot, panels: &[PanelInfo], mode: &Mode, level: usize, out: &mut String) {
    let pin = match (mode, &slot.binding) {
        (Mode::Share, _) | (_, Binding::Follow) => String::new(),
        (Mode::Saved, Binding::PinPlan(id)) => format!(" [pinned plan {id}]"),
        (Mode::Saved, Binding::PinStockpile(id)) => format!(" [pinned stockpile {id}]"),
    };
    let name = display_name(&slot.panel, panels);
    out.push_str(&format!("{}{name}{pin}\n", "  ".repeat(level)));
}

/// The layout a chat paste or a shared file holds. Pins are dropped: the pasted layout follows.
pub fn parse_shared(text: &str, panels: &[PanelInfo]) -> Result<Parsed, TextError> {
    parse_first(text, panels, false)
}

/// The layout in the user's own saved text, pins included.
pub fn parse_saved(text: &str, panels: &[PanelInfo]) -> Result<Parsed, TextError> {
    parse_first(text, panels, true)
}

/// Every layout in a saved file, in order.
pub fn parse_saved_all(text: &str, panels: &[PanelInfo]) -> Result<Vec<Layout>, TextError> {
    let found = read_layouts(text, panels, true, false)?;
    Ok(found.into_iter().map(|parsed| parsed.layout).collect())
}

fn parse_first(text: &str, panels: &[PanelInfo], keep_pins: bool) -> Result<Parsed, TextError> {
    read_layouts(text, panels, keep_pins, true)?
        .into_iter()
        .next()
        .ok_or_else(no_layout)
}

fn no_layout() -> TextError {
    TextError {
        line: None,
        message: format!("No layout found: the text has no line \"{HEADER} {FORMAT}\"."),
    }
}

/// Reads the layouts in `text`, one after another. Lines before a header are skipped, fence lines
/// are skipped anywhere, and a blank line ends a layout.
fn read_layouts(
    text: &str,
    panels: &[PanelInfo],
    keep_pins: bool,
    first_only: bool,
) -> Result<Vec<Parsed>, TextError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(|(_, line)| line.trim() != FENCE);
    let mut found = Vec::new();
    while let Some((header_no, header)) = lines.next() {
        let Some(number) = header_number(header) else {
            continue;
        };
        if number != FORMAT {
            return Err(TextError::at(
                header_no,
                format!("this layout was made by a newer version of the app (format {number})."),
            ));
        }
        let body: Vec<(usize, &str)> = lines
            .by_ref()
            .take_while(|(_, line)| !line.trim().is_empty())
            .collect();
        found.push(parse_body(header_no, &body, panels, keep_pins)?);
        if first_only {
            break;
        }
    }
    if found.is_empty() {
        return Err(no_layout());
    }
    Ok(found)
}

/// The number after the header words, when the line is a header at all.
fn header_number(line: &str) -> Option<u32> {
    line.trim().strip_prefix(HEADER)?.trim().parse().ok()
}

fn parse_body(
    header_no: usize,
    body: &[(usize, &str)],
    panels: &[PanelInfo],
    keep_pins: bool,
) -> Result<Parsed, TextError> {
    let Some((&(name_no, name_line), rest)) = body.split_first() else {
        return Err(TextError::at(header_no + 1, NEED_NAME));
    };
    let name = name_line.trim().strip_prefix("name:").unwrap_or("").trim();
    if name.is_empty() {
        return Err(TextError::at(name_no, NEED_NAME));
    }
    if name.chars().count() > MAX_NAME {
        return Err(TextError::at(
            name_no,
            format!("the name is longer than {MAX_NAME} characters."),
        ));
    }
    let mut cursor = Cursor {
        lines: read_lines(rest)?,
        pos: 0,
        panels,
        keep_pins,
        unknown: Vec::new(),
    };
    let root = cursor.root(name_no + 1)?;
    if let Some(root) = &root {
        let count = root.slots().len();
        if count > MAX_PANELS {
            return Err(TextError {
                line: None,
                message: format!("too many panels: {count} (at most {MAX_PANELS})."),
            });
        }
    }
    Ok(Parsed {
        layout: Layout {
            name: name.to_string(),
            root,
        },
        unknown: cursor.unknown,
    })
}

/// One body line: its line number, its nesting level (indent over two), and its text.
#[derive(Debug, Clone, Copy)]
struct Line<'a> {
    no: usize,
    level: usize,
    text: &'a str,
}

fn read_lines<'a>(body: &[(usize, &'a str)]) -> Result<Vec<Line<'a>>, TextError> {
    body.iter()
        .map(|&(no, raw)| {
            let rest = raw.trim_start_matches(' ');
            let spaces = raw.len() - rest.len();
            if spaces % 2 != 0 || rest.starts_with(char::is_whitespace) {
                return Err(TextError::at(no, INDENT));
            }
            Ok(Line {
                no,
                level: spaces / 2,
                text: rest.trim_end(),
            })
        })
        .collect()
}

/// Reads the nodes below the name line, one top-level node at a time.
struct Cursor<'a, 'p> {
    lines: Vec<Line<'a>>,
    pos: usize,
    panels: &'p [PanelInfo],
    keep_pins: bool,
    unknown: Vec<String>,
}

impl<'a> Cursor<'a, '_> {
    /// The whole layout body: one node, or the single line `empty`.
    fn root(&mut self, need_no: usize) -> Result<Option<Node>, TextError> {
        let Some(first) = self.lines.first().copied() else {
            return Err(TextError::at(need_no, NEED_NODE));
        };
        let root = if first.level != 0 {
            return Err(TextError::at(first.no, NOT_BELOW));
        } else if first.text == "empty" {
            self.pos = 1;
            None
        } else {
            Some(self.node(first)?)
        };
        if let Some(extra) = self.lines.get(self.pos) {
            return Err(TextError::at(extra.no, NOT_BELOW));
        }
        Ok(root)
    }

    /// One node that starts at `line`, which is the next unread line.
    fn node(&mut self, line: Line<'a>) -> Result<Node, TextError> {
        if line.level >= MAX_NESTING {
            return Err(TextError::at(
                line.no,
                format!("nested too deep (at most {MAX_NESTING} levels)."),
            ));
        }
        self.pos += 1;
        if line.text == "tabs" {
            return self.tabs(line);
        }
        if let Some((dir, size)) = split_head(line.text) {
            return self.split(line, dir, size);
        }
        Ok(Node::Panel(self.slot(line)?))
    }

    /// The two nodes below a split line.
    fn split(&mut self, line: Line<'a>, dir: Dir, size: &str) -> Result<Node, TextError> {
        let percent = size
            .strip_suffix('%')
            .filter(|digits| {
                !digits.starts_with('0') && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
            .and_then(|digits| digits.parse::<u8>().ok())
            .filter(|percent| (1..=99).contains(percent));
        let Some(percent) = percent else {
            let found = if size.is_empty() { "nothing" } else { size };
            return Err(TextError::at(
                line.no,
                format!("the split size must be 1% to 99%, found {found}."),
            ));
        };
        let children = self.children(line.level)?;
        match <[(usize, Node); 2]>::try_from(children) {
            Ok([(_, first), (_, second)]) => Ok(Node::Split {
                dir,
                percent,
                first: Box::new(first),
                second: Box::new(second),
            }),
            Err(parts) => Err(TextError::at(
                line.no,
                format!(
                    "\"{}\" needs two parts below it, found {}.",
                    line.text,
                    parts.len()
                ),
            )),
        }
    }

    /// The panels below a `tabs` line.
    fn tabs(&mut self, line: Line<'a>) -> Result<Node, TextError> {
        let children = self.children(line.level)?;
        if children.len() < 2 {
            return Err(TextError::at(
                line.no,
                format!(
                    "\"tabs\" needs two or more panels below it, found {}.",
                    children.len()
                ),
            ));
        }
        let mut slots = Vec::with_capacity(children.len());
        for (no, child) in children {
            match child {
                Node::Panel(slot) => slots.push(slot),
                Node::Tabs(_) | Node::Split { .. } => {
                    return Err(TextError::at(no, ONLY_PANELS));
                }
            }
        }
        Ok(Node::Tabs(slots))
    }

    /// The nodes indented one level under `parent_level`, until a line at or above it.
    fn children(&mut self, parent_level: usize) -> Result<Vec<(usize, Node)>, TextError> {
        let mut children = Vec::new();
        while let Some(line) = self.lines.get(self.pos).copied() {
            if line.level <= parent_level {
                break;
            }
            if line.level != parent_level + 1 {
                return Err(TextError::at(line.no, NOT_BELOW));
            }
            children.push((line.no, self.node(line)?));
        }
        Ok(children)
    }

    /// A panel line, with its pin when the text is saved.
    fn slot(&mut self, line: Line<'a>) -> Result<Slot, TextError> {
        let (written, pin) = split_pin(line.text);
        let wanted = written.to_lowercase();
        let known = self
            .panels
            .iter()
            .find(|info| info.name.to_lowercase() == wanted);
        let panel = match known {
            Some(info) => PanelRef::Known(info.key),
            None => {
                if !self
                    .unknown
                    .iter()
                    .any(|seen| seen.to_lowercase() == wanted)
                {
                    self.unknown.push(written.to_string());
                }
                PanelRef::Unknown(written.to_string())
            }
        };
        let binding = match pin {
            Some(binding) if self.keep_pins => binding,
            _ => Binding::Follow,
        };
        let slot = Slot { panel, binding };
        slot.check(self.panels)
            .map_err(|CheckError::BadPin { panel }| {
                TextError::at(line.no, format!("\"{panel}\" cannot be pinned."))
            })?;
        Ok(slot)
    }
}

/// `split right 60%` gives the direction and the size text, `60%`.
fn split_head(text: &str) -> Option<(Dir, &str)> {
    let rest = text.strip_prefix("split ")?;
    let (word, size) = rest.split_once(' ').unwrap_or((rest, ""));
    let dir = match word {
        "right" => Dir::Right,
        "down" => Dir::Down,
        _ => return None,
    };
    Some((dir, size.trim()))
}

/// Splits a trailing ` [pinned plan 12]` or ` [pinned stockpile 3]` off a panel line. A pin that
/// does not read as one stays part of the name.
fn split_pin(text: &str) -> (&str, Option<Binding>) {
    let pinned = text.rsplit_once(" [pinned ").and_then(|(name, rest)| {
        let (kind, id) = rest.strip_suffix(']')?.split_once(' ')?;
        let id = id.parse::<i64>().ok()?;
        let binding = match kind {
            "plan" => Binding::PinPlan(id),
            "stockpile" => Binding::PinStockpile(id),
            _ => return None,
        };
        Some((name.trim(), binding))
    });
    match pinned {
        Some((name, binding)) => (name, Some(binding)),
        None => (text.trim(), None),
    }
}
