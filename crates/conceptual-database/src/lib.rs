//! An in-memory order table with explicit index access paths and atomic batches.
pub mod database;
pub mod http;
pub mod query;
mod tree;

pub use database::{CustomerId, Database, Error, Operation, Order, OrderId, OrderState, ProductId};
pub use query::{IndexHint, Plan, Query, QueryResult};
pub use tree::Tree;
