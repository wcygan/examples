use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct OrderId(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct CustomerId(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct ProductId(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OrderState {
    Placed,
    Shipped,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Order {
    pub order_id: OrderId,
    pub customer_id: CustomerId,
    pub state: OrderState,
    pub product_id: ProductId,
    /// Unix seconds; all date range bounds are inclusive.
    pub placed_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Duplicate(OrderId),
    Missing(OrderId),
    InvalidQuery(&'static str),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Create { order: Order },
    Replace { order: Order },
    Delete { order_id: OrderId },
}

#[derive(Clone, Default)]
pub struct Database {
    pub(crate) orders: HashMap<OrderId, Order>,
    pub(crate) by_state: HashMap<OrderState, HashSet<OrderId>>,
    pub(crate) by_customer_date: BTreeMap<(CustomerId, u64), BTreeSet<OrderId>>,
}

#[derive(Debug, Serialize)]
pub struct IndexSnapshot {
    pub primary: Vec<Order>,
    pub by_state: BTreeMap<OrderState, BTreeSet<OrderId>>,
    pub by_customer_date: Vec<CustomerDateEntry>,
}

#[derive(Debug, Serialize)]
pub struct CustomerDateEntry {
    pub customer_id: CustomerId,
    pub placed_at: u64,
    pub order_ids: BTreeSet<OrderId>,
}

impl Database {
    pub fn create(&mut self, order: Order) -> Result<(), Error> {
        if self.orders.contains_key(&order.order_id) {
            return Err(Error::Duplicate(order.order_id));
        }
        self.insert(order);
        Ok(())
    }

    pub fn replace(&mut self, order: Order) -> Result<(), Error> {
        self.delete(order.order_id)?;
        self.insert(order);
        Ok(())
    }

    fn insert(&mut self, order: Order) {
        self.by_state
            .entry(order.state)
            .or_default()
            .insert(order.order_id);
        self.by_customer_date
            .entry((order.customer_id, order.placed_at))
            .or_default()
            .insert(order.order_id);
        self.orders.insert(order.order_id, order);
    }

    pub fn delete(&mut self, id: OrderId) -> Result<Order, Error> {
        let old = self.orders.remove(&id).ok_or(Error::Missing(id))?;
        let ids = self
            .by_state
            .get_mut(&old.state)
            .expect("state index invariant");
        ids.remove(&id);
        if ids.is_empty() {
            self.by_state.remove(&old.state);
        }
        let key = (old.customer_id, old.placed_at);
        let ids = self
            .by_customer_date
            .get_mut(&key)
            .expect("composite index invariant");
        ids.remove(&id);
        if ids.is_empty() {
            self.by_customer_date.remove(&key);
        }
        Ok(old)
    }

    /// Clone, apply in request order, and publish only on success. No external effects.
    pub fn transaction(&mut self, operations: Vec<Operation>) -> Result<usize, Error> {
        let mut candidate = self.clone();
        let applied = operations.len();
        for operation in operations {
            match operation {
                Operation::Create { order } => candidate.create(order)?,
                Operation::Replace { order } => candidate.replace(order)?,
                Operation::Delete { order_id } => {
                    candidate.delete(order_id)?;
                }
            }
        }
        *self = candidate;
        Ok(applied)
    }

    /// Rebuild both secondary indexes from the primary records and compare exactly.
    pub fn check_indexes(&self) -> Result<(), String> {
        let mut expected = Self::default();
        for (id, order) in &self.orders {
            if *id != order.order_id {
                return Err("primary key does not match order_id".into());
            }
            expected.insert(order.clone());
        }
        if self.by_state != expected.by_state {
            return Err("state index differs from primary records".into());
        }
        if self.by_customer_date != expected.by_customer_date {
            return Err("customer/date index differs from primary records".into());
        }
        Ok(())
    }

    pub fn indexes(&self) -> IndexSnapshot {
        let mut primary: Vec<_> = self.orders.values().cloned().collect();
        primary.sort_by_key(|order| order.order_id);
        IndexSnapshot {
            primary,
            by_state: self
                .by_state
                .iter()
                .map(|(state, ids)| (*state, ids.iter().copied().collect()))
                .collect(),
            by_customer_date: self
                .by_customer_date
                .iter()
                .map(|((customer_id, placed_at), ids)| CustomerDateEntry {
                    customer_id: *customer_id,
                    placed_at: *placed_at,
                    order_ids: ids.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checker_detects_missing_stale_and_dangling_entries() {
        let order = Order {
            order_id: OrderId(1),
            customer_id: CustomerId(42),
            state: OrderState::Placed,
            product_id: ProductId(7),
            placed_at: 100,
        };
        let mut db = Database::default();
        db.create(order).unwrap();
        assert_eq!(db.check_indexes(), Ok(()));
        let mut missing = db.clone();
        missing.by_state.clear();
        assert_eq!(
            missing.check_indexes(),
            Err("state index differs from primary records".into())
        );
        let mut stale = db.clone();
        stale
            .by_customer_date
            .insert((CustomerId(42), 99), BTreeSet::from([OrderId(1)]));
        assert_eq!(
            stale.check_indexes(),
            Err("customer/date index differs from primary records".into())
        );
        db.by_state
            .entry(OrderState::Placed)
            .or_default()
            .insert(OrderId(999));
        assert_eq!(
            db.check_indexes(),
            Err("state index differs from primary records".into())
        );
    }
}
