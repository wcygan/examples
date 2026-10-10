use futures_util::{StreamExt, stream};
use topcoat_core::{context::Cx, error::Result};
use topcoat_router::content::sse::{Event, KeepAlive, Sse, last_event_id};
use topcoat_router_macro::route;

#[route(GET "/api/events")]
pub async fn events(
    cx: &Cx,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event>> + use<>>> {
    let last = last_event_id(cx).and_then(|x| x.parse::<u64>().ok());
    let (replay, rx, gap) = topcoat_portal::runtime().subscribe(last).await;
    let mut initial = Vec::new();
    if gap {
        let state = topcoat_portal::runtime().status().await;
        initial.push(
            Event::new()
                .id(topcoat_portal::runtime()
                    .current_event_id()
                    .await
                    .to_string())
                .event("snapshot")
                .json_data(&state)?,
        );
    } else {
        for e in replay {
            initial.push(Event::new().id(e.id.to_string()).json_data(&e)?);
        }
    }
    let first = stream::iter(initial.into_iter().map(Ok));
    let live = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(e) => {
                let event = Event::new().id(e.id.to_string()).json_data(&e).ok()?;
                Some((Ok(event), rx))
            }
            Err(_) => None,
        }
    });
    Ok(Sse::new(first.chain(live)).keep_alive(KeepAlive::new()))
}
