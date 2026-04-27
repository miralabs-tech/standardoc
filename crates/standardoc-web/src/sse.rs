//! Server-Sent Events endpoint to push index changes to frontend in real time.
//!
//! Pattern: subscribe to `WebState` `broadcast::Receiver`, convert each
//! `IndexEvent` into an axum `Event`, and delegate keep-alive to
//! `KeepAlive::default()` (15s default, enough for typical proxies).

use crate::state::{IndexEvent, WebState};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_core::Stream;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

#[allow(clippy::unused_async)]
pub(crate) async fn events(
    State(state): State<Arc<dyn WebState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(to_sse_event);
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn to_sse_event(
    item: Result<IndexEvent, tokio_stream::wrappers::errors::BroadcastStreamRecvError>,
) -> Option<Result<Event, Infallible>> {
    // If receiver lagged (broadcast buffer full), skip silently.
    // Frontend re-fetches by revision on next heartbeat — no crash, only
    // one missed invalidation cycle.
    let event = item.ok()?;
    let name = match &event {
        IndexEvent::IndexChanged { .. } => "index_changed",
        IndexEvent::Heartbeat { .. } => "heartbeat",
    };
    let data = serde_json::to_string(&event).ok()?;
    Some(Ok(Event::default().event(name).data(data)))
}
