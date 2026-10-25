//! Compare access paths for `product_id = ? AND state = ?` on generated data.
//!
//! cargo run --release --offline -p conceptual-database --example index_strategies [orders] [std|bbtree]

mod support;

use conceptual_database::*;
use std::time::Instant;
use support::{generate, median, HOT_PRODUCT};

const STRATEGIES: [(&str, Option<IndexHint>, bool); 5] = [
    ("full scan", None, true),
    ("state index + filter", Some(IndexHint::State), false),
    ("product index + filter", Some(IndexHint::Product), false),
    (
        "intersect product ∩ state",
        Some(IndexHint::IntersectProductState),
        false,
    ),
    (
        "composite (product, state)",
        Some(IndexHint::ProductState),
        false,
    ),
];

fn main() {
    let orders = std::env::args()
        .nth(1)
        .map_or(1_000_000, |arg| arg.parse().expect("order count"));
    let tree = match std::env::args().nth(2).as_deref() {
        None | Some("std") => Tree::Std,
        Some("bbtree") => Tree::BBTree,
        Some(other) => panic!("unknown tree {other}; expected std or bbtree"),
    };
    let start = Instant::now();
    let db = generate(orders, tree);
    println!(
        "Loaded {orders} orders into {tree:?} trees in {:.2?}; each insert updated 1 primary + 4 secondary indexes.\n",
        start.elapsed()
    );
    for (label, product, state) in [
        (
            "Typical product, rare state",
            ProductId(7),
            OrderState::Cancelled,
        ),
        (
            "Typical product, common state",
            ProductId(7),
            OrderState::Shipped,
        ),
        (
            "Hot product, rare state",
            HOT_PRODUCT,
            OrderState::Cancelled,
        ),
    ] {
        println!("{label}: product_id = {} AND state = {state:?}", product.0);
        println!(
            "  {:<28} {:>11} {:>14} {:>9} {:>10}",
            "strategy", "index reads", "rows fetched", "returned", "median"
        );
        let mut expected = None;
        for (name, index, force_scan) in STRATEGIES {
            let query = Query {
                product_id: Some(product),
                state: Some(state),
                index,
                force_scan,
                ..Query::default()
            };
            let result = db.query(&query).unwrap();
            assert_eq!(
                *expected.get_or_insert_with(|| result.orders.clone()),
                result.orders,
                "{name} returned different rows"
            );
            let runs = if force_scan { 5 } else { 51 };
            let execution = &result.execution;
            println!(
                "  {name:<28} {:>11} {:>14} {:>9} {:>10.1?}",
                execution.index_entries_read,
                execution.records_examined,
                execution.records_returned,
                median(&db, &query, runs)
            );
        }
        println!();
    }
}
