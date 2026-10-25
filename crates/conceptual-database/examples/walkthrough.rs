use conceptual_database::*;

fn main() {
    let mut db = Database::default();
    for (id, customer, state, time) in [
        (101, 42, OrderState::Placed, 100),
        (102, 42, OrderState::Shipped, 200),
        (103, 99, OrderState::Placed, 300),
        (104, 42, OrderState::Placed, 400),
    ] {
        db.create(Order {
            order_id: OrderId(id),
            customer_id: CustomerId(customer),
            state,
            product_id: ProductId(7),
            placed_at: time,
        })
        .unwrap();
    }
    println!(
        "Primary lookup:\n{}",
        serde_json::to_string_pretty(&db.get(OrderId(101)).unwrap()).unwrap()
    );
    for (label, mut query) in [
        (
            "Awaiting shipment",
            Query {
                state: Some(OrderState::Placed),
                ..Query::default()
            },
        ),
        (
            "Customer history",
            Query {
                customer_id: Some(CustomerId(42)),
                from: Some(100),
                through: Some(300),
                ..Query::default()
            },
        ),
        (
            "Customer's latest order",
            Query {
                customer_id: Some(CustomerId(42)),
                latest: Some(1),
                ..Query::default()
            },
        ),
        (
            "Product index",
            Query {
                product_id: Some(ProductId(7)),
                ..Query::default()
            },
        ),
    ] {
        let indexed = db.query(&query).unwrap();
        query.force_scan = true;
        let scan = db.query(&query).unwrap();
        assert_eq!(indexed.orders, scan.orders);
        println!(
            "{label}:\n{}",
            serde_json::to_string_pretty(&indexed).unwrap()
        );
        println!(
            "Forced scan examined {} records\n",
            scan.execution.records_examined
        );
    }
    demonstrate_rollback(&mut db);
}

fn demonstrate_rollback(db: &mut Database) {
    let before = db.query(&Query::default()).unwrap().orders;
    let failure = db.transaction(vec![
        Operation::Delete {
            order_id: OrderId(101),
        },
        Operation::Create {
            order: before[1].clone(),
        },
    ]);
    assert_eq!(failure, Err(Error::Duplicate(OrderId(102))));
    assert_eq!(before, db.query(&Query::default()).unwrap().orders);
    assert_eq!(db.check_indexes(), Ok(()));
    println!("Failed batch rolled back: {failure:?}");
    println!(
        "Index contents:\n{}",
        serde_json::to_string_pretty(&db.indexes()).unwrap()
    );
}
