//! Measure bbtree against std's B-tree twice: on bare sets of the same keys,
//! then inside the database, where record fetches dilute the difference.
//! Finally swap a live database from one tree to the other and check that
//! every answer is unchanged.
//!
//! cargo run --release --offline -p conceptual-database --example tree_swap [orders]

mod support;

use bbtree::unsync::BBTreeSet;
use conceptual_database::*;
use std::{
    collections::BTreeSet,
    hint::black_box,
    time::{Duration, Instant},
};
use support::{generate, median, Rng, CUSTOMERS, HOT_PRODUCT};

/// The operations both sets share, so one benchmark drives either.
trait OrderedSet: Default {
    fn insert(&mut self, key: u64);
    fn contains(&self, key: u64) -> bool;
    fn remove(&mut self, key: u64);
    /// Sums the first `len` keys at or after `start`.
    fn range_sum(&self, start: u64, len: usize) -> u64;
    fn sum(&self) -> u64;
}

macro_rules! ordered_set {
    ($set:ty) => {
        impl OrderedSet for $set {
            fn insert(&mut self, key: u64) {
                <$set>::insert(self, key);
            }
            fn contains(&self, key: u64) -> bool {
                <$set>::contains(self, &key)
            }
            fn remove(&mut self, key: u64) {
                <$set>::remove(self, &key);
            }
            fn range_sum(&self, start: u64, len: usize) -> u64 {
                self.range(start..).take(len).sum()
            }
            fn sum(&self) -> u64 {
                self.iter().sum()
            }
        }
    };
}

ordered_set!(BTreeSet<u64>);
ordered_set!(BBTreeSet<u64>);

/// Times each operation over every key, in random key order; median of 3 rounds.
fn bare_set<S: OrderedSet>(keys: &[u64]) -> [Duration; 5] {
    let mut rounds: Vec<[Duration; 5]> = (0..3)
        .map(|_| {
            let mut set = S::default();
            let insert = timed(|| keys.iter().for_each(|&key| set.insert(key)));
            let lookup = timed(|| {
                black_box(keys.iter().filter(|&&key| set.contains(key)).count());
            });
            let range = timed(|| {
                black_box(keys.iter().map(|&key| set.range_sum(key, 10)).sum::<u64>());
            });
            let scan = timed(|| {
                black_box(set.sum());
            });
            let remove = timed(|| keys.iter().for_each(|&key| set.remove(key)));
            [insert, lookup, range, scan, remove]
        })
        .collect();
    std::array::from_fn(|op| {
        rounds.sort_by_key(|round| round[op]);
        rounds[1][op]
    })
}

/// Product/state lookups on the hot product, through two index paths.
fn queries() -> Vec<(&'static str, Query)> {
    let product_state = |index| Query {
        product_id: Some(HOT_PRODUCT),
        state: Some(OrderState::Cancelled),
        index: Some(index),
        ..Query::default()
    };
    vec![
        (
            "composite bucket (2k IDs)",
            product_state(IndexHint::ProductState),
        ),
        (
            "intersection (120k IDs read)",
            product_state(IndexHint::IntersectProductState),
        ),
    ]
}

/// One customer/date range scan for each of the first 10,000 customers: mostly
/// B-tree descents to each customer's first key. Returns the rows found.
fn customer_scans(db: &Database, latest: Option<usize>) -> usize {
    (0..10_000)
        .map(|customer| {
            let query = Query {
                customer_id: Some(CustomerId(customer)),
                from: latest.is_none().then_some(250_000),
                through: latest.is_none().then_some(750_000),
                latest,
                ..Query::default()
            };
            db.query(&query).unwrap().orders.len()
        })
        .sum()
}

fn timed(work: impl FnOnce()) -> Duration {
    let start = Instant::now();
    work();
    start.elapsed()
}

/// Moves `count` random orders to another state and customer: each replace
/// removes the ID from four index sets and inserts it into four others.
fn replace_orders(db: &mut Database, orders: u64, count: u64) {
    let mut rng = Rng(7);
    for _ in 0..count {
        let id = OrderId(rng.next() % orders);
        let mut order = db.get(id).unwrap().orders.remove(0);
        order.state = match order.state {
            OrderState::Placed => OrderState::Shipped,
            OrderState::Shipped => OrderState::Cancelled,
            OrderState::Cancelled => OrderState::Placed,
        };
        order.customer_id = CustomerId(rng.next() % CUSTOMERS);
        db.replace(order).unwrap();
    }
}

fn row(label: &str, std: Duration, bbtree: Duration) {
    println!(
        "  {label:<30} {std:>10.1?} {bbtree:>10.1?} {:>8.2}x",
        std.as_secs_f64() / bbtree.as_secs_f64()
    );
}

fn main() {
    let orders = std::env::args()
        .nth(1)
        .map_or(1_000_000, |arg| arg.parse().expect("order count"));
    let updates = orders / 10;
    let header = || {
        println!(
            "  {:<30} {:>10} {:>10} {:>9}",
            "workload", "std", "bbtree", "speedup"
        );
    };

    println!("Tree only: {orders} random u64 keys in a bare set, each operation over every key.");
    header();
    let mut rng = Rng(1);
    let keys: Vec<u64> = (0..orders).map(|_| rng.next()).collect();
    let std = bare_set::<BTreeSet<u64>>(&keys);
    let bbtree = bare_set::<BBTreeSet<u64>>(&keys);
    for (op, label) in [
        "insert",
        "lookup (contains)",
        "range (next 10 keys)",
        "full scan (iterate all)",
        "remove",
    ]
    .into_iter()
    .enumerate()
    {
        row(label, std[op], bbtree[op]);
    }

    println!("\nIn the database: {orders} orders, the same 4 secondary indexes on each tree.");
    header();

    let mut on_std = None;
    let load_std = timed(|| on_std = Some(generate(orders, Tree::Std)));
    let mut on_bbtree = None;
    let load_bbtree = timed(|| on_bbtree = Some(generate(orders, Tree::BBTree)));
    let (mut on_std, mut on_bbtree) = (on_std.unwrap(), on_bbtree.unwrap());
    row("load (insert every order)", load_std, load_bbtree);

    for (label, query) in queries() {
        let (std, bbtree) = (on_std.query(&query), on_bbtree.query(&query));
        assert_eq!(std.unwrap().orders, bbtree.unwrap().orders, "{label}");
        row(
            label,
            median(&on_std, &query, 51),
            median(&on_bbtree, &query, 51),
        );
    }

    for (label, latest) in [
        ("10k customers, latest 3", Some(3)),
        ("10k customers, date range", None),
    ] {
        let (mut std_rows, mut bbtree_rows) = (0, 0);
        row(
            label,
            timed(|| std_rows = customer_scans(&on_std, latest)),
            timed(|| bbtree_rows = customer_scans(&on_bbtree, latest)),
        );
        assert_eq!(std_rows, bbtree_rows, "{label}");
    }

    row(
        &format!("{updates} replaces"),
        timed(|| replace_orders(&mut on_std, orders, updates)),
        timed(|| replace_orders(&mut on_bbtree, orders, updates)),
    );

    // Swap each database to the other tree: same records, rebuilt indexes.
    let to_bbtree = timed(|| on_std.set_tree(Tree::BBTree));
    let to_std = timed(|| on_bbtree.set_tree(Tree::Std));
    println!(
        "\nRebuild all indexes in place: std -> bbtree {to_bbtree:.1?}, bbtree -> std {to_std:.1?}"
    );
    for (label, query) in queries() {
        let swapped = on_std.query(&query).unwrap();
        let reference = on_bbtree.query(&query).unwrap();
        assert_eq!(swapped.execution.tree, Tree::BBTree);
        assert_eq!(reference.execution.tree, Tree::Std);
        assert_eq!(swapped.orders, reference.orders, "{label} after swap");
    }
    assert_eq!(
        customer_scans(&on_std, None),
        customer_scans(&on_bbtree, None)
    );
    println!("Every query returns the same rows before and after the swap.");
}
