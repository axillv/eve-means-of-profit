//! The Fuzzwork file: reading the rows of one region, and when the file is asked for again.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::rc::Rc;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use profit_core::money::Money;
    use profit_market::error::MarketError;
    use profit_market::fuzzwork::{
        BytesHttp, BytesResponse, FORGE, FuzzworkMarket, Refresh, Side, Stat, parse_aggregates,
    };

    const FIXTURE: &str = include_str!("../fixtures/aggregates-forge.csv");
    const LAST_MODIFIED: &str = "Mon, 12 Jan 1970 13:42:40 GMT";
    const T0: i64 = 1_000_000;

    /// Answers from a queue, in order, and records the `If-None-Match` of every request. Clones share the queue and log.
    #[derive(Clone, Default)]
    struct Script {
        replies: Rc<RefCell<VecDeque<BytesResponse>>>,
        calls: Rc<RefCell<Vec<Option<String>>>>,
    }

    impl Script {
        fn with(replies: impl IntoIterator<Item = BytesResponse>) -> Self {
            let script = Self::default();
            script.replies.borrow_mut().extend(replies);
            script
        }

        fn calls(&self) -> Vec<Option<String>> {
            self.calls.borrow().clone()
        }
    }

    impl BytesHttp for Script {
        fn get_bytes(
            &self,
            _url: &str,
            if_none_match: Option<&str>,
        ) -> Result<BytesResponse, MarketError> {
            self.calls
                .borrow_mut()
                .push(if_none_match.map(str::to_owned));
            let reply = self
                .replies
                .borrow_mut()
                .pop_front()
                .expect("a reply was scripted for every request");
            Ok(reply)
        }
    }

    fn m(count: i64) -> Money {
        Money::from_ten_thousandths(count)
    }

    fn gzip(text: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text).unwrap();
        encoder.finish().unwrap()
    }

    fn file(etag: Option<&str>, last_modified: &str) -> BytesResponse {
        BytesResponse {
            status: 200,
            etag: etag.map(str::to_owned),
            last_modified: Some(last_modified.to_owned()),
            retry_after: None,
            body: gzip(FIXTURE.as_bytes()),
        }
    }

    fn answer(status: u16, retry_after: Option<u64>) -> BytesResponse {
        BytesResponse {
            status,
            etag: None,
            last_modified: None,
            retry_after,
            body: Vec::new(),
        }
    }

    #[test]
    fn the_forge_rows_are_read_with_their_order_set() {
        let (rows, order_set) = parse_aggregates(&gzip(FIXTURE.as_bytes()), FORGE).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(order_set, 175_414);

        let sell = &rows[&(34, Side::Sell)];
        assert_eq!(sell.min, m(38_700));
        assert_eq!(sell.max, m(3_001_000_000_000));
        assert_eq!(sell.weighted_average, m(44_387));
        assert_eq!(sell.median, m(48_200));
        assert_eq!(sell.five_percent, m(38_950));
        assert_eq!(sell.volume, 7_073_618_405);
        assert_eq!(sell.orders, 44);

        let buy = &rows[&(34, Side::Buy)];
        assert_eq!(buy.five_percent, m(37_330));
        assert_eq!(buy.max, m(40_000));
        assert_eq!(buy.min, m(100));
        assert_eq!(buy.weighted_average, m(29_864));
    }

    #[test]
    fn another_region_reads_only_its_own_rows() {
        let (rows, order_set) = parse_aggregates(&gzip(FIXTURE.as_bytes()), 10_000_043).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[&(34, Side::Sell)].five_percent, m(45_000));
        assert_eq!(order_set, 175_414);

        let (rows, order_set) = parse_aggregates(&gzip(FIXTURE.as_bytes()), 10_000_099).unwrap();
        assert!(rows.is_empty());
        assert_eq!(order_set, 0);
    }

    #[test]
    fn an_item_with_no_orders_has_no_row() {
        let (rows, _) = parse_aggregates(&gzip(FIXTURE.as_bytes()), FORGE).unwrap();
        assert!(!rows.contains_key(&(9848, Side::Sell)));
    }

    #[test]
    fn the_summary_prices_are_chosen_by_side_and_stat() {
        let (rows, _) = parse_aggregates(&gzip(FIXTURE.as_bytes()), FORGE).unwrap();
        let sell = &rows[&(34, Side::Sell)];
        assert_eq!(sell.price(Side::Sell, Stat::Best), m(38_700));
        assert_eq!(sell.price(Side::Sell, Stat::FivePercent), m(38_950));
        assert_eq!(sell.price(Side::Sell, Stat::WeightedAverage), m(44_387));
        assert_eq!(sell.price(Side::Sell, Stat::Median), m(48_200));
        let buy = &rows[&(34, Side::Buy)];
        assert_eq!(buy.price(Side::Buy, Stat::Best), m(40_000));
        assert_eq!(buy.price(Side::Buy, Stat::Median), m(33_000));
    }

    #[test]
    fn bytes_that_are_not_gzip_are_an_error() {
        assert!(matches!(
            parse_aggregates(b"not gzip", FORGE),
            Err(MarketError::Gzip(_))
        ));
    }

    #[test]
    fn a_renamed_column_is_an_error_on_the_header() {
        let renamed = FIXTURE.replacen("weightedaverage", "weighted", 1);
        assert_eq!(
            parse_aggregates(&gzip(renamed.as_bytes()), FORGE),
            Err(MarketError::BadRow {
                line: 1,
                text: renamed.lines().next().unwrap().to_owned(),
            })
        );
    }

    #[test]
    fn an_unreadable_number_in_the_region_is_an_error_on_its_line() {
        let broken = FIXTURE.replacen("478.9,900.0", "1.2.3,900.0", 1);
        assert_eq!(
            parse_aggregates(&gzip(broken.as_bytes()), FORGE),
            Err(MarketError::BadRow {
                line: 4,
                text: broken.lines().nth(3).unwrap().to_owned(),
            })
        );
    }

    #[test]
    fn an_unreadable_number_in_another_region_is_not_read() {
        let broken = FIXTURE.replacen("10000043|34|false,5.0,", "10000043|34|false,1.2.3,", 1);
        assert_eq!(
            parse_aggregates(&gzip(broken.as_bytes()), FORGE).map(|(rows, _)| rows.len()),
            Ok(4)
        );
    }

    #[test]
    fn a_volume_with_a_fraction_is_an_error() {
        let broken = FIXTURE.replacen("749477.0", "749477.5", 1);
        assert_eq!(
            parse_aggregates(&gzip(broken.as_bytes()), FORGE),
            Err(MarketError::BadRow {
                line: 5,
                text: broken.lines().nth(4).unwrap().to_owned(),
            })
        );
    }

    /// A file of exactly `size` bytes: the header, then one row of another region whose numbers are never read,
    /// padded out with a long last field.
    fn padded_file(size: usize) -> Vec<u8> {
        let header = FIXTURE.lines().next().unwrap();
        let mut text =
            format!("{header}\n10000099|34|false,1.0,1.0,1.0,1.0,1.0,1.0,1,1.0,").into_bytes();
        text.resize(size, b'x');
        text
    }

    #[test]
    fn a_file_of_exactly_the_limit_is_read() {
        let gz = gzip(&padded_file(256 * 1024 * 1024));
        assert_eq!(
            parse_aggregates(&gz, FORGE).map(|(rows, _)| rows.len()),
            Ok(0)
        );
    }

    #[test]
    fn a_file_larger_than_the_limit_is_too_large() {
        let huge = vec![b'a'; 256 * 1024 * 1024 + 1];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&huge).unwrap();
        let gz = encoder.finish().unwrap();
        assert_eq!(parse_aggregates(&gz, FORGE), Err(MarketError::TooLarge));
    }

    #[test]
    fn a_download_holds_the_rows_of_the_region() {
        let script = Script::with([file(Some("\"a1\""), LAST_MODIFIED)]);
        let mut market = FuzzworkMarket::new(script, FORGE);
        assert!(!market.has_rows());
        assert_eq!(market.order_set(), None);

        assert_eq!(market.refresh(T0), Ok(Refresh::Downloaded));
        assert!(market.has_rows());
        assert_eq!(market.order_set(), Some(175_414));
        let held: Vec<(u32, Side)> = market
            .rows()
            .map(|(type_id, side, _)| (type_id, side))
            .collect();
        assert_eq!(
            held,
            vec![
                (34, Side::Sell),
                (34, Side::Buy),
                (11_399, Side::Buy),
                (11_481, Side::Sell)
            ]
        );
        assert_eq!(market.row(34, Side::Buy).map(|row| row.orders), Some(58));
    }

    #[test]
    fn a_304_without_an_etag_sent_is_an_error() {
        let script = Script::with([answer(304, None)]);
        let mut market = FuzzworkMarket::new(script, FORGE);
        assert_eq!(market.refresh(T0), Err(MarketError::Http { status: 304 }));
        assert!(!market.has_rows());
    }

    #[test]
    fn the_first_download_is_asked_for_without_an_etag_and_then_waits() {
        let script = Script::with([file(Some("\"a1\""), LAST_MODIFIED), answer(304, None)]);
        let mut market = FuzzworkMarket::new(script.clone(), FORGE);

        assert_eq!(market.refresh(T0), Ok(Refresh::Downloaded));
        assert_eq!(script.calls(), vec![None]);
        assert_eq!(market.published_at(), Some(999_760));
        assert_eq!(market.price(34, Side::Sell, Stat::Best), Some(m(38_700)));
        assert_eq!(market.next_refresh_at(), Some(1_000_900));

        assert_eq!(
            market.refresh(1_000_899),
            Ok(Refresh::TooSoon { next_at: 1_000_900 })
        );
        assert_eq!(script.calls().len(), 1);

        assert_eq!(market.refresh(1_000_900), Ok(Refresh::NotModified));
        assert_eq!(script.calls(), vec![None, Some("\"a1\"".to_owned())]);
        assert_eq!(market.price(34, Side::Sell, Stat::Best), Some(m(38_700)));
        assert_eq!(market.next_refresh_at(), Some(1_001_800));
    }

    #[test]
    fn a_rate_limit_keeps_the_rows_and_waits_for_retry_after() {
        let script = Script::with([
            file(Some("\"a1\""), LAST_MODIFIED),
            answer(429, Some(30)),
            answer(503, None),
            file(Some("\"a2\""), LAST_MODIFIED),
        ]);
        let mut market = FuzzworkMarket::new(script.clone(), FORGE);
        assert_eq!(market.refresh(T0), Ok(Refresh::Downloaded));

        let limited = Err(MarketError::RateLimited {
            retry_after_secs: 30,
        });
        assert_eq!(market.refresh(1_000_900), limited);
        assert_eq!(market.price(34, Side::Sell, Stat::Best), Some(m(38_700)));
        assert_eq!(market.refresh(1_000_929), limited);
        assert_eq!(script.calls().len(), 2);

        assert_eq!(
            market.refresh(1_000_930),
            Err(MarketError::Http { status: 503 })
        );
        assert_eq!(script.calls().len(), 3);
        assert_eq!(market.published_at(), Some(999_760));
    }

    #[test]
    fn a_failure_waits_a_minute_and_keeps_the_rows() {
        let script = Script::with([
            file(Some("\"a1\""), LAST_MODIFIED),
            answer(503, None),
            file(Some("\"a2\""), LAST_MODIFIED),
        ]);
        let mut market = FuzzworkMarket::new(script.clone(), FORGE);
        assert_eq!(market.refresh(T0), Ok(Refresh::Downloaded));

        assert_eq!(
            market.refresh(1_000_900),
            Err(MarketError::Http { status: 503 })
        );
        assert_eq!(
            market.refresh(1_000_959),
            Err(MarketError::Http { status: 503 })
        );
        assert_eq!(script.calls().len(), 2);
        assert_eq!(market.next_refresh_at(), Some(1_000_960));

        assert_eq!(market.refresh(1_000_960), Ok(Refresh::Downloaded));
        assert_eq!(script.calls().len(), 3);
    }

    #[test]
    fn a_body_that_is_not_gzip_keeps_the_rows() {
        let not_gzip = BytesResponse {
            status: 200,
            etag: Some("\"a2\"".to_owned()),
            last_modified: Some(LAST_MODIFIED.to_owned()),
            retry_after: None,
            body: b"plain text".to_vec(),
        };
        let script = Script::with([file(Some("\"a1\""), LAST_MODIFIED), not_gzip]);
        let mut market = FuzzworkMarket::new(script, FORGE);
        assert_eq!(market.refresh(T0), Ok(Refresh::Downloaded));

        assert!(matches!(
            market.refresh(1_000_900),
            Err(MarketError::Gzip(_))
        ));
        assert_eq!(market.price(34, Side::Sell, Stat::Best), Some(m(38_700)));
        assert_eq!(market.published_at(), Some(999_760));
    }

    #[test]
    fn an_unreadable_last_modified_keeps_the_rows() {
        let script = Script::with([
            file(Some("\"a1\""), LAST_MODIFIED),
            file(Some("\"a2\""), "soon"),
        ]);
        let mut market = FuzzworkMarket::new(script, FORGE);
        assert_eq!(market.refresh(T0), Ok(Refresh::Downloaded));

        assert!(matches!(
            market.refresh(1_000_900),
            Err(MarketError::BadHeader {
                name: "last-modified",
                ..
            })
        ));
        assert_eq!(market.price(34, Side::Sell, Stat::Best), Some(m(38_700)));
        assert_eq!(market.published_at(), Some(999_760));
    }
}
