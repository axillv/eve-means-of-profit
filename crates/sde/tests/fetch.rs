//! Acceptance tests for the fetch steps. A fake network serves the build 3607705 fixture as the zip.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::io::{Cursor, Read, Write};
    use std::net::TcpListener;
    use std::ops::ControlFlow;
    use std::path::{Path, PathBuf};
    use std::thread;

    use profit_sde::{
        Fetch, FetchError, HttpFetch, ImportReport, LATEST_URL, SdeError, download_and_import,
        import, latest_build, needs_update, update_due, zip_url,
    };
    use tempfile::TempDir;

    const BUILD: u64 = 3_607_705;
    const LATEST_BODY: &str =
        r#"{"_key": "sde", "buildNumber": 3607705, "releaseDate": "2026-10-12T11:08:38Z"}"#;
    /// What the fake sends in each chunk. Smaller than the real 64 KiB, so the fixture gives
    /// several progress calls.
    const SENT_CHUNK: usize = 16 * 1024;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde-3607705")
    }

    /// The fixture files, zipped with them at the root.
    fn fixture_zip() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for entry in fs::read_dir(fixture()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&fs::read(entry.path()).unwrap()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// The names of the files left in `work`. A directory that does not exist has none.
    fn leftovers(work: &Path) -> Vec<String> {
        if !work.exists() {
            return Vec::new();
        }
        fs::read_dir(work)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    /// A network that records the URLs asked for and serves fixed answers.
    struct Fake {
        text: String,
        zip: Vec<u8>,
        status: u16,
        /// The content length to announce. The zip's length when `None`.
        announced: Option<u64>,
        asked: RefCell<Vec<String>>,
    }

    impl Fake {
        fn serving(text: &str, zip: Vec<u8>) -> Self {
            Self {
                text: text.to_owned(),
                zip,
                status: 200,
                announced: None,
                asked: RefCell::new(Vec::new()),
            }
        }

        fn answering(status: u16) -> Self {
            Self {
                status,
                ..Self::serving("", Vec::new())
            }
        }

        fn announcing(mut self, length: u64) -> Self {
            self.announced = Some(length);
            self
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl Fetch for Fake {
        fn get_text(&self, url: &str) -> Result<String, FetchError> {
            self.asked.borrow_mut().push(url.to_owned());
            if self.status == 200 {
                Ok(self.text.clone())
            } else {
                Err(FetchError::Http {
                    status: self.status,
                })
            }
        }

        fn download(
            &self,
            url: &str,
            dest: &mut dyn Write,
            progress: &mut dyn FnMut(u64, Option<u64>) -> ControlFlow<()>,
        ) -> Result<u64, FetchError> {
            self.asked.borrow_mut().push(url.to_owned());
            if self.status != 200 {
                return Err(FetchError::Http {
                    status: self.status,
                });
            }
            let total = self.announced.unwrap_or_else(|| length(&self.zip));
            let mut sent = 0_u64;
            for chunk in self.zip.chunks(SENT_CHUNK) {
                dest.write_all(chunk)
                    .map_err(|error| FetchError::Network(error.to_string()))?;
                sent += length(chunk);
                if progress(sent, Some(total)).is_break() {
                    return Err(FetchError::Cancelled);
                }
            }
            Ok(sent)
        }
    }

    /// The byte count of a slice, as the `u64` the trait uses.
    fn length(bytes: &[u8]) -> u64 {
        u64::try_from(bytes.len()).unwrap()
    }

    fn import_fixture(db: &Path) -> ImportReport {
        import(&fixture(), db).unwrap()
    }

    /// A local server that answers the first connection with `response` and then closes it.
    /// Returns the address to ask.
    fn serve_once(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/data", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            assert!(
                stream.read(&mut request).unwrap() > 0,
                "the request was empty"
            );
            stream.write_all(&response).unwrap();
        });
        url
    }

    #[test]
    fn the_real_client_reads_a_body_as_text() {
        let url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec(),
        );
        assert_eq!(HttpFetch::new().get_text(&url).unwrap(), "hello");
    }

    #[test]
    fn the_real_client_writes_the_body_and_counts_it() {
        let url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec(),
        );
        let mut body = Vec::new();
        let mut progress = |_: u64, _: Option<u64>| ControlFlow::Continue(());
        let count = HttpFetch::new()
            .download(&url, &mut body, &mut progress)
            .unwrap();
        assert_eq!(count, 5);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn the_real_client_reports_a_short_body_as_truncated() {
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n".to_vec();
        response.extend(vec![b'x'; 500]);
        let url = serve_once(response);
        let mut body = Vec::new();
        let mut progress = |_: u64, _: Option<u64>| ControlFlow::Continue(());
        let result = HttpFetch::new().download(&url, &mut body, &mut progress);
        assert!(
            matches!(
                result,
                Err(FetchError::Truncated {
                    expected: 1_000,
                    got: 500
                })
            ),
            "{result:?}"
        );
    }

    #[test]
    fn the_real_client_refuses_a_not_found_status() {
        let url = serve_once(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        );
        let result = HttpFetch::new().get_text(&url);
        assert!(
            matches!(result, Err(FetchError::Http { status: 404 })),
            "{result:?}"
        );
    }

    #[test]
    fn the_zip_of_a_build_has_the_per_build_address() {
        assert_eq!(
            zip_url(3_607_705),
            "https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3607705-jsonl.zip"
        );
    }

    #[test]
    fn the_latest_build_is_read_from_the_first_line() {
        let fake = Fake::serving(LATEST_BODY, Vec::new());
        let build = latest_build(&fake).unwrap();
        assert_eq!(build.number, 3_607_705);
        assert_eq!(build.release_date, "2026-10-12T11:08:38Z");
        assert_eq!(fake.asked(), vec![LATEST_URL.to_owned()]);
    }

    #[test]
    fn a_trailing_newline_or_a_second_line_does_not_change_the_latest_build() {
        for body in [
            format!("{LATEST_BODY}\n"),
            format!(
                "{LATEST_BODY}\n{{\"_key\": \"sde\", \"buildNumber\": 1, \"releaseDate\": \"d\"}}\n"
            ),
        ] {
            let build = latest_build(&Fake::serving(&body, Vec::new())).unwrap();
            assert_eq!(build.number, 3_607_705);
            assert_eq!(build.release_date, "2026-10-12T11:08:38Z");
        }
    }

    #[test]
    fn a_body_that_is_not_the_latest_build_is_refused() {
        for body in [
            r#"{"_key": "sde", "buildNumber": "x"}"#,
            r#"{"_key": "other", "buildNumber": 1, "releaseDate": "d"}"#,
            "<html>",
            "",
        ] {
            let result = latest_build(&Fake::serving(body, Vec::new()));
            assert!(
                matches!(result, Err(FetchError::BadLatest(_))),
                "{body:?} gave {result:?}"
            );
        }
    }

    #[test]
    fn a_refused_latest_body_is_cut_at_200_characters() {
        let body = "x".repeat(500);
        let result = latest_build(&Fake::serving(&body, Vec::new()));
        match result {
            Err(FetchError::BadLatest(kept)) => assert_eq!(kept.chars().count(), 200),
            other => panic!("expected BadLatest, got {other:?}"),
        }
    }

    #[test]
    fn the_daily_check_is_due_by_the_clock() {
        assert!(update_due(None, 0));
        assert!(!update_due(Some(1_000), 87_399));
        assert!(update_due(Some(1_000), 87_400));
        assert!(update_due(Some(1_000), 999));
        assert!(!update_due(Some(1_000), 1_000));
    }

    #[test]
    fn a_build_is_needed_when_nothing_or_an_older_build_is_installed() {
        assert!(needs_update(None, 3_607_705));
        assert!(!needs_update(Some(3_607_705), 3_607_705));
        assert!(needs_update(Some(3_607_705), 3_607_800));
        assert!(!needs_update(Some(3_607_800), 3_607_705));
    }

    #[test]
    fn a_build_is_downloaded_checked_and_imported() {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        let db = dir.path().join("sde.db");
        let zip = fixture_zip();
        let length = u64::try_from(zip.len()).unwrap();
        let fake = Fake::serving(LATEST_BODY, zip);
        let mut calls: Vec<(u64, Option<u64>)> = Vec::new();
        let mut progress = |got: u64, total: Option<u64>| {
            calls.push((got, total));
            ControlFlow::Continue(())
        };

        let report = download_and_import(&fake, BUILD, &work, &db, &mut progress).unwrap();

        assert_eq!(fake.asked(), vec![zip_url(BUILD)]);
        assert_eq!(report, import_fixture(&dir.path().join("reference.db")));
        assert!(calls.len() > 1, "progress was called {} times", calls.len());
        assert!(calls.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(calls.last(), Some(&(length, Some(length))));
        assert!(calls.iter().all(|&(_, total)| total == Some(length)));
        assert!(leftovers(&work).is_empty());
    }

    #[test]
    fn a_cancelled_download_leaves_no_files_and_the_database_unchanged() {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        let db = dir.path().join("sde.db");
        import_fixture(&db);
        let before = fs::read(&db).unwrap();
        let fake = Fake::serving(LATEST_BODY, fixture_zip());
        let mut progress = |_: u64, _: Option<u64>| ControlFlow::Break(());

        let result = download_and_import(&fake, BUILD, &work, &db, &mut progress);

        assert!(matches!(result, Err(FetchError::Cancelled)), "{result:?}");
        assert!(leftovers(&work).is_empty());
        assert_eq!(fs::read(&db).unwrap(), before);
    }

    #[test]
    fn a_download_shorter_than_announced_is_truncated_and_leaves_no_files() {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        let db = dir.path().join("sde.db");
        let fake = Fake::serving(LATEST_BODY, vec![0; 500]).announcing(1_000);

        let result = download_and_import(&fake, BUILD, &work, &db, &mut |_, _| {
            ControlFlow::Continue(())
        });

        assert!(
            matches!(
                result,
                Err(FetchError::Truncated {
                    expected: 1_000,
                    got: 500
                })
            ),
            "{result:?}"
        );
        assert!(leftovers(&work).is_empty());
        assert!(!db.exists());
    }

    #[test]
    fn a_zip_of_another_build_is_refused_and_the_database_unchanged() {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        let db = dir.path().join("sde.db");
        import_fixture(&db);
        let before = fs::read(&db).unwrap();
        let fake = Fake::serving(LATEST_BODY, fixture_zip());

        let result = download_and_import(&fake, 3_607_800, &work, &db, &mut |_, _| {
            ControlFlow::Continue(())
        });

        assert!(
            matches!(
                result,
                Err(FetchError::BuildMismatch {
                    expected: 3_607_800,
                    got: 3_607_705
                })
            ),
            "{result:?}"
        );
        assert_eq!(fake.asked(), vec![zip_url(3_607_800)]);
        assert!(leftovers(&work).is_empty());
        assert_eq!(fs::read(&db).unwrap(), before);
    }

    fn io_error() -> FetchError {
        FetchError::Io {
            path: PathBuf::from("work/_sde.zip"),
            source: std::io::Error::other("disk full"),
        }
    }

    #[test]
    fn every_error_is_one_plain_sentence() {
        assert_eq!(
            FetchError::Network("timed out".to_owned()).to_string(),
            "cannot reach the server: timed out"
        );
        assert_eq!(
            FetchError::Http { status: 404 }.to_string(),
            "the server answered with status 404"
        );
        assert_eq!(
            FetchError::BadLatest("<html>".to_owned()).to_string(),
            "the latest-build answer is not a build: <html>"
        );
        assert_eq!(
            FetchError::Cancelled.to_string(),
            "the download was cancelled"
        );
        assert_eq!(
            FetchError::Truncated {
                expected: 1_000,
                got: 500
            }
            .to_string(),
            "the download stopped at 500 of 1000 bytes"
        );
        assert_eq!(
            FetchError::BuildMismatch {
                expected: 3_607_800,
                got: 3_607_705
            }
            .to_string(),
            "the download is build 3607705, but build 3607800 was asked for"
        );
        assert_eq!(
            io_error().to_string(),
            "cannot use work/_sde.zip: disk full"
        );
        assert_eq!(
            FetchError::Import(SdeError::MissingFile("types.jsonl".to_owned())).to_string(),
            "the static data could not be imported: the source has no types.jsonl"
        );
    }

    #[test]
    fn an_error_names_its_source_only_when_it_has_one() {
        use std::error::Error;
        assert!(io_error().source().is_some());
        assert!(FetchError::Import(SdeError::NotImported).source().is_some());
        assert!(FetchError::Http { status: 404 }.source().is_none());
    }

    #[test]
    fn the_real_client_reports_progress_in_steps_of_at_most_64_kib() {
        let size = 200 * 1024;
        let mut response =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n")
                .into_bytes();
        response.extend(vec![b'x'; size]);
        let url = serve_once(response);
        let mut body = Vec::new();
        let mut calls: Vec<(u64, Option<u64>)> = Vec::new();
        let mut progress = |got: u64, total: Option<u64>| {
            calls.push((got, total));
            ControlFlow::Continue(())
        };

        HttpFetch::new()
            .download(&url, &mut body, &mut progress)
            .unwrap();

        assert!(calls.len() > 1, "progress was called {} times", calls.len());
        let mut before = 0;
        let mut largest = 0;
        for &(got, _) in &calls {
            assert!(got - before <= 64 * 1024, "a step from {before} to {got}");
            largest = largest.max(got - before);
            before = got;
        }
        // Steps are as large as the buffer allows, not just under the limit.
        assert!(largest > 32 * 1024, "the largest step was {largest}");
        assert_eq!(calls.last(), Some(&(204_800, Some(204_800))));
    }

    #[test]
    fn a_not_found_answer_is_an_http_error_from_both_steps() {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        let db = dir.path().join("sde.db");
        let fake = Fake::answering(404);

        let latest = latest_build(&fake);
        let download = download_and_import(&fake, BUILD, &work, &db, &mut |_, _| {
            ControlFlow::Continue(())
        });

        assert!(
            matches!(latest, Err(FetchError::Http { status: 404 })),
            "{latest:?}"
        );
        assert!(
            matches!(download, Err(FetchError::Http { status: 404 })),
            "{download:?}"
        );
        assert!(leftovers(&work).is_empty());
        assert!(!db.exists());
    }
}
