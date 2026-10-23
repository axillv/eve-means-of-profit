//! Imports a real download and times a search on the result.
//!
//! `cargo run --release -p mop-sde --example import -- <zip> <db>`

use std::error::Error;
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use mop_sde::{Sde, import};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [source, db] = args.as_slice() else {
        return Err("usage: import <zip> <db>".into());
    };
    let mut out = io::stdout().lock();

    let started = Instant::now();
    let report = import(Path::new(source), Path::new(db))?;
    writeln!(out, "{report:#?}")?;
    writeln!(out, "import: {:?}", started.elapsed())?;

    let sde = Sde::open(Path::new(db))?;
    let started = Instant::now();
    let found = sde.search("hobgoblin", 10)?;
    writeln!(
        out,
        "search \"hobgoblin\": {found:?} in {:?}",
        started.elapsed()
    )?;
    Ok(())
}
