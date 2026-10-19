//! The user's saved layouts: one file in the data folder, read back at start-up. The file is the
//! user's own work, so a file that cannot be read is moved aside, never overwritten.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::layout::model::{Layout, PanelInfo};
use crate::layout::text::{self, Mode, TextError};

/// The layouts read from the file, and a notice to show when the file had to be set aside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub layouts: Vec<Layout>,
    pub notice: Option<String>,
}

/// The layouts in the file, in order. A missing or empty file has none.
pub fn load(path: &Path, panels: &[PanelInfo]) -> Result<Loaded, TextError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(TextError {
                line: None,
                message: err.to_string(),
            });
        }
    };
    let layouts = if text.trim().is_empty() {
        Vec::new()
    } else {
        text::parse_saved_all(&text, panels)?
    };
    Ok(Loaded {
        layouts,
        notice: None,
    })
}

/// Loads the file. When it cannot be read, renames it to `<file name>.unreadable-<now>`, so the
/// next save does not overwrite it, and returns no layouts with a notice that says so.
pub fn load_or_set_aside(path: &Path, panels: &[PanelInfo], now: i64) -> io::Result<Loaded> {
    match load(path, panels) {
        Ok(loaded) => Ok(loaded),
        Err(err) => {
            let base = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let kept = format!("{base}.unreadable-{now}");
            let target = path.with_file_name(&kept);
            if target.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{kept} already exists"),
                ));
            }
            fs::rename(path, target)?;
            Ok(Loaded {
                layouts: Vec::new(),
                notice: Some(format!(
                    "Your saved layouts could not be read ({err}). The file was kept as {kept}."
                )),
            })
        }
    }
}

/// Writes the layouts to the file, in order, one blank line between. The new text goes to a
/// temporary file first, which then replaces the old one. A layout that would not read back
/// (a pin its panel cannot take, a name or size over a limit) is refused, so the file never
/// becomes one that `load` rejects.
pub fn save(path: &Path, layouts: &[Layout], panels: &[PanelInfo]) -> io::Result<()> {
    let mut texts = Vec::with_capacity(layouts.len());
    for layout in layouts {
        let text = text::to_text(layout, panels, Mode::Saved);
        text::parse_saved(&text, panels)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        texts.push(text);
    }
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    fs::write(&tmp, texts.join("\n"))?;
    fs::rename(&tmp, path)
}

/// A name that no taken name has, ignoring case: `wanted`, else `wanted (2)`, `(3)`, and so on.
pub fn unique_name(wanted: &str, taken: &[&str]) -> String {
    let is_free = |candidate: &str| {
        let lower = candidate.to_lowercase();
        !taken.iter().any(|name| name.to_lowercase() == lower)
    };
    if is_free(wanted) {
        return wanted.to_string();
    }
    let mut number = 2;
    loop {
        let candidate = format!("{wanted} ({number})");
        if is_free(&candidate) {
            return candidate;
        }
        number += 1;
    }
}
