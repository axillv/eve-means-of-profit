//! Layouts read from a chat paste or a `.txt` file, written back to a file, and the native file
//! dialogs that choose those files.

use std::fs;
use std::path::{Path, PathBuf};

use crate::layout::model::{Layout, PanelInfo};
use crate::layout::saved::unique_name;
use crate::layout::text;
use crate::status::Notice;

/// Most bytes a layout file may hold.
pub const MAX_FILE_BYTES: u64 = 100_000;

/// A layout read in, and the notices that describe what came in or what was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub layout: Layout,
    pub notices: Vec<Notice>,
}

/// Reads a layout pasted as text, giving it a name no taken name has. A layout this version does
/// not fully know still comes in, with a notice for the panels that will show as empty boxes.
pub fn import_text(text: &str, panels: &[PanelInfo], taken: &[&str]) -> Result<Imported, Notice> {
    let parsed = text::parse_shared(text, panels)
        .map_err(|err| Notice::error(format!("Could not read the layout: {err}")))?;
    let mut layout = parsed.layout;
    layout.name = unique_name(&layout.name, taken);
    let mut notices = vec![Notice::info(format!("Added layout \"{}\".", layout.name))];
    if !parsed.unknown.is_empty() {
        notices.push(Notice::info(format!(
            "Not in this version: {}. They show as empty boxes.",
            parsed.unknown.join(", ")
        )));
    }
    Ok(Imported { layout, notices })
}

/// Reads a layout from a file, refusing one over [`MAX_FILE_BYTES`] before reading it.
pub fn import_file(path: &Path, panels: &[PanelInfo], taken: &[&str]) -> Result<Imported, Notice> {
    let metadata = fs::metadata(path)
        .map_err(|err| Notice::error(format!("Could not read the layout: {err}")))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Notice::error(
            "Could not read the layout: the file is larger than 100 KB.",
        ));
    }
    let text = fs::read_to_string(path)
        .map_err(|err| Notice::error(format!("Could not read the layout: {err}")))?;
    import_text(&text, panels, taken)
}

/// The answer of an open dialog: the file the player chose, if any.
pub type OpenDialog = Box<dyn FnMut() -> Option<PathBuf>>;
/// The answer of a save dialog, given the default file name: the file the player chose, if any.
pub type SaveDialog = Box<dyn FnMut(&str) -> Option<PathBuf>>;

/// The file dialogs the app uses. Tests build this with closures that return a fixed path.
pub struct Files {
    pub open: OpenDialog,
    pub save: SaveDialog,
}

impl Files {
    /// The system's own open and save dialogs, for `.txt` files.
    pub fn native() -> Self {
        Self {
            open: Box::new(|| {
                rfd::FileDialog::new()
                    .add_filter("Layout", &["txt"])
                    .pick_file()
            }),
            save: Box::new(|name| {
                rfd::FileDialog::new()
                    .add_filter("Layout", &["txt"])
                    .set_file_name(name)
                    .save_file()
            }),
        }
    }
}

/// Writes the layout text to `path`, with a notice that says where it went or what went wrong.
pub fn write_text(path: &Path, text: &str) -> Notice {
    match fs::write(path, text) {
        Ok(()) => Notice::info(format!("Saved the layout to {}.", path.display())),
        Err(err) => Notice::error(format!("Could not write the file: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::layout::model::{Binding, Dir, Node, PanelRef, Slot};
    use crate::layout::panels::panels;

    fn slot(key: &'static str) -> Slot {
        Slot {
            panel: PanelRef::Known(key),
            binding: Binding::Follow,
        }
    }

    fn unknown(name: &str) -> Slot {
        Slot {
            panel: PanelRef::Unknown(name.to_string()),
            binding: Binding::Follow,
        }
    }

    #[test]
    fn import_text_names_a_new_layout_and_reads_its_structure() {
        let text = "```\nEVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps\n```\n";
        let imported =
            import_text(text, panels(), &["Build", "Daily industry"]).expect("the layout reads");
        assert_eq!(imported.layout.name, "Daily industry (2)");
        assert_eq!(
            imported.layout.root,
            Some(Node::Split {
                dir: Dir::Right,
                percent: 50,
                first: Box::new(Node::Panel(slot("build.buy_list"))),
                second: Box::new(Node::Panel(slot("build.steps"))),
            })
        );
        assert_eq!(
            imported.notices,
            vec![Notice::info("Added layout \"Daily industry (2)\".")]
        );
    }

    #[test]
    fn import_text_keeps_unknown_panels_and_says_so() {
        let text = "EVE Means of Profit layout 1\nname: Odd\nMarket depth\n";
        let imported = import_text(text, panels(), &[]).expect("the layout reads");
        assert_eq!(
            imported.layout.root,
            Some(Node::Panel(unknown("Market depth")))
        );

        let text = "EVE Means of Profit layout 1\nname: Odd\ntabs\n  Market depth\n  Ladder\n";
        let imported = import_text(text, panels(), &[]).expect("the layout reads");
        assert_eq!(
            imported.layout.root,
            Some(Node::Tabs(
                vec![unknown("Market depth"), unknown("Ladder"),]
            ))
        );
        assert_eq!(
            imported.notices,
            vec![
                Notice::info("Added layout \"Odd\"."),
                Notice::info(
                    "Not in this version: Market depth, Ladder. They show as empty boxes."
                ),
            ]
        );
    }

    #[test]
    fn import_text_reports_read_errors() {
        let err = import_text("hello", panels(), &[]).expect_err("no layout");
        assert_eq!(
            err,
            Notice::error(
                "Could not read the layout: No layout found: the text has no line \"EVE Means of Profit layout 1\"."
            )
        );

        let err = import_text("EVE Means of Profit layout 1\nname: X\n", panels(), &[])
            .expect_err("no node");
        assert_eq!(
            err,
            Notice::error(
                "Could not read the layout: Line 3: the layout needs a panel, a split or \"empty\" below the name."
            )
        );
    }

    #[test]
    fn import_file_reads_up_to_the_size_limit() {
        let dir = tempdir().expect("a temp folder");
        let path = dir.path().join("big.txt");
        let mut text = String::from("EVE Means of Profit layout 1\nname: Big\nSteps\n");
        let limit = usize::try_from(MAX_FILE_BYTES).expect("the limit fits");
        text.push_str(&"\n".repeat(limit - text.len()));
        fs::write(&path, &text).expect("write the padded layout");
        assert_eq!(
            fs::metadata(&path).expect("the file exists").len(),
            MAX_FILE_BYTES
        );
        let imported = import_file(&path, panels(), &[]).expect("the layout reads");
        assert_eq!(imported.layout.name, "Big");
    }

    #[test]
    fn import_file_refuses_a_file_over_the_size_limit() {
        let dir = tempdir().expect("a temp folder");
        let path = dir.path().join("too-big.txt");
        fs::write(&path, vec![b'x'; 100_001]).expect("write the file");
        let err = import_file(&path, panels(), &[]).expect_err("too big");
        assert_eq!(
            err,
            Notice::error("Could not read the layout: the file is larger than 100 KB.")
        );
    }

    #[test]
    fn import_file_reports_a_missing_file() {
        let dir = tempdir().expect("a temp folder");
        let err = import_file(&dir.path().join("missing.txt"), panels(), &[]).expect_err("missing");
        assert!(err.text.starts_with("Could not read the layout: "));
    }

    #[test]
    fn write_text_saves_and_reports_where() {
        let dir = tempdir().expect("a temp folder");
        let path = dir.path().join("d.txt");
        let notice = write_text(&path, "hello");
        assert_eq!(
            notice,
            Notice::info(format!("Saved the layout to {}.", path.display()))
        );
        assert_eq!(fs::read_to_string(&path).expect("the file reads"), "hello");
    }

    #[test]
    fn write_text_reports_a_missing_folder() {
        let dir = tempdir().expect("a temp folder");
        let path = dir.path().join("missing").join("d.txt");
        let notice = write_text(&path, "hello");
        assert!(notice.text.starts_with("Could not write the file: "));
    }

    #[test]
    fn files_from_closures_return_the_given_paths() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let opened = PathBuf::from("picked.txt");
        let saved = PathBuf::from("saved.txt");
        let default_name = Rc::new(RefCell::new(None::<String>));
        let seen_name = Rc::clone(&default_name);
        let saved_path = saved.clone();
        let mut files = Files {
            open: Box::new(move || Some(opened.clone())),
            save: Box::new(move |name| {
                *seen_name.borrow_mut() = Some(name.to_string());
                Some(saved_path.clone())
            }),
        };
        assert_eq!((files.open)(), Some(PathBuf::from("picked.txt")));
        assert_eq!((files.save)("Daily industry.txt"), Some(saved));
        assert_eq!(default_name.borrow().as_deref(), Some("Daily industry.txt"));
    }

    #[test]
    fn native_files_build_without_opening_a_dialog() {
        drop(Files::native());
    }
}
