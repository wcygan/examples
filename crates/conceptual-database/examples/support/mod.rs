//! The generated order table shared by the benchmark examples.

use conceptual_database::*;
use std::{hint::black_box, time::Duration, time::Instant};

pub const PRODUCTS: u64 = 1_000;
pub const CUSTOMERS: u64 = 100_000;
pub const HOT_PRODUCT: ProductId = ProductId(0);

/// Deterministic splitmix64, so every run builds the same table.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// 10% of orders are for one hot product; states are 90% shipped, 8% placed, 2% cancelled.
pub fn generate(orders: u64, tree: Tree) -> Database {
    let mut rng = Rng(42);
    let mut db = Database::with_tree(tree);
    for id in 0..orders {
        let product = if rng.next().is_multiple_of(10) {
            HOT_PRODUCT
        } else {
            ProductId(1 + rng.next() % (PRODUCTS - 1))
        };
        let state = match rng.next() % 100 {
            0..=1 => OrderState::Cancelled,
            2..=9 => OrderState::Placed,
            _ => OrderState::Shipped,
        };
        db.create(Order {
            order_id: OrderId(id),
            customer_id: CustomerId(rng.next() % CUSTOMERS),
            state,
            product_id: product,
            placed_at: id,
        })
        .unwrap();
    }
    db
}

/// Median of repeated runs.
pub fn median(db: &Database, query: &Query, runs: usize) -> Duration {
    let mut samples: Vec<_> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            black_box(db.query(black_box(query)).unwrap());
            start.elapsed()
        })
        .collect();
    samples.sort();
    samples[runs / 2]
}
