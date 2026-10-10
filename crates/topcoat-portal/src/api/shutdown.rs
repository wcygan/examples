use topcoat_core::error::Result;
use topcoat_portal::signal_shutdown;
use topcoat_router_macro::route;

#[route(POST "/api/shutdown")]
pub async fn shutdown() -> Result<&'static str> {
    signal_shutdown();
    Ok("shutting down")
}
