//! Fetches the game's static data from the developer's servers, and checks for a newer build.
//!
//! [`LATEST_URL`] is 80 bytes and names the current build, so the daily check does not download
//! the zip. [`download_and_import`] fetches one build, checks it and imports it.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::SdeError;
use crate::import::{ImportReport, import};
use crate::query::SdeBuild;

/// The current build, as a single line of JSON.
pub const LATEST_URL: &str =
    "https://developers.eveonline.com/static-data/tranquility/latest.jsonl";

const SDE_FILE: &str = "_sde.jsonl";
const DAY_SECONDS: i64 = 86_400;
const CHUNK: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const BAD_BODY_CHARS: usize = 200;
const USER_AGENT: &str = concat!("eve-means-of-profit/", env!("CARGO_PKG_VERSION"));

/// The network side of the download, behind a trait so the steps can be tested without a server.
pub trait Fetch {
    /// The whole body of a GET, as text.
    fn get_text(&self, url: &str) -> Result<String, FetchError>;

    /// Writes the body of a GET to `dest` and returns its byte count. `progress` is called after
    /// each chunk of at most 64 KiB with the bytes so far and the content length, when the server
    /// gave one. Breaking from `progress` stops with [`FetchError::Cancelled`].
    fn download(
        &self,
        url: &str,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
    ) -> Result<u64, FetchError>;
}

/// The real client, over HTTPS through `ureq`.
pub struct HttpFetch {
    agent: ureq::Agent,
}

impl HttpFetch {
    pub fn new() -> Self {
        // The connect and the first reply have 20 s. The whole body has 15 minutes, because
        // `ureq` cannot limit the gap between two chunks, and a slow line needs minutes for 100 MB.
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(CONNECT_TIMEOUT))
            .timeout_recv_body(Some(DOWNLOAD_TIMEOUT))
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// The response to a GET. A status other than success is an error.
    fn send(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>, FetchError> {
        let response = self.agent.get(url).call().map_err(network)?;
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(FetchError::Http {
                status: status.as_u16(),
            })
        }
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetch for HttpFetch {
    fn get_text(&self, url: &str) -> Result<String, FetchError> {
        let mut response = self.send(url)?;
        response.body_mut().read_to_string().map_err(network)
    }

    fn download(
        &self,
        url: &str,
        dest: &mut dyn Write,
        progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
    ) -> Result<u64, FetchError> {
        let mut response = self.send(url)?;
        let expected = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.parse::<u64>().ok());
        // The body is about 100 MB, far above the reader's default limit.
        let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
        let mut buffer = vec![0_u8; CHUNK];
        let mut got: u64 = 0;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| short_or_network(error, expected, got))?;
            if read == 0 {
                break;
            }
            let chunk = buffer
                .get(..read)
                .ok_or_else(|| network("the reader reported more bytes than the buffer holds"))?;
            dest.write_all(chunk).map_err(network)?;
            got = got
                .checked_add(u64::try_from(read).map_err(network)?)
                .ok_or_else(|| network("the download is too large to count"))?;
            if progress(got, expected).is_break() {
                return Err(FetchError::Cancelled);
            }
        }
        match expected.filter(|&expected| expected != got) {
            Some(expected) => Err(FetchError::Truncated { expected, got }),
            None => Ok(got),
        }
    }
}

/// Why a fetch, a check or an import failed. Each variant is one plain sentence.
#[derive(Debug)]
pub enum FetchError {
    /// The server could not be reached, or the connection failed part way.
    Network(String),
    /// The server answered with a status other than success.
    Http {
        status: u16,
    },
    /// The latest-build answer is not a build. The body is cut at 200 characters.
    BadLatest(String),
    /// The download was stopped from the progress callback.
    Cancelled,
    /// The server sent fewer bytes than it announced.
    Truncated {
        expected: u64,
        got: u64,
    },
    /// The zip is a different build from the one asked for.
    BuildMismatch {
        expected: u64,
        got: u64,
    },
    /// A file in the work directory could not be written, read or removed.
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Import(SdeError),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(message) => write!(f, "cannot reach the server: {message}"),
            Self::Http { status } => write!(f, "the server answered with status {status}"),
            Self::BadLatest(body) => write!(f, "the latest-build answer is not a build: {body}"),
            Self::Cancelled => f.write_str("the download was cancelled"),
            Self::Truncated { expected, got } => {
                write!(f, "the download stopped at {got} of {expected} bytes")
            }
            Self::BuildMismatch { expected, got } => write!(
                f,
                "the download is build {got}, but build {expected} was asked for"
            ),
            Self::Io { path, source } => write!(f, "cannot use {}: {source}", path.display()),
            Self::Import(error) => write!(f, "the static data could not be imported: {error}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Import(error) => Some(error),
            Self::Network(_)
            | Self::Http { .. }
            | Self::BadLatest(_)
            | Self::Cancelled
            | Self::Truncated { .. }
            | Self::BuildMismatch { .. } => None,
        }
    }
}

/// The address of the zip for one build.
pub fn zip_url(build: u64) -> String {
    format!(
        "https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-{build}-jsonl.zip"
    )
}

#[derive(Deserialize)]
struct LatestRecord {
    #[serde(rename = "_key")]
    key: String,
    #[serde(rename = "buildNumber")]
    build_number: u64,
    #[serde(rename = "releaseDate")]
    release_date: String,
}

/// The current build, from the first line of [`LATEST_URL`].
pub fn latest_build(f: &dyn Fetch) -> Result<SdeBuild, FetchError> {
    let body = f.get_text(LATEST_URL)?;
    let bad = || FetchError::BadLatest(body.chars().take(BAD_BODY_CHARS).collect());
    let first = body.lines().next().unwrap_or("");
    let record: LatestRecord = serde_json::from_str(first).map_err(|_error| bad())?;
    if record.key != "sde" {
        return Err(bad());
    }
    Ok(SdeBuild {
        number: record.build_number,
        release_date: record.release_date,
    })
}

/// Whether the daily check is due. `last_check` and `now` are Unix seconds. A check is due when
/// there was none, when a day has passed, or when the clock has moved back.
pub fn update_due(last_check: Option<i64>, now: i64) -> bool {
    match last_check {
        None => true,
        Some(last) => {
            now < last
                || now
                    .checked_sub(last)
                    .is_none_or(|elapsed| elapsed >= DAY_SECONDS)
        }
    }
}

/// Whether `latest` is newer than the build installed, or nothing is installed.
pub fn needs_update(installed: Option<u64>, latest: u64) -> bool {
    installed.is_none_or(|build| latest > build)
}

/// Downloads `build` into `work_dir`, checks it is that build and imports it into `db`.
///
/// The download goes to a `.part` file, then to a zip. Both are removed on every error and on
/// success, so `work_dir` is left as it was. `db` is written only after the zip has been checked.
pub fn download_and_import(
    f: &dyn Fetch,
    build: u64,
    work_dir: &Path,
    db: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
) -> Result<ImportReport, FetchError> {
    let part = work_dir.join(format!("sde-{build}.zip.part"));
    let zip = work_dir.join(format!("sde-{build}.zip"));
    match fetch_and_import(f, build, work_dir, &part, &zip, db, progress) {
        Ok(report) => Ok(report),
        Err(error) => Err(discard(&part, &zip, error)),
    }
}

fn fetch_and_import(
    f: &dyn Fetch,
    build: u64,
    work_dir: &Path,
    part: &Path,
    zip: &Path,
    db: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
) -> Result<ImportReport, FetchError> {
    fs::create_dir_all(work_dir).map_err(|source| io_error(work_dir, source))?;
    download_to(f, build, part, progress)?;
    fs::rename(part, zip).map_err(|source| io_error(part, source))?;
    let got = zip_build(zip)?;
    if got != build {
        return Err(FetchError::BuildMismatch {
            expected: build,
            got,
        });
    }
    let report = import(zip, db).map_err(FetchError::Import)?;
    remove(zip).map_err(|source| io_error(zip, source))?;
    Ok(report)
}

/// Writes the zip for `build` to `part`, replacing any earlier file there.
fn download_to(
    f: &dyn Fetch,
    build: u64,
    part: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
) -> Result<(), FetchError> {
    let file = File::create(part).map_err(|source| io_error(part, source))?;
    let mut sink = FileSink { file, error: None };
    let mut announced = None;
    let mut track = |got: u64, total: Option<u64>| {
        announced = total;
        progress(got, total)
    };
    let result = f.download(&zip_url(build), &mut sink, &mut track);
    match result {
        Ok(got) => match announced.filter(|&expected| expected != got) {
            Some(expected) => Err(FetchError::Truncated { expected, got }),
            None => Ok(()),
        },
        // A failed write to the file is reported as the file's error, not as a network one.
        Err(error) => Err(match sink.error {
            Some(source) => io_error(part, source),
            None => error,
        }),
    }
}

/// The file the zip is written to. A write error is kept whole, so the caller can report it as
/// the file's error and not as a network one.
struct FileSink {
    file: File,
    error: Option<io::Error>,
}

impl Write for FileSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.file.write(buf) {
            Ok(written) => Ok(written),
            Err(source) => {
                let copy = io::Error::new(source.kind(), source.to_string());
                self.error = Some(source);
                Err(copy)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The build number in the zip's `_sde.jsonl`, from its first non-blank line.
fn zip_build(zip: &Path) -> Result<u64, FetchError> {
    #[derive(Deserialize)]
    struct BuildLine {
        #[serde(rename = "buildNumber")]
        build_number: Option<u64>,
    }

    let file = File::open(zip).map_err(|source| io_error(zip, source))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| FetchError::Import(SdeError::Zip(error)))?;
    let mut entry = archive.by_name(SDE_FILE).map_err(|error| {
        FetchError::Import(if matches!(error, zip::result::ZipError::FileNotFound) {
            SdeError::MissingFile(SDE_FILE.to_owned())
        } else {
            SdeError::Zip(error)
        })
    })?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|source| io_error(zip, source))?;
    let (line, record) = (1_usize..)
        .zip(text.lines())
        .find(|(_, line)| !line.trim().is_empty())
        .ok_or_else(|| missing_build(1))?;
    let record: BuildLine = serde_json::from_str(record).map_err(|error| {
        FetchError::Import(SdeError::Json {
            file: SDE_FILE.to_owned(),
            line,
            message: error.to_string(),
        })
    })?;
    record.build_number.ok_or_else(|| missing_build(line))
}

fn missing_build(line: usize) -> FetchError {
    FetchError::Import(SdeError::MissingField {
        file: SDE_FILE.to_owned(),
        line,
        field: "buildNumber".to_owned(),
    })
}

/// Removes both files after a failure. Both are tried. If one cannot be removed, the first such
/// error is reported, and it carries the original failure too.
fn discard(part: &Path, zip: &Path, error: FetchError) -> FetchError {
    let failures: Vec<(&Path, io::Error)> = [part, zip]
        .into_iter()
        .filter_map(|path| remove(path).err().map(|source| (path, source)))
        .collect();
    match failures.into_iter().next() {
        Some((path, source)) => io_error(
            path,
            io::Error::new(
                source.kind(),
                format!("{source} (the download had already failed: {error})"),
            ),
        ),
        None => error,
    }
}

/// A read that ended early is a truncated body when the length was announced. Any other read error
/// is a network one.
fn short_or_network(error: io::Error, expected: Option<u64>, got: u64) -> FetchError {
    match expected {
        Some(expected) if error.kind() == io::ErrorKind::UnexpectedEof => {
            FetchError::Truncated { expected, got }
        }
        Some(_) | None => network(error),
    }
}

/// Removes a file. A file that is not there is fine.
fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(source),
    }
}

fn io_error(path: &Path, source: io::Error) -> FetchError {
    FetchError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn network(error: impl fmt::Display) -> FetchError {
    FetchError::Network(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use tempfile::TempDir;

    use super::{FetchError, discard, remove, short_or_network};

    #[test]
    fn only_an_early_end_of_a_body_with_a_known_length_is_truncated() {
        let end = || io::Error::from(io::ErrorKind::UnexpectedEof);
        let reset = || io::Error::from(io::ErrorKind::ConnectionReset);
        assert!(matches!(
            short_or_network(end(), Some(1_000), 500),
            FetchError::Truncated {
                expected: 1_000,
                got: 500
            }
        ));
        assert!(matches!(
            short_or_network(reset(), Some(1_000), 500),
            FetchError::Network(_)
        ));
        assert!(matches!(
            short_or_network(end(), None, 500),
            FetchError::Network(_)
        ));
    }

    #[test]
    fn removing_a_missing_file_is_fine_but_a_failed_removal_is_an_error() {
        let dir = TempDir::new().unwrap();
        remove(&dir.path().join("absent.zip")).unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        assert!(remove(&folder).is_err());
    }

    #[test]
    fn a_failed_cleanup_is_reported_as_the_file_error() {
        let dir = TempDir::new().unwrap();
        let part = dir.path().join("stuck.part");
        std::fs::create_dir(&part).unwrap();
        let zip = dir.path().join("absent.zip");
        let FetchError::Io { path, .. } = discard(&part, &zip, FetchError::Cancelled) else {
            panic!("expected an Io error");
        };
        assert_eq!(path, Path::new(&part));
    }
}
