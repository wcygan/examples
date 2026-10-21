//! Session-only history and HTTP/SSE interfaces.
use crate::{
    audit::{Audit, Event},
    cluster,
};
use futures_util::{stream, Stream};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use topcoat::{
    context::{app_context, Cx},
    router::{
        content::{
            sse::{last_event_id, Event as SseEvent, KeepAlive, Sse},
            Json,
        },
        path_param_segment, route, Router, StatusCode,
    },
    Result,
};

#[derive(Clone)]
struct Recording {
    id: u64,
    scenario: String,
    node_count: usize,
    status: String,
    audit: Audit,
}
#[derive(Serialize)]
pub struct Run {
    pub id: u64,
    pub scenario: String,
    pub node_count: usize,
    pub status: String,
    pub events: Vec<Event>,
}
#[derive(Default)]
struct History {
    runs: BTreeMap<u64, Recording>,
    active: Option<u64>,
    next: u64,
}
#[derive(Clone, Default)]
pub struct Console(Arc<Mutex<History>>);
impl Console {
    pub fn run(&self, id: u64) -> Option<Run> {
        let history = self.0.lock().unwrap();
        history.runs.get(&id).map(|r| Run {
            id,
            scenario: r.scenario.clone(),
            node_count: r.node_count,
            status: r.status.clone(),
            events: r.audit.events(),
        })
    }
    fn next_event(&self, id: u64, cursor: u64) -> Option<(Option<Event>, bool)> {
        let history = self.0.lock().unwrap();
        let run = history.runs.get(&id)?;
        Some((run.audit.event_after(cursor), run.status == "running"))
    }
    pub fn runs(&self) -> Vec<Run> {
        self.0
            .lock()
            .unwrap()
            .runs
            .values()
            .rev()
            .map(|r| Run {
                id: r.id,
                scenario: r.scenario.clone(),
                node_count: r.node_count,
                status: r.status.clone(),
                events: vec![],
            })
            .collect()
    }
    pub fn start(&self, scenario: &str) -> std::result::Result<u64, StatusCode> {
        self.start_with_nodes(scenario, 3)
    }
    pub fn start_with_nodes(
        &self,
        scenario: &str,
        node_count: usize,
    ) -> std::result::Result<u64, StatusCode> {
        if !cluster::SCENARIOS.contains(&scenario) || !(3..=5).contains(&node_count) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let mut history = self.0.lock().unwrap();
        if history.active.is_some() {
            return Err(StatusCode::CONFLICT);
        }
        history.next += 1;
        let id = history.next;
        let audit = Audit::with_nodes(id, node_count).map_err(|_| StatusCode::BAD_REQUEST)?;
        history.active = Some(id);
        history.runs.insert(
            id,
            Recording {
                id,
                scenario: scenario.into(),
                node_count,
                status: "running".into(),
                audit: audit.clone(),
            },
        );
        drop(history);
        let console = self.clone();
        let scenario = scenario.to_owned();
        tokio::spawn(async move {
            let task_audit = audit.clone();
            let result =
                tokio::spawn(async move { cluster::run(&scenario, task_audit).await }).await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(e) => Some(e.to_string()),
            };
            if let Some(error) = &error {
                audit.finish(Some(error));
            }
            let mut history = console.0.lock().unwrap();
            history.runs.get_mut(&id).unwrap().status = if error.is_some() {
                "failed"
            } else {
                "finished"
            }
            .into();
            history.active = None;
        });
        Ok(id)
    }
}
#[route(GET "/")]
async fn index() -> Result<([(&'static str, &'static str); 1], &'static str)> {
    Ok((
        [("content-type", "text/html; charset=utf-8")],
        include_str!("ui/index.html"),
    ))
}
#[route(GET "/console.css")]
async fn css() -> Result<([(&'static str, &'static str); 1], &'static str)> {
    Ok((
        [("content-type", "text/css")],
        include_str!("ui/console.css"),
    ))
}
#[route(GET "/console.js")]
async fn javascript() -> Result<([(&'static str, &'static str); 1], &'static str)> {
    Ok((
        [("content-type", "text/javascript")],
        include_str!("ui/console.js"),
    ))
}
#[route(GET "/motion.js")]
async fn motion() -> Result<([(&'static str, &'static str); 1], &'static str)> {
    Ok((
        [("content-type", "text/javascript")],
        include_str!("ui/motion.js"),
    ))
}
#[route(GET "/api/runs")]
async fn list(cx: &Cx) -> Result<Json<Vec<Run>>> {
    Ok(Json(app_context::<Console>(cx).runs()))
}
#[derive(Deserialize)]
struct Start {
    scenario: String,
    #[serde(default = "default_node_count")]
    node_count: usize,
}
fn default_node_count() -> usize {
    3
}
#[route(POST "/api/runs")]
async fn start(cx: &Cx, Json(input): Json<Start>) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let (status, data) = match app_context::<Console>(cx)
        .start_with_nodes(&input.scenario, input.node_count)
    {
        Ok(id) => (StatusCode::CREATED, serde_json::json!({"id": id})),
        Err(status) => (
            status,
            serde_json::json!({"error": if status == StatusCode::CONFLICT {
            "A scenario is already recording" } else { "Choose a valid scenario and 3–5 Raft nodes" }}),
        ),
    };
    Ok((status, Json(data)))
}
#[route(GET "/api/runs/{id}")]
async fn detail(cx: &Cx) -> Result<(StatusCode, Json<Option<Run>>)> {
    let run = path_param_segment(cx, "id")
        .parse()
        .ok()
        .and_then(|id| app_context::<Console>(cx).run(id));
    Ok((
        if run.is_some() {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        },
        Json(run),
    ))
}
#[route(GET "/api/runs/{id}/events")]
async fn events(
    cx: &Cx,
) -> Result<(
    StatusCode,
    Sse<impl Stream<Item = Result<SseEvent>> + use<>>,
)> {
    let console = app_context::<Console>(cx).clone();
    let id = path_param_segment(cx, "id").parse().unwrap_or(0);
    let status = if console.run(id).is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let cursor = last_event_id(cx)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let stream = stream::unfold((console, id, cursor), |(console, id, cursor)| async move {
        loop {
            let (event, running) = console.next_event(id, cursor)?;
            if let Some(event) = event {
                let cursor = event.sequence;
                return Some((
                    SseEvent::new().id(cursor.to_string()).json_data(&event),
                    (console, id, cursor),
                ));
            }
            if !running {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    });
    Ok((status, Sse::new(stream).keep_alive(KeepAlive::new())))
}
pub fn router(console: Console) -> Router {
    Router::builder()
        .app_context(console)
        .route(index)
        .route(css)
        .route(javascript)
        .route(motion)
        .route(list)
        .route(start)
        .route(detail)
        .route(events)
        .build()
}
