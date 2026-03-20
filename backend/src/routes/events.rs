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
use tokio::time::{Duration, interval};

use crate::{
    auth::{authorize, now_ts, ts_to_rfc3339},
    db::poll_events_since,
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
    let mailbox_id = mailbox.id.clone();
    let db = state.db.clone();

    let connected = MailboxEvent {
        kind: "connected".to_string(),
        mailbox_id: mailbox.id,
        message_id: String::new(),
        created_at: ts_to_rfc3339(now_ts()),
        db_id: None,
    };

    let event_stream = stream! {
        if let Ok(event) = Event::default().event("connected").json_data(&connected) {
            yield Ok::<Event, Infallible>(event);
        }

        // Initialize cursor to current max event id
        let mut last_event_id: i64 = poll_events_since(&db, &mailbox_id, 0)
            .await
            .ok()
            .and_then(|rows| rows.last().map(|r| r.id))
            .unwrap_or(0);

        let mut poll_interval = interval(Duration::from_secs(3));
        poll_interval.tick().await; // consume first immediate tick

        loop {
            tokio::select! {
                result = receiver.recv() => {
                    match result {
                        Ok(payload) => {
                            // Update cursor from broadcast to avoid re-delivering via poll
                            if let Some(db_id) = payload.db_id
                                && db_id > last_event_id
                            {
                                last_event_id = db_id;
                            }
                            if let Ok(event) = Event::default().event(&payload.kind).json_data(&payload) {
                                yield Ok::<Event, Infallible>(event);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = poll_interval.tick() => {
                    // Poll DB for events from other instances
                    if let Ok(rows) = poll_events_since(&db, &mailbox_id, last_event_id).await {
                        for row in rows {
                            if row.id > last_event_id {
                                last_event_id = row.id;
                            }
                            let payload = MailboxEvent {
                                kind: row.kind,
                                mailbox_id: row.mailbox_id,
                                message_id: row.message_id,
                                created_at: ts_to_rfc3339(row.created_at),
                                db_id: Some(row.id),
                            };
                            if let Ok(event) = Event::default().event(&payload.kind).json_data(&payload) {
                                yield Ok::<Event, Infallible>(event);
                            }
                        }
                    }
                }
            }
        }
    };

    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}
