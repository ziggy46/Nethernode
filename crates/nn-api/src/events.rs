//! `GET /api/events`: the event bus as Server-Sent Events, so the UI never
//! polls. Each SSE event's name is the bus event's `type` and its data is
//! the JSON event. A `lagged` event means some were dropped; the client
//! should refetch what it shows. The stream ends when the server shuts
//! down, so it never holds graceful shutdown open.

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, WatchStream};

use crate::AppState;
use crate::middleware::CurrentUser;

pub(crate) async fn stream(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
) -> Response {
    if user.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let events = BroadcastStream::new(state.events.subscribe()).map(|item| {
        Some(match item {
            Ok(event) => {
                let json = serde_json::to_value(&event).unwrap_or_default();
                let name = json
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("event")
                    .to_owned();
                SseEvent::default().event(name).data(json.to_string())
            }
            Err(BroadcastStreamRecvError::Lagged(missed)) => {
                SseEvent::default().event("lagged").data(missed.to_string())
            }
        })
    });
    // `None` marks shutdown; `take_while` ends the stream there.
    let stop = WatchStream::new(state.shutdown.clone())
        .filter(|stopping| *stopping)
        .map(|_| None);
    let stream = events
        .merge(stop)
        .take_while(Option::is_some)
        .filter_map(|event| event.map(Ok::<_, Infallible>));
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
