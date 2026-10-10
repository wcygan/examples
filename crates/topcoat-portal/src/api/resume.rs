use topcoat_core::error::Result;
use topcoat_router_macro::route;

#[route(POST "/api/resume")]
pub async fn resume() -> Result<String> {
    topcoat_portal::runtime().resume().await;
    Ok("resumed".into())
}
