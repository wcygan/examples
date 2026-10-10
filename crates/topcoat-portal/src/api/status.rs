use topcoat_core::error::Result;
use topcoat_router::content::Json;
use topcoat_router_macro::route;

#[route(GET "/api/status")]
pub async fn status() -> Result<Json<topcoat_portal::PortalStatus>> {
    Ok(Json(topcoat_portal::runtime().status().await))
}
