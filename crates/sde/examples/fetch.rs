//! Downloads the latest build and imports it, with a progress line every 10 MB.
//!
//! `cargo run --release -p mop-sde --example fetch -- <work_dir> <db>`

use std::error::Error;
use std::io::{self, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Instant;

use mop_sde::{HttpFetch, download_and_import, latest_build};

const REPORT_EVERY: u64 = 10 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [work_dir, db] = args.as_slice() else {
        return Err("usage: fetch <work_dir> <db>".into());
    };
    let mut out = io::stdout().lock();
    let fetch = HttpFetch::new();

    let started = Instant::now();
    let latest = latest_build(&fetch)?;
    writeln!(
        out,
        "latest: build {} released {}, checked in {:?}",
        latest.number,
        latest.release_date,
        started.elapsed()
    )?;

    let started = Instant::now();
    let mut blocks_reported = 0_u64;
    let mut write_error = None;
    let result = {
        let mut progress = |bytes: u64, total: Option<u64>| {
            let block = bytes / REPORT_EVERY;
            if block > blocks_reported {
                blocks_reported = block;
                let line = match total {
                    Some(total) => format!("  {bytes} of {total} bytes"),
                    None => format!("  {bytes} bytes"),
                };
                if let Err(error) = writeln!(out, "{line}") {
                    write_error = Some(error);
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        };
        download_and_import(
            &fetch,
            latest.number,
            Path::new(work_dir),
            Path::new(db),
            &mut progress,
        )
    };
    // A failed progress line stops the download, so it is reported before the cancellation.
    if let Some(error) = write_error {
        return Err(error.into());
    }
    let report = result?;
    writeln!(out, "{report:#?}")?;
    writeln!(out, "download and import: {:?}", started.elapsed())?;
    Ok(())
}
