use crate::tree::{CustomerDate, CustomerDateIndex, IdSet, Tree};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    hash::Hash,
    mem,
};

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
    pub(crate) tree: Tree,
    pub(crate) orders: HashMap<OrderId, Order>,
    // Like InnoDB secondary indexes, IDs within a bucket stay in primary-key order.
    pub(crate) by_state: HashMap<OrderState, IdSet>,
    pub(crate) by_product: HashMap<ProductId, IdSet>,
    pub(crate) by_customer_date: CustomerDateIndex,
    pub(crate) by_product_state: HashMap<(ProductId, OrderState), IdSet>,
}

#[derive(Debug, Serialize)]
pub struct IndexSnapshot {
    pub tree: Tree,
    pub primary: Vec<Order>,
    pub by_state: BTreeMap<OrderState, BTreeSet<OrderId>>,
    pub by_product: BTreeMap<ProductId, BTreeSet<OrderId>>,
    pub by_customer_date: Vec<CustomerDateEntry>,
    pub by_product_state: Vec<ProductStateEntry>,
}

#[derive(Debug, Serialize)]
pub struct CustomerDateEntry {
    pub customer_id: CustomerId,
    pub placed_at: u64,
    pub order_ids: BTreeSet<OrderId>,
}

#[derive(Debug, Serialize)]
pub struct ProductStateEntry {
    pub product_id: ProductId,
    pub state: OrderState,
    pub order_ids: BTreeSet<OrderId>,
}

impl Database {
    /// An empty database whose secondary indexes use `tree`.
    pub fn with_tree(tree: Tree) -> Self {
        Self {
            tree,
            by_customer_date: CustomerDateIndex::new(tree),
            ..Self::default()
        }
    }

    pub fn tree(&self) -> Tree {
        self.tree
    }

    /// Rebuild every secondary index on `tree` from the primary records, like an
    /// offline index rebuild. Records and query results do not change.
    pub fn set_tree(&mut self, tree: Tree) {
        if tree == self.tree {
            return;
        }
        let mut orders: Vec<_> = mem::take(&mut self.orders).into_values().collect();
        orders.sort_unstable_by_key(|order| order.order_id);
        *self = Self::with_tree(tree);
        for order in orders {
            self.insert(order);
        }
    }

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
        let (id, tree) = (order.order_id, self.tree);
        let new_set = || IdSet::new(tree);
        self.by_state
            .entry(order.state)
            .or_insert_with(new_set)
            .insert(id);
        self.by_product
            .entry(order.product_id)
            .or_insert_with(new_set)
            .insert(id);
        self.by_product_state
            .entry((order.product_id, order.state))
            .or_insert_with(new_set)
            .insert(id);
        self.by_customer_date.insert(customer_date(&order));
        self.orders.insert(order.order_id, order);
    }

    pub fn delete(&mut self, id: OrderId) -> Result<Order, Error> {
        let old = self.orders.remove(&id).ok_or(Error::Missing(id))?;
        remove_id(&mut self.by_state, old.state, id);
        remove_id(&mut self.by_product, old.product_id, id);
        remove_id(&mut self.by_product_state, (old.product_id, old.state), id);
        let removed = self.by_customer_date.remove(customer_date(&old));
        assert!(removed, "composite index invariant");
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

    /// Rebuild every secondary index from the primary records and compare exactly.
    pub fn check_indexes(&self) -> Result<(), String> {
        let mut buckets = self
            .by_state
            .values()
            .chain(self.by_product.values())
            .chain(self.by_product_state.values());
        if self.by_customer_date.tree() != self.tree || buckets.any(|ids| ids.tree() != self.tree) {
            return Err("an index uses a different tree than the database".into());
        }
        let mut expected = Self::with_tree(self.tree);
        for (id, order) in &self.orders {
            if *id != order.order_id {
                return Err("primary key does not match order_id".into());
            }
            expected.insert(order.clone());
        }
        if self.by_state != expected.by_state {
            return Err("state index differs from primary records".into());
        }
        if self.by_product != expected.by_product {
            return Err("product index differs from primary records".into());
        }
        if self.by_customer_date != expected.by_customer_date {
            return Err("customer/date index differs from primary records".into());
        }
        if self.by_product_state != expected.by_product_state {
            return Err("product/state index differs from primary records".into());
        }
        Ok(())
    }

    pub fn indexes(&self) -> IndexSnapshot {
        let mut primary: Vec<_> = self.orders.values().cloned().collect();
        primary.sort_by_key(|order| order.order_id);
        IndexSnapshot {
            tree: self.tree,
            primary,
            by_state: self
                .by_state
                .iter()
                .map(|(state, ids)| (*state, ids.iter().copied().collect()))
                .collect(),
            by_product: self
                .by_product
                .iter()
                .map(|(product, ids)| (*product, ids.iter().copied().collect()))
                .collect(),
            by_customer_date: self.by_customer_date.iter().fold(
                Vec::<CustomerDateEntry>::new(),
                |mut entries, entry| {
                    match entries.last_mut() {
                        Some(last)
                            if (last.customer_id, last.placed_at)
                                == (entry.customer_id, entry.placed_at) =>
                        {
                            last.order_ids.insert(entry.order_id);
                        }
                        _ => entries.push(CustomerDateEntry {
                            customer_id: entry.customer_id,
                            placed_at: entry.placed_at,
                            order_ids: BTreeSet::from([entry.order_id]),
                        }),
                    }
                    entries
                },
            ),
            by_product_state: {
                let mut entries: Vec<_> = self
                    .by_product_state
                    .iter()
                    .map(|((product_id, state), ids)| ProductStateEntry {
                        product_id: *product_id,
                        state: *state,
                        order_ids: ids.iter().copied().collect(),
                    })
                    .collect();
                entries.sort_by_key(|entry| (entry.product_id, entry.state));
                entries
            },
        }
    }
}

fn customer_date(order: &Order) -> CustomerDate {
    CustomerDate {
        customer_id: order.customer_id,
        placed_at: order.placed_at,
        order_id: order.order_id,
    }
}

fn remove_id<K: Eq + Hash>(index: &mut HashMap<K, IdSet>, key: K, id: OrderId) {
    let ids = index.get_mut(&key).expect("secondary index invariant");
    ids.remove(id);
    if ids.is_empty() {
        index.remove(&key);
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
        db.create(order.clone()).unwrap();
        assert_eq!(db.check_indexes(), Ok(()));
        let mut missing = db.clone();
        missing.by_state.clear();
        assert_eq!(
            missing.check_indexes(),
            Err("state index differs from primary records".into())
        );
        let mut stale = db.clone();
        stale.by_customer_date.insert(CustomerDate {
            customer_id: CustomerId(42),
            placed_at: 99,
            order_id: OrderId(1),
        });
        assert_eq!(
            stale.check_indexes(),
            Err("customer/date index differs from primary records".into())
        );
        let mut mixed = db.clone();
        let mut ids = IdSet::new(Tree::BBTree);
        ids.insert(OrderId(1));
        mixed.by_state.insert(OrderState::Placed, ids);
        assert_eq!(
            mixed.check_indexes(),
            Err("an index uses a different tree than the database".into())
        );
        db.by_state
            .get_mut(&OrderState::Placed)
            .unwrap()
            .insert(OrderId(999));
        assert_eq!(
            db.check_indexes(),
            Err("state index differs from primary records".into())
        );
        let mut swapped = Database::with_tree(Tree::BBTree);
        swapped.create(order).unwrap();
        assert_eq!(swapped.check_indexes(), Ok(()));
    }
}
