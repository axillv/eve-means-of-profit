//! The order-book cache against a scripted ESI: which requests are made, when, and what is kept.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::{Duration, UNIX_EPOCH};

    use mop_market::cache::{Http, HttpResponse, Market};
    use mop_market::error::MarketError;

    const FIXTURE: &str = include_str!("../fixtures/orders-10000002-11481.json");
    const ETAG: &str = "\"ad32a46fbc7c5240cbbcf87775dac0d2396d06091883b20aa5f123cb\"";
    const TYPE: u32 = 11481;
    const T0: i64 = 1_000_000;

    type Reply = Result<HttpResponse, MarketError>;
    type Call = (String, Option<String>);

    /// Answers from a queue, in order, and records every request. Clones share the same queue and log.
    #[derive(Clone, Default)]
    struct Script {
        replies: Rc<RefCell<VecDeque<Reply>>>,
        calls: Rc<RefCell<Vec<Call>>>,
    }

    impl Script {
        fn with(replies: impl IntoIterator<Item = Reply>) -> Self {
            let script = Self::default();
            script.replies.borrow_mut().extend(replies);
            script
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl Http for Script {
        fn get(&self, url: &str, if_none_match: Option<&str>) -> Reply {
            self.calls
                .borrow_mut()
                .push((url.to_owned(), if_none_match.map(str::to_owned)));
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(MarketError::Network("no reply scripted".to_owned())))
        }
    }

    fn http_date(secs: i64) -> String {
        let secs = u64::try_from(secs).expect("test times are after 1970");
        httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn url(page: Option<u32>) -> String {
        let base = "https://esi.evetech.net/markets/10000002/orders/?type_id=11481&order_type=all";
        match page {
            None => base.to_owned(),
            Some(page) => format!("{base}&page={page}"),
        }
    }

    /// A 200 answer for a book fetched at `at`, expiring five minutes later as ESI's answers do.
    fn ok(at: i64, body: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            etag: Some(ETAG.to_owned()),
            last_modified: Some(http_date(at)),
            expires: Some(http_date(at + 300)),
            pages: Some(1),
            retry_after: None,
            body: body.to_owned(),
        }
    }

    fn not_modified(at: i64) -> HttpResponse {
        HttpResponse {
            status: 304,
            ..ok(at, "")
        }
    }

    fn status(code: u16) -> HttpResponse {
        HttpResponse {
            status: code,
            ..HttpResponse::default()
        }
    }

    fn one_order(order_id: u64) -> String {
        format!(
            r#"[{{"order_id":{order_id},"type_id":11481,"location_id":60003760,"is_buy_order":false,"price":478.6,"volume_remain":5}}]"#
        )
    }

    #[test]
    fn nothing_is_cached_before_the_first_book() {
        let market = Market::new(Script::default());
        assert!(market.cached(TYPE).is_none());
    }

    #[test]
    fn a_fresh_copy_is_served_without_a_request() {
        let script = Script::with([Ok(ok(T0, FIXTURE))]);
        let mut market = Market::new(script.clone());

        let first = market.book(TYPE, T0).expect("first book");
        assert_eq!(first.orders.len(), 57);
        assert!(!first.from_cache);
        assert_eq!(script.calls().len(), 1);
        assert_eq!(script.calls()[0], (url(None), None));

        let again = market.book(TYPE, T0 + 299).expect("cached book");
        assert!(again.from_cache);
        assert_eq!(again.orders.len(), 57);
        assert_eq!(script.calls().len(), 1);
    }

    #[test]
    fn an_expired_copy_is_revalidated_and_a_304_keeps_it() {
        let script = Script::with([Ok(ok(T0, FIXTURE)), Ok(not_modified(T0 + 300))]);
        let mut market = Market::new(script.clone());
        market.book(TYPE, T0).expect("first book");

        let revalidated = market.book(TYPE, T0 + 300).expect("revalidated book");
        assert!(!revalidated.from_cache);
        assert_eq!(revalidated.orders.len(), 57);
        assert_eq!(revalidated.last_modified, T0 + 300);
        assert_eq!(script.calls().len(), 2);
        assert_eq!(script.calls()[1], (url(None), Some(ETAG.to_owned())));

        assert!(market.book(TYPE, T0 + 599).expect("still fresh").from_cache);
        assert_eq!(script.calls().len(), 2);
    }

    #[test]
    fn every_page_is_fetched_and_joined() {
        let first = HttpResponse {
            pages: Some(3),
            ..ok(T0, &one_order(1))
        };
        let script = Script::with([
            Ok(first),
            Ok(ok(T0, &one_order(2))),
            Ok(ok(T0, &one_order(3))),
        ]);
        let mut market = Market::new(script.clone());

        let book = market.book(TYPE, T0).expect("three pages");
        let ids: Vec<u64> = book.orders.iter().map(|order| order.order_id).collect();
        assert_eq!(ids, [1, 2, 3]);
        let calls = script.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1].0, url(Some(2)));
        assert_eq!(calls[2].0, url(Some(3)));
    }

    /// Replies for a book of `pages` pages with one order each.
    fn paged_replies(pages: u32) -> Vec<Reply> {
        (1..=pages)
            .map(|page| {
                let reply = ok(T0, &one_order(u64::from(page)));
                Ok(HttpResponse {
                    pages: Some(pages),
                    ..reply
                })
            })
            .collect()
    }

    #[test]
    fn a_book_of_a_hundred_pages_is_fetched_whole() {
        let script = Script::with(paged_replies(100));
        let mut market = Market::new(script.clone());
        assert_eq!(market.book(TYPE, T0).expect("100 pages").orders.len(), 100);
        assert_eq!(script.calls().len(), 100);
    }

    #[test]
    fn a_book_of_more_than_a_hundred_pages_is_refused_before_paging() {
        let script = Script::with(paged_replies(101));
        let mut market = Market::new(script.clone());
        assert_eq!(
            market.book(TYPE, T0).err(),
            Some(MarketError::BadHeader {
                name: "x-pages",
                value: "101".to_owned(),
            })
        );
        assert_eq!(script.calls().len(), 1);
        assert!(market.cached(TYPE).is_none());
    }

    #[test]
    fn a_rate_limit_stops_every_request_until_the_wait_has_passed() {
        let limited = HttpResponse {
            retry_after: Some(30),
            ..status(420)
        };
        let script = Script::with([Ok(ok(T0, FIXTURE)), Ok(limited), Ok(ok(T0 + 430, FIXTURE))]);
        let mut market = Market::new(script.clone());
        market.book(TYPE, T0).expect("first book");

        let limit = MarketError::RateLimited {
            retry_after_secs: 30,
        };
        assert_eq!(market.book(TYPE, T0 + 400).err(), Some(limit.clone()));
        assert_eq!(script.calls().len(), 2);
        assert_eq!(market.cached(TYPE).map(|held| held.orders.len()), Some(57));

        assert_eq!(market.book(TYPE, T0 + 401).err(), Some(limit.clone()));
        assert_eq!(market.cached(TYPE).map(|held| held.orders.len()), Some(57));
        assert_eq!(market.book(TYPE, T0 + 429).err(), Some(limit));
        assert_eq!(market.cached(TYPE).map(|held| held.orders.len()), Some(57));
        assert_eq!(script.calls().len(), 2);

        let back = market.book(TYPE, T0 + 430).expect("the wait has passed");
        assert!(!back.from_cache);
        assert_eq!(script.calls().len(), 3);
        assert_eq!(market.cached(TYPE).map(|held| held.orders.len()), Some(57));
    }

    #[test]
    fn a_304_to_an_unconditional_request_is_an_error() {
        let script = Script::with([Ok(not_modified(T0))]);
        let mut market = Market::new(script);
        assert_eq!(
            market.book(TYPE, T0).err(),
            Some(MarketError::Http { status: 304 })
        );
        assert!(market.cached(TYPE).is_none());
    }

    #[test]
    fn a_rate_limit_without_retry_after_waits_a_minute() {
        let script = Script::with([Ok(status(429))]);
        let mut market = Market::new(script);
        assert_eq!(
            market.book(TYPE, T0).err(),
            Some(MarketError::RateLimited {
                retry_after_secs: 60,
            })
        );
    }

    #[test]
    fn a_server_error_keeps_the_old_copy_and_the_next_call_asks_again() {
        let script = Script::with([
            Ok(ok(T0, FIXTURE)),
            Ok(status(503)),
            Ok(ok(T0 + 301, FIXTURE)),
        ]);
        let mut market = Market::new(script.clone());
        market.book(TYPE, T0).expect("first book");

        assert_eq!(
            market.book(TYPE, T0 + 300).err(),
            Some(MarketError::Http { status: 503 })
        );
        let held = market.cached(TYPE).expect("old copy kept");
        assert_eq!(held.orders.len(), 57);
        assert_eq!(held.last_modified, T0);

        market
            .book(TYPE, T0 + 301)
            .expect("retried after the error");
        assert_eq!(script.calls().len(), 3);
    }

    #[test]
    fn an_unreadable_expires_header_is_an_error_and_keeps_the_old_copy() {
        let bad = HttpResponse {
            expires: Some("soon".to_owned()),
            ..ok(T0 + 300, FIXTURE)
        };
        let script = Script::with([Ok(ok(T0, FIXTURE)), Ok(bad)]);
        let mut market = Market::new(script);
        market.book(TYPE, T0).expect("first book");

        assert_eq!(
            market.book(TYPE, T0 + 300).err(),
            Some(MarketError::BadHeader {
                name: "expires",
                value: "soon".to_owned(),
            })
        );
        let held = market.cached(TYPE).expect("old copy kept");
        assert_eq!(held.orders.len(), 57);
        assert_eq!(held.last_modified, T0);
    }
}
