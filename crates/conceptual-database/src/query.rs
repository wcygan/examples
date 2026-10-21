use crate::{CustomerId, Database, Error, Order, OrderId, OrderState, ProductId};
use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, ops::RangeInclusive};

/// Filters are ANDed. A customer predicate selects the composite index before state.
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
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    PrimaryLookup,
    StateLookup,
    CustomerDateRange,
    FullScan,
}

#[derive(Debug, Serialize)]
pub struct Execution {
    pub plan: Plan,
    /// Successful buckets visited, not internal hash probes or B-tree comparisons.
    pub index_keys_visited: usize,
    /// Primary records fetched, including those rejected by residual predicates.
    pub records_examined: usize,
    pub records_returned: usize,
    pub residual_filters: Vec<&'static str>,
    pub sort_required: bool,
}

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub orders: Vec<Order>,
    pub execution: Execution,
}

/// Where a filtered query gets its candidate records.
#[derive(Clone, Copy)]
enum AccessPath {
    CustomerDate(CustomerId),
    State(OrderState),
    FullScan,
}

impl AccessPath {
    fn plan(self) -> Plan {
        match self {
            Self::CustomerDate(_) => Plan::CustomerDateRange,
            Self::State(_) => Plan::StateLookup,
            Self::FullScan => Plan::FullScan,
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
        Ok(())
    }

    /// Inclusive on both ends; a missing bound is unbounded.
    fn placed_at_range(&self) -> RangeInclusive<u64> {
        self.from.unwrap_or(0)..=self.through.unwrap_or(u64::MAX)
    }

    fn access_path(&self) -> AccessPath {
        match (self.force_scan, self.customer_id, self.state) {
            (true, _, _) => AccessPath::FullScan,
            (false, Some(customer), _) => AccessPath::CustomerDate(customer),
            (false, None, Some(state)) => AccessPath::State(state),
            (false, None, None) => AccessPath::FullScan,
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
        let by_state = matches!(path, AccessPath::State(_));
        let by_customer_date = matches!(path, AccessPath::CustomerDate(_));
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
            ("product_id", self.product_id.is_some()),
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
                index_keys_visited: 1,
                records_examined: 1,
                records_returned: 1,
                residual_filters: Vec::new(),
                sort_required: false,
            },
        })
    }

    pub fn query(&self, query: &Query) -> Result<QueryResult, Error> {
        query.validate()?;
        let path = query.access_path();
        let mut scan = Scan {
            query,
            matches: Vec::new(),
            index_keys_visited: 0,
            records_examined: 0,
        };
        match path {
            AccessPath::CustomerDate(customer) => self.scan_customer_dates(customer, &mut scan),
            AccessPath::State(state) => self.scan_state(state, &mut scan),
            AccessPath::FullScan => self.orders.values().for_each(|order| scan.examine(order)),
        }
        // The composite index already yields customer rows in time order.
        let sort_required = !matches!(path, AccessPath::CustomerDate(_));
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
                index_keys_visited: scan.index_keys_visited,
                records_examined: scan.records_examined,
                records_returned: orders.len(),
                residual_filters: query.residual_filters(path),
                sort_required,
            },
            orders,
        })
    }

    fn scan_state(&self, state: OrderState, scan: &mut Scan) {
        let Some(ids) = self.by_state.get(&state) else {
            return;
        };
        scan.index_keys_visited += 1;
        for id in ids {
            scan.examine(&self.orders[id]);
        }
    }

    /// Visits customer/date buckets oldest first, or newest first until `latest` matches.
    fn scan_customer_dates(&self, customer: CustomerId, scan: &mut Scan) {
        let (from, through) = scan.query.placed_at_range().into_inner();
        let buckets = self
            .by_customer_date
            .range((customer, from)..=(customer, through));
        if scan.query.latest.is_none() {
            for (_, ids) in buckets {
                scan.index_keys_visited += 1;
                for id in ids {
                    scan.examine(&self.orders[id]);
                }
            }
            return;
        }
        for (_, ids) in buckets.rev() {
            scan.index_keys_visited += 1;
            for id in ids.iter().rev() {
                scan.examine(&self.orders[id]);
                if scan.has_latest() {
                    return;
                }
            }
        }
    }
}
