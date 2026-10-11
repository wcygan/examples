//! Local session history and Topcoat HTTP routes.
use crate::demo::{Fault, Scenario};
use crate::events::{EventKind, Observer, ProtocolEvent};
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
            sse::{last_event_id, Event, KeepAlive, Sse},
            Json,
        },
        path_param_segment, route, Router, StatusCode,
    },
    Result,
};

#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub id: u64,
    pub scenario: String,
    pub status: String,
    pub events: Vec<ProtocolEvent>,
}
#[derive(Default)]
struct History {
    runs: BTreeMap<u64, Run>,
    active: Option<u64>,
    next_id: u64,
}
#[derive(Clone, Default)]
pub struct Console(Arc<Mutex<History>>);
impl Console {
    pub fn runs(&self) -> Vec<Run> {
        self.0
            .lock()
            .unwrap()
            .runs
            .values()
            .rev()
            .map(|r| Run {
                events: vec![],
                ..r.clone()
            })
            .collect()
    }
    pub fn run(&self, id: u64) -> Option<Run> {
        self.0.lock().unwrap().runs.get(&id).cloned()
    }
    pub fn start(&self, scenario: &str, pace: Duration) -> std::result::Result<u64, StatusCode> {
        let fault = Fault::parse(scenario).ok_or(StatusCode::BAD_REQUEST)?;
        let mut history = self.0.lock().unwrap();
        if history.active.is_some() {
            return Err(StatusCode::CONFLICT);
        }
        history.next_id += 1;
        let id = history.next_id;
        history.active = Some(id);
        history.runs.insert(
            id,
            Run {
                id,
                scenario: scenario.into(),
                status: "running".into(),
                events: vec![],
            },
        );
        drop(history);
        let console = self.clone();
        std::thread::spawn(move || {
            let sink = console.clone();
            let observer = Observer::new(id, pace, move |event| {
                sink.0
                    .lock()
                    .unwrap()
                    .runs
                    .get_mut(&id)
                    .unwrap()
                    .events
                    .push(event);
            });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Scenario::new().run_observed(fault, observer.clone())
            }));
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("Scenario thread panicked".into()),
            };
            if let Some(error) = &error {
                observer.emit(
                    EventKind::Failed,
                    "coordinator",
                    error.clone(),
                    None,
                    |_| {},
                );
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
#[route(GET "/state-machines.js")]
async fn state_machines() -> Result<([(&'static str, &'static str); 1], &'static str)> {
    Ok((
        [("content-type", "text/javascript")],
        include_str!("ui/state-machines.js"),
    ))
}
#[route(GET "/message-demo.js")]
async fn message_demo() -> Result<([(&'static str, &'static str); 1], &'static str)> {
    Ok((
        [("content-type", "text/javascript")],
        include_str!("ui/message-demo.js"),
    ))
}
#[route(GET "/api/runs")]
async fn list(cx: &Cx) -> Result<Json<Vec<Run>>> {
    Ok(Json(app_context::<Console>(cx).runs()))
}
#[derive(Deserialize)]
struct Start {
    scenario: String,
}
#[route(POST "/api/runs")]
async fn start(cx: &Cx, Json(input): Json<Start>) -> Result<(StatusCode, Json<serde_json::Value>)> {
    match app_context::<Console>(cx).start(&input.scenario, Duration::ZERO) {
        Ok(id) => Ok((StatusCode::CREATED, Json(serde_json::json!({"id": id})))),
        Err(status) => Ok((
            status,
            Json(
                serde_json::json!({"error": if status == StatusCode::CONFLICT { "A scenario is already running" } else { "Unknown scenario" }}),
            ),
        )),
    }
}
fn run_id(cx: &Cx) -> Option<u64> {
    path_param_segment(cx, "id").parse().ok()
}
#[route(GET "/api/runs/{id}")]
async fn detail(cx: &Cx) -> Result<(StatusCode, Json<Option<Run>>)> {
    let run = run_id(cx).and_then(|id| app_context::<Console>(cx).run(id));
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
async fn events(cx: &Cx) -> Result<(StatusCode, Sse<impl Stream<Item = Result<Event>> + use<>>)> {
    let console = app_context::<Console>(cx).clone();
    let id = run_id(cx).unwrap_or(0);
    let status = if console.run(id).is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    };
    let cursor = last_event_id(cx)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let events = stream::unfold(
        (console, id, cursor),
        |(console, id, mut cursor)| async move {
            loop {
                let run = console.run(id)?;
                if let Some(event) = run.events.into_iter().find(|event| event.sequence > cursor) {
                    cursor = event.sequence;
                    let item = Event::new().id(cursor.to_string()).json_data(&event);
                    return Some((item, (console, id, cursor)));
                }
                if run.status != "running" {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        },
    );
    Ok((status, Sse::new(events).keep_alive(KeepAlive::new())))
}
pub fn router(console: Console) -> Router {
    Router::builder()
        .app_context(console)
        .route(index)
        .route(css)
        .route(javascript)
        .route(state_machines)
        .route(message_demo)
        .route(list)
        .route(start)
        .route(detail)
        .route(events)
        .build()
}
