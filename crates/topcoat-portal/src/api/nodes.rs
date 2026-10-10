use serde::Deserialize;
use topcoat_core::error::Result;
use topcoat_portal::{NodeOperationResult, runtime};
use topcoat_router::content::Json;
use topcoat_router_macro::route;

#[derive(Deserialize)]
struct Input {
    op: String,
    shard: Option<String>,
    node_id: Option<u64>,
}
#[route(POST "/api/nodes")]
pub async fn nodes(Json(input): Json<Input>) -> Result<Json<NodeOperationResult>> {
    let (id, detail) = match input.op.as_str() {
        "add" => {
            let shard = input.shard.as_deref().unwrap_or("source");
            let id = runtime()
                .add_node(shard)
                .await
                .map_err(|e| topcoat_core::error::Error::from(std::io::Error::other(e)))?;
            (Some(id), format!("added voter {id} to {shard}"))
        }
        "remove" => {
            let id = input.node_id.ok_or_else(|| {
                topcoat_core::error::Error::from(std::io::Error::other("node_id is required"))
            })?;
            runtime()
                .remove_node(id)
                .await
                .map_err(|e| topcoat_core::error::Error::from(std::io::Error::other(e)))?;
            (Some(id), format!("removed node {id}"))
        }
        _ => {
            return Err(topcoat_core::error::Error::from(std::io::Error::other(
                "op must be add or remove",
            )));
        }
    };
    Ok(Json(NodeOperationResult {
        node_id: id,
        detail,
    }))
}
