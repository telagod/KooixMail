use std::convert::Infallible;

use async_stream::stream;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
};

use crate::{
    auth::{authorize, now_ts, ts_to_rfc3339},
    models::{AppError, AppState, EventsQuery, MailboxEvent},
    routes::event_sender,
};

pub(super) async fn stream_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let mailbox = authorize(&state, &headers, query.token.as_deref()).await?;
    if mailbox.id != query.mailbox_id {
        return Err(AppError::Forbidden("mailbox token mismatch".to_string()));
    }

    let sender = event_sender(&state, &mailbox.id).await;
    let mut receiver = sender.subscribe();
    let connected = MailboxEvent {
        kind: "connected".to_string(),
        mailbox_id: mailbox.id,
        message_id: String::new(),
        created_at: ts_to_rfc3339(now_ts()),
    };

    let event_stream = stream! {
        if let Ok(event) = Event::default().event("connected").json_data(&connected) {
            yield Ok::<Event, Infallible>(event);
        }

        loop {
            match receiver.recv().await {
                Ok(payload) => {
                    if let Ok(event) = Event::default().event(&payload.kind).json_data(&payload) {
                        yield Ok::<Event, Infallible>(event);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}
