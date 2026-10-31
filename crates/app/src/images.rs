//! Item icons and character portraits from CCP's image server, kept under the data folder after
//! the first use. A picture shows once it is on disk; until then a blank square takes its place.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{ColorImage, Context, Image, Response, Sense, TextureHandle, TextureOptions, Ui, vec2};

/// The most bytes an answer may carry: an icon is a few KiB.
const MAX_BYTES: u64 = 1024 * 1024;
/// The longest a picture request may take, including connecting and reading the body.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A picture on the image server: an item type's icon, or a character's portrait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Picture {
    Type(u32),
    Character(u64),
}

impl Picture {
    /// The image server address of the picture.
    pub fn url(self) -> String {
        match self {
            Self::Type(id) => format!("https://images.evetech.net/types/{id}/icon"),
            Self::Character(id) => {
                format!("https://images.evetech.net/characters/{id}/portrait")
            }
        }
    }

    /// The name the picture is kept under.
    pub fn file_name(self) -> String {
        match self {
            Self::Type(id) => format!("types-{id}-icon.png"),
            Self::Character(id) => format!("characters-{id}-portrait.png"),
        }
    }
}

/// The image server address of `picture`.
pub fn url(picture: Picture) -> String {
    picture.url()
}

/// The name `picture` is kept under.
pub fn file_name(picture: Picture) -> String {
    picture.file_name()
}

/// The path `picture` is kept at inside `dir`.
pub fn cache_path(dir: impl AsRef<Path>, picture: Picture) -> PathBuf {
    dir.as_ref().join(picture.file_name())
}

/// Reads a kept picture. A missing file, an answer that is not an image, or a picture with no width
/// or height is no picture.
pub fn cached(path: &Path) -> Option<ColorImage> {
    let bytes = fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let width = usize::try_from(image.width()).ok()?;
    let height = usize::try_from(image.height()).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied(
        [width, height],
        image.as_raw(),
    ))
}

/// Keeps `bytes` at `path`, but only when they are a picture. The write goes to a temporary file
/// first, so a failed write leaves no picture behind.
pub fn keep(path: &Path, bytes: &[u8]) -> bool {
    if image::load_from_memory(bytes).is_err() {
        return false;
    }
    let temporary = temporary(path);
    if fs::write(&temporary, bytes).is_err() {
        remove(&temporary);
        return false;
    }
    if fs::rename(&temporary, path).is_err() {
        remove(&temporary);
        return false;
    }
    true
}

/// The temporary file beside `path` that a kept picture is written to first.
fn temporary(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Removes `path` when it is there, ignoring every failure.
fn remove(path: &Path) {
    fs::remove_file(path).unwrap_or(());
}

/// What the pictures keep between frames: where they are kept, the loaded textures, the requests
/// already running and the pictures that failed this run.
struct State {
    dir: PathBuf,
    textures: HashMap<Picture, TextureHandle>,
    pending: HashSet<Picture>,
    failed: HashSet<Picture>,
    fetch: Arc<dyn Fetch>,
}

fn key() -> egui::Id {
    egui::Id::new("images")
}

fn state(ctx: &Context) -> Option<Arc<Mutex<State>>> {
    ctx.data(|data| data.get_temp::<Arc<Mutex<State>>>(key()))
}

/// Sets where pictures are kept and how they are fetched, and makes the folder when it can.
/// Called once by [`crate::App::new`].
pub fn init(ctx: &Context, dir: impl Into<PathBuf>, fetch: Arc<dyn Fetch>) {
    let dir = dir.into();
    fs::create_dir_all(&dir).unwrap_or(());
    ctx.data_mut(|data| {
        data.insert_temp(
            key(),
            Arc::new(Mutex::new(State {
                dir,
                textures: HashMap::new(),
                pending: HashSet::new(),
                failed: HashSet::new(),
                fetch,
            })),
        );
    });
}

/// Whether the texture of `picture` is loaded in this `ctx`.
pub fn is_loaded(ctx: &Context, picture: Picture) -> bool {
    state(ctx).is_some_and(|state| {
        state
            .lock()
            .is_ok_and(|state| state.textures.contains_key(&picture))
    })
}

/// Draws `picture` at `size` by `size`: the loaded texture, else the kept file, else a blank square
/// while one thread fetches it. A picture that failed this run is not fetched again.
pub fn show(ui: &mut Ui, picture: Picture, size: f32) -> Response {
    let Some(state) = state(ui.ctx()) else {
        return blank(ui, size);
    };
    if let Some(texture) = state
        .lock()
        .ok()
        .and_then(|state| state.textures.get(&picture).cloned())
    {
        return draw_texture(ui, &texture, size);
    }
    let Some(path) = state
        .lock()
        .ok()
        .map(|state| state.dir.join(picture.file_name()))
    else {
        return blank(ui, size);
    };
    if let Some(image) = cached(&path) {
        let texture = ui
            .ctx()
            .load_texture(picture.file_name(), image, TextureOptions::LINEAR);
        if let Ok(mut state) = state.lock() {
            state.textures.insert(picture, texture.clone());
        }
        return draw_texture(ui, &texture, size);
    }
    let fetch = {
        let Ok(mut state) = state.lock() else {
            return blank(ui, size);
        };
        if state.pending.contains(&picture) || state.failed.contains(&picture) {
            return blank(ui, size);
        }
        state.pending.insert(picture);
        Arc::clone(&state.fetch)
    };
    let ctx = ui.ctx().clone();
    let thread_state = Arc::clone(&state);
    std::thread::spawn(move || {
        let result = fetch.get(&url(picture));
        if let Ok(mut state) = thread_state.lock() {
            state.pending.remove(&picture);
            match &result {
                Ok(bytes) => {
                    if !keep(&path, bytes) {
                        state.failed.insert(picture);
                    }
                }
                Err(_) => {
                    state.failed.insert(picture);
                }
            }
        }
        ctx.request_repaint();
    });
    blank(ui, size)
}

/// Draws a loaded texture at exactly `size` by `size`.
fn draw_texture(ui: &mut Ui, texture: &TextureHandle, size: f32) -> Response {
    ui.add(Image::new(texture).fit_to_exact_size(vec2(size, size)))
}

/// Allocates the blank square that holds a picture's place until it loads.
fn blank(ui: &mut Ui, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, ui.visuals().faint_bg_color);
    response
}

/// The network side of the pictures, behind a trait so the cache can be tested without a server.
pub trait Fetch: Send + Sync {
    /// The body of a GET, or why it could not be read.
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// The real client, over HTTPS through `ureq`.
pub struct Ureq {
    agent: ureq::Agent,
}

impl Ureq {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(mop_core::user_agent::USER_AGENT)
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for Ureq {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for Ureq {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("the image server answered {status}"));
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_BYTES)
            .read_to_vec()
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use egui::{Context, RawInput, vec2};
    use tempfile::tempdir;

    use super::*;

    /// A 16 by 16 picture that is red everywhere.
    fn red_png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("the test picture encodes");
        bytes
    }

    struct Offline;

    impl Fetch for Offline {
        fn get(&self, _url: &str) -> Result<Vec<u8>, String> {
            Err("offline".to_owned())
        }
    }

    #[test]
    fn a_type_picture_has_the_icon_address_and_name() {
        assert_eq!(
            url(Picture::Type(34)),
            "https://images.evetech.net/types/34/icon"
        );
        assert_eq!(
            cache_path("/d/images", Picture::Type(34)),
            PathBuf::from("/d/images/types-34-icon.png")
        );
    }

    #[test]
    fn a_character_picture_has_the_portrait_name() {
        assert_eq!(
            file_name(Picture::Character(90_000_001)),
            "characters-90000001-portrait.png"
        );
        assert_eq!(
            url(Picture::Character(90_000_001)),
            "https://images.evetech.net/characters/90000001/portrait"
        );
    }

    #[test]
    fn a_kept_picture_reads_back_as_rgba() {
        let dir = tempdir().expect("a temporary folder");
        let path = cache_path(dir.path(), Picture::Type(34));
        assert!(keep(&path, &red_png()));
        assert!(path.exists());

        let image = cached(&path).expect("the kept picture");
        assert_eq!((image.width(), image.height()), (16, 16));
        assert_eq!(image.pixels[0].to_array(), [255, 0, 0, 255]);
    }

    #[test]
    fn a_missing_file_and_a_text_file_are_no_picture() {
        let dir = tempdir().expect("a temporary folder");
        let missing = cache_path(dir.path(), Picture::Type(34));
        assert!(cached(&missing).is_none());

        let text = cache_path(dir.path(), Picture::Type(99));
        fs::write(&text, b"not a picture").expect("the text file");
        assert!(cached(&text).is_none());
        assert!(!keep(&text, b"not a picture"));
    }

    #[test]
    fn a_failed_keep_leaves_no_file() {
        let dir = tempdir().expect("a temporary folder");
        let path = cache_path(dir.path(), Picture::Type(34));
        assert!(!keep(&path, b"not a picture"));
        assert!(!path.exists());
        assert!(!temporary(&path).exists());
    }

    /// Draws one frame with a central panel, and drops the textures the test does not upload.
    fn frame(ctx: &Context, mut draw: impl FnMut(&mut Ui)) {
        let mut output = ctx.run_ui(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| draw(ui));
        });
        output.textures_delta.clear();
    }

    #[test]
    fn show_allocates_a_square_and_later_loads_the_kept_file() {
        let dir = tempdir().expect("a temporary folder");
        let ctx = Context::default();
        init(&ctx, dir.path(), Arc::new(Offline));
        let mut rect = egui::Rect::NOTHING;
        frame(&ctx, |ui| rect = show(ui, Picture::Type(34), 16.0).rect);
        assert!(!is_loaded(&ctx, Picture::Type(34)));
        assert_eq!(rect.size(), vec2(16.0, 16.0));

        let path = cache_path(dir.path(), Picture::Type(34));
        assert!(keep(&path, &red_png()));
        frame(&ctx, |ui| rect = show(ui, Picture::Type(34), 16.0).rect);
        assert!(is_loaded(&ctx, Picture::Type(34)));
        assert_eq!(rect.size(), vec2(16.0, 16.0));
    }

    struct Counting(AtomicUsize);

    impl Fetch for Counting {
        fn get(&self, _url: &str) -> Result<Vec<u8>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("offline".to_owned())
        }
    }

    #[test]
    fn a_failed_picture_is_not_fetched_again() {
        let dir = tempdir().expect("a temporary folder");
        let ctx = Context::default();
        let fetch = Arc::new(Counting(AtomicUsize::new(0)));
        let images_fetch = Arc::clone(&fetch);
        let images_fetch: Arc<dyn Fetch> = images_fetch;
        init(&ctx, dir.path(), images_fetch);
        for _ in 0..3 {
            frame(&ctx, |ui| {
                show(ui, Picture::Type(34), 16.0);
            });
            // Let the fetch thread record its failure before the next frame asks again.
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fetch.0.load(Ordering::SeqCst), 1);
    }
}
