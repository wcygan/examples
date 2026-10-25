use crate::{
    tree::{CustomerDate, IdSet},
    CustomerId, Database, Error, Order, OrderId, OrderState, ProductId, Tree,
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::{Ordering, Reverse},
    ops::RangeInclusive,
};

/// Filters are ANDed. Without a hint, the planner prefers customer/date, then
/// product/state, then product, then state.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub state: Option<OrderState>,
    pub customer_id: Option<CustomerId>,
    pub product_id: Option<ProductId>,
    pub from: Option<u64>,
    pub through: Option<u64>,
    pub latest: Option<usize>,
    #[serde(default)]
    pub force_scan: bool,
    /// Like MySQL `FORCE INDEX`: choose the access path instead of the planner.
    pub index: Option<IndexHint>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndexHint {
    State,
    Product,
    CustomerDate,
    ProductState,
    /// Merge the product and state ID lists, like MySQL `Using intersect(...)`.
    IntersectProductState,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    PrimaryLookup,
    StateLookup,
    ProductLookup,
    CustomerDateRange,
    ProductStateLookup,
    IndexIntersection,
    FullScan,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Execution {
    pub plan: Plan,
    /// The B-tree implementation that held the secondary indexes.
    pub tree: Tree,
    /// Successful buckets visited, not internal hash probes or B-tree comparisons.
    pub index_keys_visited: usize,
    /// Order IDs read from secondary index buckets.
    pub index_entries_read: usize,
    /// Primary records fetched, including those rejected by residual predicates.
    pub records_examined: usize,
    pub records_returned: usize,
    pub residual_filters: Vec<&'static str>,
    pub sort_required: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct QueryResult {
    pub orders: Vec<Order>,
    pub execution: Execution,
}

/// Where a filtered query gets its candidate records.
#[derive(Clone, Copy)]
enum AccessPath {
    CustomerDate(CustomerId),
    State(OrderState),
    Product(ProductId),
    ProductState(ProductId, OrderState),
    Intersect(ProductId, OrderState),
    FullScan,
}

impl AccessPath {
    fn plan(self) -> Plan {
        match self {
            Self::CustomerDate(_) => Plan::CustomerDateRange,
            Self::State(_) => Plan::StateLookup,
            Self::Product(_) => Plan::ProductLookup,
            Self::ProductState(..) => Plan::ProductStateLookup,
            Self::Intersect(..) => Plan::IndexIntersection,
            Self::FullScan => Plan::FullScan,
        }
    }

    /// Which of (state, product, customer and date) the path guarantees.
    fn covers(self) -> (bool, bool, bool) {
        match self {
            Self::CustomerDate(_) => (false, false, true),
            Self::State(_) => (true, false, false),
            Self::Product(_) => (false, true, false),
            Self::ProductState(..) | Self::Intersect(..) => (true, true, false),
            Self::FullScan => (false, false, false),
        }
    }
}

impl Query {
    fn validate(&self) -> Result<(), Error> {
        if self.placed_at_range().is_empty() {
            return Err(Error::InvalidQuery("from must be <= through"));
        }
        if self.latest.is_some() && self.customer_id.is_none() {
            return Err(Error::InvalidQuery("latest requires customer_id"));
        }
        if self.latest == Some(0) {
            return Err(Error::InvalidQuery("latest must be greater than zero"));
        }
        if self.force_scan && self.index.is_some() {
            return Err(Error::InvalidQuery("force_scan conflicts with index"));
        }
        Ok(())
    }

    /// Inclusive on both ends; a missing bound is unbounded.
    fn placed_at_range(&self) -> RangeInclusive<u64> {
        self.from.unwrap_or(0)..=self.through.unwrap_or(u64::MAX)
    }

    fn access_path(&self) -> Result<AccessPath, Error> {
        let product_state = self.product_id.zip(self.state);
        let hinted = match self.index {
            None if self.force_scan => Some(AccessPath::FullScan),
            None => Some(self.planned_path()),
            Some(IndexHint::State) => self.state.map(AccessPath::State),
            Some(IndexHint::Product) => self.product_id.map(AccessPath::Product),
            Some(IndexHint::CustomerDate) => self.customer_id.map(AccessPath::CustomerDate),
            Some(IndexHint::ProductState) => {
                product_state.map(|(product, state)| AccessPath::ProductState(product, state))
            }
            Some(IndexHint::IntersectProductState) => {
                product_state.map(|(product, state)| AccessPath::Intersect(product, state))
            }
        };
        hinted.ok_or(Error::InvalidQuery(
            "index hint requires the predicates it indexes",
        ))
    }

    fn planned_path(&self) -> AccessPath {
        match (self.customer_id, self.product_id, self.state) {
            (Some(customer), _, _) => AccessPath::CustomerDate(customer),
            (None, Some(product), Some(state)) => AccessPath::ProductState(product, state),
            (None, Some(product), None) => AccessPath::Product(product),
            (None, None, Some(state)) => AccessPath::State(state),
            (None, None, None) => AccessPath::FullScan,
        }
    }

    fn matches(&self, order: &Order) -> bool {
        self.state.is_none_or(|state| order.state == state)
            && self.customer_id.is_none_or(|id| order.customer_id == id)
            && self.product_id.is_none_or(|id| order.product_id == id)
            && self.placed_at_range().contains(&order.placed_at)
    }

    /// Predicates the access path does not already guarantee.
    fn residual_filters(&self, path: AccessPath) -> Vec<&'static str> {
        let (by_state, by_product, by_customer_date) = path.covers();
        [
            ("state", self.state.is_some() && !by_state),
            (
                "customer_id",
                self.customer_id.is_some() && !by_customer_date,
            ),
            (
                "placed_at",
                (self.from.is_some() || self.through.is_some()) && !by_customer_date,
            ),
            ("product_id", self.product_id.is_some() && !by_product),
        ]
        .into_iter()
        .filter_map(|(name, residual)| residual.then_some(name))
        .collect()
    }

    /// Customer queries order by time, newest first for `latest`; others by order ID.
    fn sort(&self, orders: &mut [Order]) {
        let time = |order: &Order| (order.placed_at, order.order_id);
        match (self.customer_id, self.latest) {
            (Some(_), None) => orders.sort_by_key(time),
            (Some(_), Some(_)) => orders.sort_by_key(|order| Reverse(time(order))),
            (None, _) => orders.sort_by_key(|order| order.order_id),
        }
    }
}

/// Candidate records fetched by an access path, with the work counted.
struct Scan<'q> {
    query: &'q Query,
    matches: Vec<Order>,
    index_keys_visited: usize,
    index_entries_read: usize,
    records_examined: usize,
}

impl Scan<'_> {
    fn examine(&mut self, order: &Order) {
        self.records_examined += 1;
        if self.query.matches(order) {
            self.matches.push(order.clone());
        }
    }

    fn has_latest(&self) -> bool {
        self.query.latest == Some(self.matches.len())
    }
}

impl Database {
    pub fn get(&self, id: OrderId) -> Result<QueryResult, Error> {
        let order = self.orders.get(&id).ok_or(Error::Missing(id))?;
        Ok(QueryResult {
            orders: vec![order.clone()],
            execution: Execution {
                plan: Plan::PrimaryLookup,
                tree: self.tree,
                index_keys_visited: 1,
                index_entries_read: 0,
                records_examined: 1,
                records_returned: 1,
                residual_filters: Vec::new(),
                sort_required: false,
            },
        })
    }

    pub fn query(&self, query: &Query) -> Result<QueryResult, Error> {
        query.validate()?;
        let path = query.access_path()?;
        let mut scan = Scan {
            query,
            matches: Vec::new(),
            index_keys_visited: 0,
            index_entries_read: 0,
            records_examined: 0,
        };
        match path {
            AccessPath::CustomerDate(customer) => self.scan_customer_dates(customer, &mut scan),
            AccessPath::State(state) => self.scan_bucket(self.by_state.get(&state), &mut scan),
            AccessPath::Product(product) => {
                self.scan_bucket(self.by_product.get(&product), &mut scan);
            }
            AccessPath::ProductState(product, state) => {
                self.scan_bucket(self.by_product_state.get(&(product, state)), &mut scan);
            }
            AccessPath::Intersect(product, state) => {
                self.scan_intersection(product, state, &mut scan);
            }
            AccessPath::FullScan => self.orders.values().for_each(|order| scan.examine(order)),
        }
        // Customer/date buckets yield time order; other buckets yield order-ID order.
        let sort_required = match path {
            AccessPath::CustomerDate(_) => false,
            AccessPath::FullScan => true,
            _ => query.customer_id.is_some(),
        };
        let mut orders = scan.matches;
        if sort_required {
            query.sort(&mut orders);
        }
        if let Some(limit) = query.latest {
            orders.truncate(limit);
        }
        Ok(QueryResult {
            execution: Execution {
                plan: path.plan(),
                tree: self.tree,
                index_keys_visited: scan.index_keys_visited,
                index_entries_read: scan.index_entries_read,
                records_examined: scan.records_examined,
                records_returned: orders.len(),
                residual_filters: query.residual_filters(path),
                sort_required,
            },
            orders,
        })
    }

    fn scan_bucket(&self, ids: Option<&IdSet>, scan: &mut Scan) {
        let Some(ids) = ids else {
            return;
        };
        scan.index_keys_visited += 1;
        ids.iter().for_each(|id| {
            scan.index_entries_read += 1;
            scan.examine(&self.orders[id]);
        });
    }

    /// Both lists are in ID order, so one merge pass finds the shared IDs before
    /// any primary record is fetched. It reads entries until either list ends.
    fn scan_intersection(&self, product: ProductId, state: OrderState, scan: &mut Scan) {
        let (Some(products), Some(states)) =
            (self.by_product.get(&product), self.by_state.get(&state))
        else {
            return;
        };
        scan.index_keys_visited += 2;
        let (mut products, mut states) = (products.iter().peekable(), states.iter().peekable());
        while let (Some(&&left), Some(&&right)) = (products.peek(), states.peek()) {
            scan.index_entries_read += 1;
            match left.cmp(&right) {
                Ordering::Less => {
                    products.next();
                }
                Ordering::Greater => {
                    states.next();
                }
                Ordering::Equal => {
                    scan.index_entries_read += 1;
                    products.next();
                    states.next();
                    scan.examine(&self.orders[&left]);
                }
            }
        }
    }

    /// Visits customer/date entries oldest first, or newest first until `latest` matches.
    fn scan_customer_dates(&self, customer: CustomerId, scan: &mut Scan) {
        let (from, through) = scan.query.placed_at_range().into_inner();
        let entry = |placed_at, order_id| CustomerDate {
            customer_id: customer,
            placed_at,
            order_id: OrderId(order_id),
        };
        let entries = self
            .by_customer_date
            .range(entry(from, 0)..=entry(through, u64::MAX));
        // Entries sharing a timestamp form one logical key, counted once.
        let mut previous = None;
        let mut read = |scan: &mut Scan, entry: &CustomerDate| {
            if previous.replace(entry.placed_at) != Some(entry.placed_at) {
                scan.index_keys_visited += 1;
            }
            scan.index_entries_read += 1;
            scan.examine(&self.orders[&entry.order_id]);
        };
        if scan.query.latest.is_none() {
            entries.for_each(|entry| read(scan, entry));
            return;
        }
        for entry in entries.rev() {
            read(scan, entry);
            if scan.has_latest() {
                return;
            }
        }
    }
}
