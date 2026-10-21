use conceptual_database::*;

fn order(id: u64, customer: u64, state: OrderState, time: u64) -> Order {
    Order {
        order_id: OrderId(id),
        customer_id: CustomerId(customer),
        state,
        product_id: ProductId(7),
        placed_at: time,
    }
}

fn fixture() -> Database {
    let mut db = Database::default();
    for row in [
        order(1, 42, OrderState::Placed, 0),
        order(2, 42, OrderState::Shipped, 100),
        order(3, 42, OrderState::Placed, 100),
        order(4, 42, OrderState::Placed, u64::MAX),
        order(5, 43, OrderState::Cancelled, 100),
        order(6, 43, OrderState::Shipped, 200),
    ] {
        db.create(row).unwrap();
    }
    db
}

fn ids(result: &QueryResult) -> Vec<u64> {
    result.orders.iter().map(|row| row.order_id.0).collect()
}

#[test]
fn primary_state_and_composite_have_concrete_work_counts() {
    let db = fixture();
    let primary = db.get(OrderId(2)).unwrap();
    assert_eq!(primary.orders, vec![order(2, 42, OrderState::Shipped, 100)]);
    assert_eq!(primary.execution.plan, Plan::PrimaryLookup);
    assert_eq!(primary.execution.records_examined, 1);
    let state = db
        .query(&Query {
            state: Some(OrderState::Placed),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(ids(&state), [1, 3, 4]);
    assert_eq!(state.execution.plan, Plan::StateLookup);
    assert_eq!(state.execution.index_keys_visited, 1);
    assert_eq!(state.execution.records_examined, 3);
    let range = db
        .query(&Query {
            customer_id: Some(CustomerId(42)),
            from: Some(100),
            through: Some(100),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(ids(&range), [2, 3]);
    assert_eq!(range.execution.plan, Plan::CustomerDateRange);
    assert_eq!(range.execution.index_keys_visited, 1);
    assert_eq!(range.execution.records_examined, 2);
    assert!(!range.execution.sort_required);
}

#[test]
fn indexed_and_scan_results_match_across_filters_bounds_and_limits() {
    let db = fixture();
    for customer_id in [
        None,
        Some(CustomerId(42)),
        Some(CustomerId(43)),
        Some(CustomerId(999)),
    ] {
        for state in [
            None,
            Some(OrderState::Placed),
            Some(OrderState::Shipped),
            Some(OrderState::Cancelled),
        ] {
            for (from, through) in [
                (None, None),
                (Some(0), Some(0)),
                (Some(100), Some(100)),
                (Some(u64::MAX), None),
            ] {
                for latest in [None, Some(1), Some(2), Some(20)] {
                    if latest.is_some() && customer_id.is_none() {
                        continue;
                    }
                    for product_id in [None, Some(ProductId(7)), Some(ProductId(999))] {
                        let mut query = Query {
                            customer_id,
                            state,
                            from,
                            through,
                            latest,
                            product_id,
                            force_scan: false,
                        };
                        let indexed = db.query(&query).unwrap();
                        query.force_scan = true;
                        let scan = db.query(&query).unwrap();
                        assert_eq!(indexed.orders, scan.orders, "{query:?}");
                        assert_eq!(scan.execution.records_examined, 6);
                    }
                }
            }
        }
    }
}

#[test]
fn latest_stops_after_matches_with_deterministic_ties_and_residual_filtering() {
    let db = fixture();
    let query = Query {
        customer_id: Some(CustomerId(42)),
        through: Some(100),
        latest: Some(1),
        ..Query::default()
    };
    let latest = db.query(&query).unwrap();
    assert_eq!(ids(&latest), [3]);
    assert_eq!(latest.execution.records_examined, 1);
    assert_eq!(latest.execution.index_keys_visited, 1);
    let filtered = db
        .query(&Query {
            state: Some(OrderState::Shipped),
            ..query
        })
        .unwrap();
    assert_eq!(ids(&filtered), [2]);
    assert_eq!(filtered.execution.records_examined, 2);
    assert_eq!(filtered.execution.residual_filters, ["state"]);
}

#[test]
fn product_and_global_date_queries_need_scans() {
    let db = fixture();
    for query in [
        Query {
            product_id: Some(ProductId(7)),
            ..Query::default()
        },
        Query {
            from: Some(100),
            through: Some(100),
            ..Query::default()
        },
    ] {
        let result = db.query(&query).unwrap();
        assert_eq!(result.execution.plan, Plan::FullScan);
        assert_eq!(result.execution.records_examined, 6);
        assert_eq!(result.execution.index_keys_visited, 0);
    }
    let date = db
        .query(&Query {
            from: Some(100),
            through: Some(100),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(ids(&date), [2, 3, 5]);
}

#[test]
fn replace_and_delete_move_every_index_and_remove_empty_buckets() {
    let mut db = Database::default();
    db.create(order(1, 42, OrderState::Placed, 100)).unwrap();
    let replacement = order(1, 99, OrderState::Shipped, 200);
    db.replace(replacement.clone()).unwrap();
    assert_eq!(
        db.get(OrderId(1)).unwrap().orders.as_slice(),
        std::slice::from_ref(&replacement)
    );
    let snapshot = db.indexes();
    assert_eq!(snapshot.by_state.len(), 1);
    assert_eq!(
        snapshot.by_state[&OrderState::Shipped]
            .iter()
            .map(|id| id.0)
            .collect::<Vec<_>>(),
        [1]
    );
    assert_eq!(snapshot.by_customer_date.len(), 1);
    assert_eq!(snapshot.by_customer_date[0].customer_id, CustomerId(99));
    assert_eq!(snapshot.by_customer_date[0].placed_at, 200);
    assert_eq!(db.check_indexes(), Ok(()));
    assert_eq!(db.delete(OrderId(1)), Ok(replacement));
    assert_eq!(db.get(OrderId(1)).unwrap_err(), Error::Missing(OrderId(1)));
    let snapshot = db.indexes();
    assert!(
        snapshot.primary.is_empty()
            && snapshot.by_state.is_empty()
            && snapshot.by_customer_date.is_empty()
    );
    assert_eq!(db.check_indexes(), Ok(()));
}

#[test]
fn failed_batches_roll_back_creates_replacements_deletes_and_index_changes() {
    let mut db = fixture();
    let before = db.query(&Query::default()).unwrap().orders;
    let result = db.transaction(vec![
        Operation::Create {
            order: order(7, 10, OrderState::Placed, 300),
        },
        Operation::Replace {
            order: order(1, 99, OrderState::Cancelled, 400),
        },
        Operation::Delete {
            order_id: OrderId(2),
        },
        Operation::Create {
            order: order(3, 10, OrderState::Placed, 500),
        },
    ]);
    assert_eq!(result, Err(Error::Duplicate(OrderId(3))));
    assert_eq!(db.query(&Query::default()).unwrap().orders, before);
    assert_eq!(db.check_indexes(), Ok(()));
    assert_eq!(
        db.transaction(vec![
            Operation::Delete {
                order_id: OrderId(1)
            },
            Operation::Create {
                order: order(1, 99, OrderState::Shipped, 600)
            }
        ]),
        Ok(2)
    );
    assert_eq!(
        db.get(OrderId(1)).unwrap().orders,
        [order(1, 99, OrderState::Shipped, 600)]
    );
    assert_eq!(db.check_indexes(), Ok(()));
    assert_eq!(
        db.replace(order(999, 1, OrderState::Placed, 0)),
        Err(Error::Missing(OrderId(999)))
    );
}

#[test]
fn invalid_queries_return_errors_instead_of_panicking() {
    let db = fixture();
    assert_eq!(
        db.query(&Query {
            from: Some(2),
            through: Some(1),
            ..Query::default()
        })
        .unwrap_err(),
        Error::InvalidQuery("from must be <= through")
    );
    assert_eq!(
        db.query(&Query {
            latest: Some(1),
            ..Query::default()
        })
        .unwrap_err(),
        Error::InvalidQuery("latest requires customer_id")
    );
    assert_eq!(
        db.query(&Query {
            latest: Some(0),
            customer_id: Some(CustomerId(42)),
            ..Query::default()
        })
        .unwrap_err(),
        Error::InvalidQuery("latest must be greater than zero")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_batches_serialize_without_losing_index_consistency() {
    let shared = std::sync::Arc::new(tokio::sync::RwLock::new(Database::default()));
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = Vec::new();
    for customer in [42, 99] {
        let shared = shared.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            shared.write().await.transaction(vec![
                Operation::Create {
                    order: order(1, customer, OrderState::Placed, 100),
                },
                Operation::Create {
                    order: order(2, customer, OrderState::Shipped, 200),
                },
            ])
        }));
    }
    let results = [
        tasks.remove(0).await.unwrap(),
        tasks.remove(0).await.unwrap(),
    ];
    assert_eq!(results.iter().filter(|r| **r == Ok(2)).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == Err(Error::Duplicate(OrderId(1))))
            .count(),
        1
    );
    let db = shared.read().await;
    let rows = db.query(&Query::default()).unwrap().orders;
    assert_eq!(
        rows.iter().map(|o| o.order_id.0).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(rows[0].customer_id, rows[1].customer_id);
    assert_eq!(db.check_indexes(), Ok(()));
}
