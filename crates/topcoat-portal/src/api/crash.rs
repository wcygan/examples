use serde::Deserialize;
use topcoat_core::error::Result;
use topcoat_portal::{NodeOperationResult, runtime};
use topcoat_router::content::Json;
use topcoat_router_macro::route;

#[derive(Deserialize)]
struct Input {
    shard: String,
}
#[route(POST "/api/crash")]
pub async fn crash(Json(input): Json<Input>) -> Result<Json<NodeOperationResult>> {
    let id = runtime()
        .crash_follower(&input.shard)
        .await
        .map_err(|e| topcoat_core::error::Error::from(std::io::Error::other(e)))?;
    Ok(Json(NodeOperationResult {
        node_id: Some(id),
        detail: format!(
            "actual child process {id} killed; its node remains a voter and is not restarted because its store is volatile"
        ),
    }))
}
