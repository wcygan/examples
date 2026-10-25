use crate::{Database, Error, Operation, Order, OrderId, Query, Tree};
use axum::{
    extract::{Path, Query as QueryParams, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SharedDatabase = Arc<RwLock<Database>>;

pub fn router(database: SharedDatabase) -> Router {
    Router::new()
        .route("/orders", get(find).post(create))
        .route("/orders/{id}", get(lookup).put(replace).delete(delete))
        .route("/transactions", post(transaction))
        .route("/debug/indexes", get(indexes))
        .route("/debug/check", get(check))
        .route("/debug/tree", get(tree).put(set_tree))
        .with_state(database)
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Duplicate(id) => (
                StatusCode::CONFLICT,
                format!("order {} already exists", id.0),
            ),
            Self::Missing(id) => (
                StatusCode::NOT_FOUND,
                format!("order {} does not exist", id.0),
            ),
            Self::InvalidQuery(message) => (StatusCode::BAD_REQUEST, message.to_owned()),
        };
        (status, Json(json!({"error": message}))).into_response()
    }
}

async fn create(
    State(db): State<SharedDatabase>,
    Json(order): Json<Order>,
) -> Result<impl IntoResponse, Error> {
    let id = order.order_id;
    db.write().await.create(order)?;
    Ok((StatusCode::CREATED, Json(json!({"order_id": id}))))
}

async fn lookup(
    State(db): State<SharedDatabase>,
    Path(id): Path<OrderId>,
) -> Result<impl IntoResponse, Error> {
    let result = db.read().await.get(id)?;
    Ok(Json(result))
}

async fn find(
    State(db): State<SharedDatabase>,
    QueryParams(query): QueryParams<Query>,
) -> Result<impl IntoResponse, Error> {
    let result = db.read().await.query(&query)?;
    Ok(Json(result))
}

async fn replace(
    State(db): State<SharedDatabase>,
    Path(id): Path<OrderId>,
    Json(order): Json<Order>,
) -> Result<StatusCode, Error> {
    if id != order.order_id {
        return Err(Error::InvalidQuery("path ID must match body order_id"));
    }
    db.write().await.replace(order)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete(
    State(db): State<SharedDatabase>,
    Path(id): Path<OrderId>,
) -> Result<StatusCode, Error> {
    db.write().await.delete(id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    operations: Vec<Operation>,
}

async fn transaction(
    State(db): State<SharedDatabase>,
    Json(batch): Json<Batch>,
) -> Result<impl IntoResponse, Error> {
    let applied = db.write().await.transaction(batch.operations)?;
    Ok(Json(json!({"applied": applied})))
}

async fn indexes(State(db): State<SharedDatabase>) -> impl IntoResponse {
    let snapshot = db.read().await.indexes();
    Json(snapshot)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeChoice {
    tree: Tree,
}

async fn tree(State(db): State<SharedDatabase>) -> impl IntoResponse {
    let tree = db.read().await.tree();
    Json(json!({"tree": tree}))
}

/// Rebuilds every secondary index on the chosen tree while holding the write lock.
async fn set_tree(
    State(db): State<SharedDatabase>,
    Json(choice): Json<TreeChoice>,
) -> impl IntoResponse {
    db.write().await.set_tree(choice.tree);
    Json(json!({"tree": choice.tree}))
}

async fn check(State(db): State<SharedDatabase>) -> impl IntoResponse {
    let result = db.read().await.check_indexes();
    match result {
        Ok(()) => (StatusCode::OK, Json(json!({"consistent": true}))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"consistent": false, "error": error})),
        ),
    }
}
