use std::{net::SocketAddr, path::PathBuf};
use tokio::net::TcpListener;
use topcoat_core::error::Result;
use topcoat_portal::PortalRuntime;
use topcoat_router::{Router, RouterService};
use topcoat_router_macro::route;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut port = 0u16;
    let mut journal = PathBuf::from("./portal-data/coordinator.jsonl");
    let mut raft_bin = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().ok_or("--port requires a value")?.parse()?,
            "--journal" => journal = PathBuf::from(args.next().ok_or("--journal requires a path")?),
            "--raft-bin" => {
                raft_bin = Some(PathBuf::from(
                    args.next().ok_or("--raft-bin requires a path")?,
                ))
            }
            _ => return Err(format!("unknown argument {a}").into()),
        }
    }
    let bin = raft_bin.unwrap_or_else(|| {
        std::env::current_exe()
            .unwrap()
            .with_file_name("raft-sharded-tx")
    });
    let rt =
        PortalRuntime::start(bin.to_str().ok_or("non-UTF8 raft binary path")?, journal).await?;
    topcoat_portal::set_runtime(rt.clone()).map_err(|_| "duplicate portal state")?;
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let addr: SocketAddr = listener.local_addr()?;
    println!("PORTAL_READY http://{addr}");
    let router = Router::builder()
        .route(home)
        .route(api::status::status)
        .route(api::transfer::transfer)
        .route(api::recover::recover)
        .route(api::resume::resume)
        .route(api::events::events)
        .route(api::nodes::nodes)
        .route(api::crash::crash)
        .route(api::shutdown::shutdown)
        .build();
    topcoat_router::internal_serve(listener, RouterService::new(router), async {
        tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = topcoat_portal::wait_for_shutdown()=>{}}
    })
    .await?;
    rt.stop_all().await;
    Ok(())
}

#[route(GET "/")]
pub async fn home() -> Result<topcoat_router::content::Html<String>> {
    Ok(topcoat_router::content::Html(r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Two-shard 2PC lab</title><style>body{font:16px system-ui;max-width:980px;margin:2rem auto;padding:0 1rem;color:#18212b}button{padding:.6rem 1rem;margin:.25rem}pre{background:#f3f5f7;padding:1rem;white-space:pre-wrap}.grid{display:grid;grid-template-columns:1fr 1fr;gap:1rem}.card{border:1px solid #ccd3da;border-radius:8px;padding:1rem}</style></head><body><h1>Two-shard two-phase commit</h1><p>Six local OpenRaft child processes form two independent three-node groups. The portal records its decision before sending phase two.</p><div class="grid"><section class="card"><h2>Transfer</h2><label>Amount <input id="amount" type="number" min="1" max="50" value="10"></label><button onclick="run('none')">Commit</button><button onclick="run('no_vote')">Inject a NO vote</button><button onclick="run('missing_vote')">Drop a vote</button><button onclick="run('pause_after_prepare')">Pause after YES votes</button><button onclick="run('partial_decision_delivery')">Withhold destination decision</button><button onclick="recover()">Recover / resend</button><button onclick="status()">Refresh status</button><button onclick="resume()">Resume paused coordinator</button></section><section class="card"><h2>Node lifecycle</h2><button onclick="nodeOp('add','source')">Add source voter</button><button onclick="nodeOp('add','destination')">Add destination voter</button><label>Remove ID <input id="nodeId" type="number" min="1"></label><button onclick="removeNode()">Remove node</button><button onclick="crash('source')">Kill a source follower process</button><button onclick="crash('destination')">Kill a destination follower process</button><p>Removal updates OpenRaft membership before stopping a node. Crash buttons kill only a local demo child and leave membership unchanged.</p></section><section class="card"><h2>State</h2><pre id="state">Loading…</pre></section></div><h2>Event stream</h2><pre id="events"></pre><script>const log=document.querySelector('#events');const es=new EventSource('/api/events');es.onmessage=e=>{log.textContent+=e.lastEventId+' '+e.data+'\n';log.scrollTop=log.scrollHeight};es.addEventListener('snapshot',e=>{log.textContent+='snapshot '+e.data+'\n'});async function status(){const r=await fetch('/api/status');document.querySelector('#state').textContent=JSON.stringify(await r.json(),null,2)}async function run(fault){const tx='browser-'+Date.now();const r=await fetch('/api/transfer',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({tx_id:tx,amount:Number(document.querySelector('#amount').value),fault})});document.querySelector('#state').textContent=JSON.stringify(await r.json(),null,2);await status()}async function recover(){const tx=prompt('Transaction id to recover');if(tx){const r=await fetch('/api/recover',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({tx_id:tx})});document.querySelector('#state').textContent=JSON.stringify(await r.json(),null,2);await status()}}async function resume(){await fetch('/api/resume',{method:'POST'});await status()}async function nodeOp(op,shard,node_id){const r=await fetch('/api/nodes',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({op,shard,node_id})});document.querySelector('#state').textContent=JSON.stringify(await r.json(),null,2);await status()}async function removeNode(){await nodeOp('remove',null,Number(document.querySelector('#nodeId').value))}async function crash(shard){const r=await fetch('/api/crash',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({shard})});document.querySelector('#state').textContent=JSON.stringify(await r.json(),null,2);await status()}status();</script></body></html>"#.into()))
}

mod api;
