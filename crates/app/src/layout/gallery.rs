//! The Layout gallery: premade layouts the player can add to their rail with one click.

use egui::Context;

use crate::layout::panels::panels;
use crate::layout::saved::unique_name;
use crate::layout::text;
use crate::layout::user::UserLayouts;
use crate::rail::Page;
use crate::status::Notice;

/// A premade layout: the name the gallery shows, and the text its layout is read from.
struct Premade {
    name: &'static str,
    text: &'static str,
}

const DAILY_INDUSTRY: &str = "EVE Means of Profit layout 1\nname: Daily industry\nsplit right 60%\n  Stockpiles\n  split down 50%\n    Industry jobs\n    Buy list\n";
const BUILD_WITH_STOCK: &str = "EVE Means of Profit layout 1\nname: Build with stock\nsplit right 50%\n  Steps\n  split down 50%\n    Item\n    Assets\n";
const MARKET_WATCH: &str =
    "EVE Means of Profit layout 1\nname: Market watch\nsplit down 50%\n  Orders\n  Net worth\n";

/// The premade layouts, in the order the gallery shows them.
const PREMADE: [Premade; 3] = [
    Premade {
        name: "Daily industry",
        text: DAILY_INDUSTRY,
    },
    Premade {
        name: "Build with stock",
        text: BUILD_WITH_STOCK,
    },
    Premade {
        name: "Market watch",
        text: MARKET_WATCH,
    },
];

/// The egui memory key that says whether the gallery window is open.
fn open_id() -> egui::Id {
    egui::Id::new("layout gallery open")
}

/// Opens the gallery window.
pub fn open(ctx: &Context) {
    ctx.data_mut(|data| data.insert_temp(open_id(), true));
}

/// Draws the gallery window while it is open. Returns the index of the layout the player added,
/// with the notices to show, or `None` when nothing was added this frame.
pub fn show(ctx: &Context, user: &mut UserLayouts) -> Option<(usize, Vec<Notice>)> {
    let mut open = ctx.data(|data| data.get_temp::<bool>(open_id()).unwrap_or(false));
    if !open {
        return None;
    }
    let mut close = false;
    let mut clicked = None;
    egui::Window::new("Layout gallery")
        .id(egui::Id::new("layout gallery window"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Click a layout to add it to your rail.");
            for (index, premade) in PREMADE.iter().enumerate() {
                if ui.button(premade.name).clicked() {
                    clicked = Some(index);
                }
            }
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    if close || !open {
        ctx.data_mut(|data| data.remove::<bool>(open_id()));
    }
    let index = clicked?;
    let premade = PREMADE.get(index)?;
    let mut layout = text::parse_shared(premade.text, panels()).ok()?.layout;
    let taken: Vec<String> = Page::ALL
        .iter()
        .map(|page| page.label().to_owned())
        .chain(user.names())
        .collect();
    let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
    let name = unique_name(premade.name, &taken);
    layout.name = name.clone();
    let (index, saved) = user.push(layout);
    let mut notices = vec![Notice::info(format!("Added layout \"{name}\"."))];
    notices.extend(saved);
    Some((index, notices))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};

    use egui::vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use tempfile::TempDir;

    /// The layouts the tests add, and the return of every `show` call.
    struct Screen {
        layouts: UserLayouts,
        results: Vec<Option<(usize, Vec<Notice>)>>,
    }

    fn temp() -> TempDir {
        TempDir::new().expect("a temp folder")
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("the file is read")
    }

    fn screen(path: PathBuf) -> Harness<'static, Screen> {
        let (layouts, _) = UserLayouts::load(path, panels(), 0);
        Harness::builder()
            .with_size(vec2(560.0, 400.0))
            .build_ui_state(
                |ui, screen: &mut Screen| {
                    let result = show(ui.ctx(), &mut screen.layouts);
                    screen.results.push(result);
                },
                Screen {
                    layouts,
                    results: Vec::new(),
                },
            )
    }

    /// Whether `text` was painted this frame, window titles included.
    fn painted<S>(harness: &Harness<'_, S>, text: &str) -> bool {
        fn collect(shape: &egui::Shape, text: &str) -> bool {
            match shape {
                egui::Shape::Text(painted) => painted.galley.text() == text,
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| collect(shape, text)),
                egui::Shape::Noop
                | egui::Shape::Circle(_)
                | egui::Shape::Ellipse(_)
                | egui::Shape::LineSegment { .. }
                | egui::Shape::Path(_)
                | egui::Shape::Rect(_)
                | egui::Shape::Mesh(_)
                | egui::Shape::QuadraticBezier(_)
                | egui::Shape::CubicBezier(_)
                | egui::Shape::Callback(_) => false,
            }
        }
        harness
            .output()
            .shapes
            .iter()
            .any(|clipped| collect(&clipped.shape, text))
    }

    /// The last index and notices an add returned, after the frames that followed it.
    fn last_added(harness: &Harness<'_, Screen>) -> (usize, Vec<String>) {
        let (index, notices) = harness
            .state()
            .results
            .iter()
            .rev()
            .find_map(Option::as_ref)
            .expect("a layout was added");
        (
            *index,
            notices.iter().map(|notice| notice.text.clone()).collect(),
        )
    }

    fn click(harness: &mut Harness<'_, Screen>, label: &str) {
        harness.get_by_label(label).click();
        harness.run_steps(3);
    }

    /// Leaves the open window as a picture for the owner.
    fn picture(harness: &mut Harness<'_, Screen>) {
        let folder = Path::new("/tmp/opencode");
        if fs::create_dir_all(folder).is_err() {
            return;
        }
        let image = harness.render().expect("the frame draws");
        image
            .save(folder.join("layout-gallery.png"))
            .expect("the picture is saved");
    }

    #[test]
    fn the_window_paints_its_controls_and_close_leaves_the_file_alone() {
        let dir = temp();
        let path = dir.path().join("layouts.txt");
        let mut harness = screen(path.clone());
        harness.run_steps(2);
        assert!(
            harness.state().results.iter().all(Option::is_none),
            "the closed gallery adds nothing"
        );

        open(&harness.ctx);
        harness.run_steps(2);
        for text in [
            "Layout gallery",
            "Click a layout to add it to your rail.",
            "Daily industry",
            "Build with stock",
            "Market watch",
            "Close",
        ] {
            assert!(painted(&harness, text), "{text:?} is painted");
        }
        picture(&mut harness);

        click(&mut harness, "Close");
        assert!(
            harness
                .state()
                .results
                .last()
                .expect("a frame ran")
                .is_none(),
            "the next show returns None"
        );
        assert!(!path.exists(), "closing writes no file");
    }

    #[test]
    fn clicking_daily_industry_adds_it_and_writes_the_file() {
        let dir = temp();
        let path = dir.path().join("layouts.txt");
        let mut harness = screen(path.clone());
        open(&harness.ctx);
        harness.run_steps(2);

        click(&mut harness, "Daily industry");
        let (index, notices) = last_added(&harness);
        assert_eq!(index, 0);
        assert_eq!(notices, ["Added layout \"Daily industry\"."]);
        assert_eq!(read(&path), DAILY_INDUSTRY);
        assert!(painted(&harness, "Layout gallery"), "the window stays open");
    }

    #[test]
    fn a_second_daily_industry_takes_the_first_free_name() {
        let dir = temp();
        let path = dir.path().join("layouts.txt");
        let mut harness = screen(path.clone());
        open(&harness.ctx);
        harness.run_steps(2);

        click(&mut harness, "Daily industry");
        click(&mut harness, "Daily industry");
        let (index, notices) = last_added(&harness);
        assert_eq!(index, 1);
        assert_eq!(notices, ["Added layout \"Daily industry (2)\"."]);
        assert_eq!(
            read(&path),
            format!(
                "{DAILY_INDUSTRY}\n{}",
                DAILY_INDUSTRY.replacen("name: Daily industry\n", "name: Daily industry (2)\n", 1)
            )
        );
    }
}
