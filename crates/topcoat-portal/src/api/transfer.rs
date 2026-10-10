use topcoat_core::error::Result;
use topcoat_portal::{TransferRequest, TransferResult};
use topcoat_router::content::Json;
use topcoat_router_macro::route;

#[route(POST "/api/transfer")]
pub async fn transfer(Json(input): Json<TransferRequest>) -> Result<Json<TransferResult>> {
    Ok(Json(topcoat_portal::runtime().transfer(input).await))
}
