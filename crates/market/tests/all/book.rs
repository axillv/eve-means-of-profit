//! Order parsing and quotes, on a saved ESI response for R.A.M.- Robotics (11481) in The Forge.

#[cfg(test)]
mod tests {
    use profit_core::money::Money;
    use profit_market::book::{Order, PriceRule, Quote, parse_orders, quote};
    use profit_market::error::MarketError;
    use profit_market::fuzzwork::{Side, Stat};

    const FIXTURE: &str = include_str!("../fixtures/orders-10000002-11481.json");
    const JITA: u64 = 60003760;
    const JITA_SELL_UNITS: u64 = 1_948_417;
    const JITA_BUY_UNITS: u64 = 378_358;

    fn isk(text: &str) -> Money {
        Money::parse_isk(text).expect("test amount")
    }

    fn fixture() -> Vec<Order> {
        parse_orders(FIXTURE).expect("fixture parses")
    }

    fn price(rule: PriceRule, qty: u64) -> Quote {
        quote(&fixture(), JITA, rule, qty).expect("no overflow")
    }

    fn priced(quoted: Quote) -> (Money, Money, u64) {
        match quoted {
            Quote::Priced {
                total,
                per_unit,
                available,
            } => (total, per_unit, available),
            Quote::Thin { .. } | Quote::NoOrders => {
                panic!("expected a priced quote, got {quoted:?}")
            }
        }
    }

    #[test]
    fn parses_every_order_in_the_fixture() {
        let orders = fixture();
        assert_eq!(orders.len(), 57);
        let at_jita: Vec<&Order> = orders.iter().filter(|o| o.location_id == JITA).collect();
        assert_eq!(at_jita.len(), 50);
        assert_eq!(at_jita.iter().filter(|o| !o.is_buy).count(), 37);
        assert_eq!(at_jita.iter().filter(|o| o.is_buy).count(), 13);
        let cheapest = at_jita
            .iter()
            .filter(|o| !o.is_buy)
            .min_by_key(|o| o.price)
            .expect("Jita has sell orders");
        assert_eq!(cheapest.price, isk("478.60"));
        assert_eq!(cheapest.volume_remain, 7_172);
    }

    #[test]
    fn walk_sell_prices_from_the_cheapest_orders() {
        let (total, per_unit, available) = priced(price(PriceRule::WalkSell, 1));
        assert_eq!(total, isk("478.60"));
        assert_eq!(per_unit, isk("478.60"));
        assert_eq!(available, JITA_SELL_UNITS);

        let (total, per_unit, available) = priced(price(PriceRule::WalkSell, 7_172));
        assert_eq!(total, isk("3,432,519.20"));
        assert_eq!(per_unit, isk("478.60"));
        assert_eq!(available, JITA_SELL_UNITS);

        let (total, _, available) = priced(price(PriceRule::WalkSell, 7_173));
        assert_eq!(total, isk("3,432,997.90"));
        assert_eq!(available, JITA_SELL_UNITS);

        let (total, per_unit, available) = priced(price(PriceRule::WalkSell, 10_000));
        assert_eq!(total, isk("4,786,282.80"));
        assert_eq!(per_unit, isk("478.6283"));
        assert_eq!(available, JITA_SELL_UNITS);

        let (total, per_unit, available) = priced(price(PriceRule::WalkSell, 50_000));
        assert_eq!(total, isk("23,939,515.40"));
        assert_eq!(per_unit, isk("478.7903"));
        assert_eq!(available, JITA_SELL_UNITS);
    }

    #[test]
    fn walk_sell_reports_a_thin_book_instead_of_a_price() {
        assert_eq!(
            price(PriceRule::WalkSell, 1_948_418),
            Quote::Thin {
                wanted: 1_948_418,
                available: JITA_SELL_UNITS,
                total_for_available: isk("1,084,027,097.90"),
            }
        );
        let (total, _, available) = priced(price(PriceRule::WalkSell, 1_948_417));
        assert_eq!(total, isk("1,084,027,097.90"));
        assert_eq!(available, JITA_SELL_UNITS);
    }

    #[test]
    fn lowest_sell_and_highest_buy_multiply_one_price() {
        let (total, per_unit, available) = priced(price(PriceRule::LowestSell, 10_000));
        assert_eq!(total, isk("4,786,000.00"));
        assert_eq!(per_unit, isk("478.60"));
        assert_eq!(available, JITA_SELL_UNITS);

        let (total, per_unit, available) = priced(price(PriceRule::HighestBuy, 10_000));
        assert_eq!(total, isk("4,318,000.00"));
        assert_eq!(per_unit, isk("431.80"));
        assert_eq!(available, JITA_BUY_UNITS);
    }

    #[test]
    fn zero_quantity_is_priced_at_zero_for_every_rule() {
        for (rule, available) in [
            (PriceRule::WalkSell, JITA_SELL_UNITS),
            (PriceRule::LowestSell, JITA_SELL_UNITS),
            (PriceRule::HighestBuy, JITA_BUY_UNITS),
        ] {
            assert_eq!(
                priced(price(rule, 0)),
                (Money::ZERO, Money::ZERO, available),
                "{rule:?}"
            );
        }
    }

    #[test]
    fn a_location_without_orders_gives_no_orders_for_every_rule() {
        let orders = fixture();
        for rule in [
            PriceRule::WalkSell,
            PriceRule::LowestSell,
            PriceRule::HighestBuy,
        ] {
            for qty in [0, 10_000] {
                assert_eq!(
                    quote(&orders, 1, rule, qty),
                    Ok(Quote::NoOrders),
                    "{rule:?} qty {qty}"
                );
            }
        }
    }

    #[test]
    fn a_side_without_orders_gives_no_orders_even_for_zero_quantity() {
        let sells_only: Vec<Order> = fixture().into_iter().filter(|o| !o.is_buy).collect();
        assert_eq!(
            quote(&sells_only, JITA, PriceRule::HighestBuy, 10_000),
            Ok(Quote::NoOrders)
        );
        assert_eq!(
            quote(&sells_only, JITA, PriceRule::HighestBuy, 0),
            Ok(Quote::NoOrders)
        );
        let sell_only_walk = quote(&sells_only, JITA, PriceRule::WalkSell, 1);
        assert!(matches!(sell_only_walk, Ok(Quote::Priced { .. })));
    }

    #[test]
    fn a_price_too_large_for_the_money_type_is_an_overflow() {
        let book = vec![Order {
            order_id: 1,
            type_id: 11481,
            location_id: JITA,
            is_buy: false,
            price: isk("9,000,000,000,000"),
            volume_remain: 2_000_000,
        }];
        assert_eq!(
            quote(&book, JITA, PriceRule::WalkSell, 2_000_000),
            Err(MarketError::Overflow)
        );
    }

    #[test]
    fn an_unreadable_price_names_the_order() {
        let json = r#"[{"order_id":42,"type_id":11481,"location_id":60003760,"is_buy_order":false,"price":478.60001,"volume_remain":5}]"#;
        assert_eq!(
            parse_orders(json),
            Err(MarketError::BadPrice {
                order_id: 42,
                text: "478.60001".to_owned(),
            })
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        let cases = [
            (
                MarketError::Network("timed out".to_owned()),
                "network request failed: timed out",
            ),
            (
                MarketError::Http { status: 503 },
                "ESI answered with status 503",
            ),
            (
                MarketError::RateLimited {
                    retry_after_secs: 30,
                },
                "ESI rate limit reached; retry in 30 seconds",
            ),
            (
                MarketError::Json("expected value".to_owned()),
                "ESI sent unreadable JSON: expected value",
            ),
            (
                MarketError::BadPrice {
                    order_id: 42,
                    text: "478.60001".to_owned(),
                },
                "order 42 has an unreadable price \"478.60001\"",
            ),
            (
                MarketError::BadHeader {
                    name: "expires",
                    value: "soon".to_owned(),
                },
                "ESI sent an unreadable expires header \"soon\"",
            ),
            (
                MarketError::Overflow,
                "the price times the quantity does not fit the supported range",
            ),
        ];
        for (error, sentence) in cases {
            assert_eq!(error.to_string(), sentence);
        }
    }

    #[test]
    fn broken_json_is_a_json_error() {
        assert!(matches!(parse_orders("[{"), Err(MarketError::Json(_))));
    }

    #[test]
    fn the_default_rule_is_the_sell_side_five_percent_price() {
        assert_eq!(PriceRule::default(), PriceRule::SellFivePercent);
    }

    #[test]
    fn each_rule_reads_its_side_and_statistic_from_the_summary_file() {
        assert_eq!(
            PriceRule::SellFivePercent.summary(),
            Some((Side::Sell, Stat::FivePercent))
        );
        assert_eq!(
            PriceRule::LowestSell.summary(),
            Some((Side::Sell, Stat::Best))
        );
        assert_eq!(
            PriceRule::HighestBuy.summary(),
            Some((Side::Buy, Stat::Best))
        );
        assert_eq!(
            PriceRule::BuyMedian.summary(),
            Some((Side::Buy, Stat::Median))
        );
        assert_eq!(PriceRule::WalkSell.summary(), None);
    }

    #[test]
    fn a_summary_rule_asked_to_quote_from_the_order_list_is_an_error() {
        let orders = fixture();
        for rule in [
            PriceRule::SellFivePercent,
            PriceRule::SellWeightedAverage,
            PriceRule::SellMedian,
            PriceRule::BuyFivePercent,
            PriceRule::BuyWeightedAverage,
            PriceRule::BuyMedian,
        ] {
            assert_eq!(
                quote(&orders, JITA, rule, 10),
                Err(MarketError::SummaryRule),
                "{rule:?}"
            );
        }
    }
}
