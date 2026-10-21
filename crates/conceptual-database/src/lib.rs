//! An in-memory order table with explicit index access paths and atomic batches.
pub mod database;
pub mod http;
pub mod query;

pub use database::{CustomerId, Database, Error, Operation, Order, OrderId, OrderState, ProductId};
pub use query::{Plan, Query, QueryResult};
