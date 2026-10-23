use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

use eve_means_of_profit::{App, AppConfig};
use mop_sde::HttpFetch;

fn main() -> Result<(), Box<dyn Error>> {
    let dirs = directories::ProjectDirs::from("", "", "eve-means-of-profit")
        .ok_or("no home folder to keep the data in")?;
    let config = AppConfig {
        data_dir: dirs.data_dir().to_path_buf(),
        cache_dir: dirs.cache_dir().to_path_buf(),
        fetch: Box::new(HttpFetch::new()),
        clock: Box::new(system_clock),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EVE Means of Profit")
            .with_min_inner_size([1024.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "EVE Means of Profit",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx, config)?))),
    )?;
    Ok(())
}

fn system_clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
