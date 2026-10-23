//! A manual live login: prints the browser address, waits for EVE's answer, and reports the character.
//! Run with: `EVE_CLIENT_ID=<id> cargo run -p mop-esi --example login -- <folder for the login files>`.
//! Never prints tokens.

use std::error::Error;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mop_esi::auth::Auth;
use mop_esi::callback::wait_for_code;
use mop_esi::http::UreqHttp;
use mop_esi::secrets::open_secrets;

const WAIT: Duration = Duration::from_secs(5 * 60);

fn main() -> Result<(), Box<dyn Error>> {
    let dir = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("pass a folder for the login files")?,
    );
    let client_id = std::env::var("EVE_CLIENT_ID").unwrap_or_default();
    let secrets = open_secrets(&dir);
    if let Some(reason) = &secrets.fallback {
        writeln!(
            stdout(),
            "{}",
            mop_esi::secrets::fallback_warning(&dir, reason)
        )?;
    }
    let mut auth = Auth::open(&dir, Box::new(UreqHttp::new()), secrets, &client_id)?;
    let start = auth.begin_login()?;
    writeln!(
        stdout(),
        "Open this address in the browser and log in:\n{}",
        start.url
    )?;

    let cancel = AtomicBool::new(false);
    let code = wait_for_code(start.pkce.state(), Instant::now() + WAIT, &cancel)?;
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let character = auth.finish_login(&start, &code, now)?;
    writeln!(
        stdout(),
        "Logged in as {} ({} scopes).",
        character.name,
        character.scopes.len()
    )?;
    Ok(())
}
