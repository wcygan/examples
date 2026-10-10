use serde::Deserialize;
use topcoat_core::error::Result;
use topcoat_portal::TransferResult;
use topcoat_router::content::Json;
use topcoat_router_macro::route;

#[derive(Deserialize)]
struct Input {
    tx_id: String,
}
#[route(POST "/api/recover")]
pub async fn recover(Json(input): Json<Input>) -> Result<Json<TransferResult>> {
    Ok(Json(topcoat_portal::runtime().recover(&input.tx_id).await))
}
