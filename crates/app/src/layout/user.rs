//! The player's layouts held in memory: loaded at start-up, shown, edited, renamed, copied,
//! deleted, and written back to the file after every change.

use std::path::PathBuf;

use egui::Ui;
use egui_dock::DockState;

use crate::dock::{UserNames, show_user};
use crate::layout::dock_convert::{from_dock, to_dock};
use crate::layout::model::{Layout, PanelInfo, Slot};
use crate::layout::saved::{self, load_or_set_aside, unique_name};
use crate::layout::text::{self, Mode};
use crate::rail::Page;
use crate::status::Notice;
use crate::theme::Style;

/// The player's layouts and the file they live in. The dock of the layout on show is kept between
/// frames; a change to the layouts drops it.
pub struct UserLayouts {
    pub layouts: Vec<Layout>,
    path: PathBuf,
    panels: &'static [PanelInfo],
    shown: Option<(usize, DockState<Slot>)>,
}

impl UserLayouts {
    /// Reads the file. A file that cannot be read is set aside by [`load_or_set_aside`], with its
    /// notice as an info notice; an IO error gives no layouts and an error notice.
    pub fn load(path: PathBuf, panels: &'static [PanelInfo], now: i64) -> (Self, Vec<Notice>) {
        let mut layouts = Self {
            layouts: Vec::new(),
            path,
            panels,
            shown: None,
        };
        match load_or_set_aside(&layouts.path, panels, now) {
            Ok(loaded) => {
                layouts.layouts = loaded.layouts;
                match loaded.notice {
                    Some(text) => (layouts, vec![Notice::info(text)]),
                    None => (layouts, Vec::new()),
                }
            }
            Err(err) => (
                layouts,
                vec![Notice::error(format!("Could not read layouts: {err}"))],
            ),
        }
    }

    /// The names of the layouts, in order.
    pub fn names(&self) -> Vec<String> {
        self.layouts
            .iter()
            .map(|layout| layout.name.clone())
            .collect()
    }

    /// Writes every layout to the file. The error notice says the file was not written.
    pub fn save(&mut self) -> Option<Notice> {
        match saved::save(&self.path, &self.layouts, self.panels) {
            Ok(()) => None,
            Err(err) => Some(Notice::error(format!("Could not save layouts: {err}"))),
        }
    }

    /// Shows the layout at `index`. The dock is built when that layout is not the cached one. When
    /// no pointer button is down and the drawn arrangement differs from the stored layout, the
    /// change is kept and saved; a failed save is not retried until the next change.
    pub fn show(
        &mut self,
        index: usize,
        ui: &mut Ui,
        theme: &Style,
        names: &UserNames,
        mut body: impl FnMut(&mut Ui, &Slot),
    ) -> Option<Notice> {
        if self.shown.as_ref().map(|(shown, _)| *shown) != Some(index) {
            let layout = self.layouts.get(index)?;
            self.shown = Some((index, to_dock(layout)));
        }
        let dock = self.shown.as_mut().map(|(_, dock)| dock)?;
        show_user(ui, theme, dock, self.panels, names, &mut body);
        if ui.input(|input| input.pointer.any_down()) {
            return None;
        }
        let current = self.layouts.get(index)?;
        let name = current.name.clone();
        let edited = from_dock(dock, &name);
        if edited == *current {
            return None;
        }
        if let Some(current) = self.layouts.get_mut(index) {
            *current = edited;
        }
        self.shown = None;
        self.save()
    }

    /// Appends `layout` under the name it already carries, and saves.
    pub fn push(&mut self, layout: Layout) -> (usize, Option<Notice>) {
        self.layouts.push(layout);
        self.shown = None;
        (self.layouts.len() - 1, self.save())
    }

    /// Appends `arrangement` as a copy of `page_label`, under the first free name among the page
    /// labels and the layout names, and saves.
    pub fn save_copy(&mut self, page_label: &str, arrangement: Layout) -> (usize, Option<Notice>) {
        let wanted = format!("{page_label} (copy)");
        let taken: Vec<&str> = Page::ALL
            .iter()
            .map(|page| page.label())
            .chain(self.layouts.iter().map(|layout| layout.name.as_str()))
            .collect();
        let mut arrangement = arrangement;
        arrangement.name = unique_name(&wanted, &taken);
        self.push(arrangement)
    }

    /// Gives the layout at `index` a new name, and saves.
    pub fn rename(&mut self, index: usize, name: String) -> Option<Notice> {
        if let Some(layout) = self.layouts.get_mut(index) {
            layout.name = name;
        }
        self.shown = None;
        self.save()
    }

    /// Removes the layout at `index`, and saves.
    pub fn delete(&mut self, index: usize) -> Option<Notice> {
        if index < self.layouts.len() {
            self.layouts.remove(index);
        }
        self.shown = None;
        self.save()
    }

    /// The chat text for the layout at `index`: the share form inside two fence lines.
    pub fn copy_text(&self, index: usize) -> String {
        self.layouts
            .get(index)
            .map_or_else(String::new, |layout| text::copy_text(layout, self.panels))
    }

    /// The file name `<name>.txt` and the share text without the fence lines, for the layout at
    /// `index`.
    pub fn file_text(&self, index: usize) -> (String, String) {
        match self.layouts.get(index) {
            Some(layout) => (
                format!("{}.txt", layout.name.trim()),
                text::to_text(layout, self.panels, Mode::Share),
            ),
            None => (String::new(), String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::Path;

    use egui::{Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
    use egui_kittest::Harness;
    use tempfile::TempDir;

    use crate::layout::model::Binding;
    use crate::layout::panels::panels;
    use crate::status::Level;

    const DAILY: &str = "EVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps [pinned plan 1]\n";
    const DAILY_WITH_BUY_CLOSED: &str =
        "EVE Means of Profit layout 1\nname: Daily industry\nSteps [pinned plan 1]\n";

    fn temp() -> TempDir {
        TempDir::new().expect("a temp folder")
    }

    fn write(dir: &TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("layouts.txt");
        fs::write(&path, text).expect("the file is written");
        path
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("the file is read")
    }

    /// The state the show tests keep: the layouts, and the notice every frame returned.
    struct Screen {
        layouts: UserLayouts,
        notices: Vec<Option<Notice>>,
    }

    fn screen(path: PathBuf) -> Harness<'static, Screen> {
        let (layouts, _) = UserLayouts::load(path, panels(), 0);
        let style = Style::eve_dense();
        Harness::builder()
            .with_size(vec2(800.0, 600.0))
            .build_ui_state(
                move |ui, screen: &mut Screen| {
                    style.apply(ui.ctx());
                    let pin_name = |binding: &Binding| match binding {
                        Binding::PinPlan(_) | Binding::PinStockpile(_) => Some("Plan".to_owned()),
                        Binding::Follow => None,
                    };
                    let names = UserNames {
                        selected_plan: None,
                        selected_stockpile: None,
                        pin_name: &pin_name,
                    };
                    let notice = screen.layouts.show(0, ui, &style, &names, |_, _| {});
                    screen.notices.push(notice);
                },
                Screen {
                    layouts,
                    notices: Vec::new(),
                },
            )
    }

    /// The rect of the topmost painted text equal to `text`. Tab titles are painted, not widgets.
    fn text_rect(harness: &Harness<'_, Screen>, text: &str) -> Rect {
        fn collect(shape: &egui::Shape, text: &str, found: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Text(painted) if painted.galley.text() == text => {
                    found.push(Rect::from_min_size(painted.pos, painted.galley.size()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, text, found);
                    }
                }
                egui::Shape::Noop
                | egui::Shape::Circle(_)
                | egui::Shape::Ellipse(_)
                | egui::Shape::LineSegment { .. }
                | egui::Shape::Path(_)
                | egui::Shape::Rect(_)
                | egui::Shape::Text(_)
                | egui::Shape::Mesh(_)
                | egui::Shape::QuadraticBezier(_)
                | egui::Shape::CubicBezier(_)
                | egui::Shape::Callback(_) => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &harness.output().shapes {
            collect(&clipped.shape, text, &mut found);
        }
        found
            .into_iter()
            .min_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap_or_else(|| panic!("\"{text}\" is not painted"))
    }

    fn click_at(harness: &mut Harness<'_, Screen>, pos: Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        harness.run_steps(3);
    }

    /// Clicks the close button at the right of the tab titled `name`. The close button is 24 points
    /// wide with 8 points of padding before the title, so its centre sits 20 points right of the
    /// painted title.
    fn close_tab(harness: &mut Harness<'_, Screen>, name: &str) {
        let title = text_rect(harness, name);
        click_at(harness, pos2(title.right() + 20.0, title.center().y));
    }

    #[test]
    fn load_reads_every_layout_in_order() {
        let dir = temp();
        let path = write(
            &dir,
            &format!("{DAILY}\nEVE Means of Profit layout 1\nname: Ideas\nempty\n"),
        );
        let (layouts, notices) = UserLayouts::load(path, panels(), 0);
        assert_eq!(layouts.names(), ["Daily industry", "Ideas"]);
        assert!(notices.is_empty(), "a readable file needs no notice");
    }

    #[test]
    fn an_unreadable_file_is_set_aside_with_one_info_notice() {
        let dir = temp();
        let path = write(&dir, "nonsense");
        let (layouts, notices) = UserLayouts::load(path.clone(), panels(), 1_000_000);
        assert!(layouts.layouts.is_empty(), "no layout is read");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].level, Level::Notice);
        assert_eq!(
            notices[0].text,
            "Your saved layouts could not be read (No layout found: the text has no line \"EVE Means of Profit layout 1\".). The file was kept as layouts.txt.unreadable-1000000."
        );
        assert!(!path.exists(), "the bad file is moved aside");
        assert!(
            dir.path().join("layouts.txt.unreadable-1000000").exists(),
            "the bad file is kept under the new name"
        );
    }

    #[test]
    fn closing_a_tab_writes_the_file_and_the_next_frame_leaves_it_alone() {
        let dir = temp();
        let path = write(&dir, DAILY);
        let mut harness = screen(path.clone());
        harness.run_steps(2);
        assert_eq!(read(&path), DAILY, "showing a layout does not write");

        close_tab(&mut harness, "Buy list");
        assert_eq!(read(&path), DAILY_WITH_BUY_CLOSED);

        harness.state_mut().notices.clear();
        harness.run_steps(2);
        assert_eq!(
            harness.state().notices,
            [None, None],
            "an unchanged frame saves nothing"
        );
        assert_eq!(read(&path), DAILY_WITH_BUY_CLOSED);
    }

    #[test]
    fn a_failed_save_is_reported_once_and_retried_only_after_the_next_change() {
        let dir = temp();
        let path = write(&dir, DAILY);
        let mut harness = screen(path.clone());
        harness.run_steps(2);

        fs::remove_file(&path).expect("the file is removed");
        fs::create_dir(&path).expect("a folder takes its place");
        harness.state_mut().notices.clear();
        close_tab(&mut harness, "Buy list");
        assert!(
            harness
                .state()
                .notices
                .iter()
                .flatten()
                .any(|notice| notice.level == Level::Error
                    && notice.text.starts_with("Could not save layouts: ")),
            "a failed write is an error notice"
        );

        harness.state_mut().notices.clear();
        harness.run_steps(2);
        assert!(
            harness.state().notices.iter().all(Option::is_none),
            "a failed save is not retried until the next change"
        );

        fs::remove_dir(&path).expect("the folder is removed");
        close_tab(&mut harness, "Steps");
        assert_eq!(
            read(&path),
            "EVE Means of Profit layout 1\nname: Daily industry\nempty\n",
            "the next change saves again"
        );
    }

    #[test]
    fn copies_take_the_first_free_name_ignoring_case() {
        let dir = temp();
        let path = write(&dir, DAILY);
        let (mut layouts, _) = UserLayouts::load(path.clone(), panels(), 0);
        let arrangement = Layout {
            name: "ignored".to_owned(),
            root: None,
        };

        let (index, notice) = layouts.save_copy("Build", arrangement.clone());
        assert_eq!(index, 1);
        assert_eq!(notice, None);
        assert_eq!(layouts.names(), ["Daily industry", "Build (copy)"]);
        assert!(read(&path).contains("name: Build (copy)\n"));

        let (index, _) = layouts.save_copy("Build", arrangement.clone());
        assert_eq!(index, 2);
        assert_eq!(
            layouts.names(),
            ["Daily industry", "Build (copy)", "Build (copy) (2)"]
        );

        let dir = temp();
        let path = write(&dir, DAILY);
        let (mut layouts, _) = UserLayouts::load(path, panels(), 0);
        layouts.push(Layout {
            name: "build (copy)".to_owned(),
            root: None,
        });
        let (index, _) = layouts.save_copy("Build", arrangement);
        assert_eq!(index, 2);
        assert_eq!(
            layouts.names(),
            ["Daily industry", "build (copy)", "Build (copy) (2)"],
            "an existing name in another case still takes the plain copy name"
        );
    }

    #[test]
    fn rename_and_delete_change_the_names_and_the_file() {
        let dir = temp();
        let path = write(&dir, DAILY);
        let (mut layouts, _) = UserLayouts::load(path.clone(), panels(), 0);

        assert_eq!(layouts.rename(0, "Mine".to_owned()), None);
        assert_eq!(layouts.names(), ["Mine"]);
        assert!(read(&path).contains("name: Mine\n"));

        assert_eq!(layouts.delete(0), None);
        assert!(layouts.names().is_empty());
        assert_eq!(read(&path), "");
    }

    #[test]
    fn copy_text_has_fences_and_file_text_does_not() {
        let dir = temp();
        let path = write(&dir, DAILY);
        let (layouts, _) = UserLayouts::load(path, panels(), 0);

        assert_eq!(
            layouts.copy_text(0),
            "```\nEVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps\n```\n"
        );
        assert_eq!(
            layouts.file_text(0),
            (
                "Daily industry.txt".to_owned(),
                "EVE Means of Profit layout 1\nname: Daily industry\nsplit right 50%\n  Buy list\n  Steps\n"
                    .to_owned(),
            )
        );
    }
}
